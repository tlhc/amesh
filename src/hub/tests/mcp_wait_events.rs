use super::*;

#[tokio::test]
async fn mcp_wait_reports_the_answer_without_the_question() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "pi", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let (tx, _rx) = mpsc::unbounded_channel();
    inner.lock().await.sockets.insert("boss".into(), (1, tx));
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "q".repeat(600)}),
    )
    .await;
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_wait",
        json!({"from_peer": "boss", "correlation_id": cid, "timeout_seconds": 0}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let open = tool_json(&body);
    assert_eq!(open["open"], true);
    assert_eq!(
        open["timed_out"], true,
        "open after the deadline is a timeout: {open}"
    );
    assert!(
        open.get("text").is_none(),
        "the asker's own question must not be echoed: {open}"
    );
    let hint = open["hint"].as_str().unwrap_or("");
    assert!(
        hint.contains("peer-message") && hint.contains("at most once"),
        "an open wait must tell a connected asker the ack is pushed and the wait is called once: {open}"
    );
    let mut legacy = open.clone();
    for field in [
        "schema_version",
        "captured_at",
        "state",
        "from_peer_id",
        "actions",
        "for_secs",
    ] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    assert!(legacy.to_string().len() < 300);
    let _ = json_req(
        app.clone(),
        "POST",
        "/ack",
        json!({"correlation_id": cid, "message": "done"}),
    )
    .await;
    let (_, body) = mcp_tool(
        app.clone(),
        "amesh_wait",
        json!({"from_peer": "boss", "correlation_id": cid}),
    )
    .await;
    let closed = tool_json(&body);
    assert_eq!(closed["open"], false);
    assert_eq!(closed["timed_out"], false);
    assert_eq!(closed["reply"], "done");
    assert_eq!(closed["from_peer"], "boss");
    assert_eq!(closed["to_peer"], "worker");
    assert!(closed.get("text").is_none(), "{closed}");
    assert!(
        closed.get("hint").is_none(),
        "a closed ask needs no hint: {closed}"
    );
    let (_, full) = json_req(
        app,
        "POST",
        &format!("/asks/{cid}/wait"),
        json!({"timeout_seconds": 0}),
    )
    .await;
    assert_eq!(
        full["text"].as_str().unwrap().len(),
        600,
        "HTTP keeps the full record"
    );
}

#[tokio::test]
async fn mcp_wait_hints_only_an_asker_the_ack_can_reach() {
    let (app, inner, _state) = test_app_with_hub();
    for id in ["recv", "live", "bare", "drop", "worker"] {
        register(app.clone(), id, "pi", "one").await;
    }
    let (tx, _rx) = mpsc::unbounded_channel();
    {
        let mut hub = inner.lock().await;
        /* promised recv once, then its connection closed: acks only queue for it */
        hub.recv_known.insert("recv".into());
        hub.sockets.insert("live".into(), (1, tx.clone()));
        hub.sockets.insert("drop".into(), (2, tx));
    }
    let mut asks = HashMap::new();
    for asker in ["recv", "live", "bare", "drop"] {
        let (_, ask) = json_req(
            app.clone(),
            "POST",
            "/ask",
            json!({"from_peer": asker, "to_peer": "worker", "text": "q"}),
        )
        .await;
        asks.insert(asker, ask["correlation_id"].as_str().unwrap().to_string());
    }
    let mut wrong = Vec::new();
    for (asker, caller, hinted) in [
        ("live", "live", true),
        ("recv", "recv", false),
        ("bare", "bare", false),
        ("recv", "live", false),
        ("recv", "ghost", false),
    ] {
        let (_, body) = mcp_tool(
            app.clone(),
            "amesh_wait",
            json!({"from_peer": caller, "correlation_id": asks[asker], "timeout_seconds": 0}),
        )
        .await;
        let got = tool_json(&body);
        if got.get("hint").is_some() != hinted {
            wrong.push(format!("asker {asker} caller {caller}: {got}"));
        }
    }
    let waiting = tokio::spawn(mcp_tool(
        app.clone(),
        "amesh_wait",
        json!({"from_peer": "drop", "correlation_id": asks["drop"], "timeout_seconds": 1}),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    inner.lock().await.sockets.remove("drop");
    let got = tool_json(&waiting.await.unwrap().1);
    if got.get("hint").is_some() {
        wrong.push(format!("asker drop lost its socket mid-wait: {got}"));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[tokio::test]
async fn mcp_wait_defaults_to_the_codex_exec_budget() {
    let (app, _state) = test_app();
    register(app.clone(), "cx", "codex", "one").await;
    register(app.clone(), "boss", "pi", "one").await;
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "cx", "text": "hi"}),
    )
    .await;
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    let _ = json_req(app.clone(), "POST", "/ack", json!({"correlation_id": cid})).await;
    for (caller, expected) in [("cx", 8), ("boss", 45), ("ghost", 45)] {
        let (st, body) = mcp_tool(
            app.clone(),
            "amesh_wait",
            json!({"from_peer": caller, "correlation_id": cid}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            tool_json(&body)["timeout_seconds"],
            expected,
            "caller {caller}"
        );
    }
    for (requested, effective) in [(3, 3), (900, 50)] {
        let (_, body) = mcp_tool(
            app.clone(),
            "amesh_wait",
            json!({"from_peer": "cx", "correlation_id": cid, "timeout_seconds": requested}),
        )
        .await;
        assert_eq!(
            tool_json(&body)["timeout_seconds"],
            effective,
            "requested {requested}"
        );
    }
    let tools = mcp_tools();
    let wait = tools.iter().find(|t| t["name"] == "amesh_wait").unwrap();
    let description = wait["description"].as_str().unwrap();
    assert!(
        description.contains(&WAIT_MAX_SECS.to_string()),
        "the cap must be stated: {description}"
    );
    assert!(
        description.contains("Codex"),
        "the codex budget must be stated: {description}"
    );
    assert!(
        description.contains("at most once per ask")
            && description.contains("when the result carries a hint")
            && description.contains("pushed to you as a peer-message"),
        "the push is expected under the hint and the wait is called once: {description}"
    );
}

#[tokio::test]
async fn mcp_events_returns_the_newest_trimmed_entries() {
    let (app, _state) = test_app();
    register(app.clone(), "here", "pi", "one").await;
    for i in 0..60 {
        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/events/chat",
            json!({"peer": "here", "role": "user", "text": format!("{i:03}-{}", "x".repeat(1000))}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, body) = mcp_tool(app.clone(), "amesh_events", json!({"from_peer": "here"})).await;
    assert_eq!(st, StatusCode::OK);
    let rows = tool_json(&body);
    let rows = rows.as_array().unwrap();
    /* every chat row is followed by its notify copy, so 20 rows span 050..059 */
    let body_of = |row: &Value| {
        row["text"]
            .as_str()
            .or(row["message"].as_str())
            .unwrap()
            .to_string()
    };
    assert_eq!(rows.len(), 20, "newest 20 by default");
    assert!(body_of(&rows[19]).starts_with("059-"), "{:?}", rows[19]);
    assert!(body_of(&rows[0]).starts_with("050-"), "{:?}", rows[0]);
    assert!(
        rows.iter().all(|r| body_of(r).chars().count() <= 203),
        "text must be trimmed to 200 chars plus an ellipsis"
    );
    for (limit, expected) in [(5, 5), (500, 50), (0, 1)] {
        let (_, body) = mcp_tool(
            app.clone(),
            "amesh_events",
            json!({"from_peer": "here", "limit": limit}),
        )
        .await;
        let rows = tool_json(&body);
        assert_eq!(rows.as_array().unwrap().len(), expected, "limit {limit}");
    }
    let (_, all) = json_req(app, "GET", "/events", json!({})).await;
    let all = all.as_array().unwrap();
    assert_eq!(
        all.len(),
        120,
        "HTTP keeps the whole ring, chat rows and notify copies"
    );
    assert_eq!(
        all[0]["text"].as_str().unwrap().len(),
        1004,
        "HTTP keeps full text"
    );
    let tools = mcp_tools();
    let events = tools.iter().find(|t| t["name"] == "amesh_events").unwrap();
    assert_eq!(
        events["inputSchema"]["properties"]["limit"]["type"],
        "integer"
    );
    assert!(events["description"].as_str().unwrap().contains("restart"));
}

#[tokio::test]
async fn a_refused_second_answer_points_at_notify() {
    let (app, _state) = test_app();
    register(app.clone(), "worker", "pi", "one").await;
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"}),
    )
    .await;
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    let _ = json_req(
        app.clone(),
        "POST",
        "/ack",
        json!({"correlation_id": cid, "message": "first"}),
    )
    .await;
    let (st, body) = json_req(
        app,
        "POST",
        "/ack",
        json!({"correlation_id": cid, "message": "second"}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(
        body["hint"]
            .as_str()
            .unwrap_or("")
            .contains("amesh_notify_peer"),
        "the refusal must say how to send a follow-up: {body}"
    );
}

#[tokio::test]
async fn mcp_events_page_forward_from_a_cursor() {
    let (app, _state) = test_app();
    register(app.clone(), "here", "pi", "one").await;
    for i in 0..30 {
        let _ = json_req(
            app.clone(),
            "POST",
            "/events/chat_delta",
            json!({"peer": "here", "role": "user", "text": format!("e{i:02}")}),
        )
        .await;
    }
    let (_, all) = json_req(app.clone(), "GET", "/events", json!({})).await;
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 30);
    let since = all[9]["id"].as_str().unwrap().to_string();
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_events",
        json!({"from_peer": "here", "since": since, "limit": 5}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let page = tool_json(&body);
    let page = page.as_array().unwrap();
    assert_eq!(page.len(), 5);
    assert_eq!(
        page[0]["text"], "e10",
        "a cursor pages forward from the oldest unread: {page:?}"
    );
    assert_eq!(page[4]["text"], "e14");
    let next = page[4]["id"].as_str().unwrap().to_string();
    let (_, body) = mcp_tool(
        app.clone(),
        "amesh_events",
        json!({"from_peer": "here", "since": next}),
    )
    .await;
    let rest = tool_json(&body);
    let rest = rest.as_array().unwrap();
    assert_eq!(rest[0]["text"], "e15");
    assert_eq!(rest.len(), 15, "the remainder fits in one default page");
    let (_, body) = mcp_tool(
        app,
        "amesh_events",
        json!({"from_peer": "here", "limit": 5}),
    )
    .await;
    let newest = tool_json(&body);
    assert_eq!(
        newest.as_array().unwrap()[4]["text"],
        "e29",
        "no cursor means the newest entries"
    );
}

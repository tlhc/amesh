use super::*;

/* boss asked worker at t0; worker reported idle before the ask and never reported again */
fn watched_hub(
    name: &str,
) -> (
    Hub,
    TempState,
    mpsc::UnboundedReceiver<Outbound>,
    mpsc::UnboundedReceiver<Outbound>,
    u64,
) {
    let path = temp_state(name);
    let mut hub = Hub::open(&path).unwrap();
    let boss = recv_peer(&mut hub, "boss");
    let worker = recv_peer(&mut hub, "worker");
    let t0 = now_unix();
    hub.activity.insert(
        "worker".into(),
        Activity {
            state: "idle".into(),
            since: t0 - 5,
            observed_at: t0 - 5,
            source: "test".into(),
            reason: None,
        },
    );
    hub.asks.insert(
        "ask-1".into(),
        serde_json::from_value(json!({
            "correlation_id": "ask-1", "from_peer": "boss", "to_peer": "worker",
            "to_peer_id": "worker", "from_circle": "default", "to_circle": "default",
            "text": "ping", "open": true, "reply": null, "opened_at": t0
        }))
        .unwrap(),
    );
    (hub, path, boss, worker, t0)
}

fn pushed(rx: &mut mpsc::UnboundedReceiver<Outbound>) -> Vec<Value> {
    let mut events = Vec::new();
    while let Ok(outbound) = rx.try_recv() {
        events.push(outbound.event().clone());
    }
    events
}

#[test]
fn a_watch_pushes_stalled_once_and_nudges_the_recipient() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-stalled");
    let registered = register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    assert_eq!(registered["watch_until"], t0 + WATCH_DEFAULT_SECS);
    advance_asks(&mut hub, t0 + 5);
    assert!(
        pushed(&mut boss).is_empty(),
        "a five second old ask is still pending"
    );
    advance_asks(&mut hub, t0 + 11);
    let stalled = pushed(&mut boss);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["type"], "notify");
    assert_eq!(stalled[0]["event"], "stalled");
    assert_eq!(stalled[0]["topic"], "ask-1");
    assert_eq!(stalled[0]["from_peer"], "amesh");
    assert_eq!(stalled[0]["correlation_id"], "ask-1");
    let message = stalled[0]["message"].as_str().unwrap();
    assert!(
        message.contains("ask-1") && message.contains("nudged once"),
        "the text names the ask and says the recipient was nudged: {message}"
    );
    let nudged = pushed(&mut worker);
    assert_eq!(nudged.len(), 1, "{nudged:?}");
    assert_eq!(nudged[0]["topic"], "ask-1");
    assert_eq!(nudged[0]["correlation_id"], "ask-1");
    assert!(nudged[0]["message"].as_str().unwrap().contains("amesh_ack"));
    advance_asks(&mut hub, t0 + 12);
    advance_asks(&mut hub, t0 + 600);
    assert!(
        pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty(),
        "stalled is pushed once and the recipient is nudged once"
    );
    /* re-arming keeps what was already pushed and only moves the deadline */
    let again = register_watch(&mut hub, "ask-1", Some(500), None, t0 + 12).unwrap();
    assert_eq!(again["watch_until"], t0 + 12 + 500);
    assert_eq!(hub.asks["ask-1"].watch.as_ref().unwrap().sent, "stalled");
    advance_asks(&mut hub, t0 + 20);
    assert!(
        pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty(),
        "re-arming does not repeat stalled"
    );
}

#[test]
fn an_away_asker_keeps_its_events_pending() {
    let (mut hub, _state, _boss, mut worker, t0) = watched_hub("watch-away-asker");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    /* the asker's row was pruned: no socket, no backlog, no owed entry */
    hub.peers.remove("boss");
    hub.sockets.remove("boss");
    advance_asks(&mut hub, t0 + 11);
    assert_eq!(
        hub.asks["ask-1"].watch.as_ref().unwrap().sent,
        "",
        "an undeliverable event is not recorded as pushed"
    );
    assert!(
        pushed(&mut worker).is_empty(),
        "the nudge waits with the event it belongs to"
    );
    /* the asker is back: the pending stall fires on the next tick */
    let mut boss = recv_peer(&mut hub, "boss");
    advance_asks(&mut hub, t0 + 12);
    let stalled = pushed(&mut boss);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["event"], "stalled");
    assert_eq!(pushed(&mut worker).len(), 1);
}

#[test]
fn an_offline_recipient_is_reported_once_without_a_nudge() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-offline");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    advance_asks(&mut hub, t0 + 1);
    let offline = pushed(&mut boss);
    assert_eq!(offline.len(), 1, "{offline:?}");
    assert_eq!(offline[0]["event"], "offline");
    assert!(
        pushed(&mut worker).is_empty(),
        "an offline recipient is not nudged"
    );
    /* back online and idle: stalled follows once; a second drop is not reported again */
    hub.peers.get_mut("worker").unwrap().status = "online".into();
    advance_asks(&mut hub, t0 + 11);
    let stalled = pushed(&mut boss);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["event"], "stalled");
    assert_eq!(pushed(&mut worker).len(), 1);
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    advance_asks(&mut hub, t0 + 12);
    assert!(
        pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty(),
        "each state is pushed once"
    );
    assert_eq!(
        hub.asks["ask-1"].watch.as_ref().unwrap().sent,
        "offline stalled"
    );
}

#[test]
fn a_recipient_gone_with_a_kept_backlog_is_reported_once() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-gone");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    /* the prune kept the recipient's backlog for its session and dropped its row */
    hub.peers.remove("worker");
    hub.sockets.remove("worker");
    hub.owed.insert(
        "worker".into(),
        Owed {
            since: t0,
            owner: "s-worker".into(),
        },
    );
    advance_asks(&mut hub, t0 + 1);
    let gone = pushed(&mut boss);
    assert_eq!(gone.len(), 1, "{gone:?}");
    assert_eq!(gone[0]["event"], "gone");
    assert!(
        gone[0]["message"].as_str().unwrap().contains("session"),
        "the text says the backlog is kept: {gone:?}"
    );
    assert!(pushed(&mut worker).is_empty());
    advance_asks(&mut hub, t0 + 2);
    assert!(pushed(&mut boss).is_empty());
}

#[test]
fn the_deadline_pushes_alone_when_a_state_event_is_due() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-together");
    register_watch(&mut hub, "ask-1", Some(30), None, t0).unwrap();
    /* one tick a minute later owes a stall and the deadline at once; the deadline carries the
    stall inside its own text, so nothing else goes out */
    advance_asks(&mut hub, t0 + 30);
    let due = pushed(&mut boss);
    let events: Vec<&str> = due
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(events, ["deadline"], "{due:?}");
    assert_eq!(
        due[0]["message"],
        "ask ask-1 to worker deadline: the deadline passed with the ask still open; the watch ended; worker idle for 30s",
        "{due:?}"
    );
    assert!(
        pushed(&mut worker).is_empty(),
        "a deadline that ends the watch does not nudge the recipient"
    );
    assert!(hub.asks["ask-1"].watch.is_none());
    advance_asks(&mut hub, t0 + 60);
    assert!(pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty());
}

#[test]
fn the_deadline_names_a_working_recipient_and_its_age() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-deadline-working");
    hub.activity.insert(
        "worker".into(),
        Activity {
            state: "work".into(),
            since: t0 - 45,
            observed_at: t0 - 1,
            source: "test".into(),
            reason: None,
        },
    );
    register_watch(&mut hub, "ask-1", Some(30), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 30);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!(
        due[0]["message"],
        "ask ask-1 to worker deadline: the deadline passed with the ask still open; the watch ended; worker working for 75s",
        "{due:?}"
    );
    assert!(pushed(&mut worker).is_empty());
}

#[test]
fn the_deadline_reads_an_age_of_minutes_from_the_ask() {
    let (mut hub, _state, mut boss, _worker, t0) = watched_hub("watch-deadline-minutes");
    /* no activity report: the recipient is pending since the ask opened */
    hub.activity.remove("worker");
    register_watch(&mut hub, "ask-1", Some(240), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 240);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert!(
        due[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("; worker pending for 4m"),
        "{due:?}"
    );
}

#[test]
fn the_deadline_names_an_offline_recipient() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-deadline-offline");
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    register_watch(&mut hub, "ask-1", Some(30), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 30);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert!(
        due[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("; worker is offline"),
        "{due:?}"
    );
    assert!(pushed(&mut worker).is_empty());
}

#[test]
fn the_deadline_names_a_recipient_that_left_the_hub() {
    let (mut hub, _state, mut boss, _worker, t0) = watched_hub("watch-deadline-gone");
    hub.peers.remove("worker");
    hub.sockets.remove("worker");
    hub.owed.insert(
        "worker".into(),
        Owed {
            since: t0,
            owner: "s-worker".into(),
        },
    );
    register_watch(&mut hub, "ask-1", Some(30), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 30);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert!(
        due[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("; worker has left the hub"),
        "{due:?}"
    );
}

#[test]
fn the_catalogue_states_the_watch_default_and_the_deadline_cycle() {
    let tools = mcp_tools();
    let description = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .and_then(|tool| tool["description"].as_str())
            .unwrap()
            .to_string()
    };
    assert!(description("amesh_ask").contains("arms a watch by default (deadline 300 s"));
    assert!(description("amesh_ask_many").contains("each ask arms a watch by default"));
    let watch = description("amesh_watch");
    assert!(watch.contains("amesh_ask and job_create arm it by default"));
    assert!(watch.contains("after the deadline a new registration starts a fresh cycle"));
    assert!(watch.contains("call this to re-arm or to register late"));
    for job in ["amesh_job_create", "amesh_job_update"] {
        assert!(
            description(job).contains("watch (default true) and deadline_seconds (default 1200)"),
            "{job} names the dispatch watch"
        );
    }
}

#[test]
fn an_acked_ask_ends_its_watch_without_a_push() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-acked");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    let ask = hub.asks.get_mut("ask-1").unwrap();
    ask.open = false;
    ask.reply = Some("pong".into());
    ask.closed_at = Some(t0 + 3);
    ask.closed_by = Some("recipient".into());
    advance_asks(&mut hub, t0 + 11);
    assert!(
        hub.asks["ask-1"].watch.is_none(),
        "the watch ends with the ask"
    );
    assert!(pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty());
}

#[test]
fn a_watch_pushes_the_deadline_once_and_ends() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-deadline");
    /* no activity report: the recipient stays pending, so only the deadline can fire */
    hub.activity.remove("worker");
    register_watch(&mut hub, "ask-1", Some(30), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 29);
    assert!(
        pushed(&mut boss).is_empty(),
        "one second before the deadline"
    );
    advance_asks(&mut hub, t0 + 30);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!(due[0]["event"], "deadline");
    assert_eq!(due[0]["topic"], "ask-1");
    assert!(
        hub.asks["ask-1"].watch.is_none(),
        "the deadline ends the watch"
    );
    advance_asks(&mut hub, t0 + 600);
    assert!(pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty());
}

#[test]
fn a_watch_survives_a_reload_without_repeating_itself() {
    let (mut hub, state, mut boss, _worker, t0) = watched_hub("watch-reload");
    register_watch(&mut hub, "ask-1", Some(100), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 11);
    assert_eq!(pushed(&mut boss).len(), 1, "stalled before the restart");
    drop(hub);
    let mut again = Hub::open(&state).unwrap();
    let watch = again.asks["ask-1"]
        .watch
        .clone()
        .expect("the watch is in the state file");
    assert_eq!(watch.until, t0 + 100);
    /* the restarted hub hears the same idle report again and the asker reconnects */
    let mut boss = recv_peer(&mut again, "boss");
    again.activity.insert(
        "worker".into(),
        Activity {
            state: "idle".into(),
            since: t0 - 5,
            observed_at: t0 + 20,
            source: "test".into(),
            reason: None,
        },
    );
    advance_asks(&mut again, t0 + 40);
    assert!(
        pushed(&mut boss).is_empty(),
        "stalled was already pushed before the restart"
    );
    advance_asks(&mut again, t0 + 100);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!(due[0]["event"], "deadline");
}

#[tokio::test]
async fn amesh_watch_registers_over_mcp_and_http() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "pi", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"}),
    )
    .await;
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_watch",
        json!({"from_peer": "boss", "correlation_id": cid, "deadline_seconds": 60}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let registered = tool_json(&body);
    assert_eq!(registered["ok"], true);
    assert_eq!(registered["correlation_id"], cid);
    let until = registered["watch_until"].as_u64().unwrap();
    let now = now_unix();
    assert!((now + 59..=now + 61).contains(&until), "{until} vs {now}");
    assert_eq!(
        inner.lock().await.asks[&cid]
            .watch
            .as_ref()
            .map(|w| w.until),
        Some(until)
    );
    let (st, body) = json_req(
        app.clone(),
        "POST",
        &format!("/asks/{cid}/watch"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body["watch_until"].as_u64().unwrap() >= now + WATCH_DEFAULT_SECS - 1,
        "no deadline_seconds means the default: {body}"
    );
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_watch",
        json!({"from_peer": "worker", "correlation_id": cid}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "only the asker can register the watch");
    let (_, body) = mcp_tool(
        app.clone(),
        "amesh_watch",
        json!({"from_peer": "boss", "correlation_id": cid, "deadline_seconds": 10_000_000}),
    )
    .await;
    let capped = tool_json(&body)["watch_until"].as_u64().unwrap();
    let now = now_unix();
    assert!(
        (now + WATCH_MAX_SECS - 1..=now + WATCH_MAX_SECS + 1).contains(&capped),
        "the deadline is capped: {capped} vs {now}"
    );
    let (_, orphan) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "nobody", "to_peer": "worker", "text": "ping"}),
    )
    .await;
    let orphan = orphan["correlation_id"].as_str().unwrap().to_string();
    let (st, body) = json_req(
        app.clone(),
        "POST",
        &format!("/asks/{orphan}/watch"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "the asker is not a registered peer");
    let (st, body) = json_req(app.clone(), "POST", "/asks/nope/watch", json!({})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown ask");
    inner.lock().await.asks.get_mut(&cid).unwrap().open = false;
    let (st, body) = json_req(app, "POST", &format!("/asks/{cid}/watch"), json!({})).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "ask is closed");
    let tool = mcp_tools()
        .into_iter()
        .find(|tool| tool["name"] == "amesh_watch")
        .expect("amesh_watch is in the catalogue");
    assert_eq!(tool["inputSchema"]["required"], json!(["correlation_id"]));
}

#[tokio::test]
async fn an_ask_arms_its_watch_at_creation() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "pi", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let before = now_unix();
    let (st, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ask}");
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    let until = ask["watch_until"]
        .as_u64()
        .expect("an ask arms its watch by default");
    assert!(
        (before + WATCH_ASK_DEFAULT_SECS..=before + WATCH_ASK_DEFAULT_SECS + 1).contains(&until),
        "{until} vs {before}"
    );
    assert!(
        ask["recipient"].is_string(),
        "the response names the recipient's state: {ask}"
    );
    {
        let hub = inner.lock().await;
        let watch = hub.asks[&cid]
            .watch
            .as_ref()
            .expect("the watch is on the ask");
        assert_eq!(watch.until, until);
        assert_eq!(watch.sent, "", "a pending recipient seeds nothing");
    }
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping", "deadline_seconds": 7}),
    )
    .await;
    let until = ask["watch_until"].as_u64().unwrap();
    let now = now_unix();
    assert!(
        (now + 6..=now + 8).contains(&until),
        "a deadline arms the watch for that long: {until} vs {now}"
    );
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping", "deadline_seconds": 10_000_000}),
    )
    .await;
    let capped = ask["watch_until"].as_u64().unwrap();
    let now = now_unix();
    assert!(
        (now + WATCH_MAX_SECS - 1..=now + WATCH_MAX_SECS + 1).contains(&capped),
        "the deadline is capped: {capped} vs {now}"
    );
    let (st, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping", "watch": false}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ask}");
    assert!(
        ask["watch_until"].is_null(),
        "watch=false leaves the ask unwatched: {ask}"
    );
    let unwatched = ask["correlation_id"].as_str().unwrap().to_string();
    assert!(inner.lock().await.asks[&unwatched].watch.is_none());
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_ask",
        json!({"from_peer": "boss", "peer_name": "worker", "query": "ping", "deadline_seconds": 9}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let until = tool_json(&body)["watch_until"]
        .as_u64()
        .expect("the MCP tool passes deadline_seconds on");
    let now = now_unix();
    assert!((now + 8..=now + 10).contains(&until), "{until} vs {now}");
    let (_, body) = mcp_tool(
        app,
        "amesh_ask",
        json!({"from_peer": "boss", "peer_name": "worker", "query": "ping", "watch": false}),
    )
    .await;
    assert!(
        tool_json(&body)["watch_until"].is_null(),
        "the MCP tool passes watch=false on: {body}"
    );
    for name in ["amesh_ask", "amesh_ask_many"] {
        let tool = mcp_tools()
            .into_iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        let properties = &tool["inputSchema"]["properties"];
        assert!(
            properties.get("watch").is_some() && properties.get("deadline_seconds").is_some(),
            "{name} declares watch and deadline_seconds: {properties}"
        );
    }
}

#[tokio::test]
async fn an_anonymous_ask_is_unwatched_unless_asked() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "worker", "pi", "one").await;
    let (st, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"to_peer": "worker", "text": "ping"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ask}");
    assert!(
        ask["watch_until"].is_null(),
        "nobody to push to, so no watch: {ask}"
    );
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    assert!(inner.lock().await.asks[&cid].watch.is_none());
    for body in [
        json!({"to_peer": "worker", "text": "ping", "watch": true}),
        json!({"from_peer": "nobody", "to_peer": "worker", "text": "ping", "deadline_seconds": 5}),
    ] {
        let (st, refused) = json_req(app.clone(), "POST", "/ask", body).await;
        assert_eq!(st, StatusCode::CONFLICT, "{refused}");
        assert_eq!(refused["error"], "the asker is not a registered peer");
    }
    assert_eq!(
        inner.lock().await.asks.len(),
        1,
        "a refused ask is not written"
    );
}

#[tokio::test]
async fn ask_many_arms_each_child() {
    let (app, inner, _state) = test_app_with_hub();
    for id in ["boss", "worker", "other"] {
        register(app.clone(), id, "pi", "one").await;
    }
    let (st, batch) = json_req(
        app.clone(),
        "POST",
        "/ask-many",
        json!({"from_peer": "boss", "to_peers": ["worker", "other"], "text": "ping", "deadline_seconds": 8}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{batch}");
    let asks: Vec<String> = batch["asks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(asks.len(), 2);
    let watches = batch["watches"]
        .as_object()
        .expect("the batch lists each child's watch");
    let now = now_unix();
    for cid in &asks {
        let until = watches[cid].as_u64().unwrap();
        assert!((now + 7..=now + 9).contains(&until), "{until} vs {now}");
        assert_eq!(
            inner.lock().await.asks[cid].watch.as_ref().map(|w| w.until),
            Some(until)
        );
    }
    let (_, body) = mcp_tool(
        app,
        "amesh_ask_many",
        json!({"from_peer": "boss", "to_peers": ["worker"], "text": "ping", "watch": false}),
    )
    .await;
    let batch = tool_json(&body);
    assert_eq!(
        batch["watches"],
        json!({}),
        "watch=false arms nothing: {batch}"
    );
    let cid = batch["asks"][0].as_str().unwrap().to_string();
    assert!(inner.lock().await.asks[&cid].watch.is_none());
}

#[tokio::test]
async fn an_ask_to_an_offline_recipient_pushes_only_what_changes() {
    let path = temp_state("watch-seeded");
    let mut hub = Hub::open(&path).unwrap();
    let mut boss = recv_peer(&mut hub, "boss");
    let mut worker = recv_peer(&mut hub, "worker");
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    let inner = Arc::new(Mutex::new(hub));
    let app = App {
        inner: inner.clone(),
        token: None,
        state_path: path.to_path_buf(),
    };
    let t0 = now_unix();
    let opened = open_ask(
        State(app.clone()),
        auth_headers(&app),
        Json(AskReq {
            from_peer: Some("boss".into()),
            to_peer: "worker".into(),
            text: "ping".into(),
            attachments: None,
            cross_circle: false,
            watch: None,
            deadline_seconds: Some(60),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(opened["recipient"], "offline", "{opened}");
    let cid = opened["correlation_id"].as_str().unwrap().to_string();
    let mut hub = inner.lock().await;
    assert_eq!(hub.asks[&cid].watch.as_ref().unwrap().sent, "offline");
    pushed(&mut worker);
    advance_asks(&mut hub, t0 + 1);
    advance_asks(&mut hub, t0 + 11);
    assert!(
        pushed(&mut boss).is_empty(),
        "the asker was told offline in the answer and is not told again"
    );
    /* the worker comes back and sits idle: that is a change */
    hub.peers.get_mut("worker").unwrap().status = "online".into();
    hub.activity.insert(
        "worker".into(),
        Activity {
            state: "idle".into(),
            since: t0 + 12,
            observed_at: t0 + 12,
            source: "test".into(),
            reason: None,
        },
    );
    advance_asks(&mut hub, t0 + 13);
    let stalled = pushed(&mut boss);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["event"], "stalled");
    assert_eq!(pushed(&mut worker).len(), 1, "the recipient is nudged once");
}

#[tokio::test]
async fn a_deadline_arms_the_watch_even_with_watch_false() {
    let (app, inner, _state) = test_app_with_hub();
    for id in ["boss", "worker", "other"] {
        register(app.clone(), id, "pi", "one").await;
    }
    let (st, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping", "watch": false, "deadline_seconds": 7}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ask}");
    let until = ask["watch_until"]
        .as_u64()
        .expect("a deadline arms the watch whatever watch says");
    let now = now_unix();
    assert!((now + 6..=now + 8).contains(&until), "{until} vs {now}");
    let (_, body) = mcp_tool(
        app.clone(),
        "amesh_ask",
        json!({"from_peer": "boss", "peer_name": "worker", "query": "ping", "watch": false, "deadline_seconds": 7}),
    )
    .await;
    assert!(
        tool_json(&body)["watch_until"].is_u64(),
        "the MCP tool follows the same rule: {body}"
    );
    let (_, batch) = json_req(
        app,
        "POST",
        "/ask-many",
        json!({"from_peer": "boss", "to_peers": ["worker", "other"], "text": "ping", "watch": false, "deadline_seconds": 7}),
    )
    .await;
    assert_eq!(
        batch["watches"].as_object().map(|w| w.len()),
        Some(2),
        "every child is armed: {batch}"
    );
    let cid = batch["asks"][0].as_str().unwrap().to_string();
    assert!(inner.lock().await.asks[&cid].watch.is_some());
}

#[test]
fn a_re_arm_after_the_deadline_starts_a_fresh_cycle() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-rearm-expired");
    register_watch(&mut hub, "ask-1", Some(20), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 11);
    assert_eq!(pushed(&mut boss).len(), 1, "the first cycle pushes stalled");
    pushed(&mut worker);
    /* the deadline has passed but no tick has run yet: the old watch row is still there */
    register_watch(&mut hub, "ask-1", Some(10), None, t0 + 21).unwrap();
    assert_eq!(
        hub.asks["ask-1"].watch.as_ref().unwrap().sent,
        "",
        "a registration after the deadline inherits nothing"
    );
    advance_asks(&mut hub, t0 + 22);
    let again = pushed(&mut boss);
    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(again[0]["event"], "stalled");
    assert_eq!(
        pushed(&mut worker).len(),
        1,
        "the recipient is nudged again"
    );
}

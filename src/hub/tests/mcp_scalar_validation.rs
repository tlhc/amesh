use super::*;

fn business_state(hub: &Hub) -> (Value, Inbox) {
    (
        json!({
            "asks": hub.asks, "jobs": hub.jobs, "schedules": hub.schedules,
            "events": hub.events,
            "watches": hub.asks.iter().map(|(id, ask)| (id, &ask.watch)).collect::<HashMap<_, _>>()
        }),
        hub.inbox.clone(),
    )
}

async fn rejects_scalar(tool: &str, field: &str) {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "codex", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let mut args = json!({
        "from_peer":"boss", "peer_name":"worker", "to_peer":"worker", "to_peers":["worker"],
        "query":"question", "message":"message", "text":"question", "title":"job",
        "in_seconds":1
    });
    if matches!(tool, "amesh_watch" | "amesh_wait") {
        let (_, ask) = json_req(
            app.clone(),
            "POST",
            "/ask",
            json!({"from_peer":"boss", "to_peer":"worker", "text":"q", "watch":false}),
        )
        .await;
        args["correlation_id"] = ask["correlation_id"].clone();
        if tool == "amesh_wait" {
            json_req(
                app.clone(),
                "POST",
                "/ack",
                json!({"correlation_id":args["correlation_id"]}),
            )
            .await;
        }
    }
    args[field] = json!("wrong type");
    let before = business_state(&*inner.lock().await);
    let (status, body) = mcp_tool(app, tool, args).await;
    assert_eq!(
        business_state(&*inner.lock().await),
        before,
        "{tool}/{field}: {body}"
    );
    assert_eq!(status, StatusCode::OK, "{tool}/{field}: {body}");
    assert_eq!(body["jsonrpc"], "2.0", "{body}");
    assert_eq!(body["id"], 1, "{body}");
    assert_eq!(body["result"]["isError"], true, "{body}");
    let error = tool_json(&body);
    assert_eq!(error["code"], "INVALID_ARGUMENT", "{error}");
    assert_eq!(error["field"], field, "{error}");
    assert!(error["error"].as_str().unwrap().contains(field), "{error}");
    assert!(
        error["hint"].as_str().unwrap().contains("resubmit"),
        "{error}"
    );
}

macro_rules! rejects {
    ($name:ident, $tool:literal, $field:literal) => {
        #[tokio::test]
        async fn $name() {
            rejects_scalar($tool, $field).await;
        }
    };
}

rejects!(ask_cross, "amesh_ask", "cross_circle");
rejects!(ask_watch, "amesh_ask", "watch");
rejects!(ask_deadline, "amesh_ask", "deadline_seconds");
rejects!(notify_cross, "amesh_notify_peer", "cross_circle");
rejects!(broadcast_cross, "amesh_broadcast", "cross_circle");
rejects!(job_watch, "amesh_job_create", "watch");
rejects!(job_deadline, "amesh_job_create", "deadline_seconds");
rejects!(schedule_cross, "amesh_schedule_create", "cross_circle");
rejects!(many_cross, "amesh_ask_many", "cross_circle");
rejects!(many_watch, "amesh_ask_many", "watch");
rejects!(many_deadline, "amesh_ask_many", "deadline_seconds");
rejects!(watch_deadline, "amesh_watch", "deadline_seconds");
rejects!(wait_timeout, "amesh_wait", "timeout_seconds");
rejects!(events_cross, "amesh_events", "cross_circle");
rejects!(events_limit, "amesh_events", "limit");
rejects!(peers_cross, "amesh_list_peers", "cross_circle");
rejects!(peers_detail, "amesh_list_peers", "detail");
rejects!(jobs_cross, "amesh_job_list", "cross_circle");
rejects!(jobs_detail, "amesh_job_list", "detail");
rejects!(schedules_cross, "amesh_schedule_list", "cross_circle");

#[tokio::test]
async fn unmarked_errors_keep_their_http_status() {
    let (app, _state) = test_app();
    for (name, args, message) in [
        ("unknown_tool", json!({}), "unknown tool"),
        (
            "amesh_ask_many",
            json!({"to_peers":[]}),
            "to_peers required",
        ),
    ] {
        let (status, body) = mcp_tool(app.clone(), name, args).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], message, "{body}");
        assert!(body.get("result").is_none(), "{body}");
    }
}

async fn rejects_target(tool: &str, field: &str, value: Value, indices: Option<Value>) {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "codex", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let mut args = json!({
        "from_peer":"boss", "title":"job", "prompt":"question",
        "to_peers":["worker"], "text":"question", "watch":false
    });
    args[field] = value;
    let before = business_state(&*inner.lock().await);
    let (status, body) = mcp_tool(app, tool, args).await;
    assert_eq!(business_state(&*inner.lock().await), before, "{body}");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["isError"], true, "{body}");
    let error: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["code"], "INVALID_ARGUMENT");
    assert_eq!(error["field"], field);
    assert!(
        error["error"].as_str().unwrap().contains("string"),
        "{error}"
    );
    let hint = error["hint"].as_str().unwrap();
    assert!(hint.contains("resubmit"), "{error}");
    assert!(
        hint.contains(if field == "assigned_peer" {
            "no job was created"
        } else {
            "no ask was opened"
        }),
        "{error}"
    );
    assert_eq!(error.get("invalid_indices"), indices.as_ref());
}

#[tokio::test]
async fn assigned_peer_type_error_is_actionable() {
    rejects_target("amesh_job_create", "assigned_peer", json!(42), None).await;
}

#[tokio::test]
async fn recipient_type_error_lists_every_invalid_index() {
    rejects_target(
        "amesh_ask_many",
        "to_peers",
        json!(["worker", 42, false, null, {}, [], 1.5, -1, 0]),
        Some(json!([1, 2, 3, 4, 5, 6, 7, 8])),
    )
    .await;
}

#[tokio::test]
async fn recipient_list_type_error_is_actionable() {
    rejects_target("amesh_ask_many", "to_peers", json!("worker"), None).await;
}

#[tokio::test]
async fn target_arguments_keep_defaults_and_valid_values() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "codex", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    for value in [None, Some(Value::Null), Some(json!("worker"))] {
        let mut args = json!({"from_peer":"boss", "title":"job", "watch":false});
        if let Some(value) = value {
            args["assigned_peer"] = value;
        }
        let (status, body) = mcp_tool(app.clone(), "amesh_job_create", args.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_ne!(body["result"]["isError"], true, "{body}");
        let job: Value =
            serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(job["assigned_peer"], args["assigned_peer"]);
    }
    for value in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!(["worker"])),
    ] {
        let mut args = json!({"from_peer":"boss", "text":"q", "watch":false});
        if let Some(value) = value {
            args["to_peers"] = value;
        }
        let before = inner.lock().await.asks.len();
        let (status, body) = mcp_tool(app.clone(), "amesh_ask_many", args.clone()).await;
        if args["to_peers"] == json!(["worker"]) {
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_ne!(body["result"]["isError"], true, "{body}");
            assert_eq!(inner.lock().await.asks.len(), before + 1);
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["error"], "to_peers required");
            assert_eq!(inner.lock().await.asks.len(), before);
        }
    }
}

#[tokio::test]
async fn optional_scalar_values_keep_their_meaning() {
    for args in [json!({}), json!({"value":null})] {
        assert_eq!(mcp_bool(&args, "value").unwrap(), None);
        assert_eq!(mcp_u64(&args, "value").unwrap(), None);
    }
    for value in [false, true] {
        assert_eq!(
            mcp_bool(&json!({"value":value}), "value").unwrap(),
            Some(value)
        );
    }
    for value in [0, u64::MAX] {
        assert_eq!(
            mcp_u64(&json!({"value":value}), "value").unwrap(),
            Some(value)
        );
    }
    for value in [json!("false"), json!(0), json!([]), json!({})] {
        assert_eq!(
            mcp_bool(&json!({"value":value}), "value").unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }
    for value in [json!("5"), json!(-1), json!(1.5), json!(false), json!([])] {
        assert_eq!(
            mcp_u64(&json!({"value":value}), "value").unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "pi", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let (status, body) = mcp_tool(
        app,
        "amesh_ask",
        json!({
            "from_peer":"boss", "peer_name":"worker", "query":"q",
            "watch":null, "deadline_seconds":null, "cross_circle":null
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = tool_json(&body);
    assert!(
        inner.lock().await.asks[result["correlation_id"].as_str().unwrap()]
            .watch
            .is_some()
    );
}

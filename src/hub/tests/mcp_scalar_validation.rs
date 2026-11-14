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
    assert_eq!(status, StatusCode::BAD_REQUEST, "{tool}/{field}: {body}");
    assert!(body.to_string().contains(field), "{body}");
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

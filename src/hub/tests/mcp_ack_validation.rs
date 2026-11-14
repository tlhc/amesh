use super::*;

async fn rejected_ack_keeps_ask(mut args: Value, field: &str) {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "pi", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer":"boss", "to_peer":"worker", "text":"question"}),
    )
    .await;
    let cid = ask["correlation_id"].as_str().unwrap();
    if args.is_array() {
        args[0] = json!(cid);
    } else if args.is_object() && args.get("correlation_id").is_none() {
        args["correlation_id"] = json!(cid);
    }
    let (inbox, events) = {
        let hub = inner.lock().await;
        (hub.inbox.clone(), hub.events.clone())
    };
    let (status, body) = mcp_tool(app.clone(), "amesh_ack", args).await;
    {
        let hub = inner.lock().await;
        let row = &hub.asks[cid];
        assert!(row.open, "invalid {field} closed the ask: {body}");
        assert_eq!(row.reply, None);
        assert!(!row.failed);
        assert_eq!(hub.inbox, inbox, "invalid ACK must not enqueue delivery");
        assert_eq!(hub.events, events, "invalid ACK must not emit events");
    }
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains(field), "{body}");
    if field == "result" {
        for key in ["correlation_id", "message", "failed", "from_peer"] {
            assert!(body.to_string().contains(key), "{body}");
        }
    }
    let reply = "full answer with unicode: 回复\nsecond line";
    let (status, body) = mcp_tool(
        app.clone(),
        "amesh_ack",
        json!({"correlation_id":cid, "message":reply, "from_peer":"worker"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hub = inner.lock().await;
    assert!(!hub.asks[cid].open);
    assert_eq!(hub.asks[cid].reply.as_deref(), Some(reply));
    assert!(hub
        .inbox
        .records("boss")
        .iter()
        .any(|row| row.event()["message"] == reply));
}

#[tokio::test]
async fn mcp_ack_rejects_result_before_closing() {
    rejected_ack_keeps_ask(json!({"result":"answer"}), "result").await;
}
#[tokio::test]
async fn mcp_ack_rejects_numeric_message_before_closing() {
    rejected_ack_keeps_ask(json!({"message":123}), "message").await;
}
#[tokio::test]
async fn mcp_ack_rejects_string_failed_before_closing() {
    rejected_ack_keeps_ask(json!({"failed":"true"}), "failed").await;
}
#[tokio::test]
async fn mcp_ack_rejects_numeric_id_before_closing() {
    rejected_ack_keeps_ask(json!({"correlation_id":123}), "correlation_id").await;
}
#[tokio::test]
async fn mcp_ack_rejects_numeric_sender_before_closing() {
    rejected_ack_keeps_ask(json!({"from_peer":123}), "from_peer").await;
}

#[tokio::test]
async fn mcp_ack_preserves_bare_and_conflict_contracts() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "worker", "pi", "one").await;
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer":"boss", "to_peer":"worker", "text":"question"}),
    )
    .await;
    let cid = &ask["correlation_id"];
    let (status, body) = mcp_tool(app.clone(), "amesh_ack", json!({"correlation_id":cid})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!inner.lock().await.asks[cid.as_str().unwrap()].open);
    assert_eq!(inner.lock().await.asks[cid.as_str().unwrap()].reply, None);
    let (status, _) = mcp_tool(app.clone(), "amesh_ack", json!({"correlation_id":cid})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = mcp_tool(
        app,
        "amesh_ack",
        json!({"correlation_id":cid,"message":"followup"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn mcp_ack_rejects_positional_arguments_before_closing() {
    rejected_ack_keeps_ask(json!(["id", "answer", false, "worker"]), "object").await;
}

#[tokio::test]
async fn mcp_ack_accepts_injected_sender_and_delivers_reply() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "pi", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer":"boss", "to_peer":"worker", "text":"question"}),
    )
    .await;
    let cid = ask["correlation_id"].as_str().unwrap();
    let reply = "intact answer";
    let (status, body) = mcp_tool(
        app,
        "amesh_ack",
        json!({"correlation_id":cid, "message":reply, "from_peer":"worker"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hub = inner.lock().await;
    assert_eq!(hub.asks[cid].reply.as_deref(), Some(reply));
    assert!(hub
        .inbox
        .records("boss")
        .iter()
        .any(|row| row.event()["message"] == reply));
}

use super::*;

/* s sends; v and w never acknowledge and have no socket, so what reaches them waits in their
queues. w is also named "worker", so a request can name it twice */
async fn cap_hub() -> (Router, Arc<Mutex<Hub>>, TempState) {
    let (app, hub, state) = test_app_with_hub();
    register(app.clone(), "s", "pi", "default").await;
    register(app.clone(), "v", "pi", "default").await;
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "w", "backend": "pi", "circle": "default", "session_id": "sess-w"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    (app, hub, state)
}

/* n notifies from s to w, every one accepted; their ids in order */
async fn fill(app: &Router, n: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for i in 0..n {
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/notify",
            json!({"from_peer": "s", "to_peer": "w", "message": format!("m{i}")}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        ids.push(body["id"].as_str().unwrap().to_string());
    }
    ids
}

fn full_w(held: usize) -> String {
    format!("inbox full: w holds {held} undelivered records")
}

/* what a refused send leaves as it was: the event ring, the asks and every queue */
fn minted(hub: &Hub) -> (u64, usize, usize) {
    let queued = hub.inbox.iter().map(|(_, records)| records.len()).sum();
    (hub.event_seq, hub.asks.len(), queued)
}

/* one request at w once its queue holds INBOX_MAX records: refused, naming w and its depth,
with nothing minted */
async fn refused<F>(send: impl FnOnce(Router) -> F)
where
    F: std::future::Future<Output = (StatusCode, Value)>,
{
    let (app, hub, _state) = cap_hub().await;
    fill(&app, INBOX_MAX).await;
    let before = minted(&*hub.lock().await);
    let (st, body) = send(app).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"], full_w(INBOX_MAX), "{body}");
    assert_eq!(
        body["hint"], "nothing was sent; retry once the peer has read its queue",
        "{body}"
    );
    assert_eq!(
        minted(&*hub.lock().await),
        before,
        "a refused send mints nothing"
    );
}

#[tokio::test]
async fn a_notify_past_the_cap_is_refused_and_nothing_is_evicted() {
    let (app, hub, _state) = cap_hub().await;
    let ids = fill(&app, INBOX_MAX).await;
    let before = minted(&*hub.lock().await);
    let (st, body) = json_req(
        app,
        "POST",
        "/notify",
        json!({"from_peer": "s", "to_peer": "w", "message": "one more"}),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"], full_w(INBOX_MAX), "{body}");
    let hub = hub.lock().await;
    assert_eq!(minted(&hub), before, "a refused send mints nothing");
    let held: Vec<&str> = hub
        .inbox
        .records("w")
        .iter()
        .filter_map(|record| record.event()["id"].as_str())
        .collect();
    assert_eq!(held, ids, "every accepted record is still queued");
}

#[tokio::test]
async fn an_ask_to_a_full_peer_opens_nothing() {
    refused(|app| {
        json_req(
            app,
            "POST",
            "/ask",
            json!({"from_peer": "s", "to_peer": "w", "text": "q"}),
        )
    })
    .await;
}

#[tokio::test]
async fn ask_many_opens_nothing_when_one_target_is_full() {
    /* the full target comes last, so a fan-out that checked only as it went would have opened
    the ask to v already */
    refused(|app| {
        json_req(
            app,
            "POST",
            "/ask-many",
            json!({"from_peer": "s", "to_peers": ["v", "w"], "text": "q"}),
        )
    })
    .await;
}

#[tokio::test]
async fn ask_many_cut_short_by_a_full_target_names_the_ask_it_opened() {
    use std::future::Future;
    use std::task::{Context, Waker};

    /* the lock serves waiters in order: the fan-out's check passes with w one short of the cap,
    the notify queued behind it fills w, and the ask to v is open before w refuses */
    let (app, hub, _state) = cap_hub().await;
    fill(&app, INBOX_MAX - 1).await;
    let held = hub.lock().await;
    let fan_out = json_req(
        app.clone(),
        "POST",
        "/ask-many",
        json!({"from_peer": "s", "to_peers": ["v", "w"], "text": "q"}),
    );
    let last = json_req(
        app.clone(),
        "POST",
        "/notify",
        json!({"from_peer": "s", "to_peer": "w", "message": "last"}),
    );
    tokio::pin!(fan_out, last);
    let mut context = Context::from_waker(Waker::noop());
    assert!(fan_out.as_mut().poll(&mut context).is_pending());
    assert!(last.as_mut().poll(&mut context).is_pending());
    drop(held);
    let ((st, body), (sent, _)) = tokio::join!(fan_out, last);
    assert_eq!(sent, StatusCode::OK);
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(
        body["hint"], "the asks listed were opened; check them before retrying",
        "{body}"
    );
    let opened: Vec<String> = serde_json::from_value(body["asks"].clone()).unwrap();
    let hub = hub.lock().await;
    assert_eq!(opened.len(), 1, "{body}");
    assert_eq!(hub.asks[&opened[0]].to_peer, "v");
    assert_eq!(
        hub.batches[body["parent_id"].as_str().unwrap()],
        opened,
        "the batch can be read back"
    );
}

#[tokio::test]
async fn ask_many_counts_a_target_named_twice() {
    let (app, hub, _state) = cap_hub().await;
    fill(&app, INBOX_MAX - 1).await;
    let before = minted(&*hub.lock().await);
    let (st, body) = json_req(
        app,
        "POST",
        "/ask-many",
        json!({"from_peer": "s", "to_peers": ["w", "worker"], "text": "q"}),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"], full_w(INBOX_MAX - 1), "{body}");
    assert_eq!(
        minted(&*hub.lock().await),
        before,
        "a refused fan-out opens no ask"
    );
}

#[tokio::test]
async fn a_broadcast_skips_a_full_recipient_and_reaches_the_rest() {
    let (app, hub, _state) = cap_hub().await;
    fill(&app, INBOX_MAX).await;
    let (st, body) = json_req(
        app,
        "POST",
        "/broadcast",
        json!({"from_peer": "s", "message": "to all"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["sent_to"], json!(["v"]), "{body}");
    let failed = body["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{body}");
    assert_eq!(failed[0]["peer"], "w");
    assert_eq!(failed[0]["error"]["error"], full_w(INBOX_MAX));
    let hub = hub.lock().await;
    assert_eq!(
        hub.inbox.count("w"),
        INBOX_MAX,
        "the full recipient gets nothing"
    );
    assert!(hub.inbox.records("v").iter().any(|record| {
        record.event()["type"] == "broadcast" && record.event()["id"] == body["id"]
    }));
}

#[tokio::test]
async fn a_chat_line_for_a_full_peer_is_refused() {
    refused(|app| {
        json_req(
            app,
            "POST",
            "/events/chat",
            json!({"peer": "w", "role": "user", "text": "hi"}),
        )
    })
    .await;
}

#[tokio::test]
async fn a_session_notify_to_a_full_peer_is_refused() {
    refused(|app| {
        json_req(
            app,
            "POST",
            "/sessions/sess-w/controls/notify",
            json!({"text": "resume"}),
        )
    })
    .await;
    refused(|app| {
        json_req(
            app,
            "POST",
            "/sessions/resume",
            json!({"session_id": "sess-w"}),
        )
    })
    .await;
}

#[tokio::test]
async fn a_blocking_ask_to_a_full_peer_is_refused() {
    refused(|app| {
        json_req(
            app,
            "POST",
            "/questions/ask-blocking",
            json!({"prompt": "choose", "to_peer": "w", "from_peer": "s", "timeout_seconds": 0}),
        )
    })
    .await;
}

#[tokio::test]
async fn mcp_notify_to_a_full_peer_is_refused() {
    refused(|app| {
        mcp_tool(
            app,
            "amesh_notify_peer",
            json!({"from_peer": "s", "peer_name": "w", "message": "hi"}),
        )
    })
    .await;
}

#[tokio::test]
async fn mcp_ask_to_a_full_peer_is_refused() {
    refused(|app| {
        mcp_tool(
            app,
            "amesh_ask",
            json!({"from_peer": "s", "peer_name": "w", "query": "q"}),
        )
    })
    .await;
}

#[tokio::test]
async fn an_ack_reply_is_queued_past_the_cap() {
    let (app, hub, _state) = cap_hub().await;
    let (st, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "w", "to_peer": "v", "text": "q", "watch": false}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ask}");
    let cid = ask["correlation_id"].as_str().unwrap();
    fill(&app, INBOX_MAX).await;
    let (st, body) = json_req(
        app,
        "POST",
        "/ack",
        json!({"correlation_id": cid, "from_peer": "v", "message": "done"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let hub = hub.lock().await;
    let queue = hub.inbox.records("w");
    assert_eq!(
        queue.len(),
        INBOX_MAX + 1,
        "the asker's answer is never refused"
    );
    assert_eq!(queue[INBOX_MAX].event()["type"], "ack");
    assert_eq!(queue[INBOX_MAX].event()["correlation_id"], cid);
}

#[tokio::test]
async fn a_dropped_legacy_link_hands_back_everything_past_the_cap() {
    let (mut hub, path) = undelivered_hub();
    queue_inbox(
        &mut hub,
        "worker",
        (0..INBOX_MAX).map(|n| json!({"type": "notify", "id": format!("held-{n}")})),
        now_unix(),
    );
    let (tx, rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (1, tx));
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    let on_the_link = (0..10)
        .map(|n| test_outbound(json!({"type": "notify", "id": format!("link-{n}")})))
        .collect();
    close_connection(&state, "worker", 1, false, rx, on_the_link).await;
    let hub = state.inner.lock().await;
    let ids: Vec<&str> = hub
        .inbox
        .records("worker")
        .iter()
        .filter_map(|record| record.event()["id"].as_str())
        .collect();
    let want: Vec<String> = (0..INBOX_MAX)
        .map(|n| format!("held-{n}"))
        .chain((0..10).map(|n| format!("link-{n}")))
        .collect();
    assert_eq!(
        ids, want,
        "what the link held comes back whole, past the cap"
    );
}

#[tokio::test]
async fn an_acknowledging_peer_is_never_refused() {
    let (app, hub, _state) = cap_hub().await;
    {
        let mut hub = hub.lock().await;
        let _ = recv_peer(&mut hub, "r");
        /* away, with only the persisted promise to acknowledge */
        hub.sockets.remove("r");
        hub.recv_live.remove("r");
        queue_inbox(
            &mut hub,
            "r",
            (0..INBOX_MAX * 2).map(|n| json!({"type": "notify", "id": format!("old-{n}")})),
            now_unix(),
        );
    }
    for (route, body) in [
        (
            "/notify",
            json!({"from_peer": "s", "to_peer": "r", "message": "m"}),
        ),
        (
            "/ask",
            json!({"from_peer": "s", "to_peer": "r", "text": "q", "watch": false}),
        ),
        (
            "/ask-many",
            json!({"from_peer": "s", "to_peers": ["r", "r"], "text": "q", "watch": false}),
        ),
        (
            "/events/chat",
            json!({"peer": "r", "role": "user", "text": "hi"}),
        ),
        ("/broadcast", json!({"from_peer": "s", "message": "to all"})),
    ] {
        let (st, res) = json_req(app.clone(), "POST", route, body).await;
        assert_eq!(st, StatusCode::OK, "{route}: {res}");
        assert!(
            res["failed"].as_array().is_none_or(Vec::is_empty),
            "{route}: {res}"
        );
    }
    assert_eq!(hub.lock().await.inbox.count("r"), INBOX_MAX * 2 + 6);
}

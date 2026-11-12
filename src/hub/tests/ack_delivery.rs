use super::*;

#[tokio::test]
async fn the_hook_does_not_take_what_an_attached_acknowledger_is_still_owed() {
    let path = temp_state("pending");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let _rx = {
        let mut hub = state.inner.lock().await;
        let rx = recv_peer(&mut hub, "w");
        persist_then_deliver(
            &mut hub,
            "w",
            json!({"type": "notify", "message": "in flight"}),
            now_unix(),
        )
        .unwrap();
        rx
    };
    let (status, body) = json_req(
        router(state.clone()),
        "GET",
        "/asks/pending?peer_id=w",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["inbox"].as_array().map(Vec::len),
        Some(0),
        "a copy already on the socket must not also come back through the hook"
    );
    assert_eq!(
        state.inner.lock().await.inbox.records("w").len(),
        1,
        "the record stays until the peer's recv"
    );
    {
        let mut hub = state.inner.lock().await;
        hub.sockets.remove("w");
        hub.recv_live.remove("w");
    }
    let (status, body) = json_req(
        router(state.clone()),
        "GET",
        "/asks/pending?peer_id=w",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["inbox"].as_array().map(Vec::len),
        Some(1),
        "with nobody attached the hook is the delivery"
    );
    let (status, _) = json_req(
        router(state.clone()),
        "POST",
        "/asks/pending/settle",
        json!({"peer_id": "w", "lease": body["lease"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.inner.lock().await.inbox.count("w") == 0);
    let _ = fs::remove_file(&path);
}

async fn notify_worker(app: &Router, message: &str) {
    let (status, body) = json_req(
        app.clone(),
        "POST",
        "/notify",
        json!({"from_peer": "boss", "to_peer": "w", "message": message}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn read_pending(app: &Router) -> Value {
    let (status, body) = json_req(app.clone(), "GET", "/asks/pending?peer_id=w", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn messages(read: &Value) -> Vec<&str> {
    read["inbox"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["message"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_pending_read_leases_what_it_hands_out_until_the_reader_settles() {
    let (app, hub, path) = test_app_with_hub();
    register(app.clone(), "w", "codex", "default").await;
    notify_worker(&app, "first").await;
    notify_worker(&app, "second").await;
    let read = read_pending(&app).await;
    assert_eq!(messages(&read), ["first", "second"]);
    assert!(
        read["inbox"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["id"].as_str().is_some_and(|id| !id.is_empty())),
        "every record handed out carries its id: {read}"
    );
    let lease = read["lease"]
        .as_str()
        .expect("a read that hands records out names its lease");
    let again = read_pending(&app).await;
    assert!(
        messages(&again).is_empty(),
        "a second read inside the lease shows none of it: {again}"
    );
    assert!(again["lease"].is_null(), "{again}");
    notify_worker(&app, "third").await;
    let later = read_pending(&app).await;
    assert_eq!(
        messages(&later),
        ["third"],
        "a live lease holds back only its own records"
    );
    let (status, settled) = json_req(
        app.clone(),
        "POST",
        "/asks/pending/settle",
        json!({"peer_id": "w", "lease": lease}),
    )
    .await;
    assert_eq!((status, settled), (StatusCode::OK, json!({"ok": true})));
    assert_eq!(hub.lock().await.inbox.count("w"), 1, "only third is left");
    let (status, _) = json_req(
        app,
        "POST",
        "/asks/pending/settle",
        json!({"peer_id": "w", "lease": lease}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "settling twice is a no-op");
    let reopened = Hub::open(&path).unwrap();
    assert_eq!(
        reopened.inbox.records("w").len(),
        1,
        "the settled records are gone from the state file too"
    );
}

#[tokio::test]
async fn a_pending_read_drops_an_ask_closed_since_it_was_queued() {
    let (app, hub, _path) = test_app_with_hub();
    register(app.clone(), "w", "codex", "default").await;
    let (status, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "w", "text": "still needed?"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ask}");
    let (status, closed) = json_req(
        app.clone(),
        "POST",
        "/ack",
        json!({"correlation_id": ask["correlation_id"], "message": "answered elsewhere"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{closed}");
    assert_eq!(
        hub.lock().await.inbox.count("w"),
        1,
        "the ask is still queued"
    );
    let read = read_pending(&app).await;
    assert!(
        read["inbox"].as_array().unwrap().is_empty(),
        "a closed ask is not handed out: {read}"
    );
    assert_eq!(
        hub.lock().await.inbox.count("w"),
        0,
        "a closed ask leaves the queue instead of counting against it"
    );
}

#[tokio::test]
async fn a_hub_restart_hands_an_unsettled_lease_out_again() {
    let path = temp_state("lease-restart");
    let open = || {
        router(App {
            inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
            token: None,
            state_path: path.to_path_buf(),
        })
    };
    let first = {
        let app = open();
        register(app.clone(), "w", "codex", "default").await;
        notify_worker(&app, "kept").await;
        let read = read_pending(&app).await;
        assert!(read["lease"].is_string(), "{read}");
        read["inbox"].clone()
    };
    let read = read_pending(&open()).await;
    assert_eq!(messages(&read), ["kept"], "{read}");
    assert_eq!(
        read["inbox"], first,
        "a restart forgets the lease and hands out the same records, ids included"
    );
}

async fn settle_w(app: &Router, lease: &Value) -> StatusCode {
    let body = json!({"peer_id": "w", "lease": lease});
    json_req(app.clone(), "POST", "/asks/pending/settle", body)
        .await
        .0
}

#[tokio::test]
async fn a_failed_write_elsewhere_keeps_a_pending_read_lease() {
    let (app, hub, _path) = test_app_with_hub();
    register(app.clone(), "w", "codex", "default").await;
    notify_worker(&app, "held").await;
    let lease = read_pending(&app).await["lease"].clone();
    hub.lock().await.db.execute_batch("CREATE TEMP TRIGGER fail_write BEFORE INSERT ON inbox BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
    let refused = json!({"from_peer": "boss", "to_peer": "w", "message": "refused"});
    let (status, _) = json_req(app.clone(), "POST", "/notify", refused).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let read = read_pending(&app).await;
    assert!(
        messages(&read).is_empty(),
        "the lease still holds what it handed out: {read}"
    );
    assert_eq!(settle_w(&app, &lease).await, StatusCode::OK);
    assert_eq!(
        hub.lock().await.inbox.count("w"),
        0,
        "and its settle takes it"
    );
}

#[tokio::test]
async fn a_failed_settle_keeps_its_lease_for_the_retry() {
    let (app, hub, _path) = test_app_with_hub();
    register(app.clone(), "w", "codex", "default").await;
    notify_worker(&app, "held").await;
    let lease = read_pending(&app).await["lease"].clone();
    hub.lock().await.db.execute_batch("CREATE TEMP TRIGGER fail_write BEFORE DELETE ON inbox BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
    assert_eq!(
        settle_w(&app, &lease).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let read = read_pending(&app).await;
    assert!(
        messages(&read).is_empty(),
        "the records are back and still held for the reader: {read}"
    );
    assert_eq!(settle_w(&app, &lease).await, StatusCode::OK);
    assert_eq!(
        hub.lock().await.inbox.count("w"),
        0,
        "the retry settles them"
    );
}

#[tokio::test]
async fn a_failed_pending_read_holds_nothing_back() {
    let (app, hub, _path) = test_app_with_hub();
    register(app.clone(), "w", "codex", "default").await;
    {
        let mut hub = hub.lock().await;
        let kept = json!({"type": "notify", "id": "kept", "message": "kept"});
        let closed = json!({"type": "ask", "id": "stale", "correlation_id": "closed"});
        queue_inbox(&mut hub, "w", [kept, closed], now_unix());
        persist_ok(&mut hub).unwrap();
        hub.db.execute_batch("CREATE TEMP TRIGGER fail_write BEFORE DELETE ON inbox BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
    }
    let (status, _) = json_req(app.clone(), "GET", "/asks/pending?peer_id=w", json!({})).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let read = read_pending(&app).await;
    assert_eq!(
        messages(&read),
        ["kept"],
        "a read that failed handed nothing out, so it holds nothing back: {read}"
    );
}

#[tokio::test]
async fn a_closing_acknowledging_connection_hands_nothing_back() {
    let path = temp_state("recvclose");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let (mut rx, gen) = {
        let mut hub = state.inner.lock().await;
        let rx = recv_peer(&mut hub, "w");
        let gen = hub.conn_gen;
        persist_then_deliver(
            &mut hub,
            "w",
            json!({"type": "notify", "message": "m"}),
            now_unix(),
        )
        .unwrap();
        (rx, gen)
    };
    let copy = rx.try_recv().unwrap();
    assert_eq!(state.inner.lock().await.inbox.records("w").len(), 1);
    /* the write of that copy fails and the link dies: the record is already in the
    inbox, so handing the copy back must not make a second one */
    close_connection(&state, "w", gen, true, rx, vec![copy]).await;
    let hub = state.inner.lock().await;
    assert_eq!(
        hub.inbox.records("w").len(),
        1,
        "a closing acknowledging connection must not duplicate its records"
    );
    assert!(!hub.sockets.contains_key("w"));
    assert!(!hub.recv_live.contains("w"));
    assert!(
        hub.recv_known.contains("w"),
        "the promise outlives the connection"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn an_absent_acknowledging_peer_is_owed_records_it_can_name() {
    let path = temp_state("absent");
    let mut hub = Hub::open(&path).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    hub.sockets.remove("w");
    hub.recv_live.remove("w");
    persist_then_deliver(
        &mut hub,
        "w",
        json!({"type": "ask", "correlation_id": "ask-1", "text": "q"}),
        now_unix(),
    )
    .unwrap();
    let owed = &hub.inbox.records("w");
    assert_eq!(owed.len(), 1);
    assert!(
        owed[0].event()["id"]
            .as_str()
            .unwrap_or("")
            .starts_with("evt-"),
        "what an absent acknowledger is owed still needs an id it can recv: {owed:?}"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_first_time_acknowledger_gets_ids_on_its_backlog() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = temp_state("legacy");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let http = router(state.clone());
    let serve_app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    for id in ["p-worker", "p-boss"] {
        let _ = json_req(
            router(state.clone()),
            "POST",
            "/peers",
            json!({"name": id, "backend": "pi", "peer_id": id}),
        )
        .await;
    }
    /* queued while the peer was an old-style client with no socket: no id on disk */
    let (status, _) = json_req(
        http.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "p-boss", "to_peer": "p-worker", "text": "old queued ask"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let legacy = Hub::open(&path).unwrap();
    assert!(
        legacy.inbox.records("p-worker")[0].event()["id"].is_null(),
        "precondition: the backlog has no id"
    );

    let (mut w, mut r) = {
        let (socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        let (mut w, mut r) = socket.split();
        w.send(WsMsg::Text(
            r#"{"type":"connect","peer_id":"p-worker","recv":true}"#.into(),
        ))
        .await
        .unwrap();
        assert!(r
            .next()
            .await
            .unwrap()
            .unwrap()
            .to_string()
            .contains("connected"));
        (w, r)
    };
    let frame = tokio::time::timeout(Duration::from_secs(3), r.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let replayed: Value = serde_json::from_str(&frame.to_string()).unwrap();
    let id = replayed["id"].as_str().unwrap_or("").to_string();
    assert!(
        id.starts_with("evt-"),
        "the backlog is replayed with an id the peer can name: {replayed}"
    );
    assert_eq!(
        Hub::open(&path).unwrap().inbox.records("p-worker")[0].event()["id"].as_str(),
        Some(id.as_str()),
        "the id is on the record on disk, not only on the copy"
    );
    w.send(WsMsg::Text(
        json!({"type": "recv", "id": id}).to_string().into(),
    ))
    .await
    .unwrap();
    until(
        &state,
        |hub| hub.inbox.count("p-worker") == 0,
        "recv of a backlog record never drained it",
    )
    .await;
    let _ = fs::remove_file(&path);
}

#[test]
fn an_acknowledging_peer_is_owed_every_event_until_it_says_recv() {
    let path = temp_state("recv");
    let mut hub = Hub::open(&path).unwrap();
    let mut rx = recv_peer(&mut hub, "w");
    persist_then_deliver(
        &mut hub,
        "w",
        json!({"type": "ack", "message": "reply"}),
        now_unix(),
    )
    .unwrap();
    let owed = hub.inbox.records("w").to_vec();
    assert_eq!(
        owed.len(),
        1,
        "the record is written before anything goes down the socket"
    );
    let id = owed[0].event()["id"].as_str().unwrap().to_string();
    assert!(
        id.starts_with("evt-"),
        "an event without an id gets one the peer can name"
    );
    let copy = rx.try_recv().unwrap();
    assert_eq!(
        copy.event()["id"],
        id,
        "what goes down the socket is a copy of the same record"
    );
    assert_eq!(
        Hub::open(&path).unwrap().inbox.records("w").len(),
        1,
        "the record is on disk before the send"
    );
    assert!(acknowledge_event(&mut hub, "w", &id));
    assert!(hub.inbox.count("w") == 0, "recv takes the record away");
    assert!(
        !acknowledge_event(&mut hub, "w", &id),
        "a repeated recv is a no-op"
    );
    assert!(
        !acknowledge_event(&mut hub, "w", "evt-nope"),
        "an unknown id is a no-op"
    );
    persist(&mut hub).unwrap();
    assert!(Hub::open(&path).unwrap().inbox.count("w") == 0);
    let _ = fs::remove_file(&path);
}

#[test]
fn what_an_acknowledging_peer_is_owed_is_never_evicted() {
    let path = temp_state("owed");
    let mut hub = Hub::open(&path).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    /* the peer drops off: no socket, no live promise, only the persisted one */
    hub.sockets.remove("w");
    hub.recv_live.remove("w");
    let chatter = |n: usize| (0..n).map(|i| json!({"type": "notify", "id": format!("n{i}")}));
    queue_inbox(&mut hub, "w", chatter(INBOX_MAX + 10), now_unix());
    assert_eq!(
        hub.inbox.records("w").len(),
        INBOX_MAX + 10,
        "no cap while the peer is away"
    );
    persist(&mut hub).unwrap();
    let mut reopened = Hub::open(&path).unwrap();
    assert!(
        reopened.recv_known.contains("w"),
        "the promise survives a restart"
    );
    assert_eq!(reopened.inbox.records("w").len(), INBOX_MAX + 10);
    queue_inbox(&mut reopened, "w", chatter(10), now_unix());
    assert_eq!(
        reopened.inbox.records("w").len(),
        INBOX_MAX + 20,
        "no cap after a restart either"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_reconnecting_acknowledging_client_gets_copies_not_duplicates() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = temp_state("replay");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let http = router(state.clone());
    let serve_app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    for id in ["p-worker", "p-boss"] {
        let _ = json_req(
            router(state.clone()),
            "POST",
            "/peers",
            json!({"name": id, "backend": "pi", "peer_id": id}),
        )
        .await;
    }
    let connect = || async {
        let (socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        let (mut w, mut r) = socket.split();
        w.send(WsMsg::Text(
            r#"{"type":"connect","peer_id":"p-worker","recv":true}"#.into(),
        ))
        .await
        .unwrap();
        assert!(r
            .next()
            .await
            .unwrap()
            .unwrap()
            .to_string()
            .contains("connected"));
        (w, r)
    };
    let owed = |state: &App| {
        let state = state.clone();
        async move {
            let hub = state.inner.lock().await;
            hub.inbox.count("p-worker")
        }
    };
    async fn read_ids<S>(r: &mut S, n: usize) -> Vec<String>
    where
        S: futures_util::Stream<
                Item = Result<
                    tokio_tungstenite::tungstenite::Message,
                    tokio_tungstenite::tungstenite::Error,
                >,
            > + Unpin,
    {
        let mut ids = Vec::new();
        for _ in 0..n {
            let frame =
                tokio::time::timeout(Duration::from_secs(3), futures_util::StreamExt::next(r))
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
            let v: Value = serde_json::from_str(&frame.to_string()).unwrap();
            ids.push(v["id"].as_str().unwrap().to_string());
        }
        ids
    }

    let (w1, mut r1) = connect().await;
    for text in ["one", "two"] {
        let (status, _) = json_req(
            http.clone(),
            "POST",
            "/notify",
            json!({"from_peer": "p-boss", "to_peer": "p-worker", "message": text}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let first = read_ids(&mut r1, 2).await;
    assert_eq!(
        owed(&state).await,
        2,
        "taking the frames is not the same as acknowledging them"
    );
    drop(r1);
    drop(w1);
    until(
        &state,
        |hub| !hub.sockets.contains_key("p-worker"),
        "socket never closed",
    )
    .await;
    assert_eq!(
        owed(&state).await,
        2,
        "a dropped connection must not duplicate what was owed"
    );

    let (mut w2, mut r2) = connect().await;
    let replayed = read_ids(&mut r2, 2).await;
    assert_eq!(
        replayed, first,
        "the next connection gets copies of the same records, in order"
    );
    assert_eq!(owed(&state).await, 2, "replaying is not acknowledging");
    for id in &first {
        w2.send(WsMsg::Text(
            json!({"type": "recv", "id": id}).to_string().into(),
        ))
        .await
        .unwrap();
    }
    until(
        &state,
        |hub| hub.inbox.count("p-worker") == 0,
        "recv never drained the inbox",
    )
    .await;
    assert!(
        Hub::open(&path).unwrap().inbox.count("p-worker") == 0,
        "acknowledged records leave the disk too"
    );
    drop(r2);
    drop(w2);
    until(
        &state,
        |hub| !hub.sockets.contains_key("p-worker"),
        "socket never closed",
    )
    .await;
    let (_w3, mut r3) = connect().await;
    let nothing = tokio::time::timeout(Duration::from_millis(500), r3.next()).await;
    assert!(
        nothing.is_err(),
        "nothing is owed, so nothing is replayed: {nothing:?}"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_dropped_link_owes_its_events_to_the_inbox_not_to_itself() {
    let path = temp_state("drop");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let gen = {
        let mut hub = state.inner.lock().await;
        hub.peers.insert(
            "worker".into(),
            Peer {
                peer_id: "worker".into(),
                name: "worker".into(),
                path: "/tmp".into(),
                backend: "pi".into(),
                circle: "default".into(),
                status: "online".into(),
                description: String::new(),
                session_id: String::new(),
                last_seen: now_unix(),
            },
        );
        hub.conn_gen += 1;
        let gen = hub.conn_gen;
        hub.sockets.insert("worker".into(), (gen, tx.clone()));
        gen
    };
    /* one event the socket task had taken off the channel and failed to write, and one
    still sitting in the channel when the link died: both are owed to the peer */
    let taken = vec![json!({"type": "notify", "message": "owed-taken"})];
    tx.send(test_outbound(
        json!({"type": "notify", "message": "owed-buffered"}),
    ))
    .unwrap();
    drop(tx);
    close_connection(
        &state,
        "worker",
        gen,
        false,
        rx,
        taken.into_iter().map(test_outbound).collect(),
    )
    .await;
    let hub = state.inner.lock().await;
    let held: Vec<String> = hub
        .inbox
        .records("worker")
        .iter()
        .map(|event| {
            event.event()["message"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        held,
        vec!["owed-taken".to_string(), "owed-buffered".to_string()],
        "a closing connection must not post what it owes back into its own channel"
    );
    assert!(!hub.sockets.contains_key("worker"));
    assert_eq!(hub.peers["worker"].status, "offline");
    drop(hub);
    assert!(Hub::open(&path).unwrap().inbox.count("worker") > 0);
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn second_connection_displaces_the_incumbent() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = temp_state("displace");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let http = router(state.clone());
    let serve_app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    let _ = json_req(
        http,
        "POST",
        "/peers",
        json!({"name": "worker", "backend": "pi", "peer_id": "p-worker"}),
    )
    .await;

    let (first, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut first_w, mut first_r) = first.split();
    first_w
        .send(WsMsg::Text(
            r#"{"type":"connect","peer_id":"p-worker"}"#.into(),
        ))
        .await
        .unwrap();
    assert!(first_r
        .next()
        .await
        .unwrap()
        .unwrap()
        .to_string()
        .contains("connected"));

    let (second, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut second_w, mut second_r) = second.split();
    second_w
        .send(WsMsg::Text(
            r#"{"type":"connect","peer_id":"p-worker"}"#.into(),
        ))
        .await
        .unwrap();
    assert!(second_r
        .next()
        .await
        .unwrap()
        .unwrap()
        .to_string()
        .contains("connected"));

    let notice = first_r.next().await.unwrap().unwrap().to_string();
    assert!(
        notice.contains(DISPLACED),
        "incumbent must learn it lost the peer, got: {notice}"
    );
}

async fn socket_hub() -> (App, TempState, String) {
    socket_hub_on(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap()).await
}

async fn socket_hub_on(listener: tokio::net::TcpListener) -> (App, TempState, String) {
    let path = temp_state("recv-regression");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    register(router(state.clone()), "worker", "pi", "default").await;
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, path, url)
}

type PeerSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_worker(url: &str, recv: bool) -> PeerSocket {
    let (socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send_connect(socket, recv).await
}

async fn send_connect(mut socket: PeerSocket, recv: bool) -> PeerSocket {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    socket
        .send(WsMsg::Text(
            json!({"type": "connect", "peer_id": "worker", "recv": recv})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    socket
}

async fn socket_frame(socket: &mut PeerSocket) -> Value {
    use futures_util::StreamExt;

    let frame = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("the expected socket frame did not arrive")
        .unwrap()
        .unwrap();
    serde_json::from_str(&frame.to_string()).unwrap()
}

/* AMESH_WS_WRITE_SECS is process-wide and the unit tests share one process, so a case that
shortens the write deadline runs in a child; the parent only checks that the child ran and
passed */
fn ran_in_child(test: &str, deadline_secs: &str) -> bool {
    if std::env::var_os("AMESH_WRITE_DEADLINE_CHILD").is_some() {
        return false;
    }
    let filter = format!("{}::{test}", module_path!().split_once("::").unwrap().1);
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &filter, "--nocapture"])
        .env("AMESH_WRITE_DEADLINE_CHILD", "1")
        .env("AMESH_WS_WRITE_SECS", deadline_secs)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.contains("test result: ok. 1 passed;"),
        "{test} must run and pass with a {deadline_secs} s write deadline:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    true
}

/* a tuned loopback absorbs tens of MiB from a peer that stops reading; with small buffers at
both ends the hub's writes block after a few frames */
const TIGHT_BUFFER: u32 = 64 * 1024;

async fn tight_socket_hub() -> (App, TempState, String) {
    let listener = tokio::net::TcpSocket::new_v4().unwrap();
    /* an accepted socket takes the listener's send buffer */
    listener.set_send_buffer_size(TIGHT_BUFFER).unwrap();
    listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    socket_hub_on(listener.listen(16).unwrap()).await
}

/* connects with a tight receive buffer and reads nothing */
async fn connect_tight(url: &str, recv: bool) -> PeerSocket {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(TIGHT_BUFFER).unwrap();
    let addr = url.trim_start_matches("ws://").trim_end_matches("/ws");
    let stream = socket.connect(addr.parse().unwrap()).await.unwrap();
    let (socket, _) =
        tokio_tungstenite::client_async(url, tokio_tungstenite::MaybeTlsStream::Plain(stream))
            .await
            .unwrap();
    send_connect(socket, recv).await
}

/* takes the connected frame and nothing after it */
async fn connect_stalled_worker(url: &str, recv: bool) -> PeerSocket {
    let mut socket = connect_tight(url, recv).await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    socket
}

/* far more than the tight buffers hold, and fewer than the INBOX_MAX a legacy inbox keeps,
so nothing handed back is evicted */
async fn flood(state: &App) -> Vec<Value> {
    let mut hub = state.inner.lock().await;
    (0..40)
        .map(|seq| {
            let event = json!({"type": "notify", "seq": seq, "message": "x".repeat(128 * 1024)});
            persist_then_deliver(&mut hub, "worker", event.clone(), now_unix()).unwrap();
            event
        })
        .collect()
}

/* the child's one second deadline ends the connection well inside this bound */
async fn cut_off(state: &App, failure: &str) {
    let closed = tokio::time::timeout(
        Duration::from_secs(10),
        until(
            state,
            |hub| {
                !hub.sockets.contains_key("worker")
                    && hub
                        .peers
                        .get("worker")
                        .is_some_and(|peer| peer.status == "offline")
            },
            failure,
        ),
    )
    .await;
    assert!(closed.is_ok(), "{failure}");
}

fn events_of(records: &[QueuedRecord]) -> Vec<Value> {
    records
        .iter()
        .map(|record| record.event().clone())
        .collect()
}

async fn owed_events(state: &App) -> Vec<Value> {
    events_of(state.inner.lock().await.inbox.records("worker"))
}

fn disk_events(hub: &Hub) -> Vec<Value> {
    let disk = read_snapshot(&hub.db).unwrap();
    events_of(disk.inbox.get("worker").map_or(&[], Vec::as_slice))
}

/* the payloads are large, so a mismatch names the records by seq */
fn seqs(events: &[Value]) -> Vec<Option<u64>> {
    events.iter().map(|event| event["seq"].as_u64()).collect()
}

#[tokio::test]
async fn an_acknowledger_that_stops_reading_is_cut_off_and_replayed_on_reconnect() {
    if ran_in_child(
        "an_acknowledger_that_stops_reading_is_cut_off_and_replayed_on_reconnect",
        "1",
    ) {
        return;
    }
    let (state, _path, url) = tight_socket_hub().await;
    let stalled = connect_stalled_worker(&url, true).await;
    let sent = flood(&state).await;
    let owed = owed_events(&state).await;
    assert_eq!(seqs(&owed), seqs(&sent));
    cut_off(&state, "the hub kept a connection that stopped reading").await;
    let left = owed_events(&state).await;
    assert!(
        left == owed,
        "copies the peer never took stay owed: {:?}",
        seqs(&left)
    );
    drop(stalled);
    let stalled = connect_stalled_worker(&url, true).await;
    cut_off(
        &state,
        "the hub kept replaying to a connection that stopped reading",
    )
    .await;
    let left = owed_events(&state).await;
    assert!(
        left == owed,
        "an unread replay stays owed: {:?}",
        seqs(&left)
    );
    drop(stalled);
    let mut socket = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    for (seq, record) in owed.iter().enumerate() {
        let replay = socket_frame(&mut socket).await;
        assert!(
            replay == *record,
            "record {seq} must replay in order, got seq {}",
            replay["seq"]
        );
    }
}

#[tokio::test]
async fn a_legacy_peer_that_stops_reading_gets_its_unwritten_frames_back() {
    use futures_util::StreamExt;

    if ran_in_child(
        "a_legacy_peer_that_stops_reading_gets_its_unwritten_frames_back",
        "1",
    ) {
        return;
    }
    let (state, _path, url) = tight_socket_hub().await;
    let mut stalled = connect_stalled_worker(&url, false).await;
    let sent = flood(&state).await;
    cut_off(&state, "the hub kept a connection that stopped reading").await;
    let returned = owed_events(&state).await;
    assert!(
        !returned.is_empty(),
        "the frames the hub could not write come back to the inbox"
    );
    /* what the hub did write is still in the socket buffers: the peer takes those frames, then
    the end of the connection */
    let mut taken = Vec::new();
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(Duration::from_secs(3), stalled.next()).await
    {
        taken.push(serde_json::from_str::<Value>(&frame.to_string()).unwrap());
    }
    taken.extend(returned);
    assert!(
        taken == sent,
        "every frame is either written or handed back, once and in order: {:?}",
        seqs(&taken)
    );
}

/* the acknowledging link that takes the name over is handed what the legacy link had not
written; those records were never in the inbox, so they must land there before the new link
can lose them by stalling too. The longer deadline lets the takeover happen while the legacy
link is still stuck */
#[tokio::test]
async fn a_legacy_link_taken_over_by_an_acknowledger_that_stops_reading_loses_nothing() {
    use futures_util::StreamExt;

    if ran_in_child(
        "a_legacy_link_taken_over_by_an_acknowledger_that_stops_reading_loses_nothing",
        "3",
    ) {
        return;
    }
    let (state, _path, url) = tight_socket_hub().await;
    let mut legacy = connect_stalled_worker(&url, false).await;
    let sent = flood(&state).await;
    let taker = connect_stalled_worker(&url, true).await;
    /* the handover is written down while the acknowledging link still holds the name: that
    link's own cut persists the hub again and would hide a handover that skipped its write */
    let (held, on_disk, taker_live) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let hub = state.inner.lock().await;
                if !hub.inbox.records("worker").is_empty() {
                    return (
                        events_of(hub.inbox.records("worker")),
                        disk_events(&hub),
                        hub.sockets.contains_key("worker"),
                    );
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the frames the legacy link could not write never reached the inbox");
    assert!(
        taker_live,
        "the acknowledging link was cut before the handover could be checked"
    );
    assert!(
        held == on_disk,
        "the handover is on disk: memory {:?}, disk {:?}",
        seqs(&held),
        seqs(&on_disk)
    );
    cut_off(
        &state,
        "the hub kept the acknowledging link that stopped reading",
    )
    .await;
    drop(taker);
    let mut taken = Vec::new();
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(Duration::from_secs(3), legacy.next()).await
    {
        let event = serde_json::from_str::<Value>(&frame.to_string()).unwrap();
        if event.get("seq").is_some() {
            taken.push(event);
        }
    }
    let mut socket = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    while taken.len() < sent.len() {
        /* the inbox names a record by an id so the peer can acknowledge it */
        let mut replay = socket_frame(&mut socket).await;
        replay.as_object_mut().unwrap().remove("id");
        taken.push(replay);
    }
    assert!(
        taken == sent,
        "every accepted record reaches the peer once and in order: {:?}",
        seqs(&taken)
    );
}

/* the greeting carries the session id, so a long one is a frame the tight buffers cannot
take */
#[tokio::test]
async fn a_peer_whose_greeting_cannot_be_written_is_cut_off() {
    if ran_in_child("a_peer_whose_greeting_cannot_be_written_is_cut_off", "1") {
        return;
    }
    let (state, _path, url) = tight_socket_hub().await;
    if let Some(peer) = state.inner.lock().await.peers.get_mut("worker") {
        peer.session_id = "s".repeat(1024 * 1024);
    }
    let silent = connect_tight(&url, true).await;
    cut_off(
        &state,
        "the hub kept a connection whose greeting it could not write",
    )
    .await;
    drop(silent);
}

/* a legacy link takes its backlog out of the inbox when it connects, so a greeting it cannot
write must hand that backlog back */
#[tokio::test]
async fn a_legacy_peer_whose_greeting_cannot_be_written_keeps_its_backlog() {
    if ran_in_child(
        "a_legacy_peer_whose_greeting_cannot_be_written_keeps_its_backlog",
        "1",
    ) {
        return;
    }
    let (state, _path, url) = tight_socket_hub().await;
    let waiting = json!({"type": "notify", "seq": 0, "message": "waiting"});
    {
        let mut hub = state.inner.lock().await;
        persist_then_deliver(&mut hub, "worker", waiting.clone(), now_unix()).unwrap();
        if let Some(peer) = hub.peers.get_mut("worker") {
            peer.session_id = "s".repeat(1024 * 1024);
        }
    }
    let silent = connect_tight(&url, false).await;
    cut_off(
        &state,
        "the hub kept a connection whose greeting it could not write",
    )
    .await;
    drop(silent);
    let hub = state.inner.lock().await;
    let backlog = vec![waiting];
    assert_eq!(events_of(hub.inbox.records("worker")), backlog);
    assert_eq!(
        disk_events(&hub),
        backlog,
        "the returned backlog is on disk"
    );
}

#[tokio::test]
async fn a_legacy_connection_is_refused_without_losing_an_acknowledgers_backlog() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let (state, path, url) = socket_hub().await;
    let mut socket = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    drop(socket);
    until(
        &state,
        |hub| !hub.sockets.contains_key("worker"),
        "first socket stayed attached",
    )
    .await;
    {
        let mut hub = state.inner.lock().await;
        for i in 0..3 {
            persist_then_deliver(
                &mut hub,
                "worker",
                json!({"type": "notify", "message": format!("owed-{i}")}),
                now_unix(),
            )
            .unwrap();
        }
        *hub = Hub::open(&path).unwrap();
        assert!(hub.recv_known.contains("worker"));
    }
    let mut legacy = connect_worker(&url, false).await;
    assert_eq!(
        socket_frame(&mut legacy).await["error"],
        "this peer requires delivery acknowledgements; connect with recv:true"
    );
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), legacy.next())
            .await
            .expect("the refused legacy connection must close"),
        Some(Ok(WsMsg::Close(_))) | None
    ));
    drop(legacy);
    let expected = {
        let mut hub = state.inner.lock().await;
        assert!(!hub.sockets.contains_key("worker"));
        for i in 3..23 {
            persist_then_deliver(
                &mut hub,
                "worker",
                json!({"type": "notify", "message": format!("owed-{i}")}),
                now_unix(),
            )
            .unwrap();
        }
        hub.inbox
            .records("worker")
            .iter()
            .map(|record| record.event().clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(expected.len(), 23);
    assert_eq!(Hub::open(&path).unwrap().inbox.count("worker"), 23);
    let mut socket = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    for (i, record) in expected.iter().enumerate() {
        let replay = socket_frame(&mut socket).await;
        assert_eq!(replay, *record, "record {i} must replay once in order");
        assert!(replay["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert_eq!(replay["message"], format!("owed-{i}"));
        socket
            .send(WsMsg::Text(
                json!({"type": "recv", "id": replay["id"]})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    }
    until(
        &state,
        |hub| hub.inbox.count("worker") == 0,
        "recv did not retire every replayed record",
    )
    .await;
    assert_eq!(Hub::open(&path).unwrap().inbox.count("worker"), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_refused_legacy_connection_keeps_the_acknowledging_socket_attached() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let (state, _path, url) = socket_hub().await;
    let mut socket = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    let gen = state.inner.lock().await.sockets["worker"].0;
    let mut legacy = connect_worker(&url, false).await;
    assert_eq!(
        socket_frame(&mut legacy).await["error"],
        "this peer requires delivery acknowledgements; connect with recv:true"
    );
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), legacy.next())
            .await
            .expect("the refused legacy connection must close"),
        Some(Ok(WsMsg::Close(_))) | None
    ));
    {
        let mut hub = state.inner.lock().await;
        assert_eq!(hub.sockets["worker"].0, gen);
        assert!(hub.recv_live.contains("worker"));
        persist_then_deliver(
            &mut hub,
            "worker",
            json!({"type": "notify", "message": "still attached"}),
            now_unix(),
        )
        .unwrap();
    }
    let frame = socket_frame(&mut socket).await;
    assert_eq!(frame["message"], "still attached");
    socket
        .send(WsMsg::Text(
            json!({"type": "recv", "id": frame["id"]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    until(
        &state,
        |hub| hub.inbox.count("worker") == 0,
        "the incumbent could not acknowledge its record",
    )
    .await;
}

#[test]
fn a_legacy_socket_keeps_new_public_records_owed_to_an_acknowledger() {
    let (mut hub, path) = undelivered_hub();
    let mut rx = recv_peer(&mut hub, "worker");
    hub.recv_live.remove("worker");
    persist_then_deliver(
        &mut hub,
        "worker",
        json!({"type": "notify", "message": "live"}),
        now_unix(),
    )
    .unwrap();
    let copy = rx.try_recv().unwrap();
    assert_eq!(
        hub.inbox.count("worker"),
        1,
        "a legacy socket carries only a copy for a recv-promised peer"
    );
    assert_eq!(
        hub.inbox.records("worker")[0].event()["id"],
        copy.event()["id"]
    );
    assert!(copy.event()["id"].as_str().unwrap().starts_with("evt-"));
    assert_eq!(Hub::open(&path).unwrap().inbox.count("worker"), 1);
}

#[test]
fn a_legacy_socket_keeps_new_internal_records_owed_to_an_acknowledger() {
    let (mut hub, path) = undelivered_hub();
    let mut rx = recv_peer(&mut hub, "worker");
    hub.recv_live.remove("worker");
    let queued = queue_replies(
        &mut hub,
        vec![json!({"type": "notify", "to_peer": "worker", "message": "live"})],
        now_unix(),
    );
    persist_ok(&mut hub).unwrap();
    deliver_queued(&mut hub, queued);
    let copy = rx.try_recv().unwrap();
    assert_eq!(
        hub.inbox.count("worker"),
        1,
        "a legacy socket must not retire an internal record without recv"
    );
    assert_eq!(
        hub.inbox.records("worker")[0].event()["id"],
        copy.event()["id"]
    );
    assert_eq!(Hub::open(&path).unwrap().inbox.count("worker"), 1);
}

#[tokio::test]
async fn a_closing_legacy_connection_does_not_duplicate_its_durable_copies() {
    let (mut hub, path) = undelivered_hub();
    let rx = recv_peer(&mut hub, "worker");
    let gen = hub.conn_gen;
    persist_then_deliver(
        &mut hub,
        "worker",
        json!({"type": "notify", "message": "live"}),
        now_unix(),
    )
    .unwrap();
    let mut rx = rx;
    let copy = rx.try_recv().unwrap();
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    close_connection(&state, "worker", gen, false, rx, vec![copy]).await;
    assert_eq!(
        state.inner.lock().await.inbox.count("worker"),
        1,
        "a returned legacy copy is already owed"
    );
    assert_eq!(Hub::open(&path).unwrap().inbox.count("worker"), 1);
}

#[tokio::test]
async fn a_failed_connect_persist_reports_an_error_and_closes() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let (state, _path, url) = socket_hub().await;
    {
        let mut hub = state.inner.lock().await;
        persist_then_deliver(
            &mut hub,
            "worker",
            json!({"type": "notify", "message": "older"}),
            now_unix(),
        )
        .unwrap();
        hub.db.execute_batch("CREATE TEMP TRIGGER fail_connect BEFORE INSERT ON recv_peers BEGIN SELECT RAISE(ABORT, 'forced connect failure'); END;").unwrap();
    }
    let mut socket = connect_worker(&url, true).await;
    let error = socket_frame(&mut socket).await;
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|text| text.contains("forced connect failure")),
        "failed ownership persist must be an error, not connected: {error}"
    );
    let closed = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    assert!(
        matches!(closed, Some(Ok(WsMsg::Close(_)))),
        "failed ownership persist must close the socket: {closed:?}"
    );
    {
        let mut hub = state.inner.lock().await;
        assert!(
            !hub.sockets.contains_key("worker"),
            "failed connection must not own the socket"
        );
        assert!(!hub.recv_live.contains("worker"));
        assert!(!hub.recv_known.contains("worker"));
        hub.db.execute_batch("DROP TRIGGER fail_connect").unwrap();
        persist_then_deliver(
            &mut hub,
            "worker",
            json!({"type": "notify", "message": "newer"}),
            now_unix(),
        )
        .unwrap();
    }
    let mut socket = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    let older = socket_frame(&mut socket).await;
    let newer = socket_frame(&mut socket).await;
    assert_eq!(older["message"], "older");
    assert_eq!(newer["message"], "newer");
    assert!(older["id"].is_string() && newer["id"].is_string());
    assert_ne!(older["id"], newer["id"]);
}

#[tokio::test]
async fn a_failed_replacement_keeps_the_incumbent_registered() {
    let (state, _path, url) = socket_hub().await;
    let mut incumbent = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut incumbent).await["type"], "connected");
    let gen = {
        let hub = state.inner.lock().await;
        hub.db.execute_batch("CREATE TEMP TRIGGER fail_connect BEFORE INSERT ON recv_peers BEGIN SELECT RAISE(ABORT, 'forced connect failure'); END;").unwrap();
        hub.sockets["worker"].0
    };
    let mut failed = connect_worker(&url, true).await;
    let _ = socket_frame(&mut failed).await;
    {
        let mut hub = state.inner.lock().await;
        assert_eq!(
            hub.sockets.get("worker").map(|(gen, _)| *gen),
            Some(gen),
            "a failed replacement must leave the incumbent registered"
        );
        hub.db.execute_batch("DROP TRIGGER fail_connect").unwrap();
        persist_then_deliver(
            &mut hub,
            "worker",
            json!({"type": "notify", "message": "for incumbent"}),
            now_unix(),
        )
        .unwrap();
    }
    assert_eq!(
        socket_frame(&mut incumbent).await["message"],
        "for incumbent"
    );
    let mut successor = connect_worker(&url, true).await;
    assert_eq!(socket_frame(&mut successor).await["type"], "connected");
    assert_eq!(
        socket_frame(&mut successor).await["message"],
        "for incumbent"
    );
    assert_ne!(state.inner.lock().await.sockets["worker"].0, gen);
}

#[tokio::test]
async fn a_received_copy_is_not_requeued_when_a_legacy_connection_closes() {
    let (mut hub, path) = undelivered_hub();
    let mut rx = recv_peer(&mut hub, "worker");
    let gen = hub.conn_gen;
    persist_then_deliver(
        &mut hub,
        "worker",
        json!({"type": "notify", "message": "live"}),
        now_unix(),
    )
    .unwrap();
    let copy = rx.try_recv().unwrap();
    assert!(acknowledge_event(
        &mut hub,
        "worker",
        copy.event()["id"].as_str().unwrap()
    ));
    persist_ok(&mut hub).unwrap();
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    close_connection(&state, "worker", gen, false, rx, vec![copy]).await;
    assert_eq!(
        state.inner.lock().await.inbox.count("worker"),
        0,
        "closing a legacy socket must not resurrect a received copy"
    );
    assert_eq!(Hub::open(&path).unwrap().inbox.count("worker"), 0);
}

#[tokio::test]
async fn a_failed_request_does_not_undo_what_a_dropped_link_handed_back() {
    const FAIL: &str = "CREATE TEMP TRIGGER fail_write BEFORE INSERT ON inbox BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;";
    let ids = |hub: &Hub| -> Vec<String> {
        hub.inbox
            .records("worker")
            .iter()
            .filter_map(|record| record.event()["id"].as_str().map(str::to_string))
            .collect()
    };
    let (mut hub, path) = undelivered_hub();
    persist_ok(&mut hub).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (1, tx));
    hub.db.execute_batch(FAIL).unwrap();
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    let on_the_link = vec![test_outbound(json!({"type": "notify", "id": "on-link"}))];
    close_connection(&state, "worker", 1, false, rx, on_the_link).await;
    let mut hub = state.inner.lock().await;
    for id in ["refused", "refused-again"] {
        let refused = json!({"type": "notify", "id": id});
        assert!(persist_then_deliver(&mut hub, "worker", refused, now_unix()).is_err());
        assert_eq!(
            ids(&hub),
            ["on-link"],
            "each failed request is undone and what the link handed back stays"
        );
    }
    assert_eq!(
        hub.peers["worker"].status, "offline",
        "the link stays dropped"
    );
    hub.db.execute_batch("DROP TRIGGER fail_write").unwrap();
    let landed = json!({"type": "notify", "id": "landed"});
    persist_then_deliver(&mut hub, "worker", landed, now_unix()).unwrap();
    hub.db.execute_batch(FAIL).unwrap();
    let refused = json!({"type": "notify", "id": "refused-after"});
    assert!(persist_then_deliver(&mut hub, "worker", refused, now_unix()).is_err());
    assert_eq!(
        ids(&hub),
        ["on-link", "landed"],
        "a failure after a write that landed goes back to that write"
    );
    drop(hub);
    assert_eq!(ids(&Hub::open(&path).unwrap()), ["on-link", "landed"]);
}

#[tokio::test]
async fn the_roster_names_each_socket_build_and_flags_a_stale_one() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = temp_state("builds");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    let own = crate::hub::own_build().expect("a unix binary has a build");
    let mut sockets = Vec::new();
    for (id, build) in [
        ("fresh", Some(own)),
        ("old", Some("0:0")),
        ("legacy", None),
        ("away", None),
    ] {
        let _ = json_req(
            router(state.clone()),
            "POST",
            "/peers",
            json!({"name": id, "backend": "pi", "peer_id": id}),
        )
        .await;
        if id == "away" {
            continue;
        }
        let (socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        let (mut w, mut r) = socket.split();
        let mut connect = json!({"type": "connect", "peer_id": id, "recv": true});
        if let Some(build) = build {
            connect["build"] = json!(build);
        }
        w.send(WsMsg::Text(connect.to_string().into()))
            .await
            .unwrap();
        assert!(r
            .next()
            .await
            .unwrap()
            .unwrap()
            .to_string()
            .contains("connected"));
        sockets.push((w, r));
    }
    let (_, health) = json_req(router(state.clone()), "GET", "/health", json!({})).await;
    assert_eq!(health["build"], own, "the hub names the build it runs");
    let (_, snap) = json_req(router(state.clone()), "GET", "/snapshot", json!({})).await;
    let row = |id: &str| {
        snap["roster"]
            .as_array()
            .unwrap()
            .iter()
            .find(|peer| peer["peer_id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no {id} in {snap}"))
    };
    assert_eq!(row("fresh")["build"], own);
    assert_eq!(
        row("fresh")["stale"],
        false,
        "a client on the hub's build is current"
    );
    assert_eq!(
        row("old")["stale"],
        true,
        "a client on another build is stale"
    );
    assert_eq!(
        row("legacy")["stale"],
        true,
        "a client that names no build predates the field"
    );
    assert!(
        row("away")["stale"].is_null(),
        "without a socket there is nothing to judge"
    );
    assert!(row("away")["build"].is_null());
}

#[cfg(unix)]
#[test]
fn a_build_changes_when_its_file_is_rewritten_in_place() {
    use std::os::unix::fs::MetadataExt;
    let path = temp_state("build-of");
    fs::write(&*path, b"first build").unwrap();
    let first = crate::hub::build_of(&path).unwrap();
    let inode = fs::metadata(&*path).unwrap().ino();
    std::thread::sleep(Duration::from_millis(20));
    /* the same length, so only the time tells the two apart */
    fs::write(&*path, b"other build").unwrap();
    assert_eq!(
        fs::metadata(&*path).unwrap().ino(),
        inode,
        "precondition: a rewrite in place keeps the inode"
    );
    assert_ne!(
        crate::hub::build_of(&path).unwrap(),
        first,
        "a rewrite in place is another build"
    );
}

/* how many waiters the hub's change signal has: a connection a lease holds back is one */
async fn watchers_reach(state: &App, count: usize, failure: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.inner.lock().await.changes.receiver_count() != count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect(failure);
}

async fn notify_socket_worker(app: &Router, message: &str) {
    let (status, body) = json_req(
        app.clone(),
        "POST",
        "/notify",
        json!({"from_peer": "boss", "to_peer": "worker", "message": message}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn settle_worker(app: &Router, lease: &str) {
    let (status, body) = json_req(
        app.clone(),
        "POST",
        "/asks/pending/settle",
        json!({"peer_id": "worker", "lease": lease}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/* worker's one record, "held", went out to a pending read under the returned lease */
async fn leased_socket_hub() -> (App, TempState, String, Router, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (state, path, url) = socket_hub_on(listener).await;
    let app = router(state.clone());
    notify_socket_worker(&app, "held").await;
    let (status, read) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=worker",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let lease = read["lease"]
        .as_str()
        .expect("the read leases what it hands out")
        .to_string();
    (state, path, url, app, lease)
}

/* a hook reads the backlog over HTTP and settles once it has shown it; a drainer that connects
in between must not get the same records replayed, so its connection waits out the lease */
#[tokio::test]
async fn a_connection_waits_out_a_pending_read_lease_instead_of_replaying_it() {
    let (state, _path, url, app, lease) = leased_socket_hub().await;
    let watchers = state.inner.lock().await.changes.receiver_count();
    let mut socket = connect_worker(&url, true).await;
    watchers_reach(
        &state,
        watchers + 1,
        "the connection attached while a pending read held the backlog",
    )
    .await;
    settle_worker(&app, &lease).await;
    notify_socket_worker(&app, "after").await;
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    assert_eq!(
        socket_frame(&mut socket).await["message"],
        "after",
        "the record the hook showed and settled is not replayed"
    );
}

/* a reader that never settles sends the hub nothing more, so the lease running out is what
lets the connection attach, and the connection gets the record again. Pi pings every 10 s,
also before it is greeted, and a ping read while the connection waits keeps it waiting */
#[tokio::test]
async fn a_connection_held_by_an_unsettled_lease_attaches_when_it_runs_out() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (state, _path, url) = socket_hub_on(listener).await;
    notify_socket_worker(&router(state.clone()), "held").await;
    let watchers = {
        let mut hub = state.inner.lock().await;
        hub.inbox.ensure_ids("worker");
        let id = hub.inbox.records("worker")[0].event()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let end = Instant::now() + Duration::from_secs(1);
        hub.inbox.lease("worker", "lease-short", vec![id], end);
        hub.changes.receiver_count()
    };
    let mut socket = connect_worker(&url, true).await;
    watchers_reach(
        &state,
        watchers + 1,
        "the connection attached while the lease was live",
    )
    .await;
    socket
        .send(WsMsg::Text(json!({"type": "ping"}).to_string().into()))
        .await
        .unwrap();
    assert_eq!(socket_frame(&mut socket).await["type"], "connected");
    assert_eq!(
        socket_frame(&mut socket).await["message"],
        "held",
        "the record comes back once the lease runs out"
    );
}

/* a client that gives up while its connection waits must not take the name later from one
that is still there */
#[tokio::test]
async fn a_connection_closed_while_it_waits_leaves_the_name_to_the_one_still_waiting() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let (state, _path, url, app, lease) = leased_socket_hub().await;
    let watchers = state.inner.lock().await.changes.receiver_count();
    let mut kept = connect_worker(&url, true).await;
    watchers_reach(&state, watchers + 1, "the first connection attached early").await;
    let mut gone = connect_worker(&url, true).await;
    watchers_reach(&state, watchers + 2, "the second connection attached early").await;
    gone.send(WsMsg::Close(None)).await.unwrap();
    drop(gone);
    watchers_reach(
        &state,
        watchers + 1,
        "the hub kept waiting for a connection that closed",
    )
    .await;
    settle_worker(&app, &lease).await;
    notify_socket_worker(&app, "after").await;
    assert_eq!(socket_frame(&mut kept).await["type"], "connected");
    assert_eq!(
        socket_frame(&mut kept).await["message"],
        "after",
        "the connection still there keeps the name"
    );
}

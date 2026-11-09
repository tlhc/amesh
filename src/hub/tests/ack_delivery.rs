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
    assert!(state.inner.lock().await.inbox.count("w") == 0);
    let _ = fs::remove_file(&path);
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
    /* a peer that never promised recv keeps the old cap */
    reopened
        .peers
        .insert("x".into(), reopened.peers["w"].clone());
    queue_inbox(&mut reopened, "x", chatter(INBOX_MAX + 10), now_unix());
    assert_eq!(reopened.inbox.records("x").len(), INBOX_MAX);
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
    let path = temp_state("recv-regression");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    register(router(state.clone()), "worker", "pi", "default").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, path, url)
}

type PeerSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_worker(url: &str, recv: bool) -> PeerSocket {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
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

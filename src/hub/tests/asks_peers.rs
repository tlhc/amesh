use super::*;

#[tokio::test]
async fn a_sessionless_reregister_keeps_the_bound_session() {
    let (app, _state) = test_app();
    let (_, bound) = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "worker", "backend": "pi",
               "circle": "default", "session_id": "session-a"}),
    )
    .await;
    assert_eq!(bound["peer_id"], "worker");

    /* the drainer announcing itself carries no session */
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "worker", "backend": "pi", "circle": "default"}),
    )
    .await;
    let (_, peers) = json_req(app.clone(), "GET", "/peers", json!({})).await;
    let row = &peers.as_array().unwrap()[0];
    assert_eq!(
        row["session_id"], "session-a",
        "a sessionless re-register must not erase the bound session"
    );

    let (_, moved) = json_req(
        app,
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "worker", "backend": "pi",
               "circle": "default", "session_id": "session-b"}),
    )
    .await;
    assert_eq!(moved["ok"], true, "an explicit session must still win");
}

#[tokio::test]
async fn a_second_answer_is_refused_instead_of_dropped() {
    let (app, _state) = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "worker", "backend": "pi", "circle": "default"}),
    )
    .await;
    let open_ask = |app: Router| async move {
        let (_, ask) = json_req(
            app,
            "POST",
            "/ask",
            json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"}),
        )
        .await;
        ask["correlation_id"].as_str().unwrap().to_string()
    };
    let ack = |app: Router, body: Value| async move { json_req(app, "POST", "/ack", body).await };

    let cid = open_ask(app.clone()).await;
    let (st, _) = ack(
        app.clone(),
        json!({"correlation_id": cid, "message": "first"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "the recipient's answer must land");

    let (st, body) = ack(
        app.clone(),
        json!({"correlation_id": cid, "message": "second"}),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "a different answer must not be swallowed"
    );
    assert_eq!(
        body["reply"], "first",
        "the stored answer must be reported back"
    );

    let (st, body) = ack(
        app.clone(),
        json!({"correlation_id": cid, "message": "first"}),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "retrying the same answer stays idempotent"
    );
    assert_eq!(body["ok"], true);

    let (st, _) = ack(app.clone(), json!({"correlation_id": cid})).await;
    assert_eq!(st, StatusCode::OK, "a bare ack has no content to lose");

    let (_, stored) = json_req(
        app,
        "POST",
        &format!("/asks/{cid}/wait"),
        json!({"timeout_seconds": 0}),
    )
    .await;
    assert_eq!(
        stored["reply"], "first",
        "the refused answer must not have overwritten"
    );
}

fn counted_waker() -> (
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    std::task::Waker,
) {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let waker = std::task::Waker::from(std::sync::Arc::new(WakeCount(count.clone())));
    (count, waker)
}

fn poll_now<F: std::future::Future>(
    future: std::pin::Pin<&mut F>,
    waker: &std::task::Waker,
) -> std::task::Poll<F::Output> {
    let mut cx = std::task::Context::from_waker(waker);
    future.poll(&mut cx)
}

#[tokio::test]
async fn a_wait_wakes_on_the_commit_that_closes_its_ask() {
    let path = temp_state("wake");
    let app = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    let worker =
        json!({"name": "worker", "peer_id": "worker", "backend": "pi", "circle": "default"});
    let _ = json_req(router(app.clone()), "POST", "/peers", worker).await;
    let ping = json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"});
    let (_, ask) = json_req(router(app.clone()), "POST", "/ask", ping).await;
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    let started = tokio::time::Instant::now();
    let (wakes, waker) = counted_waker();
    let mut waiter = Box::pin(wait_capture(&app, &cid, WAIT_MAX_SECS, None));
    assert!(
        poll_now(waiter.as_mut(), &waker).is_pending(),
        "the wait must be pending before the ack"
    );
    let subscribed = app.inner.lock().await.changes.receiver_count();
    assert_eq!(subscribed, 1, "the wait subscribed before the ack");
    let pong = json!({"correlation_id": cid, "message": "pong"});
    let before = wakes.load(std::sync::atomic::Ordering::SeqCst);
    let _ = json_req(router(app.clone()), "POST", "/ack", pong).await;
    assert!(
        wakes.load(std::sync::atomic::Ordering::SeqCst) > before,
        "the ack must wake the wait through the change signal"
    );
    let captured = waiter.await.unwrap();
    let summary = wait_summary(&captured, 0);
    assert_eq!(summary["open"], false, "{summary}");
    assert_eq!(
        summary["timed_out"], false,
        "an ack is not a timeout: {summary}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(WAIT_MAX_SECS),
        "the commit woke the wait before its deadline"
    );
}

#[tokio::test]
async fn a_wait_ends_when_its_recipient_stops_during_it() {
    let path = temp_state("stall");
    let app = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    let report = |peer: &str, state: &str| {
        let body = json!({"peer_id": peer, "state": state});
        json_req(router(app.clone()), "POST", "/activity", body)
    };
    for peer in ["worker", "other"] {
        let row = json!({"name": peer, "peer_id": peer, "backend": "pi", "circle": "default"});
        let _ = json_req(router(app.clone()), "POST", "/peers", row).await;
    }
    report("worker", "idle").await;
    let ping = json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"});
    let (_, ask) = json_req(router(app.clone()), "POST", "/ask", ping).await;
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    /* the recipient went idle before the ask, which is nine seconds old: still inside the
    pickup grace when the wait begins */
    {
        let mut hub = app.inner.lock().await;
        hub.activity.get_mut("worker").unwrap().since = now_unix() - 20;
        hub.asks.get_mut(&cid).unwrap().opened_at = Some(now_unix() - 9);
    }
    let started = tokio::time::Instant::now();
    let (wakes, waker) = counted_waker();
    let mut waiter = Box::pin(wait_capture(&app, &cid, WAIT_MAX_SECS, None));
    assert!(
        poll_now(waiter.as_mut(), &waker).is_pending(),
        "the first look must leave the wait pending"
    );
    /* the grace runs out with no word from the recipient */
    app.inner.lock().await.asks.get_mut(&cid).unwrap().opened_at = Some(now_unix() - 15);
    for (peer, state) in [("other", "work"), ("other", "idle"), ("worker", "work")] {
        let before = wakes.load(std::sync::atomic::Ordering::SeqCst);
        report(peer, state).await;
        assert!(
            wakes.load(std::sync::atomic::Ordering::SeqCst) > before,
            "the report {peer} {state} must wake the wait"
        );
        assert!(
            poll_now(waiter.as_mut(), &waker).is_pending(),
            "elapsed time woken by another peer, or a resumed recipient, leaves it running"
        );
    }
    let before = wakes.load(std::sync::atomic::Ordering::SeqCst);
    report("worker", "wait").await;
    assert!(
        wakes.load(std::sync::atomic::Ordering::SeqCst) > before,
        "the stop report must wake the wait"
    );
    let captured = match poll_now(waiter.as_mut(), &waker) {
        std::task::Poll::Ready(result) => result.expect("the stop ends the wait"),
        std::task::Poll::Pending => panic!("the stop must complete the wait at once"),
    };
    assert!(captured.ask.open);
    let summary = wait_summary(&captured, 0);
    assert_eq!(
        summary["timed_out"], false,
        "a stop is not a timeout: {summary}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(WAIT_MAX_SECS),
        "the recipient's prompt ended the wait before its deadline"
    );
    let started = tokio::time::Instant::now();
    let (wakes, waker) = counted_waker();
    let mut again = Box::pin(wait_capture(&app, &cid, WAIT_MAX_SECS, None));
    assert!(
        poll_now(again.as_mut(), &waker).is_pending(),
        "the second wait must be pending"
    );
    let before = wakes.load(std::sync::atomic::Ordering::SeqCst);
    report("other", "work").await;
    assert!(
        wakes.load(std::sync::atomic::Ordering::SeqCst) > before,
        "a report from another peer must wake the wait"
    );
    assert!(
        poll_now(again.as_mut(), &waker).is_pending(),
        "a stop from before the wait leaves it running"
    );
    /* a turn that resumed and ended before the wait's next look: watch coalesces the two
    notifications into one wake, and the single look must see the idle and stop */
    {
        let mut hub = app.inner.lock().await;
        let before = wakes.load(std::sync::atomic::Ordering::SeqCst);
        for state in ["work", "idle"] {
            let report: ActivityReport = serde_json::from_value(json!({"state": state})).unwrap();
            set_activity(&mut hub, "worker", &report, now_unix());
        }
        assert!(
            wakes.load(std::sync::atomic::Ordering::SeqCst) > before,
            "the resumed turn and its end must wake the wait"
        );
    }
    let captured = match poll_now(again.as_mut(), &waker) {
        std::task::Poll::Ready(result) => {
            result.expect("a turn that resumed and ended between two looks ended the wait")
        }
        std::task::Poll::Pending => panic!("the stopped turn must complete the wait"),
    };
    assert!(captured.ask.open);
    assert!(
        started.elapsed() < Duration::from_secs(WAIT_MAX_SECS),
        "a turn that resumed and ended between two looks ended the wait"
    );
    /* the worker has already stopped and the next wait starts after that stop: the deadline
    ends it, and the idle progress must not be mistaken for a stop */
    let captured = wait_capture(&app, &cid, 0, None).await.unwrap();
    let summary = wait_summary(&captured, 0);
    assert_eq!(summary["state"]["progress"]["kind"], "idle", "{summary}");
    assert_eq!(summary["timed_out"], true, "{summary}");
    /* a stop and the deadline meet in one check: the wait must not call it a timeout. The
    first poll records the activity the wait began on; the lock then holds the next look back
    until the deadline has passed */
    let mut w = Box::pin(wait_capture(&app, &cid, 1, None));
    let (_, waker) = counted_waker();
    assert!(
        poll_now(w.as_mut(), &waker).is_pending(),
        "the first poll must wait"
    );
    {
        let mut hub = app.inner.lock().await;
        let activity = hub.activity.get_mut("worker").unwrap();
        activity.state = "wait".into();
        activity.since = now_unix();
        std::thread::sleep(Duration::from_millis(1200));
    }
    let captured = w.await.unwrap();
    let summary = wait_summary(&captured, 0);
    assert_eq!(summary["open"], true, "{summary}");
    assert_eq!(
        summary["timed_out"], false,
        "a stop at the deadline is not a timeout: {summary}"
    );
}

#[tokio::test]
async fn ask_ack_roundtrip() {
    let (app, _state) = test_app();
    let (_, reg) = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "backend": "pi", "circle": "default"}),
    )
    .await;
    assert_eq!(reg["ok"], true);
    let peer_id = reg["peer_id"].as_str().unwrap();

    let (st, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cid = ask["correlation_id"].as_str().unwrap();
    assert!(cid.starts_with("ask-"));

    let (_, pending) = json_req(
        app.clone(),
        "GET",
        &format!("/asks/pending?peer_id={peer_id}"),
        json!({}),
    )
    .await;
    assert_eq!(pending["asks"].as_array().unwrap().len(), 1);

    let (st, ack) = json_req(
        app,
        "POST",
        "/ack",
        json!({"correlation_id": cid, "message": "pong"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ack["ok"], true);
}

#[tokio::test]
async fn broadcast_same_circle_skips_self_and_other_circle() {
    let (app, _state) = test_app();
    for (name, circle) in [("a", "default"), ("b", "default"), ("c", "other")] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": name, "peer_id": name, "backend": "pi", "circle": circle}),
        )
        .await;
    }
    let (st, body) = json_req(
        app,
        "POST",
        "/broadcast",
        json!({"from_peer": "a", "message": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sent = body["sent_to"].as_array().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], "b");
    assert!(body["failed"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn broadcast_unknown_sender_is_error() {
    let (app, _state) = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "b", "peer_id": "b", "backend": "pi", "circle": "default"}),
    )
    .await;
    let (st, body) = json_req(
        app,
        "POST",
        "/broadcast",
        json!({"from_peer": "ghost", "message": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown sender");
}

#[tokio::test]
async fn session_id_reuse_keeps_peer_id_and_circle_when_cwd_changes() {
    let (app, _state) = test_app();
    let (st, first) = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({
            "name": "orig",
            "backend": "claude-code",
            "path": "/tmp/proj-a",
            "circle": "circ-a",
            "session_id": "sess-keep"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = first["peer_id"].as_str().unwrap().to_string();
    let (st, second) = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({
            "backend": "claude-code",
            "path": "/tmp/proj-b",
            "circle": "circ-b",
            "session_id": "sess-keep"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(second["peer_id"], id);
    assert_eq!(second["circle"], "circ-a");
    let (_, listed) = json_req(app, "GET", "/peers", json!({})).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[test]
fn a_restart_gives_persisted_peers_a_reconnect_window() {
    let path = temp_state("restart");
    {
        let mut hub = Hub::open(&path).unwrap();
        hub.peers.insert(
            "w".into(),
            Peer {
                peer_id: "w".into(),
                name: "w".into(),
                path: "/tmp".into(),
                backend: "claude-code".into(),
                circle: "default".into(),
                status: "online".into(),
                description: String::new(),
                session_id: "sess-w".into(),
                last_seen: now_unix().saturating_sub(PEER_ONLINE_SECS * 10),
            },
        );
        persist(&mut hub).unwrap();
    }
    let mut hub = Hub::open(&path).unwrap();
    assert!(
        !refresh_peers(&mut hub).0,
        "a stale persisted last_seen must not prune a peer right after a restart"
    );
    assert_eq!(hub.peers["w"].session_id, "sess-w");
}

#[test]
fn a_full_inbox_drops_chatter_before_an_ask() {
    let path = temp_state("inbox");
    let mut hub = Hub::open(&path).unwrap();
    queue_inbox(
        &mut hub,
        "w",
        [json!({"type": "ask", "id": "keep-me"})],
        now_unix(),
    );
    let chatter = (0..INBOX_MAX * 2).map(|n| json!({"type": "broadcast", "id": n}));
    queue_inbox(&mut hub, "w", chatter, now_unix());
    let queue = &hub.inbox.records("w");
    assert_eq!(queue.len(), INBOX_MAX);
    assert!(
        queue.iter().any(|held| held.event()["id"] == "keep-me"),
        "chatter must be evicted before the ask someone is waiting on"
    );
    assert_eq!(
        queue.last().unwrap().event()["id"],
        json!(INBOX_MAX * 2 - 1)
    );
    let asks = (0..INBOX_MAX * 2).map(|n| json!({"type": "ask", "id": n}));
    queue_inbox(&mut hub, "x", asks, now_unix());
    assert_eq!(
        hub.inbox.records("x").len(),
        INBOX_MAX * 2,
        "a queue of nothing but asks grows past the cap instead of stranding an asker"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn a_restart_ignores_inbox_keys_without_a_peer() {
    let path = temp_state("orphan-load");
    let mut hub = Hub::open(&path).unwrap();
    hub.peers.insert(
        "offline".into(),
        Peer {
            peer_id: "offline".into(),
            name: "offline".into(),
            path: "/tmp".into(),
            backend: "pi".into(),
            circle: "default".into(),
            status: "offline".into(),
            description: String::new(),
            session_id: String::new(),
            last_seen: now_unix(),
        },
    );
    hub.inbox.put(
        "offline",
        vec![QueuedRecord::new(
            json!({"type": "notify", "id": "keep"}),
            now_unix(),
        )],
    );
    hub.inbox.put(
        "old-name",
        vec![QueuedRecord::new(
            json!({"type": "ack", "id": "legacy-reply"}),
            now_unix(),
        )],
    );
    persist(&mut hub).unwrap();
    let reopened = Hub::open(&path).unwrap();
    assert_eq!(reopened.inbox.records("offline")[0].event()["id"], "keep");
    assert!(
        reopened.inbox.count("old-name") == 0,
        "inbox keys that are not a registered peer_id must not reload"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_reply_to_a_departed_peer_leaves_no_permanent_queue() {
    let path = temp_state("orphan");
    let mut hub = Hub::open(&path).unwrap();
    hub.peers.insert(
        "live".into(),
        Peer {
            peer_id: "live".into(),
            name: "live".into(),
            path: "/tmp".into(),
            backend: "pi".into(),
            circle: "default".into(),
            status: "online".into(),
            description: String::new(),
            session_id: String::new(),
            last_seen: now_unix(),
        },
    );
    hub.asks.insert(
        "ask-gone".into(),
        Ask {
            correlation_id: "ask-gone".into(),
            from_peer: "amesh-cli".into(),
            to_peer: "live".into(),
            to_peer_id: "live".into(),
            text: "q".into(),
            open: true,
            reply: None,
            failed: false,
            closed_at: None,
            opened_at: None,
            closed_by: None,
            from_circle: String::new(),
            to_circle: String::new(),
            watch: None,
        },
    );
    persist(&mut hub).unwrap();
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.clone(),
    };
    let http = router(state);
    let (status, body) = json_req(
        http,
        "POST",
        "/ack",
        json!({"correlation_id": "ask-gone", "message": "answer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reopened = Hub::open(&path).unwrap();
    let ask = &reopened.asks["ask-gone"];
    assert!(!ask.open, "ack must persist the closed ask");
    assert_eq!(ask.reply.as_deref(), Some("answer"));
    assert!(
        reopened.inbox.count("amesh-cli") == 0,
        "a target that is no peer must not open a queue nothing will ever collect"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn probe_prunes_stale_peers_before_list_or_broadcast() {
    let path = temp_state("probe");
    let mut hub = Hub::open(&path).unwrap();
    let now = now_unix();
    let peer = |id: &str, seen: u64| Peer {
        peer_id: id.into(),
        name: id.into(),
        path: "/tmp".into(),
        backend: "pi".into(),
        circle: "default".into(),
        status: "online".into(),
        description: String::new(),
        session_id: String::new(),
        last_seen: seen,
    };
    hub.peers.insert("live".into(), peer("live", now));
    hub.peers.insert(
        "stale".into(),
        peer("stale", now.saturating_sub(PEER_ONLINE_SECS + 1)),
    );
    assert!(refresh_peers(&mut hub).0);
    assert!(hub.peers.contains_key("live"));
    assert!(!hub.peers.contains_key("stale"));
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn ask_and_notify_cross_circle_and_broadcast_can_target_circle() {
    let (app, _state) = test_app();
    for (name, circle) in [("a", "default"), ("b", "default"), ("c", "other")] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": name, "peer_id": name, "backend": "pi", "circle": circle}),
        )
        .await;
    }
    let (st, denied) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "a", "to_peer": "c", "text": "cross"}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(denied["error"], "cross-circle requires cross_circle");
    let (st, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "a", "to_peer": "c", "text": "cross", "cross_circle": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(ask["correlation_id"].as_str().unwrap().starts_with("ask-"));
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/notify",
        json!({"from_peer": "a", "to_peer": "c", "message": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, n) = json_req(
        app.clone(),
        "POST",
        "/notify",
        json!({"from_peer": "a", "to_peer": "c", "message": "hi", "cross_circle": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(n["ok"], true);
    let (st, bdenied) = json_req(
        app.clone(),
        "POST",
        "/broadcast",
        json!({"from_peer": "a", "circle": "other", "message": "there"}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(bdenied["error"], "cross-circle requires cross_circle");
    let (st, body) = json_req(
        app,
        "POST",
        "/broadcast",
        json!({"from_peer": "a", "circle": "other", "cross_circle": true, "message": "there"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sent = body["sent_to"].as_array().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], "c");
}

#[tokio::test]
async fn activity_keeps_a_socketless_peer_alive() {
    let path = temp_state("touch");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    let http = router(state.clone());
    for id in ["worker", "bystander"] {
        let _ = json_req(
            http.clone(),
            "POST",
            "/peers",
            json!({"name": id, "peer_id": id, "backend": "pi"}),
        )
        .await;
    }
    {
        let mut hub = state.inner.lock().await;
        for id in ["worker", "bystander"] {
            hub.peers.get_mut(id).unwrap().last_seen = 1;
        }
    }
    let (st, _) = json_req(
        http.clone(),
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "amesh_whoami", "arguments": {"from_peer": "worker"}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, list) = json_req(http, "GET", "/peers", json!({})).await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|peer| peer["peer_id"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"worker"),
        "activity must keep a socketless caller alive: {names:?}"
    );
    assert!(
        !names.contains(&"bystander"),
        "an equally stale but idle peer must still be pruned: {names:?}"
    );
}

#[tokio::test]
async fn unknown_peer_errors_name_online_circle_mates() {
    let (app, inner, _state) = test_app_with_hub();
    for (id, circle) in [
        ("here", "one"),
        ("mate", "one"),
        ("gone", "one"),
        ("away", "two"),
    ] {
        register(app.clone(), id, "pi", circle).await;
    }
    inner.lock().await.peers.get_mut("gone").unwrap().status = "offline".into();
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "here", "to_peer": "nobody", "text": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown peer");
    assert_eq!(
        body["peers"],
        json!(["mate"]),
        "online circle mates, caller excluded: {body}"
    );
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/notify",
        json!({"from_peer": "here", "to_peer": "nobody", "message": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["peers"], json!(["mate"]), "{body}");
    for n in 0..10 {
        register(app.clone(), &format!("p{n:02}"), "pi", "three").await;
    }
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "p00", "to_peer": "nobody", "text": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        body["peers"].as_array().unwrap().len(),
        8,
        "nine mates are capped to eight: {body}"
    );
    for from in [
        json!({}),
        json!({"from_peer": ""}),
        json!({"from_peer": "ghost"}),
    ] {
        let mut req = from;
        req["to_peer"] = json!("nobody");
        req["text"] = json!("hi");
        let (st, body) = json_req(app.clone(), "POST", "/ask", req).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(
            body["peers"],
            json!([]),
            "no caller circle means no candidates: {body}"
        );
    }
    let (st, body) = mcp_tool(
        app,
        "amesh_ask",
        json!({"from_peer": "here", "peer_name": "nobody", "query": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        body["peers"],
        json!(["mate"]),
        "MCP surfaces the same candidates: {body}"
    );
}

#[tokio::test]
async fn an_ask_rejects_an_empty_body() {
    let (app, hub, path) = test_app_with_hub();
    register(app.clone(), "worker", "pi", "default").await;
    for text in ["", "  ", "\n\t\u{2003}"] {
        let (status, body) = json_req(
            app.clone(),
            "POST",
            "/ask",
            json!({"to_peer": "worker", "text": text}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "empty ask text must be rejected: {text:?}: {body}"
        );
        assert_eq!(body["error"], "ask text must not be empty");
        assert!(hub.lock().await.asks.is_empty());
        assert!(Hub::open(&path).unwrap().asks.is_empty());
    }
}

#[tokio::test]
async fn every_message_body_rejects_blank_text() {
    let (app, hub, _path) = test_app_with_hub();
    register(app.clone(), "here", "pi", "default").await;
    register(app.clone(), "worker", "pi", "default").await;
    let mut accepted = Vec::new();
    for text in ["", "  ", "\n\t\u{2003}"] {
        for (uri, body, error) in [
            (
                "/notify",
                json!({"from_peer": "here", "to_peer": "worker", "message": text}),
                "notify message must not be empty",
            ),
            (
                "/broadcast",
                json!({"from_peer": "here", "message": text}),
                "broadcast message must not be empty",
            ),
            (
                "/schedules",
                json!({"from_peer": "here", "to_peer": "worker", "text": text, "in_seconds": 60}),
                "schedule text must not be empty",
            ),
        ] {
            let (status, reply) = json_req(app.clone(), "POST", uri, body).await;
            if status != StatusCode::BAD_REQUEST || reply["error"] != error {
                accepted.push(format!("{uri} {text:?}: {status} {reply}"));
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "blank bodies got through: {accepted:#?}"
    );
    let hub = hub.lock().await;
    assert_eq!(
        hub.inbox.count("worker"),
        0,
        "nothing reached the recipient"
    );
    assert!(hub.schedules.is_empty(), "no schedule was created");
}

#[tokio::test]
async fn mcp_ask_rejects_a_missing_body_without_opening_an_ask() {
    let (app, hub, path) = test_app_with_hub();
    register(app.clone(), "worker", "pi", "default").await;
    let (status, body) = mcp_tool(app, "amesh_ask", json!({"peer_name": "worker"})).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a missing MCP ask body must be rejected: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("ask text must not be empty")),
        "a missing MCP ask body must return an error: {body}"
    );
    assert!(hub.lock().await.asks.is_empty());
    assert!(Hub::open(&path).unwrap().asks.is_empty());
}

#[tokio::test]
async fn mcp_ask_accepts_the_message_alias() {
    let (app, hub, _path) = test_app_with_hub();
    register(app.clone(), "worker", "pi", "default").await;
    let (status, body) = mcp_tool(
        app,
        "amesh_ask",
        json!({"peer_name": "worker", "message": "message alias"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tool_json(&body)["ok"], true);
    assert_eq!(
        hub.lock().await.asks.values().next().unwrap().text,
        "message alias",
        "MCP must use the same message alias as HTTP"
    );
}

#[test]
fn the_ask_schema_requires_query() {
    let tool = mcp_tools()
        .into_iter()
        .find(|tool| tool["name"] == "amesh_ask")
        .unwrap();
    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["query"]),
        "the ask schema must require a body"
    );
}

#[tokio::test]
async fn ask_many_rejects_an_empty_body() {
    let (app, hub, _path) = test_app_with_hub();
    register(app.clone(), "worker", "pi", "default").await;
    let (status, body) = json_req(
        app,
        "POST",
        "/ask-many",
        json!({"to_peers": ["worker"], "text": "  "}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "ask-many must share ask text validation: {body}"
    );
    assert!(hub.lock().await.asks.is_empty());
}

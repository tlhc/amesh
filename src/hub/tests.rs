use super::*;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn test_app() -> Router {
    let path = std::env::temp_dir().join(format!("amesh-test-{}.db", Uuid::new_v4()));
    router(App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path,
    })
}

async fn json_req(app: Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn a_sessionless_reregister_keeps_the_bound_session() {
    let app = test_app();
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
    let app = test_app();
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

#[tokio::test]
async fn ask_ack_roundtrip() {
    let app = test_app();
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
    let app = test_app();
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
    let app = test_app();
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
    let app = test_app();
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
    let path = std::env::temp_dir().join(format!("amesh-restart-{}.db", Uuid::new_v4()));
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
        !refresh_peers(&mut hub),
        "a stale persisted last_seen must not prune a peer right after a restart"
    );
    assert_eq!(hub.peers["w"].session_id, "sess-w");
}

#[test]
fn a_full_inbox_drops_chatter_before_an_ask() {
    let path = std::env::temp_dir().join(format!("amesh-inbox-{}.db", Uuid::new_v4()));
    let mut hub = Hub::open(&path).unwrap();
    queue_inbox(&mut hub, "w", [json!({"type": "ask", "id": "keep-me"})]);
    let chatter = (0..INBOX_MAX * 2).map(|n| json!({"type": "broadcast", "id": n}));
    queue_inbox(&mut hub, "w", chatter);
    let queue = &hub.inbox["w"];
    assert_eq!(queue.len(), INBOX_MAX);
    assert!(
        queue.iter().any(|held| held["id"] == "keep-me"),
        "chatter must be evicted before the ask someone is waiting on"
    );
    assert_eq!(queue.last().unwrap()["id"], json!(INBOX_MAX * 2 - 1));
    let asks = (0..INBOX_MAX * 2).map(|n| json!({"type": "ask", "id": n}));
    queue_inbox(&mut hub, "x", asks);
    assert_eq!(
        hub.inbox["x"].len(),
        INBOX_MAX * 2,
        "a queue of nothing but asks grows past the cap instead of stranding an asker"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn a_restart_ignores_inbox_keys_without_a_peer() {
    let path = std::env::temp_dir().join(format!("amesh-orphan-load-{}.db", Uuid::new_v4()));
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
    hub.inbox.insert(
        "offline".into(),
        vec![json!({"type": "notify", "id": "keep"})],
    );
    hub.inbox.insert(
        "old-name".into(),
        vec![json!({"type": "ack", "id": "legacy-reply"})],
    );
    persist(&mut hub).unwrap();
    let reopened = Hub::open(&path).unwrap();
    assert_eq!(reopened.inbox["offline"][0]["id"], "keep");
    assert!(
        !reopened.inbox.contains_key("old-name"),
        "inbox keys that are not a registered peer_id must not reload"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_reply_to_a_departed_peer_leaves_no_permanent_queue() {
    let path = std::env::temp_dir().join(format!("amesh-orphan-{}.db", Uuid::new_v4()));
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
        !reopened.inbox.contains_key("amesh-cli"),
        "a target that is no peer must not open a queue nothing will ever collect"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn probe_prunes_stale_peers_before_list_or_broadcast() {
    let path = std::env::temp_dir().join(format!("amesh-probe-{}.db", Uuid::new_v4()));
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
    assert!(refresh_peers(&mut hub));
    assert!(hub.peers.contains_key("live"));
    assert!(!hub.peers.contains_key("stale"));
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn ask_and_notify_cross_circle_and_broadcast_can_target_circle() {
    let app = test_app();
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
async fn mcp_tools_have_property_schemas() {
    let app = test_app();
    let (st, body) = json_req(
        app,
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let tools = body["result"]["tools"].as_array().unwrap();
    let ask = tools.iter().find(|t| t["name"] == "amesh_ask").unwrap();
    assert_eq!(
        ask["inputSchema"]["properties"]["peer_name"]["type"],
        "string"
    );
    assert_eq!(ask["inputSchema"]["properties"]["query"]["type"], "string");
    assert!(ask["inputSchema"]["properties"].get("text").is_none());
    let sched = tools
        .iter()
        .find(|t| t["name"] == "amesh_schedule_create")
        .unwrap();
    assert_eq!(sched["inputSchema"]["properties"]["text"]["type"], "string");
    assert!(sched["inputSchema"]["properties"].get("message").is_none());
    let ack = tools.iter().find(|t| t["name"] == "amesh_ack").unwrap();
    assert_eq!(ack["inputSchema"]["required"][0], "correlation_id");
}

#[tokio::test]
async fn mcp_ask_keeps_hidden_text_alias_and_prefers_query() {
    let app = test_app();
    for name in ["boss", "worker"] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": name, "peer_id": name, "backend": "pi"}),
        )
        .await;
    }
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "amesh_ask", "arguments": {
                   "from_peer": "boss", "peer_name": "worker", "text": "legacy-only"}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, asks) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=worker",
        json!({}),
    )
    .await;
    assert_eq!(asks["asks"][0]["text"], "legacy-only");
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "amesh_ask", "arguments": {
                   "from_peer": "boss", "peer_name": "worker",
                   "query": "visible", "text": "hidden-dropped"}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, asks) = json_req(app, "GET", "/asks/pending?peer_id=worker", json!({})).await;
    let texts: Vec<&str> = asks["asks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["text"].as_str().unwrap())
        .collect();
    assert!(texts.contains(&"visible"));
    assert!(!texts.contains(&"hidden-dropped"));
}

#[tokio::test]
async fn mcp_schedule_hidden_message_alias_and_long_bodies_persist() {
    let path = std::env::temp_dir().join(format!("amesh-long-{}.db", Uuid::new_v4()));
    let app = router(App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    });
    for name in ["boss", "worker"] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": name, "peer_id": name, "backend": "pi"}),
        )
        .await;
    }
    let mut long = String::from("HEAD-MARKER\n");
    let chunk = "段落 中文\n```c\nint x = 1;\n```\n";
    while long.len() < 4096 {
        long.push_str(chunk);
    }
    long.push_str("TAIL-MARKER");
    assert!(long.len() >= 4096);
    assert!(long.chars().count() > 62);

    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "amesh_ask", "arguments": {
                   "from_peer": "boss", "peer_name": "worker", "query": long}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let opened: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let cid = opened["correlation_id"].as_str().unwrap().to_string();

    let disk = Hub::open(&path).unwrap();
    assert_eq!(disk.asks[&cid].text, long);
    let inbox_ask = disk
        .inbox
        .get("worker")
        .into_iter()
        .flatten()
        .find(|e| e["type"] == "ask" && e["correlation_id"] == cid)
        .expect("ask must be persisted on inbox before pending consumes it");
    assert_eq!(inbox_ask["text"].as_str().unwrap(), long);
    drop(disk);

    let (_, events) = json_req(app.clone(), "GET", "/events", json!({})).await;
    let delivered = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["correlation_id"] == cid)
        .unwrap();
    assert_eq!(delivered["text"].as_str().unwrap(), long);
    let (_, pending) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=worker",
        json!({}),
    )
    .await;
    assert_eq!(pending["asks"][0]["text"].as_str().unwrap(), long);
    let inbox_ask = pending["inbox"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "ask")
        .expect("ask must be on the delivery inbox");
    assert_eq!(inbox_ask["text"].as_str().unwrap(), long);

    let call = |id: u64, args: Value| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": {"name": "amesh_schedule_create", "arguments": args}})
    };
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/mcp",
        call(
            2,
            json!({
                "from_peer": "boss", "to_peer": "worker",
                "message": "legacy-sched", "in_seconds": 60
            }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let created: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(created["text"].as_str().unwrap(), "legacy-sched");

    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/mcp",
        call(
            3,
            json!({
                "from_peer": "boss", "to_peer": "worker",
                "text": "sched-visible", "message": "sched-dropped", "in_seconds": 60
            }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let created: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(created["text"].as_str().unwrap(), "sched-visible");

    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/mcp",
        call(
            4,
            json!({
                "from_peer": "boss", "to_peer": "worker",
                "text": long, "in_seconds": 60
            }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let created: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(created["text"].as_str().unwrap(), long);
    let long_sid = created["schedule_id"].as_str().unwrap().to_string();

    let disk = Hub::open(&path).unwrap();
    assert_eq!(disk.asks[&cid].text, long);
    assert_eq!(disk.schedules[&long_sid].text, long);
    let sched_texts: Vec<&str> = disk.schedules.values().map(|s| s.text.as_str()).collect();
    assert!(sched_texts.contains(&"legacy-sched"));
    assert!(sched_texts.contains(&"sched-visible"));
    assert!(!sched_texts.contains(&"sched-dropped"));
    drop(disk);

    let (_, listed) = json_req(app, "GET", "/schedules", json!({})).await;
    let texts: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["text"].as_str().unwrap())
        .collect();
    assert!(texts.contains(&"legacy-sched"));
    assert!(texts.contains(&"sched-visible"));
    assert!(texts.contains(&long.as_str()));
    assert!(!texts.contains(&"sched-dropped"));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(path.with_extension("db-wal"));
    let _ = fs::remove_file(path.with_extension("db-shm"));
}

#[tokio::test]
async fn mcp_list_peers_exposes_status() {
    let app = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "worker", "backend": "codex", "circle": "one"}),
    )
    .await;
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "amesh_list_peers", "arguments": {"from_peer": "worker"}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    let row: Vec<&str> = text.lines().next().unwrap().split('\t').collect();
    assert_eq!(row.len(), 5, "status column missing: {text}");
    let (_, peers) = json_req(app, "GET", "/peers", json!({})).await;
    assert_eq!(row[4], peers[0]["status"].as_str().unwrap());
}

fn peer_ids(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.split('\t').next().unwrap().to_string())
        .collect();
    ids.sort_unstable();
    ids
}

async fn mcp_list(app: Router, args: Value) -> (StatusCode, Value) {
    json_req(
        app,
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "amesh_list_peers", "arguments": args}}),
    )
    .await
}

#[tokio::test]
async fn mcp_list_peers_defaults_to_caller_circle() {
    let app = test_app();
    for (id, circle) in [("here", "one"), ("away", "two")] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": id, "peer_id": id, "backend": "pi", "circle": circle}),
        )
        .await;
    }
    let ids = |body: &Value| peer_ids(body["result"]["content"][0]["text"].as_str().unwrap());
    for args in [
        json!({"from_peer": "here"}),
        json!({"from_peer": "here", "circle": "one"}),
        json!({"from_peer": "here", "circle": ""}),
    ] {
        let (st, body) = mcp_list(app.clone(), args).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ids(&body), ["here".to_string()]);
    }
    let (st, body) = mcp_list(app.clone(), json!({"from_peer": "here", "circle": "two"})).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "cross-circle requires cross_circle");
    let (st, body) = mcp_list(
        app.clone(),
        json!({"from_peer": "here", "circle": "two", "cross_circle": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&body), ["away".to_string()]);
    let (st, body) = mcp_list(
        app.clone(),
        json!({"from_peer": "here", "cross_circle": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&body), ["away".to_string(), "here".to_string()]);
    for args in [
        json!({}),
        json!({"from_peer": ""}),
        json!({"from_peer": "gone"}),
        json!({"from_peer": "gone", "cross_circle": true}),
    ] {
        let (st, body) = mcp_list(app.clone(), args).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "unknown caller");
    }
    let (_, peers) = json_req(app, "GET", "/peers", json!({})).await;
    assert_eq!(peers.as_array().unwrap().len(), 2);
}

#[test]
fn list_peers_schema_exposes_circle_and_cross_circle() {
    let tools = mcp_tools();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "amesh_list_peers")
        .unwrap();
    assert_eq!(
        tool["inputSchema"]["properties"]["circle"]["type"],
        "string"
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["cross_circle"]["type"],
        "boolean"
    );
    assert_eq!(
        tool["description"],
        "List peers in your circle; cross_circle without circle lists all"
    );
}

async fn mcp_tool(app: Router, name: &str, args: Value) -> (StatusCode, Value) {
    json_req(
        app,
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": name, "arguments": args}}),
    )
    .await
}

fn ledger_ids(body: &Value, field: &str) -> Vec<String> {
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    let rows: Vec<Value> = serde_json::from_str(text).unwrap();
    let mut ids: Vec<String> = rows
        .iter()
        .filter_map(|row| row[field].as_str().map(str::to_string))
        .collect();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn mcp_job_list_defaults_to_caller_circle() {
    let app = test_app();
    for (id, circle) in [("here", "one"), ("away", "two")] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": id, "peer_id": id, "backend": "pi", "circle": circle}),
        )
        .await;
    }
    let (st, here_job) = mcp_tool(
        app.clone(),
        "amesh_job_create",
        json!({"from_peer": "here", "title": "mine"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let here_id = here_job["result"]["content"][0]["text"].as_str().unwrap();
    let here_id = serde_json::from_str::<Value>(here_id).unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        serde_json::from_str::<Value>(here_job["result"]["content"][0]["text"].as_str().unwrap(),)
            .unwrap()["circle"],
        "one"
    );
    let (st, away_job) = mcp_tool(
        app.clone(),
        "amesh_job_create",
        json!({"from_peer": "away", "title": "theirs"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let away_id =
        serde_json::from_str::<Value>(away_job["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap()["job_id"]
            .as_str()
            .unwrap()
            .to_string();
    let (st, blank) = json_req(app.clone(), "POST", "/jobs", json!({"title": "orphan"})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(blank["circle"], "");
    let blank_id = blank["job_id"].as_str().unwrap().to_string();
    let ids = |body: &Value| ledger_ids(body, "job_id");
    for args in [
        json!({"from_peer": "here"}),
        json!({"from_peer": "here", "circle": "one"}),
        json!({"from_peer": "here", "circle": ""}),
    ] {
        let (st, body) = mcp_tool(app.clone(), "amesh_job_list", args).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ids(&body), [here_id.clone()]);
    }
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_job_list",
        json!({"from_peer": "here", "circle": "two"}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "cross-circle requires cross_circle");
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_job_list",
        json!({"from_peer": "here", "circle": "two", "cross_circle": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&body), [away_id.clone()]);
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_job_list",
        json!({"from_peer": "here", "cross_circle": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&body), {
        let mut all = vec![away_id.clone(), blank_id.clone(), here_id.clone()];
        all.sort();
        all
    });
    for args in [
        json!({}),
        json!({"from_peer": ""}),
        json!({"from_peer": "gone"}),
        json!({"from_peer": "gone", "cross_circle": true}),
    ] {
        let (st, body) = mcp_tool(app.clone(), "amesh_job_list", args).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "unknown caller");
    }
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_job_create",
        json!({"from_peer": "gone", "title": "nope"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown caller");
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/jobs",
        json!({"title": "bad-assign", "assigned_peer": "gone"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown peer");
    let (_, jobs) = json_req(app.clone(), "GET", "/jobs", json!({})).await;
    assert!(jobs.as_array().unwrap().len() >= 3);
    let (st, _) = mcp_tool(app.clone(), "amesh_job_cancel", json!({"job_id": here_id})).await;
    assert_eq!(st, StatusCode::OK);
    let (st, status) = mcp_tool(app.clone(), "amesh_job_status", json!({"job_id": here_id})).await;
    assert_eq!(st, StatusCode::OK);
    assert!(status["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("cancelled"));
    let (st, _) = mcp_tool(app.clone(), "amesh_job_delete", json!({"job_id": here_id})).await;
    assert_eq!(st, StatusCode::OK);
    let (st, body) = mcp_tool(app.clone(), "amesh_job_status", json!({"job_id": here_id})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown job");
    let (st, body) = mcp_tool(app, "amesh_job_delete", json!({"job_id": here_id})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown job");
}

#[tokio::test]
async fn mcp_schedule_list_stamps_creator_circle() {
    let app = test_app();
    for (id, circle) in [("here", "one"), ("away", "two")] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": id, "peer_id": id, "backend": "pi", "circle": circle}),
        )
        .await;
    }
    let (st, created) = mcp_tool(
        app.clone(),
        "amesh_schedule_create",
        json!({
            "from_peer": "here",
            "to_peer": "away",
            "text": "later",
            "in_seconds": 86400
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let row =
        serde_json::from_str::<Value>(created["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap();
    assert_eq!(row["circle"], "one");
    let sid = row["schedule_id"].as_str().unwrap().to_string();
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_schedule_list",
        json!({"from_peer": "here"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ledger_ids(&body, "schedule_id"), [sid.clone()]);
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_schedule_list",
        json!({"from_peer": "away"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ledger_ids(&body, "schedule_id"), [] as [String; 0]);
    let (_, all) = json_req(app.clone(), "GET", "/schedules", json!({})).await;
    assert_eq!(all.as_array().unwrap().len(), 1);
    let (st, _) = mcp_tool(
        app.clone(),
        "amesh_schedule_delete",
        json!({"schedule_id": sid}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn schedule_circle_survives_restart_old_schema_and_missing_target() {
    let path = std::env::temp_dir().join(format!("amesh-sched-mig-{}.db", Uuid::new_v4()));
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE schedules (
              schedule_id TEXT PRIMARY KEY,
              from_peer TEXT NOT NULL,
              to_peer TEXT NOT NULL,
              text TEXT NOT NULL,
              kind TEXT NOT NULL,
              fire_at INTEGER NOT NULL,
              every_seconds INTEGER
            );
            INSERT INTO schedules VALUES ('sched-old','ghost','ghost','x','notify',4102444800,NULL);",
        )
        .unwrap();
    }
    {
        let hub = Hub::open(&path).unwrap();
        assert_eq!(hub.schedules["sched-old"].circle, "");
    }
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let app = router(state.clone());
    for (id, circle) in [("here", "one"), ("away", "two")] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": id, "peer_id": id, "backend": "pi", "circle": circle}),
        )
        .await;
    }
    let (st, created) = mcp_tool(
        app.clone(),
        "amesh_schedule_create",
        json!({
            "from_peer": "here",
            "to_peer": "away",
            "text": "later",
            "in_seconds": 86400
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let row =
        serde_json::from_str::<Value>(created["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap();
    assert_eq!(row["circle"], "one");
    let sid = row["schedule_id"].as_str().unwrap().to_string();
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_schedule_list",
        json!({"from_peer": "here"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ledger_ids(&body, "schedule_id"), [sid.clone()]);
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_schedule_list",
        json!({"from_peer": "here", "cross_circle": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let mut listed = vec!["sched-old".to_string(), sid.clone()];
    listed.sort();
    assert_eq!(ledger_ids(&body, "schedule_id"), listed);
    drop(app);
    drop(state);
    {
        let hub = Hub::open(&path).unwrap();
        assert_eq!(hub.schedules[&sid].circle, "one");
        assert_eq!(hub.schedules["sched-old"].circle, "");
    }
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    {
        let mut hub = state.inner.lock().await;
        hub.peers.remove("away");
        persist(&mut hub).unwrap();
    }
    let app = router(state.clone());
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_schedule_list",
        json!({"from_peer": "here"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ledger_ids(&body, "schedule_id"), [sid.clone()]);
    let (_, all) = json_req(app.clone(), "GET", "/schedules", json!({})).await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    let (st, _) = mcp_tool(
        app.clone(),
        "amesh_schedule_delete",
        json!({"schedule_id": sid}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    drop(app);
    drop(state);
    let hub = Hub::open(&path).unwrap();
    assert!(!hub.schedules.contains_key(&sid));
    assert!(hub.schedules.contains_key("sched-old"));
    let _ = fs::remove_file(&path);
}

#[test]
fn job_circle_survives_restart_and_old_schema() {
    let path = std::env::temp_dir().join(format!("amesh-job-mig-{}.db", Uuid::new_v4()));
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE jobs (
              job_id TEXT PRIMARY KEY,
              title TEXT NOT NULL,
              prompt TEXT NOT NULL,
              path TEXT NOT NULL,
              backend TEXT NOT NULL,
              assigned_peer TEXT,
              state TEXT NOT NULL,
              result_summary TEXT
            );
            INSERT INTO jobs VALUES ('job-old','t','p','','pi',NULL,'queued',NULL);",
        )
        .unwrap();
    }
    {
        let hub = Hub::open(&path).unwrap();
        assert_eq!(hub.jobs["job-old"].circle, "");
    }
    {
        let mut hub = Hub::open(&path).unwrap();
        hub.peers.insert(
            "here".into(),
            Peer {
                peer_id: "here".into(),
                name: "here".into(),
                path: "/tmp".into(),
                backend: "pi".into(),
                circle: "one".into(),
                status: "online".into(),
                description: String::new(),
                session_id: String::new(),
                last_seen: now_unix(),
            },
        );
        hub.jobs.insert(
            "job-new".into(),
            Job {
                job_id: "job-new".into(),
                title: "n".into(),
                prompt: String::new(),
                path: String::new(),
                backend: "pi".into(),
                assigned_peer: None,
                state: "queued".into(),
                result_summary: None,
                circle: "one".into(),
            },
        );
        persist(&mut hub).unwrap();
    }
    {
        let mut hub = Hub::open(&path).unwrap();
        assert_eq!(hub.jobs["job-new"].circle, "one");
        assert_eq!(hub.jobs["job-old"].circle, "");
        hub.jobs.remove("job-new");
        persist(&mut hub).unwrap();
    }
    let hub = Hub::open(&path).unwrap();
    assert!(!hub.jobs.contains_key("job-new"));
    let _ = fs::remove_file(&path);
}

#[test]
fn job_list_schema_exposes_circle_and_delete() {
    let tools = mcp_tools();
    let list = tools
        .iter()
        .find(|t| t["name"] == "amesh_job_list")
        .unwrap();
    assert_eq!(
        list["inputSchema"]["properties"]["circle"]["type"],
        "string"
    );
    assert_eq!(
        list["inputSchema"]["properties"]["cross_circle"]["type"],
        "boolean"
    );
    assert!(tools.iter().any(|t| t["name"] == "amesh_job_delete"));
    let sched = tools
        .iter()
        .find(|t| t["name"] == "amesh_schedule_list")
        .unwrap();
    assert_eq!(
        sched["inputSchema"]["properties"]["circle"]["type"],
        "string"
    );
}

#[tokio::test]
async fn job_and_schedule() {
    let app = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "backend": "codex"}),
    )
    .await;

    let (st, job) = json_req(
        app.clone(),
        "POST",
        "/jobs",
        json!({"title": "t", "prompt": "do", "backend": "claude-code"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(job["state"], "queued");
    assert_eq!(job["backend"], "claude-code");
    let id = job["job_id"].as_str().unwrap();

    let (st, job) = json_req(
        app.clone(),
        "PATCH",
        &format!("/jobs/{id}"),
        json!({"state": "done", "result_summary": "ok"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(job["state"], "done");

    let (st, sched) = json_req(
        app.clone(),
        "POST",
        "/schedules",
        json!({"to_peer": "worker", "text": "wake", "in_seconds": 60}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sid = sched["schedule_id"].as_str().unwrap();

    let (st, _) = json_req(app, "DELETE", &format!("/schedules/{sid}"), json!({})).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn ws_unicast_not_broadcast() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = std::env::temp_dir().join(format!("amesh-ws-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let http = router(state.clone());
    let serve_app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });

    let _ = json_req(
        http.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "backend": "pi", "peer_id": "p-worker"}),
    )
    .await;
    let _ = json_req(
        http.clone(),
        "POST",
        "/peers",
        json!({"name": "other", "backend": "pi", "peer_id": "p-other"}),
    )
    .await;

    let (ws_w, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut w_write, mut w_read) = ws_w.split();
    w_write
        .send(WsMsg::Text(
            r#"{"type":"connect","peer_id":"p-worker"}"#.into(),
        ))
        .await
        .unwrap();
    let connected = w_read.next().await.unwrap().unwrap();
    assert!(connected.to_string().contains("connected"));

    let (ws_o, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut o_write, mut o_read) = ws_o.split();
    o_write
        .send(WsMsg::Text(
            r#"{"type":"connect","peer_id":"p-other"}"#.into(),
        ))
        .await
        .unwrap();
    let _ = o_read.next().await;

    let _ = json_req(
        http,
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "hi"}),
    )
    .await;
    let got = tokio::time::timeout(std::time::Duration::from_millis(800), w_read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(got.to_string().contains("hi"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), o_read.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn jobs_persist_and_pending_is_id_only() {
    let path = std::env::temp_dir().join(format!("amesh-persist-{}.db", Uuid::new_v4()));
    let app = router(App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    });
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "alias-b", "backend": "pi", "peer_id": "peer-a", "circle": "default"}),
    )
    .await;
    let _ = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "alias-b", "text": "for-a"}),
    )
    .await;
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "other", "backend": "pi", "peer_id": "alias-b", "circle": "default"}),
    )
    .await;
    let (_, pending) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=alias-b",
        json!({}),
    )
    .await;
    assert_eq!(pending["asks"].as_array().unwrap().len(), 0);

    let (st, _job) = json_req(
        app,
        "POST",
        "/jobs",
        json!({"title": "keep", "prompt": "x"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let n: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM jobs WHERE title = 'keep'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(n >= 1);
}

#[tokio::test]
async fn inbox_caps_at_50() {
    let app = test_app();
    let (_, reg) = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "sink", "backend": "pi", "circle": "default"}),
    )
    .await;
    let peer_id = reg["peer_id"].as_str().unwrap().to_string();
    for i in 0..51 {
        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/notify",
            json!({"from_peer": "boss", "to_peer": "sink", "message": format!("m{i}")}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }
    let (_, pending) = json_req(
        app,
        "GET",
        &format!("/asks/pending?peer_id={peer_id}"),
        json!({}),
    )
    .await;
    assert_eq!(pending["inbox"].as_array().unwrap().len(), 50);
}

#[tokio::test]
async fn job_assigned_peer_and_chat_go_to_inbox() {
    let app = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "worker", "backend": "pi"}),
    )
    .await;
    let (st, job) = json_req(
        app.clone(),
        "POST",
        "/jobs",
        json!({"title": "do-it", "assigned_peer": "worker"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let jid = job["job_id"].as_str().unwrap();
    let (_, pending) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=worker",
        json!({}),
    )
    .await;
    let inbox = pending["inbox"].as_array().unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0]["type"], "notify");
    assert!(inbox[0]["message"].as_str().unwrap().contains(jid));
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/events/chat",
        json!({"peer": "worker", "role": "user", "text": "chat-hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, pending) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=worker",
        json!({}),
    )
    .await;
    let inbox = pending["inbox"].as_array().unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0]["message"], "chat-hi");
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/events/chat_delta",
        json!({"peer": "worker", "role": "user", "text": "tok"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=worker", json!({})).await;
    assert!(pending["inbox"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn job_notifies_pi_codex_and_claude_assignees() {
    let app = test_app();
    for (id, backend) in [("p", "pi"), ("x", "codex"), ("c", "claude-code")] {
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": id, "peer_id": id, "backend": backend}),
        )
        .await;
        let (st, job) = json_req(
            app.clone(),
            "POST",
            "/jobs",
            json!({"title": format!("t-{id}"), "assigned_peer": id, "backend": backend}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{id}");
        let jid = job["job_id"].as_str().unwrap();
        let (_, pending) = json_req(
            app.clone(),
            "GET",
            &format!("/asks/pending?peer_id={id}"),
            json!({}),
        )
        .await;
        let inbox = pending["inbox"].as_array().unwrap();
        assert_eq!(inbox.len(), 1, "{id} {inbox:?}");
        assert_eq!(inbox[0]["type"], "notify");
        assert!(inbox[0]["message"].as_str().unwrap().contains(jid));
    }
}

#[tokio::test]
async fn job_without_assignee_does_not_notify() {
    let app = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "peer_id": "worker", "backend": "pi"}),
    )
    .await;
    let (st, _) = json_req(app.clone(), "POST", "/jobs", json!({"title": "orphan"})).await;
    assert_eq!(st, StatusCode::OK);
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=worker", json!({})).await;
    assert!(pending["inbox"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ask_many_events_and_wait() {
    let app = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "a", "backend": "pi", "peer_id": "a", "circle": "default"}),
    )
    .await;
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "b", "backend": "pi", "peer_id": "b", "circle": "default"}),
    )
    .await;
    let (st, many) = json_req(
        app.clone(),
        "POST",
        "/ask-many",
        json!({"from_peer": "a", "to_peers": ["b"], "text": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cid = many["asks"][0].as_str().unwrap();
    let parent = many["parent_id"].as_str().unwrap();
    let (st, batch) = json_req(
        app.clone(),
        "GET",
        &format!("/ask-many/{parent}"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(batch["asks"].as_array().unwrap().len(), 1);
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/events/chat",
        json!({"peer": "b", "role": "user", "text": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, ev) = json_req(app.clone(), "GET", "/events", json!({})).await;
    assert_eq!(st, StatusCode::OK);
    let events = ev.as_array().unwrap();
    assert!(events.iter().any(|e| e["type"] == "chat"));
    assert!(events.iter().any(|e| e["type"] == "ask"));
    let (st, _) = json_req(
        app.clone(),
        "POST",
        &format!("/asks/{cid}/wait"),
        json!({"timeout_seconds": 0}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = json_req(app.clone(), "POST", "/peers/b/mcp", json!({"name": "demo"})).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn blocking_answer_session_and_delta() {
    let app = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "a", "backend": "pi", "peer_id": "a", "circle": "default", "session_id": "sess-a"}),
    )
    .await;
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "b", "backend": "pi", "peer_id": "b", "circle": "default"}),
    )
    .await;
    let (st, blocked) = json_req(
        app.clone(),
        "POST",
        "/questions/ask-blocking",
        json!({"prompt": "choose", "to_peer": "b", "from_peer": "a", "timeout_seconds": 0}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cid = blocked["correlation_id"].as_str().unwrap_or("");
    if !cid.is_empty() {
        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/answer",
            json!({"correlation_id": cid, "message": "ok"}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/events/chat_delta",
        json!({"peer": "b", "role": "assistant", "text": "chunk"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/sessions/sess-a/controls/notify",
        json!({"text": "hello", "to_peer": "b"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = json_req(
        app.clone(),
        "GET",
        "/deliveries/pending?peer_id=b",
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, tl) = json_req(app.clone(), "GET", "/peers/b/timeline", json!({})).await;
    assert_eq!(st, StatusCode::OK);
    assert!(tl.get("asks").is_some());
    let (st, _) = json_req(app, "DELETE", "/peers/b/mcp/demo", json!({})).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn attachment_get_returns_bytes() {
    let app = test_app();
    let (st, body) = json_req(
        app.clone(),
        "POST",
        "/attachments",
        json!({"filename": "n.txt", "content_base64": "aGk="}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = body["id"].as_str().unwrap();
    let req = Request::builder()
        .method("GET")
        .uri(format!("/attachments/{id}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"hi");
}

#[tokio::test]
async fn peer_mcp_survives_reopen() {
    let path = std::env::temp_dir().join(format!("amesh-mcp-{}.db", Uuid::new_v4()));
    {
        let app = router(App {
            inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
            token: None,
            state_path: path.clone(),
        });
        let _ = json_req(
            app.clone(),
            "POST",
            "/peers",
            json!({"name": "w", "peer_id": "w", "backend": "pi"}),
        )
        .await;
        let (st, _) = json_req(
            app,
            "POST",
            "/peers/w/mcp",
            json!({"name": "demo", "command": "echo"}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }
    let hub = Hub::open(&path).unwrap();
    assert_eq!(hub.mcp_servers["w"][0]["name"], "demo");
}

#[tokio::test]
async fn an_upload_stays_next_to_its_own_state_file() {
    let dir = std::env::temp_dir().join(format!("amesh-attach-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.db");
    let app = router(App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    });
    let home_dir = dirs_home().join(".amesh").join("attachments");
    let home_before = fs::read_dir(&home_dir).map(|d| d.count()).unwrap_or(0);
    let boundary = "----ameshform";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"n.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri("/attachments/form")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let uploaded: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = uploaded["id"].as_str().unwrap();
    assert!(
        dir.join("attachments").join(id).is_file(),
        "the upload belongs beside the state file that owns it"
    );
    let home_after = fs::read_dir(&home_dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(
        home_after, home_before,
        "a daemon on its own state file must not write into the operator's home"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn attachment_form_upload_then_get_bytes() {
    let app = test_app();
    let boundary = "----ameshform";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"n.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri("/attachments/form")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let uploaded: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = uploaded["id"].as_str().unwrap();
    assert_eq!(uploaded["filename"], "n.txt");
    let get = Request::builder()
        .method("GET")
        .uri(format!("/attachments/{id}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(get).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"hi");
}

#[tokio::test]
async fn health_register_alias_and_session_resume() {
    let app = test_app();
    let (st, health) = json_req(app.clone(), "GET", "/health", json!({})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(health["ok"], true);
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/peer/register",
        json!({
            "name": "sess",
            "peer_id": "sess-peer",
            "backend": "pi",
            "session_id": "sess-live"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "other", "peer_id": "other", "backend": "pi"}),
    )
    .await;
    let (st, n) = json_req(
        app,
        "POST",
        "/sessions/resume",
        json!({
            "session_id": "sess-live",
            "text": "wake",
            "to_peer": "other"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(n["ok"], true);
}

#[tokio::test]
async fn a_dead_peer_name_is_reclaimed_on_reregister() {
    let path = std::env::temp_dir().join(format!("amesh-reclaim-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path,
    };
    let http = router(state.clone());
    let body = json!({"path": "/tmp/proj-x", "backend": "codex", "circle": "default"});
    let (_, first) = json_req(http.clone(), "POST", "/peers", body.clone()).await;
    assert_eq!(first["peer_id"], "proj-x-codex");
    {
        let mut hub = state.inner.lock().await;
        hub.peers.get_mut("proj-x-codex").unwrap().last_seen = 1;
    }
    let (_, again) = json_req(http, "POST", "/peers", body).await;
    assert_eq!(
        again["peer_id"], "proj-x-codex",
        "a restarted runtime must get its dead predecessor's name back, not -2"
    );
}

#[tokio::test]
async fn a_socketless_predecessor_is_reclaimed_before_prune() {
    let path = std::env::temp_dir().join(format!("amesh-reclaim2-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path,
    };
    let http = router(state.clone());
    let body = json!({"path": "/tmp/proj-y", "backend": "codex", "circle": "default"});
    let (_, first) = json_req(http.clone(), "POST", "/peers", body.clone()).await;
    assert_eq!(first["peer_id"], "proj-y-codex");
    let (_, fresh) = json_req(http.clone(), "POST", "/peers", body.clone()).await;
    assert_eq!(
        fresh["peer_id"], "proj-y-codex-2",
        "a fresh sibling in the same path stays distinct"
    );
    {
        let mut hub = state.inner.lock().await;
        hub.peers.remove("proj-y-codex-2");
        let prev = hub.peers.get_mut("proj-y-codex").unwrap();
        prev.status = "offline".into();
    }
    let (_, again) = json_req(http.clone(), "POST", "/peers", body.clone()).await;
    assert_eq!(
        again["peer_id"], "proj-y-codex",
        "an offline predecessor is reclaimed at once, no 30s wait"
    );
    /* keep the receiver alive: a dropped receiver reads as a closed socket and probe_peers
    would prune it, which is exactly the dead case this assertion must not exercise */
    let (tx, _rx) = mpsc::unbounded_channel();
    {
        let mut hub = state.inner.lock().await;
        hub.sockets.insert("proj-y-codex".into(), (1, tx));
        let prev = hub.peers.get_mut("proj-y-codex").unwrap();
        prev.status = "offline".into();
    }
    let (_, live) = json_req(http.clone(), "POST", "/peers", body.clone()).await;
    assert_eq!(
        live["peer_id"], "proj-y-codex-2",
        "a predecessor holding a socket is alive and must not be stolen"
    );
    state.inner.lock().await.sockets.remove("proj-y-codex");
    let mut bound = body.clone();
    bound["session_id"] = json!("hook-session");
    let (_, hooked) = json_req(http, "POST", "/peers", bound).await;
    assert_eq!(
        hooked["peer_id"], "proj-y-codex-3",
        "a session-bound registration never reclaims a sessionless name"
    );
}
#[tokio::test]
async fn peer_list_drops_stale() {
    let path = std::env::temp_dir().join(format!("amesh-stale-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path,
    };
    let http = router(state.clone());
    let _ = json_req(
        http.clone(),
        "POST",
        "/peers",
        json!({"name": "ghost", "peer_id": "ghost", "backend": "pi"}),
    )
    .await;
    {
        let mut hub = state.inner.lock().await;
        hub.peers.get_mut("ghost").unwrap().last_seen = 1;
    }
    let (_, list) = json_req(http, "GET", "/peers", json!({})).await;
    let peers = list.as_array().unwrap();
    assert!(peers.iter().all(|peer| peer["peer_id"] != "ghost"));
}

fn undelivered_hub() -> Hub {
    let path = std::env::temp_dir().join(format!("amesh-undeliv-{}.db", Uuid::new_v4()));
    let mut hub = Hub::open(&path).unwrap();
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
    hub
}

#[test]
fn undelivered_events_for_an_unknown_peer_are_dropped() {
    let mut hub = undelivered_hub();
    let owed = vec![json!({"type": "notify", "id": "x"})];
    assert!(!return_undelivered(&mut hub, "ghost", owed));
    assert!(
        hub.inbox.get("ghost").is_none(),
        "unknown canonical peer_id must not grow an inbox"
    );
}

#[test]
fn undelivered_events_return_to_the_inbox() {
    let mut hub = undelivered_hub();
    let owed = vec![
        json!({"type": "notify", "id": "a"}),
        json!({"type": "notify", "id": "b"}),
    ];
    assert!(return_undelivered(&mut hub, "worker", owed));
    let queued = hub.inbox.get("worker").expect("events must be recoverable");
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0]["id"], "a");
    assert_eq!(queued[1]["id"], "b");
}

#[test]
fn undelivered_events_reach_the_successor_socket() {
    let mut hub = undelivered_hub();
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (7, tx));
    let owed = vec![json!({"type": "notify", "id": "a"})];
    assert!(!return_undelivered(&mut hub, "worker", owed));
    assert_eq!(rx.try_recv().unwrap()["id"], "a");
    assert!(
        hub.inbox.get("worker").is_none(),
        "successor must not double-queue"
    );
}

#[test]
fn undelivered_events_survive_a_dead_successor() {
    let mut hub = undelivered_hub();
    let (tx, rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (7, tx));
    drop(rx);
    let owed = vec![
        json!({"type": "notify", "id": "a"}),
        json!({"type": "notify", "id": "b"}),
    ];
    assert!(
        return_undelivered(&mut hub, "worker", owed),
        "falling back to the inbox changes hub state and must be persisted"
    );
    let queued = hub
        .inbox
        .get("worker")
        .expect("a dead successor must not eat events");
    assert_eq!(
        queued.len(),
        2,
        "the rejected event and the rest must both survive"
    );
    assert_eq!(queued[0]["id"], "a");
    assert_eq!(queued[1]["id"], "b");
    assert!(
        hub.sockets.get("worker").is_none(),
        "the dead socket must be dropped"
    );
}

#[test]
fn displaced_notice_is_never_replayed() {
    let mut hub = undelivered_hub();
    let notice = vec![json!({"type": DISPLACED, "peer_id": "worker"})];
    assert!(!return_undelivered(&mut hub, "worker", notice.clone()));
    assert!(
        hub.inbox.get("worker").is_none(),
        "notice must not reach the inbox"
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (7, tx));
    let mixed = vec![notice[0].clone(), json!({"type": "notify", "id": "real"})];
    assert!(!return_undelivered(&mut hub, "worker", mixed));
    assert_eq!(rx.try_recv().unwrap()["id"], "real");
    assert!(
        rx.try_recv().is_err(),
        "only the real event may be forwarded"
    );
}

#[tokio::test]
async fn a_due_schedule_waits_for_an_absent_target() {
    let path = std::env::temp_dir().join(format!("amesh-sched-{}.db", Uuid::new_v4()));
    let app_for = |hub: Hub| App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.clone(),
    };
    let later = |seen: u64| Peer {
        peer_id: "later".into(),
        name: "later".into(),
        path: "/tmp".into(),
        backend: "pi".into(),
        circle: "default".into(),
        status: "online".into(),
        description: String::new(),
        session_id: String::new(),
        last_seen: seen,
    };
    let due = now_unix() - 5;
    let state = app_for(Hub::open(&path).unwrap());
    {
        let mut hub = state.inner.lock().await;
        hub.schedules.insert(
            "s1".into(),
            Schedule {
                schedule_id: "s1".into(),
                from_peer: "boss".into(),
                to_peer: "later".into(),
                text: "wake".into(),
                kind: "notify".into(),
                fire_at: due,
                every_seconds: None,
                circle: "default".into(),
            },
        );
        persist(&mut hub).unwrap();
    }
    tick_schedules(&state).await;
    drop(state);
    /* the hub that was waiting goes away; the one that comes back must still be waiting */
    let reopened = Hub::open(&path).unwrap();
    assert!(
        reopened.schedules.contains_key("s1"),
        "a due schedule must wait for its target instead of firing into nothing"
    );
    assert_eq!(
        reopened.schedules["s1"].fire_at, due,
        "fire_at must not move while the target is away"
    );
    assert!(
        !reopened.inbox.contains_key("later"),
        "waiting must not open an inbox for a peer that is not there"
    );
    let state = app_for(reopened);
    state
        .inner
        .lock()
        .await
        .peers
        .insert("later".into(), later(now_unix()));
    tick_schedules(&state).await;
    tick_schedules(&state).await;
    {
        let hub = state.inner.lock().await;
        assert!(
            !hub.schedules.contains_key("s1"),
            "a one-shot schedule is consumed once it fires"
        );
        let held = &hub.inbox["later"];
        assert_eq!(
            held.len(),
            1,
            "a second tick must not deliver a consumed schedule again"
        );
        assert_eq!(held[0]["type"], "notify");
        assert_eq!(held[0]["message"], "wake");
    }
    let settled = Hub::open(&path).unwrap();
    assert!(!settled.schedules.contains_key("s1"));
    assert_eq!(
        settled.inbox["later"].len(),
        1,
        "the delivery has to be on disk, not only in memory"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn the_sweep_keeps_every_known_peer_and_clears_the_rest() {
    let dir = std::env::temp_dir().join(format!("amesh-sweep-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.db");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    {
        let mut hub = state.inner.lock().await;
        hub.peers.insert(
            "known".into(),
            Peer {
                peer_id: "known".into(),
                name: "known".into(),
                path: "/tmp".into(),
                backend: "pi".into(),
                circle: "default".into(),
                status: "offline".into(),
                description: String::new(),
                session_id: String::new(),
                last_seen: now_unix(),
            },
        );
    }
    let kept = dir.join("hook-ws-known.inbox");
    let gone = dir.join("hook-ws-forgotten.inbox");
    let dead = dir.join("ws-forgotten.pid");
    fs::write(&kept, "sock\n1\n").unwrap();
    fs::write(&gone, "sock\n1\n").unwrap();
    fs::write(&dead, "999999999\n").unwrap();
    fs::write(dir.join("state.db-wal"), "x").unwrap();
    sweep_runtime_files(&state).await;
    assert!(
        kept.exists(),
        "a peer the hub knows, online or not, keeps its stamp"
    );
    assert!(!gone.exists(), "a stamp nobody owns is cleared");
    assert!(!dead.exists(), "a pid file for a dead process is cleared");
    assert!(
        path.exists() && dir.join("state.db-wal").exists(),
        "state files are never touched"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_sweep_caps_oversized_logs_in_place() {
    let dir = std::env::temp_dir().join(format!("amesh-sweep-cap-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.db");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    {
        let mut hub = state.inner.lock().await;
        hub.peers.insert(
            "known".into(),
            Peer {
                peer_id: "known".into(),
                name: "known".into(),
                path: "/tmp".into(),
                backend: "pi".into(),
                circle: "default".into(),
                status: "offline".into(),
                description: String::new(),
                session_id: String::new(),
                last_seen: now_unix(),
            },
        );
    }
    let kept = dir.join("hook-ws-known.log");
    let serve = dir.join("serve.log");
    let gone = dir.join("hook-ws-forgotten.log");
    fs::write(&kept, vec![b'x'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    fs::write(&serve, vec![b'y'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    fs::write(&gone, vec![b'z'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    sweep_runtime_files(&state).await;
    assert!(kept.exists(), "a known peer keeps its log");
    assert_eq!(
        fs::metadata(&kept).unwrap().len(),
        0,
        "oversize live log is truncated in place"
    );
    assert_eq!(
        fs::metadata(&serve).unwrap().len(),
        0,
        "serve.log is truncated in place"
    );
    assert!(
        !gone.exists(),
        "an unowned oversize log is still leftover garbage"
    );
    let _ = fs::remove_dir_all(&dir);
}

/* an acknowledging peer bound to a session that dropped off with two records owed */
fn owed_hub(path: &Path, session: &str) -> Hub {
    let mut hub = Hub::open(path).unwrap();
    let _rx = recv_peer(&mut hub, "tmp-pi");
    {
        let peer = hub.peers.get_mut("tmp-pi").unwrap();
        peer.session_id = session.into();
        peer.backend = "pi".into();
        peer.path = "/tmp".into();
    }
    hub.sockets.remove("tmp-pi");
    hub.recv_live.remove("tmp-pi");
    for text in ["owed-1", "owed-2"] {
        persist_then_deliver(
            &mut hub,
            "tmp-pi",
            json!({"type": "notify", "message": text}),
        )
        .unwrap();
    }
    hub.peers.get_mut("tmp-pi").unwrap().last_seen = 1;
    hub
}

#[tokio::test]
async fn a_runtime_that_restarts_with_its_session_keeps_its_name() {
    /* the old instance is still attached when the new one announces itself with the
    same session: the name must not drift to -2, the old socket is displaced later */
    let path = std::env::temp_dir().join(format!("amesh-takeover-{}.db", Uuid::new_v4()));
    let mut hub = Hub::open(&path).unwrap();
    let _old_rx = recv_peer(&mut hub, "tmp-codex");
    {
        let old = hub.peers.get_mut("tmp-codex").unwrap();
        old.session_id = "S-codex".into();
        old.backend = "codex".into();
        old.path = "/tmp".into();
    }
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.clone(),
    };
    let (status, body) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"path": "/tmp", "backend": "codex", "session_id": "S-codex"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["peer_id"], "tmp-codex",
        "the same session gets the same name, not a suffix"
    );
    let hub = state.inner.lock().await;
    assert_eq!(hub.peers.len(), 1, "one runtime, one row");
    assert!(!hub.peers.contains_key("tmp-codex-2"));
    drop(hub);
    let (_, body) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"path": "/tmp", "backend": "codex", "session_id": "S-other"}),
    )
    .await;
    assert_eq!(
        body["peer_id"], "tmp-codex-2",
        "a different session in the same folder is a different peer"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn a_session_bound_backlog_outlives_its_pruned_row() {
    let path = std::env::temp_dir().join(format!("amesh-owed1-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&path, "S1");
    let before = now_unix();
    assert!(refresh_peers(&mut hub));
    assert!(
        !hub.peers.contains_key("tmp-pi"),
        "the row itself is pruned as before"
    );
    assert_eq!(
        hub.inbox["tmp-pi"].len(),
        2,
        "what the session was owed stays behind for it"
    );
    assert!(hub.recv_known.contains("tmp-pi"));
    let owed = &hub.owed["tmp-pi"];
    assert_eq!(owed.owner, "S1");
    assert!(owed.since >= before);
    /* a peer that never bound a session has nobody to keep the backlog for */
    let sessionless = std::env::temp_dir().join(format!("amesh-owed1b-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&sessionless, "");
    assert!(refresh_peers(&mut hub));
    assert!(!hub.inbox.contains_key("tmp-pi"));
    assert!(!hub.owed.contains_key("tmp-pi"));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&sessionless);
}

#[test]
fn a_pruned_backlog_keeps_its_clock_across_a_restart() {
    let path = std::env::temp_dir().join(format!("amesh-owed2-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&path, "S1");
    refresh_peers(&mut hub);
    let since = now_unix() - 100;
    hub.owed.get_mut("tmp-pi").unwrap().since = since;
    persist(&mut hub).unwrap();
    let mut reopened = Hub::open(&path).unwrap();
    assert_eq!(
        reopened.owed["tmp-pi"].since, since,
        "the clock started at the first prune and does not restart"
    );
    assert_eq!(reopened.owed["tmp-pi"].owner, "S1");
    assert_eq!(reopened.inbox["tmp-pi"].len(), 2);
    assert!(reopened.recv_known.contains("tmp-pi"));
    assert!(
        !refresh_peers(&mut reopened) || reopened.owed["tmp-pi"].since == since,
        "a refresh must not restart it either"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn another_session_cannot_take_a_name_with_a_waiting_backlog() {
    let path = std::env::temp_dir().join(format!("amesh-owed3-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&path, "S1");
    refresh_peers(&mut hub);
    assert_eq!(
        allocate_peer_id(&hub, "/tmp", "pi", "S2", None),
        "tmp-pi-2",
        "a newcomer in the same folder must not inherit the name and its backlog"
    );
    assert_eq!(
        allocate_peer_id(&hub, "/tmp", "pi", "", None),
        "tmp-pi-2",
        "nor may a sessionless newcomer"
    );
    assert_eq!(
        allocate_peer_id(&hub, "/tmp", "pi", "S1", None),
        "tmp-pi",
        "the owner gets its name back"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn claiming_the_name_with_another_session_discards_the_backlog() {
    let path = std::env::temp_dir().join(format!("amesh-owed3b-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&path, "S1");
    refresh_peers(&mut hub);
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.clone(),
    };
    let (status, body) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"peer_id": "tmp-pi", "name": "tmp-pi", "path": "/tmp", "backend": "pi", "session_id": "S2"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["peer_id"], "tmp-pi");
    let hub = state.inner.lock().await;
    assert!(
        !hub.inbox.contains_key("tmp-pi"),
        "another session's messages are not handed over"
    );
    assert!(!hub.owed.contains_key("tmp-pi"));
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn the_owner_session_comes_back_to_its_backlog() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = std::env::temp_dir().join(format!("amesh-owed4-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&path, "S1");
    refresh_peers(&mut hub);
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    /* more than 30s later, the same session registers again from the same folder */
    let (status, body) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"path": "/tmp", "backend": "pi", "session_id": "S1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["peer_id"], "tmp-pi",
        "the owner gets its own name back"
    );
    {
        let hub = state.inner.lock().await;
        assert!(!hub.owed.contains_key("tmp-pi"), "adopted");
        assert_eq!(
            hub.inbox["tmp-pi"].len(),
            2,
            "and everything it was owed is still there"
        );
    }
    let (socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut w, mut r) = socket.split();
    w.send(WsMsg::Text(
        r#"{"type":"connect","peer_id":"tmp-pi","recv":true}"#.into(),
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
    let mut got = Vec::new();
    for _ in 0..2 {
        let frame = tokio::time::timeout(Duration::from_secs(3), r.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&frame.to_string()).unwrap();
        got.push((
            v["message"].as_str().unwrap().to_string(),
            v["id"].as_str().unwrap().to_string(),
        ));
    }
    assert_eq!(got[0].0, "owed-1");
    assert_eq!(got[1].0, "owed-2");
    for (_, id) in &got {
        w.send(WsMsg::Text(
            json!({"type": "recv", "id": id}).to_string().into(),
        ))
        .await
        .unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.inner.lock().await.inbox.contains_key("tmp-pi") {
        assert!(
            std::time::Instant::now() < deadline,
            "recv never drained the replayed backlog"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn an_unproven_row_gets_no_replay_and_no_hook_leak_until_its_session_proves_it() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = std::env::temp_dir().join(format!("amesh-owed5-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&path, "S1");
    refresh_peers(&mut hub);
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    /* a runtime that announces itself without a session, the way an MCP does first */
    let (status, body) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"peer_id": "tmp-pi", "name": "tmp-pi", "path": "/tmp", "backend": "pi"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["peer_id"], "tmp-pi");
    assert!(
        state.inner.lock().await.owed.contains_key("tmp-pi"),
        "identity is still open"
    );
    let (status, body) = json_req(
        router(state.clone()),
        "GET",
        "/asks/pending?peer_id=tmp-pi",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["inbox"].as_array().map(Vec::len),
        Some(0),
        "the hook must not leak an unsettled backlog"
    );
    assert_eq!(state.inner.lock().await.inbox["tmp-pi"].len(), 2);
    let (socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut w, mut r) = socket.split();
    w.send(WsMsg::Text(
        r#"{"type":"connect","peer_id":"tmp-pi","recv":true}"#.into(),
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
    let nothing = tokio::time::timeout(Duration::from_millis(500), r.next()).await;
    assert!(
        nothing.is_err(),
        "nothing is replayed before the owner is proved: {nothing:?}"
    );
    assert_eq!(state.inner.lock().await.inbox["tmp-pi"].len(), 2);
    /* the hook binds the session on the first turn: the owner is proved while attached */
    let (status, _) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"peer_id": "tmp-pi", "name": "tmp-pi", "path": "/tmp", "backend": "pi", "session_id": "S1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut ids = Vec::new();
    for _ in 0..2 {
        let frame = tokio::time::timeout(Duration::from_secs(3), r.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&frame.to_string()).unwrap();
        ids.push(v["id"].as_str().unwrap().to_string());
    }
    for id in &ids {
        w.send(WsMsg::Text(
            json!({"type": "recv", "id": id}).to_string().into(),
        ))
        .await
        .unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.inner.lock().await.inbox.contains_key("tmp-pi") {
        assert!(
            std::time::Instant::now() < deadline,
            "recv never drained the adopted backlog"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = fs::remove_file(&path);
}

/* a session S left a backlog behind under the pruned name Y: two notifies and one open
ask that still points at Y */
fn abandoned_hub(path: &Path) -> Hub {
    let mut hub = owed_hub(path, "S1");
    hub.asks.insert(
        "ask-y".into(),
        Ask {
            correlation_id: "ask-y".into(),
            from_peer: "boss".into(),
            to_peer: "tmp-pi".into(),
            to_peer_id: "tmp-pi".into(),
            text: "still open".into(),
            open: true,
            reply: None,
        },
    );
    persist_then_deliver(
        &mut hub,
        "tmp-pi",
        json!({"type": "ask", "correlation_id": "ask-y", "text": "still open"}),
    )
    .unwrap();
    refresh_peers(&mut hub);
    assert!(hub.owed.contains_key("tmp-pi"));
    hub
}

#[tokio::test]
async fn a_backlog_follows_its_session_to_a_new_name() {
    let path = std::env::temp_dir().join(format!("amesh-follow-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(abandoned_hub(&path))),
        token: None,
        state_path: path.clone(),
    };
    let (status, body) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"peer_id": "tmp-pi-2", "name": "tmp-pi-2", "path": "/tmp", "backend": "pi", "session_id": "S1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["peer_id"], "tmp-pi-2",
        "the name the runtime registered under does not change"
    );
    {
        let hub = state.inner.lock().await;
        let moved = &hub.inbox["tmp-pi-2"];
        assert_eq!(
            moved.len(),
            3,
            "everything left behind for the session is now under its new name"
        );
        assert!(
            moved
                .iter()
                .all(|r| r["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false)),
            "every moved record can be acknowledged"
        );
        assert!(
            !hub.owed.contains_key("tmp-pi")
                && !hub.inbox.contains_key("tmp-pi")
                && !hub.recv_known.contains("tmp-pi"),
            "nothing stays under the old name"
        );
        assert!(hub.recv_known.contains("tmp-pi-2"));
        assert_eq!(
            hub.asks["ask-y"].to_peer_id, "tmp-pi-2",
            "an open ask is re-pointed at the new name"
        );
    }
    let (_, pending) = json_req(
        router(state.clone()),
        "GET",
        "/asks/pending?peer_id=tmp-pi-2",
        json!({}),
    )
    .await;
    assert_eq!(
        pending["asks"].as_array().map(Vec::len),
        Some(1),
        "the new name sees the ask as pending"
    );
    assert_eq!(
        pending["inbox"].as_array().map(Vec::len),
        Some(3),
        "nobody attached, so the hook delivers the backlog"
    );
    let reopened = Hub::open(&path).unwrap();
    assert!(
        !reopened.owed.contains_key("tmp-pi") && !reopened.inbox.contains_key("tmp-pi"),
        "the move is on disk"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_backlog_does_not_follow_another_session_or_no_session() {
    let path = std::env::temp_dir().join(format!("amesh-nofollow-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(abandoned_hub(&path))),
        token: None,
        state_path: path.clone(),
    };
    for body in [
        json!({"peer_id": "tmp-pi-2", "name": "tmp-pi-2", "path": "/tmp", "backend": "pi", "session_id": "S2"}),
        json!({"peer_id": "tmp-pi-3", "name": "tmp-pi-3", "path": "/tmp", "backend": "pi"}),
    ] {
        let (status, _) = json_req(router(state.clone()), "POST", "/peers", body).await;
        assert_eq!(status, StatusCode::OK);
        let hub = state.inner.lock().await;
        assert!(
            hub.owed.contains_key("tmp-pi"),
            "only the owning session may take the backlog"
        );
        assert_eq!(hub.inbox["tmp-pi"].len(), 3);
        assert!(!hub.inbox.contains_key("tmp-pi-2") && !hub.inbox.contains_key("tmp-pi-3"));
        assert_eq!(hub.asks["ask-y"].to_peer_id, "tmp-pi");
    }
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_moved_backlog_is_deduplicated_by_id_and_only_the_new_part_is_pushed() {
    let path = std::env::temp_dir().join(format!("amesh-dedupe-{}.db", Uuid::new_v4()));
    let mut hub = abandoned_hub(&path);
    let duplicate = hub.inbox["tmp-pi"][0].clone();
    /* the new name is already attached and acknowledging, with one record in flight and
    one that is the same as a record the old name holds */
    let mut rx = recv_peer(&mut hub, "tmp-pi-2");
    hub.peers.get_mut("tmp-pi-2").unwrap().session_id = "S1".into();
    persist_then_deliver(
        &mut hub,
        "tmp-pi-2",
        json!({"type": "notify", "message": "already-in-flight"}),
    )
    .unwrap();
    let _ = rx.try_recv().unwrap();
    hub.inbox
        .get_mut("tmp-pi-2")
        .unwrap()
        .push(duplicate.clone());
    let state = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.clone(),
    };
    let (status, _) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"peer_id": "tmp-pi-2", "name": "tmp-pi-2", "path": "/tmp", "backend": "pi", "session_id": "S1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let hub = state.inner.lock().await;
    let ids: Vec<String> = hub.inbox["tmp-pi-2"]
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "a record the new name already holds is not added again: {ids:?}"
    );
    assert_eq!(
        ids.len(),
        4,
        "in-flight + duplicate + two genuinely new records"
    );
    let mut pushed = Vec::new();
    while let Ok(copy) = rx.try_recv() {
        pushed.push(copy["id"].as_str().unwrap().to_string());
    }
    assert_eq!(
        pushed.len(),
        2,
        "only what was newly moved goes down the socket, not the queue it already had: {pushed:?}"
    );
    assert!(!pushed.contains(&duplicate["id"].as_str().unwrap().to_string()));
    let _ = fs::remove_file(&path);
}

#[test]
fn a_backlog_nobody_returns_for_expires() {
    let path = std::env::temp_dir().join(format!("amesh-owed6-{}.db", Uuid::new_v4()));
    let mut hub = owed_hub(&path, "S1");
    refresh_peers(&mut hub);
    hub.owed.get_mut("tmp-pi").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
    assert!(refresh_peers(&mut hub));
    assert!(!hub.owed.contains_key("tmp-pi"));
    assert!(
        !hub.inbox.contains_key("tmp-pi"),
        "an expired backlog is not kept"
    );
    assert!(!hub.recv_known.contains("tmp-pi"));
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn the_hook_does_not_take_what_an_attached_acknowledger_is_still_owed() {
    let path = std::env::temp_dir().join(format!("amesh-pending-{}.db", Uuid::new_v4()));
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
        state.inner.lock().await.inbox["w"].len(),
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
    assert!(!state.inner.lock().await.inbox.contains_key("w"));
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_closing_acknowledging_connection_hands_nothing_back() {
    let path = std::env::temp_dir().join(format!("amesh-recvclose-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    let (mut rx, gen) = {
        let mut hub = state.inner.lock().await;
        let rx = recv_peer(&mut hub, "w");
        let gen = hub.conn_gen;
        persist_then_deliver(&mut hub, "w", json!({"type": "notify", "message": "m"})).unwrap();
        (rx, gen)
    };
    let copy = rx.try_recv().unwrap();
    assert_eq!(state.inner.lock().await.inbox["w"].len(), 1);
    /* the write of that copy fails and the link dies: the record is already in the
    inbox, so handing the copy back must not make a second one */
    close_connection(&state, "w", gen, true, rx, vec![copy]).await;
    let hub = state.inner.lock().await;
    assert_eq!(
        hub.inbox["w"].len(),
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
    let path = std::env::temp_dir().join(format!("amesh-absent-{}.db", Uuid::new_v4()));
    let mut hub = Hub::open(&path).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    hub.sockets.remove("w");
    hub.recv_live.remove("w");
    persist_then_deliver(
        &mut hub,
        "w",
        json!({"type": "ask", "correlation_id": "ask-1", "text": "q"}),
    )
    .unwrap();
    let owed = &hub.inbox["w"];
    assert_eq!(owed.len(), 1);
    assert!(
        owed[0]["id"].as_str().unwrap_or("").starts_with("evt-"),
        "what an absent acknowledger is owed still needs an id it can recv: {owed:?}"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_first_time_acknowledger_gets_ids_on_its_backlog() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = std::env::temp_dir().join(format!("amesh-legacy-{}.db", Uuid::new_v4()));
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
        legacy.inbox["p-worker"][0]["id"].is_null(),
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
        Hub::open(&path).unwrap().inbox["p-worker"][0]["id"].as_str(),
        Some(id.as_str()),
        "the id is on the record on disk, not only on the copy"
    );
    w.send(WsMsg::Text(
        json!({"type": "recv", "id": id}).to_string().into(),
    ))
    .await
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.inner.lock().await.inbox.contains_key("p-worker") {
        assert!(
            std::time::Instant::now() < deadline,
            "recv of a backlog record never drained it"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = fs::remove_file(&path);
}

fn recv_peer(hub: &mut Hub, id: &str) -> mpsc::UnboundedReceiver<Value> {
    hub.peers.insert(
        id.into(),
        Peer {
            peer_id: id.into(),
            name: id.into(),
            path: "/tmp".into(),
            backend: "pi".into(),
            circle: "default".into(),
            status: "online".into(),
            description: String::new(),
            session_id: String::new(),
            last_seen: now_unix(),
        },
    );
    let (tx, rx) = mpsc::unbounded_channel();
    hub.conn_gen += 1;
    let gen = hub.conn_gen;
    hub.sockets.insert(id.into(), (gen, tx));
    hub.recv_live.insert(id.into());
    hub.recv_known.insert(id.into());
    rx
}

#[test]
fn an_acknowledging_peer_is_owed_every_event_until_it_says_recv() {
    let path = std::env::temp_dir().join(format!("amesh-recv-{}.db", Uuid::new_v4()));
    let mut hub = Hub::open(&path).unwrap();
    let mut rx = recv_peer(&mut hub, "w");
    persist_then_deliver(&mut hub, "w", json!({"type": "ack", "message": "reply"})).unwrap();
    let owed = hub.inbox["w"].clone();
    assert_eq!(
        owed.len(),
        1,
        "the record is written before anything goes down the socket"
    );
    let id = owed[0]["id"].as_str().unwrap().to_string();
    assert!(
        id.starts_with("evt-"),
        "an event without an id gets one the peer can name"
    );
    let copy = rx.try_recv().unwrap();
    assert_eq!(
        copy["id"], id,
        "what goes down the socket is a copy of the same record"
    );
    assert_eq!(
        Hub::open(&path).unwrap().inbox["w"].len(),
        1,
        "the record is on disk before the send"
    );
    assert!(acknowledge_event(&mut hub, "w", &id));
    assert!(!hub.inbox.contains_key("w"), "recv takes the record away");
    assert!(
        !acknowledge_event(&mut hub, "w", &id),
        "a repeated recv is a no-op"
    );
    assert!(
        !acknowledge_event(&mut hub, "w", "evt-nope"),
        "an unknown id is a no-op"
    );
    persist(&mut hub).unwrap();
    assert!(!Hub::open(&path).unwrap().inbox.contains_key("w"));
    let _ = fs::remove_file(&path);
}

#[test]
fn what_an_acknowledging_peer_is_owed_is_never_evicted() {
    let path = std::env::temp_dir().join(format!("amesh-owed-{}.db", Uuid::new_v4()));
    let mut hub = Hub::open(&path).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    /* the peer drops off: no socket, no live promise, only the persisted one */
    hub.sockets.remove("w");
    hub.recv_live.remove("w");
    let chatter = |n: usize| (0..n).map(|i| json!({"type": "notify", "id": format!("n{i}")}));
    queue_inbox(&mut hub, "w", chatter(INBOX_MAX + 10));
    assert_eq!(
        hub.inbox["w"].len(),
        INBOX_MAX + 10,
        "no cap while the peer is away"
    );
    persist(&mut hub).unwrap();
    let mut reopened = Hub::open(&path).unwrap();
    assert!(
        reopened.recv_known.contains("w"),
        "the promise survives a restart"
    );
    assert_eq!(reopened.inbox["w"].len(), INBOX_MAX + 10);
    queue_inbox(&mut reopened, "w", chatter(10));
    assert_eq!(
        reopened.inbox["w"].len(),
        INBOX_MAX + 20,
        "no cap after a restart either"
    );
    /* a peer that never promised recv keeps the old cap */
    reopened
        .peers
        .insert("x".into(), reopened.peers["w"].clone());
    queue_inbox(&mut reopened, "x", chatter(INBOX_MAX + 10));
    assert_eq!(reopened.inbox["x"].len(), INBOX_MAX);
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_reconnecting_acknowledging_client_gets_copies_not_duplicates() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = std::env::temp_dir().join(format!("amesh-replay-{}.db", Uuid::new_v4()));
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
            hub.inbox.get("p-worker").map(Vec::len).unwrap_or(0)
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
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.inner.lock().await.sockets.contains_key("p-worker") {
        assert!(std::time::Instant::now() < deadline, "socket never closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while owed(&state).await != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "recv never drained the inbox"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !Hub::open(&path).unwrap().inbox.contains_key("p-worker"),
        "acknowledged records leave the disk too"
    );
    drop(r2);
    drop(w2);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.inner.lock().await.sockets.contains_key("p-worker") {
        assert!(std::time::Instant::now() < deadline, "socket never closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
    let path = std::env::temp_dir().join(format!("amesh-drop-{}.db", Uuid::new_v4()));
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
    tx.send(json!({"type": "notify", "message": "owed-buffered"}))
        .unwrap();
    drop(tx);
    close_connection(&state, "worker", gen, false, rx, taken).await;
    let hub = state.inner.lock().await;
    let held: Vec<String> = hub
        .inbox
        .get("worker")
        .map(|queue| {
            queue
                .iter()
                .map(|event| event["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        held,
        vec!["owed-taken".to_string(), "owed-buffered".to_string()],
        "a closing connection must not post what it owes back into its own channel"
    );
    assert!(!hub.sockets.contains_key("worker"));
    assert_eq!(hub.peers["worker"].status, "offline");
    drop(hub);
    assert!(Hub::open(&path).unwrap().inbox.contains_key("worker"));
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn second_connection_displaces_the_incumbent() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = std::env::temp_dir().join(format!("amesh-displace-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path,
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

#[tokio::test]
async fn activity_keeps_a_socketless_peer_alive() {
    let path = std::env::temp_dir().join(format!("amesh-touch-{}.db", Uuid::new_v4()));
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path,
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

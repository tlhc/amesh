use super::*;

#[tokio::test]
async fn ws_unicast_not_broadcast() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = temp_state("ws");
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
    let path = temp_state("persist");
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
    let (app, _state) = test_app();
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
    let (app, hub, _state) = test_app_with_hub();
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
    advance_jobs(&mut *hub.lock().await);
    let (_, pending) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=worker",
        json!({}),
    )
    .await;
    let inbox = pending["inbox"].as_array().unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0]["type"], "ask");
    assert!(inbox[0]["text"].as_str().unwrap().contains(jid));
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
    let (app, hub, _state) = test_app_with_hub();
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
        advance_jobs(&mut *hub.lock().await);
        let (_, pending) = json_req(
            app.clone(),
            "GET",
            &format!("/asks/pending?peer_id={id}"),
            json!({}),
        )
        .await;
        let inbox = pending["inbox"].as_array().unwrap();
        assert_eq!(inbox.len(), 1, "{id} {inbox:?}");
        assert_eq!(inbox[0]["type"], "ask");
        assert!(inbox[0]["text"].as_str().unwrap().contains(jid));
    }
}

#[tokio::test]
async fn job_without_assignee_does_not_notify() {
    let (app, _state) = test_app();
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
    let (app, _state) = test_app();
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
    let (app, _state) = test_app();
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
    let cid = blocked["correlation_id"]
        .as_str()
        .expect("ask-blocking must return a correlation id");
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/answer",
        json!({"correlation_id": cid, "message": "ok"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
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
    let (app, _state) = test_app();
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
    let path = temp_state("mcp");
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
async fn attachment_get_refuses_a_path_out_of_the_directory() {
    let (app, state) = test_app();
    fs::create_dir_all(state.with_file_name("attachments")).unwrap();
    let canary = state.with_file_name("canary.txt");
    fs::write(&canary, "private").unwrap();
    let absolute = canary.to_str().unwrap().replace('/', "%2F");
    let mut statuses = Vec::new();
    for id in [absolute.as_str(), "..%2Fcanary.txt"] {
        let req = Request::builder()
            .method("GET")
            .uri(format!("/attachments/{id}"))
            .body(Body::empty())
            .unwrap();
        statuses.push(app.clone().oneshot(req).await.unwrap().status());
    }
    assert_eq!(
        statuses,
        [StatusCode::NOT_FOUND, StatusCode::NOT_FOUND],
        "an absolute path and a ../ stay out of reach"
    );
}

#[tokio::test]
async fn attachments_up_to_the_cap_fit_either_upload_route() {
    let (app, _state) = test_app();
    let json_body =
        json!({"filename": "big.bin", "content_base64": "AAAA".repeat(8 * 1024 * 1024 / 3)})
            .to_string()
            .into_bytes();
    let boundary = "----ameshbig";
    let mut form = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"big.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    form.extend(std::iter::repeat_n(0u8, 10 * 1024 * 1024));
    form.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    let mut statuses = Vec::new();
    for (uri, kind, body) in [
        ("/attachments", "application/json".to_string(), json_body),
        (
            "/attachments/form",
            format!("multipart/form-data; boundary={boundary}"),
            form,
        ),
    ] {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", kind)
            .body(Body::from(body))
            .unwrap();
        statuses.push(app.clone().oneshot(req).await.unwrap().status());
    }
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::OK],
        "8 MiB as base64 JSON and 10 MiB in a form are both within the cap"
    );
}

#[tokio::test]
async fn attachment_form_upload_then_get_bytes() {
    let (app, _state) = test_app();
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
    let (app, _state) = test_app();
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
    let path = temp_state("reclaim");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
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
    let path = temp_state("reclaim2");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
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
    let path = temp_state("stale");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
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

#[test]
fn undelivered_events_for_an_unknown_peer_are_dropped() {
    let (mut hub, _state) = undelivered_hub();
    let owed = vec![json!({"type": "notify", "id": "x"})];
    assert!(!return_undelivered(
        &mut hub,
        "ghost",
        owed.into_iter().map(test_outbound).collect()
    ));
    assert!(
        hub.inbox.records("ghost").is_empty(),
        "unknown canonical peer_id must not grow an inbox"
    );
}

fn check_queued_reply_after_socket_closes(acknowledging: bool) {
    let (mut hub, _state) = undelivered_hub();
    let (tx, rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (1, tx));
    if acknowledging {
        hub.recv_live.insert("worker".into());
        hub.recv_known.insert("worker".into());
    }
    let queued = queue_replies(
        &mut hub,
        vec![json!({"type": "ack", "to_peer": "worker", "message": "expired"})],
        now_unix(),
    );
    let ack = queued[0].1.event().clone();
    persist(&mut hub).unwrap();
    drop(rx);
    deliver_queued(&mut hub, queued);
    assert!(
        !hub.sockets.contains_key("worker"),
        "the failed socket must be removed"
    );
    assert_eq!(
        hub.inbox
            .records("worker")
            .iter()
            .map(QueuedRecord::event)
            .collect::<Vec<_>>(),
        vec![&ack]
    );
    let disk = read_snapshot(&hub.db).unwrap();
    assert_eq!(
        disk.inbox["worker"]
            .iter()
            .map(QueuedRecord::event)
            .collect::<Vec<_>>(),
        vec![&ack]
    );
}

#[test]
fn queued_reply_survives_a_closed_acknowledging_socket() {
    check_queued_reply_after_socket_closes(true);
}

#[test]
fn queued_reply_survives_a_closed_legacy_socket() {
    check_queued_reply_after_socket_closes(false);
}

#[test]
fn undelivered_events_return_to_the_inbox() {
    let (mut hub, _state) = undelivered_hub();
    let owed = vec![
        json!({"type": "notify", "id": "a"}),
        json!({"type": "notify", "id": "b"}),
    ];
    assert!(return_undelivered(
        &mut hub,
        "worker",
        owed.into_iter().map(test_outbound).collect()
    ));
    let queued = hub.inbox.records("worker");
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].event()["id"], "a");
    assert_eq!(queued[1].event()["id"], "b");
}

#[test]
fn undelivered_events_reach_the_successor_socket() {
    let (mut hub, _state) = undelivered_hub();
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (7, tx));
    let owed = vec![json!({"type": "notify", "id": "a"})];
    assert!(!return_undelivered(
        &mut hub,
        "worker",
        owed.into_iter().map(test_outbound).collect()
    ));
    assert_eq!(rx.try_recv().unwrap().event()["id"], "a");
    assert!(
        hub.inbox.records("worker").is_empty(),
        "successor must not double-queue"
    );
}

#[test]
fn undelivered_events_survive_a_dead_successor() {
    let (mut hub, _state) = undelivered_hub();
    let (tx, rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (7, tx));
    drop(rx);
    let owed = vec![
        json!({"type": "notify", "id": "a"}),
        json!({"type": "notify", "id": "b"}),
    ];
    assert!(
        return_undelivered(
            &mut hub,
            "worker",
            owed.into_iter().map(test_outbound).collect()
        ),
        "falling back to the inbox changes hub state and must be persisted"
    );
    let queued = hub.inbox.records("worker");
    assert_eq!(
        queued.len(),
        2,
        "the rejected event and the rest must both survive"
    );
    assert_eq!(queued[0].event()["id"], "a");
    assert_eq!(queued[1].event()["id"], "b");
    assert!(
        !hub.sockets.contains_key("worker"),
        "the dead socket must be dropped"
    );
}

#[test]
fn displaced_notice_is_never_replayed() {
    let (mut hub, _state) = undelivered_hub();
    let notice = vec![json!({"type": DISPLACED, "peer_id": "worker"})];
    assert!(!return_undelivered(
        &mut hub,
        "worker",
        notice.clone().into_iter().map(test_outbound).collect()
    ));
    assert!(
        hub.inbox.records("worker").is_empty(),
        "notice must not reach the inbox"
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (7, tx));
    let mixed = vec![notice[0].clone(), json!({"type": "notify", "id": "real"})];
    assert!(!return_undelivered(
        &mut hub,
        "worker",
        mixed.into_iter().map(test_outbound).collect()
    ));
    assert_eq!(rx.try_recv().unwrap().event()["id"], "real");
    assert!(
        rx.try_recv().is_err(),
        "only the real event may be forwarded"
    );
}

#[tokio::test]
async fn the_snapshot_says_how_each_peer_is_reached() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "worker", "pi", "one").await;
    register(app.clone(), "quiet", "codex", "one").await;
    let (tx, _rx) = mpsc::unbounded_channel();
    {
        let mut hub = inner.lock().await;
        hub.sockets.insert("worker".into(), (1, tx));
        hub.recv_live.insert("worker".into());
        hub.inbox.put(
            "worker",
            vec![
                QueuedRecord::new(json!({"type": "notify", "id": "n1"}), now_unix()),
                QueuedRecord::new(json!({"type": "notify", "id": "n2"}), now_unix()),
            ],
        );
    }
    let (status, snap) = json_req(app, "GET", "/snapshot?circle=one", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        snap["capabilities"]["delivery"], true,
        "a monitor must know the hub fills the delivery fields"
    );
    let row = |id: &str| {
        snap["roster"]
            .as_array()
            .unwrap()
            .iter()
            .find(|peer| peer["peer_id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no {id} in {snap}"))
    };
    let worker = row("worker");
    assert_eq!(worker["push"], true, "a live socket is a push channel");
    assert_eq!(worker["acks"], true, "that socket confirms each frame");
    assert_eq!(worker["queued"], 2, "both records wait in the inbox");
    let quiet = row("quiet");
    assert_eq!(quiet["push"], false, "no socket, no push channel");
    assert_eq!(quiet["acks"], false);
    assert_eq!(quiet["queued"], 0, "an empty inbox counts nothing");
}

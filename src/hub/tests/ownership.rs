use super::*;

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
            now_unix(),
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
    let path = temp_state("takeover");
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
    let path = temp_state("owed1");
    let mut hub = owed_hub(&path, "S1");
    let before = now_unix();
    assert!(refresh_peers(&mut hub).0);
    assert!(
        !hub.peers.contains_key("tmp-pi"),
        "the row itself is pruned as before"
    );
    assert_eq!(
        hub.inbox.records("tmp-pi").len(),
        2,
        "what the session was owed stays behind for it"
    );
    assert!(hub.recv_known.contains("tmp-pi"));
    let owed = &hub.owed["tmp-pi"];
    assert_eq!(owed.owner, "S1");
    assert!(owed.since >= before);
    /* a peer that never bound a session has nobody to keep the backlog for */
    let sessionless = temp_state("owed1b");
    let mut hub = owed_hub(&sessionless, "");
    assert!(refresh_peers(&mut hub).0);
    assert!(hub.inbox.count("tmp-pi") == 0);
    assert!(!hub.owed.contains_key("tmp-pi"));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&sessionless);
}

#[test]
fn a_pruned_backlog_keeps_its_clock_across_a_restart() {
    let path = temp_state("owed2");
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
    assert_eq!(reopened.inbox.records("tmp-pi").len(), 2);
    assert!(reopened.recv_known.contains("tmp-pi"));
    assert!(
        !refresh_peers(&mut reopened).0 || reopened.owed["tmp-pi"].since == since,
        "a refresh must not restart it either"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn another_session_cannot_take_a_name_with_a_waiting_backlog() {
    let path = temp_state("owed3");
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

fn hook_row(session: &str) -> Value {
    json!({"path": "/tmp", "backend": "codex", "circle": "c", "session_id": session})
}

/* a Codex thread's hook row has no drainer until its MCP binds, so once the row is pruned
only its session vouches for what is still waiting on it */
async fn pruned_hook_row(
    tool: &str,
    mut arguments: Value,
    collected: bool,
) -> (Router, Arc<Mutex<Hub>>, TempState) {
    let (app, hub, state) = test_app_with_hub();
    let boss = json!({"peer_id": "boss", "name": "boss", "backend": "pi", "circle": "c"});
    let _ = json_req(app.clone(), "POST", "/peers", boss).await;
    let (_, a) = json_req(app.clone(), "POST", "/peers", hook_row("A")).await;
    assert_eq!(a["peer_id"], "tmp-codex");
    arguments["from_peer"] = json!("boss");
    arguments["peer_name"] = json!("tmp-codex");
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}});
    let (st, reply) = json_req(app.clone(), "POST", "/mcp", call).await;
    assert_eq!(st, StatusCode::OK, "{reply}");
    if collected {
        let _ = json_req(
            app.clone(),
            "GET",
            "/asks/pending?peer_id=tmp-codex",
            json!({}),
        )
        .await;
    }
    hub.lock()
        .await
        .peers
        .get_mut("tmp-codex")
        .unwrap()
        .last_seen = 1;
    let _ = json_req(app.clone(), "GET", "/peers", json!({})).await;
    (app, hub, state)
}

#[tokio::test]
async fn a_pruned_hook_row_keeps_its_name_while_its_ask_is_open() {
    /* its hook already showed the ask, so only the open ask still points at the name */
    let (app, _, _state) = pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let (_, b) = json_req(app.clone(), "POST", "/peers", hook_row("B")).await;
    assert_eq!(b["peer_id"], "tmp-codex-2", "B must not inherit A's ask");
    let (_, a) = json_req(app.clone(), "POST", "/peers", hook_row("A")).await;
    assert_eq!(a["peer_id"], "tmp-codex");
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert_eq!(pending["asks"][0]["text"], "for A only", "{pending}");
}

#[tokio::test]
async fn an_open_ask_keeps_the_pruned_name_owed_across_a_restart() {
    let (_, hub, _state) = pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let path = hub.lock().await.db.path().unwrap().to_string();
    let reopened = Hub::open(Path::new(&path)).unwrap();
    assert_eq!(reopened.owed["tmp-codex"].owner, "A");
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn an_ask_its_session_never_came_back_for_is_closed_not_handed_on() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    hub.lock().await.owed.get_mut("tmp-codex").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
    let (_, b) = json_req(app.clone(), "POST", "/peers", hook_row("B")).await;
    assert_eq!(b["peer_id"], "tmp-codex", "the name is free again");
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");
    let hub = hub.lock().await;
    let ask = hub.asks.values().next().unwrap();
    assert!(!ask.open);
    assert!(
        ask.reply.as_deref().unwrap().contains("did not come back"),
        "{:?}",
        ask.reply
    );
}

#[tokio::test]
async fn ownership_expiry_notifies_the_connected_asker_after_commit() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let mut rx = {
        let mut hub = hub.lock().await;
        let rx = recv_peer(&mut hub, "boss");
        hub.owed.get_mut("tmp-codex").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
        persist(&mut hub).unwrap();
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE UPDATE ON asks BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
        rx
    };
    let (status, _) = json_req(app.clone(), "GET", "/peers", json!({})).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        rx.try_recv().is_err(),
        "an uncommitted expiry must not be delivered"
    );
    {
        let hub = hub.lock().await;
        assert!(hub.asks.values().next().unwrap().open);
        assert!(hub.inbox.count("boss") == 0);
        hub.db.execute_batch("DROP TRIGGER fail_write").unwrap();
    }
    let (status, _) = json_req(app, "GET", "/peers", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let ack = rx.try_recv().expect("the asker must receive the expiry");
    assert_eq!(ack.event()["type"], "ack");
    let hub = hub.lock().await;
    let disk = read_snapshot(&hub.db).unwrap();
    let ask = disk.asks.values().next().unwrap();
    assert!(!ask.open);
    assert_eq!(ack.event()["correlation_id"], ask.correlation_id);
    assert_eq!(
        ack.event()["message"],
        format!("[failed] {}", ask.reply.as_deref().unwrap())
    );
    assert!(ack.event()["message"]
        .as_str()
        .unwrap()
        .contains("did not come back"));
    assert_eq!(disk.inbox["boss"][0].event(), ack.event());
}

#[tokio::test]
async fn ownership_expiry_retains_the_offline_askers_ack_across_restart() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let path = {
        let mut hub = hub.lock().await;
        hub.owed.get_mut("tmp-codex").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
        hub.db.path().unwrap().to_string()
    };
    let (status, _) = json_req(app, "GET", "/peers", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let reopened = Hub::open(Path::new(&path)).unwrap();
    let ask = reopened.asks.values().next().unwrap();
    let ack = &reopened.inbox.records("boss")[0];
    assert_eq!(ack.event()["type"], "ack");
    assert_eq!(ack.event()["correlation_id"], ask.correlation_id);
    assert_eq!(
        ack.event()["message"],
        format!("[failed] {}", ask.reply.as_deref().unwrap())
    );
    assert!(!ask.open);
}

#[tokio::test]
async fn ownership_expiry_sends_ack_from_the_schedule_tick() {
    let (_, hub, _state) = pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let (mut rx, path) = {
        let mut hub = hub.lock().await;
        let rx = recv_peer(&mut hub, "boss");
        hub.owed.get_mut("tmp-codex").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
        persist(&mut hub).unwrap();
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE UPDATE ON asks BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
        (rx, PathBuf::from(hub.db.path().unwrap()))
    };
    let app = App {
        inner: hub.clone(),
        token: None,
        state_path: path,
    };
    tick_schedules(&app).await;
    assert!(
        rx.try_recv().is_err(),
        "an uncommitted expiry must not be delivered"
    );
    {
        let hub = hub.lock().await;
        assert!(hub.asks.values().next().unwrap().open);
        hub.db.execute_batch("DROP TRIGGER fail_write").unwrap();
    }
    tick_schedules(&app).await;
    let ack = rx
        .try_recv()
        .expect("the scheduler must deliver the expiry");
    assert_eq!(ack.event()["type"], "ack");
    assert!(ack.event()["message"]
        .as_str()
        .unwrap()
        .contains("did not come back"));
    let hub = hub.lock().await;
    let disk = read_snapshot(&hub.db).unwrap();
    assert_eq!(disk.inbox["boss"][0].event(), ack.event());
    drop(hub);
    tick_schedules(&app).await;
    assert!(rx.try_recv().is_err(), "an expiry is delivered once");
}

#[tokio::test]
async fn a_pruned_hook_row_keeps_what_its_hook_has_not_collected() {
    let (app, _, _state) =
        pruned_hook_row("amesh_notify_peer", json!({"message": "for A only"}), false).await;
    let (_, b) = json_req(app.clone(), "POST", "/peers", hook_row("B")).await;
    assert_eq!(
        b["peer_id"], "tmp-codex-2",
        "B must not inherit A's messages"
    );
    let (_, a) = json_req(app.clone(), "POST", "/peers", hook_row("A")).await;
    assert_eq!(a["peer_id"], "tmp-codex");
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert_eq!(pending["inbox"][0]["message"], "for A only", "{pending}");
}

#[tokio::test]
async fn ownership_expiry_write_failure_does_not_release_an_open_ask() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let cid = {
        let mut hub = hub.lock().await;
        hub.owed.get_mut("tmp-codex").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
        persist(&mut hub).unwrap();
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE UPDATE ON asks BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
        hub.asks.keys().next().unwrap().clone()
    };
    let (status, _) = json_req(app.clone(), "GET", "/peers", json!({})).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let (status, b) = json_req(app.clone(), "POST", "/peers", hook_row("B")).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["peer_id"], "tmp-codex");
    let (_, pending) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=tmp-codex",
        json!({}),
    )
    .await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");
    let (_, waited) = json_req(
        app,
        "POST",
        &format!("/asks/{cid}/wait"),
        json!({"timeout_seconds": 0}),
    )
    .await;
    assert_eq!(waited["open"], false, "{waited}");
    assert!(waited["reply"]
        .as_str()
        .unwrap()
        .contains("did not come back"));
}

#[tokio::test]
async fn ownership_failed_claim_keeps_the_reservation_after_restart() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_notify_peer", json!({"message": "for A only"}), false).await;
    let path = {
        let hub = hub.lock().await;
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE DELETE ON recv_peers BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
        hub.db.path().unwrap().to_string()
    };
    let mut claim = hook_row("B");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert!(pending["inbox"].as_array().unwrap().is_empty(), "{pending}");
    let reopened = Hub::open(Path::new(&path)).unwrap();
    assert_eq!(
        allocate_peer_id(&reopened, "/tmp", "codex", "B", None),
        "tmp-codex-2"
    );
    assert_eq!(
        reopened.inbox.records("tmp-codex")[0].event()["message"],
        "for A only"
    );
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn ownership_claimed_name_closes_previous_sessions_asks() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let cid = hub.lock().await.asks.keys().next().unwrap().clone();
    let mut claim = hook_row("B");
    claim["peer_id"] = json!("tmp-codex");
    let (status, b) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["peer_id"], "tmp-codex");
    let (_, pending) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=tmp-codex",
        json!({}),
    )
    .await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");
    let (_, waited) = json_req(
        app.clone(),
        "POST",
        &format!("/asks/{cid}/wait"),
        json!({"timeout_seconds": 0}),
    )
    .await;
    assert_eq!(waited["open"], false, "{waited}");
    assert!(waited["reply"]
        .as_str()
        .unwrap()
        .contains("session was replaced"));
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=boss", json!({})).await;
    assert_eq!(pending["inbox"][0]["type"], "ack", "{pending}");
    assert_eq!(pending["inbox"][0]["correlation_id"], cid);
    assert_eq!(
        pending["inbox"][0]["message"],
        format!("[failed] {}", waited["reply"].as_str().unwrap())
    );
}

#[tokio::test]
async fn ownership_replacement_notifies_the_connected_asker_after_commit() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let mut rx = {
        let mut hub = hub.lock().await;
        let rx = recv_peer(&mut hub, "boss");
        persist(&mut hub).unwrap();
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE INSERT ON peers WHEN NEW.peer_id = 'tmp-codex' BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
        rx
    };
    let mut claim = hook_row("B");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        rx.try_recv().is_err(),
        "a rolled-back closure must not be delivered"
    );
    {
        let hub = hub.lock().await;
        assert!(hub.asks.values().next().unwrap().open);
        assert!(hub.inbox.count("boss") == 0);
        hub.db.execute_batch("DROP TRIGGER fail_write").unwrap();
    }
    let (status, _) = json_req(app, "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let ack = rx.try_recv().expect("the asker must receive the closure");
    assert_eq!(ack.event()["type"], "ack");
    let hub = hub.lock().await;
    let disk = read_snapshot(&hub.db).unwrap();
    let ask = disk.asks.values().next().unwrap();
    assert!(!ask.open);
    assert_eq!(ack.event()["correlation_id"], ask.correlation_id);
    assert_eq!(
        ack.event()["message"],
        format!("[failed] {}", ask.reply.as_deref().unwrap())
    );
    assert_eq!(
        disk.inbox["boss"][0].event(),
        ack.event(),
        "recv retires the durable copy"
    );
}

#[tokio::test]
async fn ownership_replacement_waits_for_the_askers_reserved_session() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let mut rx = {
        let mut hub = hub.lock().await;
        let rx = recv_peer(&mut hub, "boss");
        hub.owed.insert(
            "boss".into(),
            Owed {
                owner: "asker".into(),
                since: now_unix(),
            },
        );
        queue_inbox(
            &mut hub,
            "boss",
            [json!({"type": "notify", "message": "held for asker"})],
            now_unix(),
        );
        persist(&mut hub).unwrap();
        rx
    };
    let mut claim = hook_row("B");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        rx.try_recv().is_err(),
        "the closure must wait for the asker's session"
    );
    assert_eq!(
        hub.lock().await.inbox.records("boss")[1].event()["type"],
        "ack"
    );
    let claim = json!({"peer_id": "boss", "backend": "pi", "session_id": "asker"});
    let (status, _) = json_req(app, "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let bound = rx
        .try_recv()
        .expect("binding precedes the reserved replies");
    assert_eq!(bound.event()["type"], "bound");
    assert_eq!(bound.event()["session_id"], "asker");
    assert_eq!(rx.try_recv().unwrap().event()["type"], "notify");
    let ack = rx.try_recv().unwrap();
    assert_eq!(ack.event()["type"], "ack");
    assert!(ack.event()["message"]
        .as_str()
        .unwrap()
        .contains("session was replaced"));
}

#[tokio::test]
async fn ownership_replacement_delivers_once_to_a_legacy_asker() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let mut rx = {
        let mut hub = hub.lock().await;
        let rx = recv_peer(&mut hub, "boss");
        hub.recv_live.remove("boss");
        hub.recv_known.remove("boss");
        persist(&mut hub).unwrap();
        rx
    };
    let mut claim = hook_row("B");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app, "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let ack = rx
        .try_recv()
        .expect("the legacy asker must receive the closure");
    assert_eq!(ack.event()["type"], "ack");
    let hub = hub.lock().await;
    assert!(hub.inbox.count("boss") == 0);
    assert!(!read_snapshot(&hub.db).unwrap().inbox.contains_key("boss"));
}

#[tokio::test]
async fn ownership_disconnected_live_name_closes_previous_sessions_asks() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), false).await;
    let (status, _) = json_req(app.clone(), "POST", "/peers", hook_row("A")).await;
    assert_eq!(status, StatusCode::OK);
    let mut claim = hook_row("B");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");
    assert!(pending["inbox"].as_array().unwrap().is_empty(), "{pending}");
    let hub = hub.lock().await;
    let ask = hub.asks.values().next().unwrap();
    assert!(!ask.open);
    assert!(ask
        .reply
        .as_deref()
        .unwrap()
        .contains("session was replaced"));
}

#[tokio::test]
async fn ownership_session_change_closes_asks_even_when_the_pin_is_connected() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), false).await;
    let (status, _) = json_req(app.clone(), "POST", "/peers", hook_row("A")).await;
    assert_eq!(status, StatusCode::OK);
    let mut claim = hook_row("");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim.clone()).await;
    assert_eq!(status, StatusCode::OK);
    {
        let hub = hub.lock().await;
        assert_eq!(hub.peers["tmp-codex"].session_id, "A");
        assert!(hub.asks.values().next().unwrap().open);
    }
    let mut rx = {
        let mut hub = hub.lock().await;
        let rx = recv_peer(&mut hub, "tmp-codex");
        hub.peers.get_mut("tmp-codex").unwrap().session_id = "A".into();
        persist(&mut hub).unwrap();
        rx
    };
    claim["session_id"] = json!("A");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim.clone()).await;
    assert_eq!(status, StatusCode::OK);
    {
        let hub = hub.lock().await;
        assert_eq!(hub.peers["tmp-codex"].session_id, "A");
        assert!(hub.asks.values().next().unwrap().open);
        assert!(rx.try_recv().is_err());
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE UPDATE ON peers BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
    }
    claim["session_id"] = json!("B");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        rx.try_recv().is_err(),
        "a failed claim must not displace the owner"
    );
    {
        let hub = hub.lock().await;
        assert_eq!(hub.peers["tmp-codex"].session_id, "A");
        assert!(hub.asks.values().next().unwrap().open);
        hub.db.execute_batch("DROP TRIGGER fail_write").unwrap();
    }
    let (status, _) = json_req(app, "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let notice = rx
        .try_recv()
        .expect("the connection must learn that its session changed");
    assert_eq!(notice.event()["type"], "replaced");
    assert_eq!(notice.event()["session_id"], "B");
    let hub = hub.lock().await;
    let ask = hub.asks.values().next().unwrap();
    assert!(
        !ask.open,
        "changing sessions retires the old ask even with a socket"
    );
    assert!(hub.inbox.count("tmp-codex") == 0);
    assert_eq!(
        hub.inbox.records("boss")[0].event()["correlation_id"],
        ask.correlation_id
    );
    assert_eq!(
        hub.inbox.records("boss")[0].event()["message"],
        format!("[failed] {}", ask.reply.as_deref().unwrap())
    );
    assert!(hub.sockets.contains_key("tmp-codex"));
    assert!(hub.recv_known.contains("tmp-codex"));
}

#[tokio::test]
async fn ownership_expired_reservation_keeps_the_live_receivers_receipts() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_notify_peer", json!({"message": "for A only"}), false).await;
    let mut claim = hook_row("");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app, "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let mut hub = hub.lock().await;
    let _rx = recv_peer(&mut hub, "tmp-codex");
    hub.owed.get_mut("tmp-codex").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
    assert!(refresh_peers(&mut hub).0);
    assert!(!hub.owed.contains_key("tmp-codex"));
    assert!(hub.inbox.count("tmp-codex") == 0);
    queue_inbox(
        &mut hub,
        "tmp-codex",
        (0..=INBOX_MAX).map(|id| json!({"type": "notify", "id": id})),
        now_unix(),
    );
    assert_eq!(hub.inbox.records("tmp-codex").len(), INBOX_MAX + 1);
}

#[test]
fn ownership_legacy_retry_discards_frames_before_the_last_replacement() {
    let path = temp_state("replaced");
    let mut hub = Hub::open(&path).unwrap();
    let mut rx = recv_peer(&mut hub, "w");
    return_undelivered(
        &mut hub,
        "w",
        vec![
            test_outbound(json!({"type": "notify", "message": "A"})),
            test_outbound(json!({"type": "replaced"})),
            test_outbound(json!({"type": "notify", "message": "B"})),
            test_outbound(json!({"type": "replaced"})),
            test_outbound(json!({"type": "notify", "message": "C"})),
            test_outbound(json!({"type": "bound", "session_id": "C"})),
            test_outbound(json!({"type": "notify", "message": "C after binding"})),
        ],
    );
    assert_eq!(rx.try_recv().unwrap().event()["message"], "C");
    assert_eq!(rx.try_recv().unwrap().event()["message"], "C after binding");
    assert!(rx.try_recv().is_err());
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn ownership_legacy_retry_waits_for_the_reserved_session() {
    let (_, hub, _state) =
        pruned_hook_row("amesh_notify_peer", json!({"message": "for A only"}), false).await;
    let mut hub = hub.lock().await;
    let mut rx = recv_peer(&mut hub, "tmp-codex");
    assert!(return_undelivered(
        &mut hub,
        "tmp-codex",
        vec![test_outbound(
            json!({"type": "notify", "message": "late A"})
        )]
    ));
    assert!(
        rx.try_recv().is_err(),
        "a successor socket still has to prove the reserved session"
    );
    assert_eq!(
        hub.inbox.records("tmp-codex").last().unwrap().event()["message"],
        "late A"
    );
    hub.peers.remove("tmp-codex");
    hub.sockets.remove("tmp-codex");
    assert!(return_undelivered(
        &mut hub,
        "tmp-codex",
        vec![test_outbound(
            json!({"type": "notify", "message": "later A"})
        )]
    ));
    assert_eq!(
        hub.inbox.records("tmp-codex").last().unwrap().event()["message"],
        "later A"
    );
}

#[tokio::test]
async fn ownership_same_session_claim_keeps_open_asks() {
    let (app, _, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), false).await;
    let mut claim = hook_row("A");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert_eq!(pending["asks"][0]["text"], "for A only", "{pending}");
    assert_eq!(pending["inbox"][0]["text"], "for A only", "{pending}");
}

#[tokio::test]
async fn ownership_first_session_binding_keeps_queued_asks() {
    let (app, hub, _state) = test_app_with_hub();
    let (status, _) = json_req(app.clone(), "POST", "/peers", hook_row("")).await;
    assert_eq!(status, StatusCode::OK);
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"to_peer": "tmp-codex", "text": "waiting for binding"}),
    )
    .await;
    assert_eq!(ask["ok"], true, "{ask}");
    let mut rx = recv_peer(&mut *hub.lock().await, "tmp-codex");
    let mut claim = hook_row("A");
    claim["peer_id"] = json!("tmp-codex");
    {
        let mut hub = hub.lock().await;
        persist(&mut hub).unwrap();
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE UPDATE ON peers BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
    }
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        rx.try_recv().is_err(),
        "a failed binding cannot change the stream session"
    );
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let bound = rx
        .try_recv()
        .expect("first binding names the stream session");
    assert_eq!(bound.event()["type"], "bound");
    assert_eq!(bound.event()["session_id"], "A");
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert_eq!(
        pending["asks"][0]["text"], "waiting for binding",
        "{pending}"
    );
    assert_eq!(
        hub.lock().await.inbox.records("tmp-codex")[0].event()["text"],
        "waiting for binding"
    );
}

#[tokio::test]
async fn ownership_unproven_socket_closes_old_asks_on_a_different_session() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), false).await;
    let mut claim = hook_row("");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let _rx = {
        let mut hub = hub.lock().await;
        let rx = recv_peer(&mut hub, "tmp-codex");
        persist(&mut hub).unwrap();
        rx
    };
    let mut claim = hook_row("B");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");
    let hub = hub.lock().await;
    assert!(hub.inbox.count("tmp-codex") == 0);
    assert!(hub.recv_known.contains("tmp-codex"));
}

#[test]
fn ownership_rollback_keeps_the_live_receivers_unacknowledged_queue() {
    let path = temp_state("live-rollback");
    let mut hub = Hub::open(&path).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    /* the peer row predates this connection's first promise to acknowledge */
    hub.recv_known.remove("w");
    persist(&mut hub).unwrap();
    hub.recv_known.insert("w".into());
    hub.db.execute_batch(
        "CREATE TEMP TRIGGER fail_write BEFORE INSERT ON recv_peers BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
    ).unwrap();
    assert!(persist_ok(&mut hub).is_err());
    hub.db.execute_batch("DROP TRIGGER fail_write").unwrap();
    queue_inbox(
        &mut hub,
        "w",
        (0..=INBOX_MAX).map(|n| json!({"type": "notify", "id": n})),
        now_unix(),
    );
    assert_eq!(hub.inbox.records("w").len(), INBOX_MAX + 1);
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn ownership_rollback_keeps_an_unproven_rows_reservation() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_notify_peer", json!({"message": "for A only"}), false).await;
    let mut claim = hook_row("");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    {
        let hub = hub.lock().await;
        assert_eq!(hub.owed["tmp-codex"].owner, "A");
        hub.db.execute_batch(
            "CREATE TEMP TRIGGER fail_write BEFORE INSERT ON recv_peers BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        ).unwrap();
    }
    let (status, _) = json_req(
        app.clone(),
        "GET",
        "/asks/pending?peer_id=tmp-codex",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert!(pending["inbox"].as_array().unwrap().is_empty(), "{pending}");
    let hub = hub.lock().await;
    let reopened = Hub::open(Path::new(hub.db.path().unwrap())).unwrap();
    assert_eq!(reopened.owed["tmp-codex"].owner, "A");
    assert_eq!(
        reopened.inbox.records("tmp-codex")[0].event()["message"],
        "for A only"
    );
}

#[tokio::test]
async fn ownership_pruning_an_unproven_row_keeps_its_previous_owner() {
    let (app, hub, _state) =
        pruned_hook_row("amesh_ask", json!({"query": "for A only"}), true).await;
    let mut claim = hook_row("");
    claim["peer_id"] = json!("tmp-codex");
    let (status, _) = json_req(app.clone(), "POST", "/peers", claim).await;
    assert_eq!(status, StatusCode::OK);
    hub.lock()
        .await
        .peers
        .get_mut("tmp-codex")
        .unwrap()
        .last_seen = 1;
    let (status, b) = json_req(app.clone(), "POST", "/peers", hook_row("B")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(b["peer_id"], "tmp-codex-2");
    let (status, a) = json_req(app.clone(), "POST", "/peers", hook_row("A")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(a["peer_id"], "tmp-codex");
    let (_, pending) = json_req(app, "GET", "/asks/pending?peer_id=tmp-codex", json!({})).await;
    assert_eq!(pending["asks"][0]["text"], "for A only", "{pending}");
}

#[tokio::test]
async fn claiming_the_name_with_another_session_discards_the_backlog() {
    let path = temp_state("owed3b");
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
        hub.inbox.count("tmp-pi") == 0,
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

    let path = temp_state("owed4");
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
            hub.inbox.records("tmp-pi").len(),
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
    let connected: Value =
        serde_json::from_str(&r.next().await.unwrap().unwrap().to_string()).unwrap();
    assert_eq!(connected["type"], "connected");
    assert_eq!(connected["session_id"], "S1");
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
    until(
        &state,
        |hub| hub.inbox.count("tmp-pi") == 0,
        "recv never drained the replayed backlog",
    )
    .await;
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn an_unproven_row_gets_no_replay_and_no_hook_leak_until_its_session_proves_it() {
    unproven_connection_waits_for_its_session(true).await;
}

#[tokio::test]
async fn ownership_unproven_legacy_connection_keeps_reserved_messages() {
    unproven_connection_waits_for_its_session(false).await;
}

async fn unproven_connection_waits_for_its_session(recv: bool) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    let path = temp_state("owed5");
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
    assert_eq!(state.inner.lock().await.inbox.records("tmp-pi").len(), 2);
    let (socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut w, mut r) = socket.split();
    w.send(WsMsg::Text(
        json!({"type": "connect", "peer_id": "tmp-pi", "recv": recv})
            .to_string()
            .into(),
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
    assert_eq!(state.inner.lock().await.inbox.records("tmp-pi").len(), 2);
    {
        let mut hub = state.inner.lock().await;
        persist_then_deliver(
            &mut hub,
            "tmp-pi",
            json!({"type": "notify", "message": "still for S1"}),
            now_unix(),
        )
        .unwrap();
    }
    let nothing = tokio::time::timeout(Duration::from_millis(200), r.next()).await;
    assert!(
        nothing.is_err(),
        "new messages must also wait for the owner: {nothing:?}"
    );
    /* the hook binds the session on the first turn: the owner is proved while attached */
    let (status, _) = json_req(
        router(state.clone()),
        "POST",
        "/peers",
        json!({"peer_id": "tmp-pi", "name": "tmp-pi", "path": "/tmp", "backend": "pi", "session_id": "S1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let bound = tokio::time::timeout(Duration::from_secs(3), r.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let bound: Value = serde_json::from_str(&bound.to_string()).unwrap();
    assert_eq!(bound["type"], "bound");
    assert_eq!(bound["session_id"], "S1");
    let mut ids = Vec::new();
    for _ in 0..3 {
        let frame = tokio::time::timeout(Duration::from_secs(3), r.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&frame.to_string()).unwrap();
        ids.push(v["id"].as_str().unwrap().to_string());
    }
    if recv {
        for id in &ids {
            w.send(WsMsg::Text(
                json!({"type": "recv", "id": id}).to_string().into(),
            ))
            .await
            .unwrap();
        }
    }
    until(
        &state,
        |hub| hub.inbox.count("tmp-pi") == 0,
        "recv never drained the adopted backlog",
    )
    .await;
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
            failed: false,
            closed_at: None,
            opened_at: None,
            closed_by: None,
            from_circle: String::new(),
            to_circle: String::new(),
            watch: None,
        },
    );
    persist_then_deliver(
        &mut hub,
        "tmp-pi",
        json!({"type": "ask", "correlation_id": "ask-y", "text": "still open"}),
        now_unix(),
    )
    .unwrap();
    refresh_peers(&mut hub);
    assert!(hub.owed.contains_key("tmp-pi"));
    hub
}

#[tokio::test]
async fn a_backlog_follows_its_session_to_a_new_name() {
    let path = temp_state("follow");
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
        let moved = &hub.inbox.records("tmp-pi-2");
        assert_eq!(
            moved.len(),
            3,
            "everything left behind for the session is now under its new name"
        );
        assert!(
            moved.iter().all(|r| r.event()["id"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false)),
            "every moved record can be acknowledged"
        );
        assert!(
            !hub.owed.contains_key("tmp-pi")
                && hub.inbox.count("tmp-pi") == 0
                && !hub.recv_known.contains("tmp-pi"),
            "nothing stays under the old name"
        );
        assert!(hub.recv_known.contains("tmp-pi-2"));
        assert_eq!(
            hub.asks["ask-y"].to_peer_id, "tmp-pi-2",
            "an open ask is re-pointed at the new name"
        );
        assert_eq!(
            hub.asks["ask-y"].to_circle, "default",
            "and at the circle the new name registered in"
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
        !reopened.owed.contains_key("tmp-pi") && reopened.inbox.count("tmp-pi") == 0,
        "the move is on disk"
    );
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_backlog_does_not_follow_another_session_or_no_session() {
    let path = temp_state("nofollow");
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
        assert_eq!(hub.inbox.records("tmp-pi").len(), 3);
        assert!(hub.inbox.count("tmp-pi-2") == 0 && hub.inbox.count("tmp-pi-3") == 0);
        assert_eq!(hub.asks["ask-y"].to_peer_id, "tmp-pi");
    }
    let _ = fs::remove_file(&path);
}

#[tokio::test]
async fn a_moved_backlog_is_deduplicated_by_id_and_only_the_new_part_is_pushed() {
    let path = temp_state("dedupe");
    let mut hub = abandoned_hub(&path);
    let duplicate = hub.inbox.records("tmp-pi")[0].clone();
    /* the new name is already attached and acknowledging, with one record in flight and
    one that is the same as a record the old name holds */
    let mut rx = recv_peer(&mut hub, "tmp-pi-2");
    hub.peers.get_mut("tmp-pi-2").unwrap().session_id = "S1".into();
    persist_then_deliver(
        &mut hub,
        "tmp-pi-2",
        json!({"type": "notify", "message": "already-in-flight"}),
        now_unix(),
    )
    .unwrap();
    let _ = rx.try_recv().unwrap();
    hub.inbox.enqueue("tmp-pi-2", [duplicate.clone()], true);
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
    let ids: Vec<String> = hub
        .inbox
        .records("tmp-pi-2")
        .iter()
        .map(|r| r.event()["id"].as_str().unwrap().to_string())
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
        pushed.push(copy.event()["id"].as_str().unwrap().to_string());
    }
    assert_eq!(
        pushed.len(),
        2,
        "only what was newly moved goes down the socket, not the queue it already had: {pushed:?}"
    );
    assert!(!pushed.contains(&duplicate.event()["id"].as_str().unwrap().to_string()));
    let _ = fs::remove_file(&path);
}

#[test]
fn a_backlog_nobody_returns_for_expires() {
    let path = temp_state("owed6");
    let mut hub = owed_hub(&path, "S1");
    refresh_peers(&mut hub);
    hub.owed.get_mut("tmp-pi").unwrap().since = now_unix() - OWED_TTL_SECS - 1;
    assert!(refresh_peers(&mut hub).0);
    assert!(!hub.owed.contains_key("tmp-pi"));
    assert!(
        hub.inbox.count("tmp-pi") == 0,
        "an expired backlog is not kept"
    );
    assert!(!hub.recv_known.contains("tmp-pi"));
    let _ = fs::remove_file(&path);
}

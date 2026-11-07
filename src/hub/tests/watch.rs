use super::*;

/* boss asked worker at t0; worker reported idle before the ask and never reported again */
fn watched_hub(
    name: &str,
) -> (
    Hub,
    TempState,
    mpsc::UnboundedReceiver<Outbound>,
    mpsc::UnboundedReceiver<Outbound>,
    u64,
) {
    let path = temp_state(name);
    let mut hub = Hub::open(&path).unwrap();
    let boss = recv_peer(&mut hub, "boss");
    let worker = recv_peer(&mut hub, "worker");
    let t0 = now_unix();
    hub.activity.insert(
        "worker".into(),
        Activity {
            state: "idle".into(),
            since: t0 - 5,
            observed_at: t0 - 5,
            source: "test".into(),
            reason: None,
        },
    );
    hub.asks.insert(
        "ask-1".into(),
        serde_json::from_value(json!({
            "correlation_id": "ask-1", "from_peer": "boss", "to_peer": "worker",
            "to_peer_id": "worker", "from_circle": "default", "to_circle": "default",
            "text": "ping", "open": true, "reply": null, "opened_at": t0
        }))
        .unwrap(),
    );
    (hub, path, boss, worker, t0)
}

fn pushed(rx: &mut mpsc::UnboundedReceiver<Outbound>) -> Vec<Value> {
    let mut events = Vec::new();
    while let Ok(outbound) = rx.try_recv() {
        events.push(outbound.event().clone());
    }
    events
}

#[test]
fn a_watch_pushes_stalled_once_and_nudges_the_recipient() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-stalled");
    let registered = register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    assert_eq!(registered["watch_until"], t0 + WATCH_DEFAULT_SECS);
    advance_asks(&mut hub, t0 + 5);
    assert!(
        pushed(&mut boss).is_empty(),
        "a five second old ask is still pending"
    );
    advance_asks(&mut hub, t0 + 11);
    let stalled = pushed(&mut boss);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["type"], "notify");
    assert_eq!(stalled[0]["event"], "stalled");
    assert_eq!(stalled[0]["topic"], "ask-1");
    assert_eq!(stalled[0]["from_peer"], "amesh");
    assert_eq!(stalled[0]["correlation_id"], "ask-1");
    let message = stalled[0]["message"].as_str().unwrap();
    assert!(
        message.contains("ask-1") && message.contains("nudged once"),
        "the text names the ask and says the recipient was nudged: {message}"
    );
    let nudged = pushed(&mut worker);
    assert_eq!(nudged.len(), 1, "{nudged:?}");
    assert_eq!(nudged[0]["topic"], "ask-1");
    assert_eq!(nudged[0]["correlation_id"], "ask-1");
    assert!(nudged[0]["message"].as_str().unwrap().contains("amesh_ack"));
    advance_asks(&mut hub, t0 + 12);
    advance_asks(&mut hub, t0 + 600);
    assert!(
        pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty(),
        "stalled is pushed once and the recipient is nudged once"
    );
    /* re-arming keeps what was already pushed and only moves the deadline */
    let again = register_watch(&mut hub, "ask-1", Some(500), None, t0 + 12).unwrap();
    assert_eq!(again["watch_until"], t0 + 12 + 500);
    assert_eq!(hub.asks["ask-1"].watch.as_ref().unwrap().sent, "stalled");
    advance_asks(&mut hub, t0 + 20);
    assert!(
        pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty(),
        "re-arming does not repeat stalled"
    );
}

#[test]
fn an_away_asker_keeps_its_events_pending() {
    let (mut hub, _state, _boss, mut worker, t0) = watched_hub("watch-away-asker");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    /* the asker's row was pruned: no socket, no backlog, no owed entry */
    hub.peers.remove("boss");
    hub.sockets.remove("boss");
    advance_asks(&mut hub, t0 + 11);
    assert_eq!(
        hub.asks["ask-1"].watch.as_ref().unwrap().sent,
        "",
        "an undeliverable event is not recorded as pushed"
    );
    assert!(
        pushed(&mut worker).is_empty(),
        "the nudge waits with the event it belongs to"
    );
    /* the asker is back: the pending stall fires on the next tick */
    let mut boss = recv_peer(&mut hub, "boss");
    advance_asks(&mut hub, t0 + 12);
    let stalled = pushed(&mut boss);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["event"], "stalled");
    assert_eq!(pushed(&mut worker).len(), 1);
}

#[test]
fn an_offline_recipient_is_reported_once_without_a_nudge() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-offline");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    advance_asks(&mut hub, t0 + 1);
    let offline = pushed(&mut boss);
    assert_eq!(offline.len(), 1, "{offline:?}");
    assert_eq!(offline[0]["event"], "offline");
    assert!(
        pushed(&mut worker).is_empty(),
        "an offline recipient is not nudged"
    );
    /* back online and idle: stalled follows once; a second drop is not reported again */
    hub.peers.get_mut("worker").unwrap().status = "online".into();
    advance_asks(&mut hub, t0 + 11);
    let stalled = pushed(&mut boss);
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!(stalled[0]["event"], "stalled");
    assert_eq!(pushed(&mut worker).len(), 1);
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    advance_asks(&mut hub, t0 + 12);
    assert!(
        pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty(),
        "each state is pushed once"
    );
    assert_eq!(
        hub.asks["ask-1"].watch.as_ref().unwrap().sent,
        "offline stalled"
    );
}

#[test]
fn a_recipient_gone_with_a_kept_backlog_is_reported_once() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-gone");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    /* the prune kept the recipient's backlog for its session and dropped its row */
    hub.peers.remove("worker");
    hub.sockets.remove("worker");
    hub.owed.insert(
        "worker".into(),
        Owed {
            since: t0,
            owner: "s-worker".into(),
        },
    );
    advance_asks(&mut hub, t0 + 1);
    let gone = pushed(&mut boss);
    assert_eq!(gone.len(), 1, "{gone:?}");
    assert_eq!(gone[0]["event"], "gone");
    assert!(
        gone[0]["message"].as_str().unwrap().contains("session"),
        "the text says the backlog is kept: {gone:?}"
    );
    assert!(pushed(&mut worker).is_empty());
    advance_asks(&mut hub, t0 + 2);
    assert!(pushed(&mut boss).is_empty());
}

#[test]
fn a_state_event_and_the_deadline_due_together_are_pushed_once_each() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-together");
    register_watch(&mut hub, "ask-1", Some(30), None, t0).unwrap();
    /* the first tick after a pause sees the stall and the deadline at once */
    advance_asks(&mut hub, t0 + 30);
    let events: Vec<String> = pushed(&mut boss)
        .into_iter()
        .map(|event| event["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(events, ["stalled", "deadline"]);
    assert_eq!(pushed(&mut worker).len(), 1, "the nudge still goes out");
    assert!(hub.asks["ask-1"].watch.is_none());
    advance_asks(&mut hub, t0 + 60);
    assert!(pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty());
}

#[test]
fn an_acked_ask_ends_its_watch_without_a_push() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-acked");
    register_watch(&mut hub, "ask-1", None, None, t0).unwrap();
    let ask = hub.asks.get_mut("ask-1").unwrap();
    ask.open = false;
    ask.reply = Some("pong".into());
    ask.closed_at = Some(t0 + 3);
    ask.closed_by = Some("recipient".into());
    advance_asks(&mut hub, t0 + 11);
    assert!(
        hub.asks["ask-1"].watch.is_none(),
        "the watch ends with the ask"
    );
    assert!(pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty());
}

#[test]
fn a_watch_pushes_the_deadline_once_and_ends() {
    let (mut hub, _state, mut boss, mut worker, t0) = watched_hub("watch-deadline");
    /* no activity report: the recipient stays pending, so only the deadline can fire */
    hub.activity.remove("worker");
    register_watch(&mut hub, "ask-1", Some(30), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 29);
    assert!(
        pushed(&mut boss).is_empty(),
        "one second before the deadline"
    );
    advance_asks(&mut hub, t0 + 30);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!(due[0]["event"], "deadline");
    assert_eq!(due[0]["topic"], "ask-1");
    assert!(
        hub.asks["ask-1"].watch.is_none(),
        "the deadline ends the watch"
    );
    advance_asks(&mut hub, t0 + 600);
    assert!(pushed(&mut boss).is_empty() && pushed(&mut worker).is_empty());
}

#[test]
fn a_watch_survives_a_reload_without_repeating_itself() {
    let (mut hub, state, mut boss, _worker, t0) = watched_hub("watch-reload");
    register_watch(&mut hub, "ask-1", Some(100), None, t0).unwrap();
    advance_asks(&mut hub, t0 + 11);
    assert_eq!(pushed(&mut boss).len(), 1, "stalled before the restart");
    drop(hub);
    let mut again = Hub::open(&state).unwrap();
    let watch = again.asks["ask-1"]
        .watch
        .clone()
        .expect("the watch is in the state file");
    assert_eq!(watch.until, t0 + 100);
    /* the restarted hub hears the same idle report again and the asker reconnects */
    let mut boss = recv_peer(&mut again, "boss");
    again.activity.insert(
        "worker".into(),
        Activity {
            state: "idle".into(),
            since: t0 - 5,
            observed_at: t0 + 20,
            source: "test".into(),
            reason: None,
        },
    );
    advance_asks(&mut again, t0 + 40);
    assert!(
        pushed(&mut boss).is_empty(),
        "stalled was already pushed before the restart"
    );
    advance_asks(&mut again, t0 + 100);
    let due = pushed(&mut boss);
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!(due[0]["event"], "deadline");
}

#[tokio::test]
async fn amesh_watch_registers_over_mcp_and_http() {
    let (app, inner, _state) = test_app_with_hub();
    register(app.clone(), "boss", "pi", "one").await;
    register(app.clone(), "worker", "pi", "one").await;
    let (_, ask) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "boss", "to_peer": "worker", "text": "ping"}),
    )
    .await;
    let cid = ask["correlation_id"].as_str().unwrap().to_string();
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_watch",
        json!({"from_peer": "boss", "correlation_id": cid, "deadline_seconds": 60}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let registered = tool_json(&body);
    assert_eq!(registered["ok"], true);
    assert_eq!(registered["correlation_id"], cid);
    let until = registered["watch_until"].as_u64().unwrap();
    let now = now_unix();
    assert!((now + 59..=now + 61).contains(&until), "{until} vs {now}");
    assert_eq!(
        inner.lock().await.asks[&cid]
            .watch
            .as_ref()
            .map(|w| w.until),
        Some(until)
    );
    let (st, body) = json_req(
        app.clone(),
        "POST",
        &format!("/asks/{cid}/watch"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body["watch_until"].as_u64().unwrap() >= now + WATCH_DEFAULT_SECS - 1,
        "no deadline_seconds means the default: {body}"
    );
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_watch",
        json!({"from_peer": "worker", "correlation_id": cid}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "only the asker can register the watch");
    let (_, body) = mcp_tool(
        app.clone(),
        "amesh_watch",
        json!({"from_peer": "boss", "correlation_id": cid, "deadline_seconds": 10_000_000}),
    )
    .await;
    let capped = tool_json(&body)["watch_until"].as_u64().unwrap();
    let now = now_unix();
    assert!(
        (now + WATCH_MAX_SECS - 1..=now + WATCH_MAX_SECS + 1).contains(&capped),
        "the deadline is capped: {capped} vs {now}"
    );
    let (_, orphan) = json_req(
        app.clone(),
        "POST",
        "/ask",
        json!({"from_peer": "nobody", "to_peer": "worker", "text": "ping"}),
    )
    .await;
    let orphan = orphan["correlation_id"].as_str().unwrap().to_string();
    let (st, body) = json_req(
        app.clone(),
        "POST",
        &format!("/asks/{orphan}/watch"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "the asker is not a registered peer");
    let (st, body) = json_req(app.clone(), "POST", "/asks/nope/watch", json!({})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown ask");
    inner.lock().await.asks.get_mut(&cid).unwrap().open = false;
    let (st, body) = json_req(app, "POST", &format!("/asks/{cid}/watch"), json!({})).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "ask is closed");
    let tool = mcp_tools()
        .into_iter()
        .find(|tool| tool["name"] == "amesh_watch")
        .expect("amesh_watch is in the catalogue");
    assert_eq!(tool["inputSchema"]["required"], json!(["correlation_id"]));
}

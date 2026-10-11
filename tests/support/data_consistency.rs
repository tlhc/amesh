#[tokio::test]
async fn consistency_fix_legacy_delivery_records_event() {
    let f = team("c1", &["boss", "worker"]).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    f.0.inner
        .lock()
        .await
        .sockets
        .insert("worker".into(), (1, tx));
    let cid = f.open_ask("boss", "worker").await;
    assert_eq!(rx.try_recv().unwrap().event()["correlation_id"], cid);
    assert!(f.0.inner.lock().await.asks.contains_key(&cid));
    let events = ring_of(&f).await;
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn consistency_fix_closed_ask_keeps_circle() {
    let f = team("c1", &["boss", "worker"]).await;
    let cid = f.open_ask("boss", "worker").await;
    f.ack(&cid, json!({"from_peer":"worker", "message":"done"}))
        .await;
    {
        let mut hub = f.0.inner.lock().await;
        for peer in hub.peers.values_mut() {
            peer.last_seen = 0;
        }
    }
    let (_, peers) = f.request("GET", "/peers", json!({})).await;
    assert!(peers.as_array().unwrap().is_empty());
    for id in ["boss", "worker"] {
        f.peer(id, "c2").await;
    }
    let (_, old) = f.request("GET", "/snapshot?circle=c1", json!({})).await;
    let (_, new) = f.request("GET", "/snapshot?circle=c2", json!({})).await;
    let (_, events) = f.request("GET", "/events?circle=c1", json!({})).await;
    assert_eq!(old["asks"][0]["correlation_id"], cid);
    assert!(new["asks"].as_array().unwrap().is_empty());
    assert_eq!(events.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn consistency_fix_ack_keeps_original_circle() {
    let f = team("c1", &["boss"]).await;
    let reg = |id: &str, name: &str, circle: &str| json!({"peer_id":id,"name":name,"circle":circle,"backend":"pi"});
    assert_eq!(
        f.request("POST", "/peers", reg("worker-id", "alias", "c1"))
            .await
            .0,
        StatusCode::OK
    );
    let cid = f.open_ask("boss", "alias").await;
    assert_eq!(
        f.request("POST", "/peers", reg("worker-id", "renamed", "c1"))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("POST", "/peers", reg("other", "alias", "c2"))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.ack(
            &cid,
            json!({"from_peer":"worker-id","message":"private-result"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(ring_of(&f).await.last().unwrap()["from_peer"], "worker-id");
    let (_, events) = f.request("GET", "/events?circle=c2", json!({})).await;
    assert!(events.as_array().unwrap().is_empty());
    let (status, rows) = f.tool("amesh_events", json!({"from_peer":"other"})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rows.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn consistency_fix_deleted_schedule_skips_dispatch() {
    let f = team("c1", &["boss", "worker"]).await;
    let (status,sched)=f.request("POST","/schedules",json!({"from_peer":"boss","to_peer":"worker","text":"cancelled-message","kind":"notify","in_seconds":0})).await;
    assert_eq!(status, StatusCode::OK);
    let id = sched["schedule_id"].as_str().unwrap();
    let held = f.0.inner.lock().await;
    let mut tick = Box::pin(tick_schedules(&f.0));
    assert!(futures_util::poll!(tick.as_mut()).is_pending());
    let mut gate1 = Box::pin(f.0.inner.lock());
    assert!(futures_util::poll!(gate1.as_mut()).is_pending());
    drop(held);
    assert!(futures_util::poll!(tick.as_mut()).is_pending());
    let guard1 = match futures_util::poll!(gate1.as_mut()) {
        std::task::Poll::Ready(g) => g,
        _ => panic!("gate1"),
    };
    let path = format!("/schedules/{id}");
    let mut deletion = Box::pin(f.request("DELETE", &path, json!({})));
    assert!(futures_util::poll!(deletion.as_mut()).is_pending());
    drop(guard1);
    assert!(futures_util::poll!(tick.as_mut()).is_pending());
    let result = match futures_util::poll!(deletion.as_mut()) {
        std::task::Poll::Ready(g) => g,
        _ => panic!("delete pending"),
    };
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1["ok"], true);
    assert!(futures_util::poll!(tick.as_mut()).is_ready());
    assert!(!f.0.inner.lock().await.schedules.contains_key(id));
    let inbox = f.inbox("worker").await;
    assert!(inbox.iter().all(|e| e["message"] != "cancelled-message"));
}

#[tokio::test]
async fn consistency_fix_ask_circles_survive_restart() {
    let f = team("c1", &["boss", "worker"]).await;
    let cid = f.open_ask("boss", "worker").await;
    let reopened = Fixture::open(f.0.state_path.clone());
    let hub = reopened.0.inner.lock().await;
    assert_eq!(hub.asks[&cid].from_circle, "c1");
    assert_eq!(hub.asks[&cid].to_circle, "c1");
}

#[tokio::test]
async fn consistency_fix_legacy_ask_has_only_unscoped_visibility() {
    let f = team("c1", &["boss", "worker"]).await;
    let cid = f.open_ask("boss", "worker").await;
    {
        let mut hub = f.0.inner.lock().await;
        let ask = hub.asks.get_mut(&cid).unwrap();
        ask.from_circle.clear();
        ask.to_circle.clear();
    }
    let (_, scoped) = f.request("GET", "/snapshot?circle=c1", json!({})).await;
    assert!(scoped["asks"].as_array().unwrap().is_empty());
    let (_, all) = f.request("GET", "/snapshot", json!({})).await;
    assert_eq!(all["asks"][0]["correlation_id"], cid);
}

#[tokio::test]
async fn consistency_fix_ack_uses_stored_circles_after_peer_removal() {
    let f = team("c1", &["boss", "worker"]).await;
    let cid = f.open_ask("boss", "worker").await;
    let mut hub = f.0.inner.lock().await;
    hub.peers.clear();
    push_event(
        &mut hub,
        json!({
            "type":"ack", "correlation_id":cid,
            "from_peer":"worker", "to_peer":"boss", "message":"done"
        }),
    );
    let ack = hub.events.last().unwrap();
    assert_eq!(ack["from_circle"], "c1");
    assert_eq!(ack["to_circle"], "c1");
}

#[tokio::test]
async fn consistency_fix_migration_takes_ask_circles_from_the_peers_on_disk() {
    let f = team("c1", &["boss", "worker"]).await;
    let cid = f.open_ask("boss", "worker").await;
    {
        let hub = f.0.inner.lock().await;
        hub.db
            .execute_batch(
                "ALTER TABLE asks DROP COLUMN from_circle;
             ALTER TABLE asks DROP COLUMN to_circle;",
            )
            .unwrap();
    }
    let reopened = Fixture::open(f.0.state_path.clone());
    let (_, scoped) = reopened
        .request("GET", "/snapshot?circle=c1", json!({}))
        .await;
    assert_eq!(scoped["asks"][0]["correlation_id"], cid, "{scoped}");
    let hub = reopened.0.inner.lock().await;
    let ask = &hub.asks[&cid];
    assert_eq!((ask.from_circle.as_str(), ask.to_circle.as_str()), ("c1", "c1"));
    assert_eq!(ask.text, "q");
}

#[tokio::test]
async fn consistency_fix_job_ack_carries_its_ask_circles() {
    let f = team("c1", &["boss", "worker"]).await;
    let id = f
        .job(json!({"title": "t", "from_peer": "boss", "assigned_peer": "worker"}))
        .await;
    f.advance().await;
    let cid = f.ask_id(&id).await;
    f.ack(&cid, json!({"from_peer": "worker", "message": "done"}))
        .await;
    let ring = ring_of(&f).await;
    let ack = ring
        .iter()
        .find(|e| e["type"] == "ack" && e["correlation_id"] == cid)
        .unwrap();
    assert_eq!(
        (&ack["from_circle"], &ack["to_circle"]),
        (&json!("c1"), &json!("c1"))
    );
}

#[tokio::test]
async fn consistency_fix_scheduled_ask_keeps_its_creator_circle() {
    let f = team("c1", &["boss"]).await;
    f.peer("worker", "c2").await;
    let (status, body) = f
        .request(
            "POST",
            "/schedules",
            json!({"from_peer": "boss", "to_peer": "worker", "text": "later", "kind": "ask", "in_seconds": 0, "cross_circle": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    f.0.inner.lock().await.peers.remove("boss");
    tick_schedules(&f.0).await;
    let hub = f.0.inner.lock().await;
    let ask = hub.asks.values().find(|ask| ask.text == "later").unwrap();
    assert_eq!(
        (ask.from_circle.as_str(), ask.to_circle.as_str()),
        ("c1", "c2")
    );
}

#[tokio::test]
async fn consistency_fix_handoff_gives_the_ask_its_coordinator_circle() {
    let f = team("c1", &["boss", "worker"]).await;
    let id = f.job(json!({"title": "t"})).await;
    assert_eq!(f.row(&id).await.circle, "", "no creator or assignee names a circle");
    let path = format!("/jobs/{id}");
    let (status, body) = f
        .request(
            "PATCH",
            &path,
            json!({"assigned_peer": "worker", "state": "queued"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    f.advance().await;
    let cid = f.ask_id(&id).await;
    let (status, body) = f
        .request("PATCH", &path, json!({"coordinator": "boss"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(f.ask(&cid).await.from_circle, "c1");
}

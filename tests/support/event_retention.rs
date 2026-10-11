#[tokio::test]
async fn day_retention_keeps_closed_asks_until_24_hours() {
    let f = team("c", &["boss", "worker"]).await;
    let cid = f.open_ask("boss", "worker").await;
    f.ack(&cid, json!({"from_peer": "worker", "message": "done"}))
        .await;
    let closed = f.ask(&cid).await.closed_at.unwrap();
    assert!(f.sweep(closed + 86_399).await.asks.is_empty());
    assert_eq!(f.sweep(closed + 86_400).await.asks, [cid]);
}

#[tokio::test]
async fn day_retention_keeps_finished_jobs_until_24_hours() {
    let f = team("c", &["boss", "worker"]).await;
    let id = f
        .job(json!({"title": "test", "from_peer": "boss", "assigned_peer": "worker"}))
        .await;
    f.advance().await;
    let cid = f.ask_id(&id).await;
    f.ack(&cid, json!({"from_peer": "worker", "message": "done"}))
        .await;
    f.advance().await;
    let ended = f.row(&id).await.finished_at.unwrap();
    assert!(f.sweep(ended + 86_399).await.jobs.is_empty());
    assert_eq!(f.sweep(ended + 86_400).await.jobs, [id]);
}

#[tokio::test]
async fn day_retention_queries_events_within_24_hours() {
    let f = team("c", &["boss"]).await;
    {
        let mut hub = f.0.inner.lock().await;
        let now = now_unix();
        for (id, age) in [("within", 86_340), ("expired", 86_460)] {
            push_event(
                &mut hub,
                json!({"id": id, "from_peer": "boss", "type": "notify"}),
            );
            hub.events.last_mut().unwrap()["at"] = json!(now - age);
        }
    }
    let result = mcp_call(
        &f.0,
        json!({"name": "amesh_events", "arguments": {"from_peer": "boss"}}),
    )
    .await
    .unwrap();
    let rows: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "within");
}

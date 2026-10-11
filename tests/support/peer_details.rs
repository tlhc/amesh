#[tokio::test]
async fn peer_details_match_snapshot_state() {
    let f = team("c1", &["boss", "worker"]).await;
    f.peer("outside", "c2").await;
    let job = f
        .job(json!({
            "title":"review", "prompt":"check", "assigned_peer":"worker", "from_peer":"boss"
        }))
        .await;
    assert!(!job.is_empty());
    f.advance().await;
    assert_eq!(
        f.request(
            "POST",
            "/activity",
            json!({
                "peer_id":"worker", "state":"wait", "source":"test", "reason":"permission"
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, snapshot) = f.request("GET", "/snapshot?circle=c1", json!({})).await;
    let (status, rows) = f
        .tool(
            "amesh_list_peers",
            json!({
                "from_peer":"boss", "detail":true
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let rows = rows.as_array().expect("detail returns JSON peers");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["peer_id"], "boss");
    assert_eq!(rows[1]["peer_id"], "worker");
    let actual = &rows[1];
    let expected = snapshot["roster"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["peer_id"] == "worker")
        .unwrap();
    for field in [
        "activity",
        "running",
        "queued",
        "push",
        "acks",
        "last_seen",
        "liveness",
    ] {
        assert_eq!(actual[field], expected[field], "{field}");
    }
    assert_eq!(
        actual["delivery"]["condition"],
        expected["delivery"]["condition"]
    );
    assert_eq!(actual["activity"]["state"], "wait");
    assert_eq!(actual["running"], 1);
    assert_eq!(actual["queued"], 1);
}

#[tokio::test]
async fn peer_details_preserve_cross_circle_rules() {
    let f = team("c1", &["boss"]).await;
    f.peer("other", "c2").await;
    let (status, _) = f
        .tool(
            "amesh_list_peers",
            json!({
                "from_peer":"boss", "circle":"c2", "detail":true
            }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, scoped) = f
        .tool(
            "amesh_list_peers",
            json!({
                "from_peer":"boss", "circle":"c2", "cross_circle":true, "detail":true
            }),
        )
        .await;
    let rows = scoped.as_array().expect("detail returns JSON peers");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["peer_id"], "other");
    let (_, all) = f
        .tool(
            "amesh_list_peers",
            json!({
                "from_peer":"boss", "cross_circle":true, "detail":true
            }),
        )
        .await;
    assert_eq!(all.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn peer_details_unknown_activity_and_empty_roster() {
    let f = team("c1", &["boss"]).await;
    let (_, rows) = f
        .tool(
            "amesh_list_peers",
            json!({
                "from_peer":"boss", "detail":true
            }),
        )
        .await;
    let rows = rows.as_array().expect("detail returns JSON peers");
    assert!(rows[0]["activity"].is_null());
    assert_eq!(rows[0]["running"], 0);
    assert_eq!(rows[0]["queued"], 0);
    assert_eq!(rows[0]["push"], false);
    assert_eq!(rows[0]["acks"], false);
    let (_, empty) = f
        .tool(
            "amesh_list_peers",
            json!({
                "from_peer":"boss", "circle":"empty", "cross_circle":true, "detail":true
            }),
        )
        .await;
    assert_eq!(empty, json!([]));
    let (_, compact) = f
        .tool("amesh_list_peers", json!({"from_peer":"boss"}))
        .await;
    assert_eq!(compact.as_str().unwrap().split('\t').count(), 5);
}

#[test]
fn peer_details_schema_is_exposed() {
    let tools = mcp_tools();
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "amesh_list_peers")
        .unwrap();
    assert_eq!(
        tool["inputSchema"]["properties"]["detail"]["type"],
        "boolean"
    );
}

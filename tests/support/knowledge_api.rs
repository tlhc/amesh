use super::*;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn request(app: Router, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn fixture() -> (App, tests::TempState) {
    let path = tests::TempState::new("knowledge-api");
    let app = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    for (id, circle) in [("author", "c"), ("reader", "c"), ("outsider", "other")] {
        let (status, _) = request(
            router(app.clone()),
            "/peers",
            json!({
                "name": id, "peer_id": id, "backend": "pi", "circle": circle,
                "session_id": format!("session-{id}")
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    (app, path)
}

fn publication() -> Value {
    json!({"from_peer":"author", "title":"inbox ACK", "body":"Keep persistent sequence numbers.",
        "applies_to":"src/hub/inbox.rs", "evidence":"410d348: head ACK changes one row"})
}

async fn publish(app: &App) -> String {
    let (status, body) = request(router(app.clone()), "/knowledge/publish", publication()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["knowledge"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn knowledge_http_round_trip_and_same_circle_revoke() {
    let (app, _path) = fixture().await;
    let id = publish(&app).await;
    let (status, body) = request(
        router(app.clone()),
        "/knowledge/search",
        json!({"from_peer":"reader","query":"ACK"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["id"], id);
    let (status, body) = request(
        router(app.clone()),
        "/knowledge/get",
        json!({"from_peer":"reader","id":id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["knowledge"]["publisher_session"], "session-author");
    let (status, _) = request(
        router(app.clone()),
        "/knowledge/revoke",
        json!({"from_peer":"reader","id":id,"reason":"Revalidated on another revision"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = request(
        router(app),
        "/knowledge/get",
        json!({"from_peer":"author","id":id}),
    )
    .await;
    assert_eq!(body["knowledge"]["status"], "revoked");
    assert_eq!(body["knowledge"]["revoked_by_peer"], "reader");
    assert_eq!(body["knowledge"]["revoked_by_session"], "session-reader");
}

#[tokio::test]
async fn knowledge_requires_registered_caller_for_every_action() {
    let (app, _path) = fixture().await;
    for action in ["publish", "search", "get", "revoke"] {
        let (status, _) = request(
            router(app.clone()),
            &format!("/knowledge/{action}"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{action}");
        let (status, _) = request(
            router(app.clone()),
            &format!("/knowledge/{action}"),
            json!({"from_peer":"unknown"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{action}");
    }
}

#[tokio::test]
async fn knowledge_rejects_caller_supplied_scope_and_identity() {
    let (app, _path) = fixture().await;
    for (field, value) in [
        ("circle", json!("other")),
        ("cross_circle", json!(true)),
        ("publisher_peer", json!("reader")),
        ("publisher_session", json!("fake")),
        ("created_at", json!(1)),
        ("expires_at", json!(i64::MAX)),
    ] {
        let mut body = publication();
        body[field] = value;
        let (status, _) = request(router(app.clone()), "/knowledge/publish", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}");
    }
}

#[tokio::test]
async fn knowledge_cross_circle_reads_writes_and_replacement_are_hidden() {
    let (app, _path) = fixture().await;
    let id = publish(&app).await;
    let (status, body) = request(
        router(app.clone()),
        "/knowledge/search",
        json!({"from_peer":"outsider","query":"ACK"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    for action in ["get", "revoke"] {
        let mut args = json!({"from_peer":"outsider", "id":id});
        if action == "revoke" {
            args["reason"] = json!("wrong circle");
        }
        let (status, _) = request(router(app.clone()), &format!("/knowledge/{action}"), args).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{action}");
    }
    let mut args = publication();
    args["from_peer"] = json!("outsider");
    args["replaces"] = json!(id);
    let (status, _) = request(router(app.clone()), "/knowledge/publish", args).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = request(
        router(app),
        "/knowledge/get",
        json!({"from_peer":"reader", "id":id}),
    )
    .await;
    assert_eq!(body["knowledge"]["status"], "active");
}

#[test]
fn knowledge_mcp_schemas_expose_only_the_four_minimal_tools() {
    let tools = mcp_tools();
    let knowledge: Vec<_> = tools
        .iter()
        .filter(|t| t["name"].as_str().unwrap().starts_with("amesh_knowledge_"))
        .collect();
    assert_eq!(knowledge.len(), 4);
    for tool in knowledge {
        let properties = tool["inputSchema"]["properties"].as_object().unwrap();
        assert!(!properties.contains_key("circle"));
        assert!(!properties.contains_key("cross_circle"));
        assert!(!properties.contains_key("publisher_peer"));
    }
}

#[tokio::test]
async fn knowledge_mcp_calls_use_the_same_store() {
    let (app, _path) = fixture().await;
    let (status, body) = request(
        router(app.clone()),
        "/mcp",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"amesh_knowledge_publish","arguments":publication()}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let id = &entry["knowledge"]["id"];
    for (action, args) in [
        ("search", json!({"query":"ACK"})),
        ("get", json!({"id":id})),
        ("revoke", json!({"id":id,"reason":"reviewed"})),
    ] {
        let mut args = args;
        args["from_peer"] = json!("reader");
        let (status, body) = request(
            router(app.clone()),
            "/mcp",
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":format!("amesh_knowledge_{action}"),"arguments":args}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{action}: {body}");
        let data: Value =
            serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(!data.is_null());
    }
}

#[tokio::test]
async fn knowledge_survives_runtime_persistence_and_restart() {
    let (app, path) = fixture().await;
    let id = publish(&app).await;
    persist(&mut *app.inner.lock().await).unwrap();
    let reopened = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    let (status, body) = request(
        router(reopened),
        "/knowledge/get",
        json!({"from_peer":"reader","id":id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["knowledge"]["publisher_session"], "session-author");
    assert_eq!(
        body["knowledge"]["body"],
        "Keep persistent sequence numbers."
    );
}

#[tokio::test]
async fn knowledge_http_requires_configured_bearer_token() {
    let (mut app, _path) = fixture().await;
    app.token = Some("secret".into());
    let (status, _) = request(router(app), "/knowledge/publish", publication()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn knowledge_cleanup_runs_on_the_existing_sweep_tick() {
    let (app, _path) = fixture().await;
    let old = publish(&app).await;
    let recent = publish(&app).await;
    let mut hub = app.inner.lock().await;
    let now = now_unix() as i64;
    hub.db
        .execute(
            "UPDATE knowledge SET created_at=?1,expires_at=?2 WHERE id=?3",
            rusqlite::params![now - 700_000, now - 604_801, old],
        )
        .unwrap();
    hub.db
        .execute(
            "UPDATE knowledge SET created_at=?1,expires_at=?2 WHERE id=?3",
            rusqlite::params![now - 200, now - 100, recent],
        )
        .unwrap();
    hub.sweep_at = now_unix() + 100;
    advance_jobs(&mut hub);
    assert_eq!(
        hub.db
            .query_row("SELECT count(*) FROM knowledge", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    hub.sweep_at = 0;
    advance_jobs(&mut hub);
    assert_eq!(
        hub.db
            .query_row("SELECT count(*) FROM knowledge", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let retained: String = hub
        .db
        .query_row("SELECT id FROM knowledge", [], |r| r.get(0))
        .unwrap();
    assert_eq!(retained, recent);
}

#[tokio::test]
async fn knowledge_failed_write_does_not_notify_waiters() {
    let (app, _path) = fixture().await;
    let changes = app.inner.lock().await.changes.subscribe();
    app.inner.lock().await.db.execute_batch(
        "CREATE TRIGGER fail_knowledge BEFORE INSERT ON knowledge BEGIN SELECT RAISE(ABORT, 'injected'); END;"
    ).unwrap();
    let (status, _) = request(router(app.clone()), "/knowledge/publish", publication()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!changes.has_changed().unwrap());
    assert_eq!(
        app.inner
            .lock()
            .await
            .db
            .query_row("SELECT count(*) FROM knowledge", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn knowledge_publish_does_not_notify_ask_waiters() {
    let (app, _path) = fixture().await;
    let changes = app.inner.lock().await.changes.subscribe();
    let id = publish(&app).await;
    let (status, body) = request(
        router(app.clone()),
        "/knowledge/get",
        json!({"from_peer":"reader", "id":id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["knowledge"]["body"],
        "Keep persistent sequence numbers."
    );
    assert!(!changes.has_changed().unwrap());
}

#[tokio::test]
async fn knowledge_revoke_does_not_notify_ask_waiters() {
    let (app, _path) = fixture().await;
    let id = publish(&app).await;
    let changes = app.inner.lock().await.changes.subscribe();
    let (status, _) = request(
        router(app.clone()),
        "/knowledge/revoke",
        json!({"from_peer":"reader", "id":id,"reason":"reviewed"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = request(
        router(app.clone()),
        "/knowledge/get",
        json!({"from_peer":"reader", "id":id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["knowledge"]["status"], "revoked");
    assert_eq!(body["knowledge"]["revoke_reason"], "reviewed");
    assert!(!changes.has_changed().unwrap());
}

#[test]
fn knowledge_mcp_ttl_bounds_match_the_publish_contract() {
    let tools = mcp_tools();
    let publish = tools
        .iter()
        .find(|tool| tool["name"] == "amesh_knowledge_publish")
        .unwrap();
    let ttl = &publish["inputSchema"]["properties"]["ttl_seconds"];
    assert_eq!(ttl["type"], "integer");
    assert_eq!(ttl["minimum"], 1);
    assert_eq!(ttl["maximum"], 2_592_000);
}

#[test]
fn knowledge_mcp_result_bounds_match_the_search_contract() {
    let tools = mcp_tools();
    let search = tools
        .iter()
        .find(|tool| tool["name"] == "amesh_knowledge_search")
        .unwrap();
    let limit = &search["inputSchema"]["properties"]["limit"];
    assert_eq!(limit["type"], "integer");
    assert_eq!(limit["minimum"], 1);
    assert_eq!(limit["maximum"], 5);
}

#[test]
fn knowledge_mcp_catalog_preserves_existing_tool_order() {
    let tools = mcp_tools();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "amesh_ask",
            "amesh_ack",
            "amesh_notify_peer",
            "amesh_broadcast",
            "amesh_list_peers",
            "amesh_whoami",
            "amesh_job_create",
            "amesh_job_list",
            "amesh_job_status",
            "amesh_job_update",
            "amesh_job_cancel",
            "amesh_job_delete",
            "amesh_schedule_create",
            "amesh_schedule_list",
            "amesh_schedule_delete",
            "amesh_ask_many",
            "amesh_wait",
            "amesh_events",
            "amesh_knowledge_publish",
            "amesh_knowledge_search",
            "amesh_knowledge_get",
            "amesh_knowledge_revoke",
        ]
    );
}

#[test]
fn knowledge_mcp_search_description_names_the_get_tool() {
    let tools = mcp_tools();
    let search = tools
        .iter()
        .find(|tool| tool["name"] == "amesh_knowledge_search")
        .unwrap();
    assert!(search["description"]
        .as_str()
        .unwrap()
        .contains("amesh_knowledge_get"));
}

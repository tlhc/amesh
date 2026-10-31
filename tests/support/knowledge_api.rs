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
    let mut missing = Vec::new();
    for (name, field, rules) in [
        (
            "publish",
            "title",
            vec!["project", "question", "identifiers"],
        ),
        (
            "publish",
            "body",
            vec![
                "first 300 characters",
                "keys:",
                "one reusable question",
                "inferred",
            ],
        ),
        (
            "publish",
            "applies_to",
            vec!["pin:", "watch:", "working-tree", "environment"],
        ),
        (
            "publish",
            "evidence",
            vec!["commands", "observed results", "durable", "temporary"],
        ),
        (
            "publish",
            "ttl_seconds",
            vec!["go past the default", "replayable", "before reuse"],
        ),
        (
            "publish",
            "replaces",
            vec![
                "same scope",
                "duplicates unchanged",
                "different version or scope",
            ],
        ),
        (
            "search",
            "",
            vec![
                "title, body and applies_to",
                "excludes evidence",
                "newest first",
                "at most 5",
            ],
        ),
        (
            "get",
            "",
            vec![
                "working-tree",
                "dependencies",
                "environment",
                "inferred",
                "git diff --stat <commit> --",
                "git status --porcelain --untracked-files=all",
            ],
        ),
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == format!("amesh_knowledge_{name}"))
            .unwrap();
        let description = if field.is_empty() {
            &tool["description"]
        } else {
            &tool["inputSchema"]["properties"][field]["description"]
        }
        .as_str()
        .unwrap_or_default();
        for rule in rules {
            if !description.contains(rule) {
                missing.push(format!("{name}.{field}: {rule}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "missing knowledge guidance: {missing:#?}"
    );
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

/* the terminal's read is a plain GET: no from_peer, and the bearer token is all it needs */
async fn get(app: Router, path: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/* active rows come first and history follows, each group newest first */
#[tokio::test]
async fn knowledge_monitor_list_orders_active_before_history() {
    let (app, _path) = fixture().await;
    let now = now_unix() as i64;
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(publish(&app).await);
    }
    let (active_old, active_new, expired, revoked) = (&ids[0], &ids[1], &ids[2], &ids[3]);
    for (sql, id) in [
        (
            "UPDATE knowledge SET created_at=?1-300, expires_at=?1+1000 WHERE id=?2",
            active_old,
        ),
        (
            "UPDATE knowledge SET created_at=?1-100, expires_at=?1+2000 WHERE id=?2",
            active_new,
        ),
        (
            "UPDATE knowledge SET created_at=?1-200, expires_at=?1-10 WHERE id=?2",
            expired,
        ),
        (
            "UPDATE knowledge SET created_at=?1-250, expires_at=?1-5, revoked_at=?1-1,
             revoke_reason='x', revoked_by_peer='author', revoked_by_session='session-author'
             WHERE id=?2",
            revoked,
        ),
    ] {
        app.inner
            .lock()
            .await
            .db
            .execute(sql, rusqlite::params![now, id])
            .unwrap();
    }
    let (status, body) = get(router(app.clone()), "/knowledge", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let order: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(order, [active_new, active_old, expired, revoked]);
    let states: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["status"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["active", "active", "expired", "revoked"]);
}

/* one page of 50, and the total the page has to declare */
#[tokio::test]
async fn knowledge_monitor_list_pages_at_fifty_and_counts_the_scope() {
    let (app, _path) = fixture().await;
    let now = now_unix() as i64;
    {
        let hub = app.inner.lock().await;
        for n in 0..55 {
            hub.db
                .execute(
                    "INSERT INTO knowledge(id,circle,title,body,applies_to,evidence,
                     publisher_peer,publisher_session,created_at,expires_at)
                     VALUES(?1,'c',?2,'b','a','e','author','session-author',?3,?4)",
                    rusqlite::params![
                        format!("kn-{n:032x}"),
                        format!("note {n}"),
                        now - 1000 + n,
                        now + 1000 + n
                    ],
                )
                .unwrap();
        }
    }
    let (status, body) = get(router(app.clone()), "/knowledge?circle=c", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 55);
    assert_eq!(body["items"].as_array().unwrap().len(), 50);
    assert_eq!(body["items"][0]["title"], "note 54");
    assert_eq!(body["items"][49]["title"], "note 5");
    let (status, body) = get(router(app.clone()), "/knowledge?circle=other", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn knowledge_monitor_detail_reads_one_row_and_scopes_its_circle() {
    let (app, _path) = fixture().await;
    let id = publish(&app).await;
    let (status, body) = get(router(app.clone()), &format!("/knowledge?id={id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], id.as_str());
    assert_eq!(body["body"], publication()["body"]);
    assert_eq!(body["publisher_session"], "session-author");
    assert_eq!(body["status"], "active");
    let (status, _) = get(
        router(app.clone()),
        &format!("/knowledge?id={id}&circle=other"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(router(app.clone()), "/knowledge?id=kn-missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/* the token is the only credential; a terminal never claims a peer */
#[tokio::test]
async fn knowledge_monitor_reads_need_the_token_but_not_a_peer() {
    let (app, _path) = fixture().await;
    let guarded = App {
        inner: app.inner.clone(),
        token: Some("terminal-token".into()),
        state_path: app.state_path.clone(),
    };
    let (status, _) = get(router(guarded.clone()), "/knowledge", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = get(router(guarded), "/knowledge", Some("terminal-token")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

use super::*;
use axum::http::Request;
use http_body_util::BodyExt;
use rusqlite::ffi;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
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
    for action in ["publish", "search", "get", "revoke", "delete"] {
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
    for action in ["get", "revoke", "delete"] {
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
fn knowledge_mcp_schemas_expose_only_the_five_minimal_tools() {
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
        (
            "delete",
            "",
            vec![
                "amesh_knowledge_revoke",
                "irreversible",
                "one id per call",
                "asks and inbox",
                "rotation",
                "404",
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
    assert_eq!(knowledge.len(), 5);
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
        ("delete", json!({"id":id})),
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
            "amesh_knowledge_delete",
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

fn person(id: &str, circle: &str) -> Peer {
    Peer {
        peer_id: id.into(),
        name: id.into(),
        path: "/tmp".into(),
        backend: "pi".into(),
        circle: circle.into(),
        status: "online".into(),
        description: String::new(),
        session_id: format!("session-{id}"),
        last_seen: now_unix(),
    }
}

fn store_publish(db: &mut Connection, peer: &Peer, body: &str, now: u64, ttl: u64) -> String {
    let out = knowledge::call(
        db,
        "publish",
        json!({"title": "t", "body": body, "applies_to": "a", "evidence": "e", "ttl_seconds": ttl}),
        peer,
        now,
    )
    .unwrap();
    out["knowledge"]["id"].as_str().unwrap().to_string()
}

fn holds(path: &Path, needle: &[u8]) -> bool {
    fs::read(path).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
}

fn wal_holds(path: &Path, needle: &[u8]) -> bool {
    holds(&path.with_extension("db-wal"), needle)
}

#[test]
fn knowledge_delete_is_open_to_the_circle_and_gone_for_everyone() {
    let path = tests::TempState::new("knowledge-delete-circle");
    let mut db = open_db(&path, now_unix()).unwrap();
    let author = person("author", "c");
    let reader = person("reader", "c");
    let outsider = person("outsider", "other");
    let now = now_unix();
    let id = store_publish(&mut db, &author, "erase the circle copy", now, 3600);
    let foreign = knowledge::call(&mut db, "delete", json!({"id": id}), &outsider, now);
    assert_eq!(foreign.unwrap_err().0, StatusCode::NOT_FOUND);
    assert_eq!(
        knowledge::call(&mut db, "get", json!({"id": id}), &author, now).unwrap()["knowledge"]
            ["id"],
        id,
        "a foreign circle deleted the row"
    );
    let out = knowledge::call(&mut db, "delete", json!({"id": id}), &reader, now).unwrap();
    assert_eq!(out["compacted"], true, "{out}");
    let busy: i64 = db
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    assert_eq!(busy, 5000, "the erase timeout was not restored");
    assert_eq!(
        knowledge::call(&mut db, "get", json!({"id": id}), &author, now)
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND,
        "the row survived its delete"
    );
    assert_eq!(
        knowledge::call(&mut db, "delete", json!({"id": id}), &author, now)
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND,
        "a repeat delete is not 404"
    );
}

#[test]
fn knowledge_delete_takes_any_state_it_is_asked_for() {
    let path = tests::TempState::new("knowledge-delete-states");
    let mut db = open_db(&path, now_unix()).unwrap();
    let peer = person("author", "c");
    let now = now_unix();
    let at = now as i64;
    for (id, expires, revoked) in [
        ("kn-active", at + 3600, false),
        ("kn-expired", at - 30, false),
        ("kn-revoked", at + 3600, true),
    ] {
        db.execute(
            "INSERT INTO knowledge(id,circle,title,body,applies_to,evidence,publisher_peer,publisher_session,created_at,expires_at,revoked_at,revoke_reason,revoked_by_peer,revoked_by_session) VALUES (?1,'c','t','b','a','e','author','session-author',?2,?3,?4,?5,?6,?6)",
            params![
                id,
                at - 60,
                expires,
                revoked.then_some(at - 10),
                revoked.then_some("why"),
                revoked.then_some("author")
            ],
        )
        .unwrap();
        let out = knowledge::call(&mut db, "delete", json!({"id": id}), &peer, now).unwrap();
        assert_eq!(out["compacted"], true, "{id}: {out}");
        let left: i64 = db
            .query_row("SELECT count(*) FROM knowledge WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(left, 0, "{id} survived");
    }
}

#[test]
fn knowledge_delete_takes_the_body_out_of_the_file_and_the_log() {
    let path = tests::TempState::new("knowledge-delete-bytes");
    let mut db = open_db(&path, now_unix()).unwrap();
    let peer = person("author", "c");
    let now = now_unix();
    let marker = b"delete-marker-7f3a91c2";
    let id = store_publish(&mut db, &peer, "delete-marker-7f3a91c2 body", now, 3600);
    assert!(wal_holds(&path, marker), "the body never reached the log");
    db.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
        .unwrap();
    assert!(holds(&path, marker), "the body never reached the file");
    knowledge::call(&mut db, "delete", json!({"id": id}), &peer, now).unwrap();
    assert!(!holds(&path, marker), "the body is still in the file");
    assert!(!wal_holds(&path, marker), "the body is still in the log");
}

#[test]
fn knowledge_delete_404_leaves_the_bytes_alone() {
    let path = tests::TempState::new("knowledge-delete-404");
    let mut db = open_db(&path, now_unix()).unwrap();
    let peer = person("author", "c");
    let now = now_unix();
    let marker = b"purged-marker-2d4c8e10";
    let id = store_publish(&mut db, &peer, "purged-marker-2d4c8e10 body", now, 1);
    db.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
        .unwrap();
    db.execute("DELETE FROM knowledge WHERE id = ?1", [&id])
        .unwrap();
    assert!(holds(&path, marker), "the deleted body left the file early");
    let gone = knowledge::call(&mut db, "delete", json!({"id": id}), &peer, now);
    assert_eq!(gone.unwrap_err().0, StatusCode::NOT_FOUND);
    assert!(holds(&path, marker), "a missing id compacted the file");
}

/* the erase budget is knowledge::ERASE_BUSY_MS, which is private to that module, so the
child pins the value it expects to spend */
const ERASE_BUSY_US: u64 = 500_000;

static ERASE_SLEEP_US: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn record_sleep(
    _vfs: *mut ffi::sqlite3_vfs,
    micros: std::ffi::c_int,
) -> std::ffi::c_int {
    if micros > 0 {
        ERASE_SLEEP_US.fetch_add(micros as u64, Ordering::SeqCst);
    }
    micros
}

/* a copy of the default VFS that accounts for the lock-wait budget the connection asks
for instead of actually sleeping; registered as the default for the child's lifetime and
restored by Drop on both the normal and the panicking path */
struct SleepCountingVfs {
    previous: *mut ffi::sqlite3_vfs,
    copy: Box<ffi::sqlite3_vfs>,
}

impl SleepCountingVfs {
    fn install() -> Self {
        unsafe {
            let previous = ffi::sqlite3_vfs_find(std::ptr::null());
            assert!(!previous.is_null(), "no default sqlite vfs");
            let mut copy = Box::new(std::ptr::read(previous));
            copy.zName = c"amesh-erase-sleep".as_ptr();
            copy.xSleep = Some(record_sleep);
            assert_eq!(
                ffi::sqlite3_vfs_register(copy.as_mut(), 1),
                ffi::SQLITE_OK,
                "the sleep-counting vfs was not registered"
            );
            Self { previous, copy }
        }
    }
}

impl Drop for SleepCountingVfs {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_vfs_unregister(self.copy.as_mut());
            ffi::sqlite3_vfs_register(self.previous, 1);
        }
    }
}

#[test]
fn knowledge_delete_beside_a_reader_reports_the_bytes_as_left_behind() {
    if std::env::var_os("AMESH_ERASE_SLEEP_CHILD").is_some() {
        erase_sleep_budget_body();
        return;
    }
    let filter = format!(
        "{}::knowledge_delete_beside_a_reader_reports_the_bytes_as_left_behind",
        module_path!().split_once("::").unwrap().1
    );
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &filter, "--nocapture"])
        .env("AMESH_ERASE_SLEEP_CHILD", "1")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the erase budget case timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    assert!(status.success(), "the erase budget case failed: {stdout}");
    assert!(
        stdout.contains("test result: ok. 1 passed;"),
        "the erase budget case did not run exactly one test: {stdout}"
    );
    assert!(
        stdout.contains(&format!("erase sleep budget {ERASE_BUSY_US}")),
        "the erase budget case never reported its budget: {stdout}"
    );
}

fn erase_sleep_budget_body() {
    let path = tests::TempState::new("knowledge-delete-reader");
    let peer = person("author", "c");
    let now = now_unix();
    let id = {
        let mut db = open_db(&path, now).unwrap();
        store_publish(&mut db, &peer, "erase beside a reader", now, 3600)
    };
    /* the guard must outlive every connection, which is why it is bound here and dropped
    last: unregistering a vfs that a live connection still points at is a use-after-free */
    let _vfs = SleepCountingVfs::install();
    ERASE_SLEEP_US.store(0, Ordering::SeqCst);
    let mut db = open_db(&path, now).unwrap();
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM peers;")
        .unwrap();
    let out = knowledge::call(&mut db, "delete", json!({"id": id}), &peer, now).unwrap();
    assert_eq!(out["compacted"], false, "{out}");
    /* a reader holding a wal snapshot makes the truncating checkpoint spend exactly the
    erase budget the connection was given, whatever the scheduler does with real time */
    let requested = ERASE_SLEEP_US.load(Ordering::SeqCst);
    assert_eq!(
        requested, ERASE_BUSY_US,
        "the erase asked for {requested}us of lock wait"
    );
    assert_eq!(
        knowledge::call(&mut db, "get", json!({"id": id}), &peer, now)
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND,
        "the row was not deleted"
    );
    let busy: i64 = db
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    assert_eq!(busy, BUSY_MS as i64, "the erase timeout was not restored");
    reader.execute_batch("ROLLBACK").unwrap();
    println!("erase sleep budget {requested}");
}

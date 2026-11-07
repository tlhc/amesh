use super::*;

#[tokio::test]
async fn mcp_tools_have_property_schemas() {
    let (app, _state) = test_app();
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
    let (app, _state) = test_app();
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
    let path = temp_state("long");
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
        .records("worker")
        .iter()
        .find(|e| e.event()["type"] == "ask" && e.event()["correlation_id"] == cid)
        .expect("ask must be persisted on inbox before pending consumes it");
    assert_eq!(inbox_ask.event()["text"].as_str().unwrap(), long);
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
    let (app, _state) = test_app();
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
    let (app, _state) = test_app();
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
    assert!(tool["description"]
        .as_str()
        .unwrap()
        .starts_with("List peers in your circle; cross_circle without circle lists all"));
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
    let (app, _state) = test_app();
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
        assert_eq!(ids(&body), std::slice::from_ref(&here_id));
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
    assert_eq!(ids(&body), std::slice::from_ref(&away_id));
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
    let (app, _state) = test_app();
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
            "in_seconds": 86400,
            "cross_circle": true
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
    assert_eq!(ledger_ids(&body, "schedule_id"), std::slice::from_ref(&sid));
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
    let path = temp_state("sched-mig");
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
            "in_seconds": 86400,
            "cross_circle": true
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
    assert_eq!(ledger_ids(&body, "schedule_id"), std::slice::from_ref(&sid));
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
    assert_eq!(ledger_ids(&body, "schedule_id"), std::slice::from_ref(&sid));
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
    let path = temp_state("job-mig");
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
                depends_on: Vec::new(),
                ask_id: None,
                from_peer: String::new(),
                dispatch: false,
                nudge_at: None,
                finished_at: None,
                created_at: None,
                handoff: None,
                watch: false,
                deadline_seconds: None,
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
    let (app, _state) = test_app();
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

async fn schedule_worker_app() -> (Router, TempState) {
    let (app, state) = test_app();
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "worker", "backend": "pi"}),
    )
    .await;
    (app, state)
}

#[tokio::test]
async fn http_rejects_every_seconds_zero() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = json_req(
        app,
        "POST",
        "/schedules",
        json!({"to_peer": "worker", "text": "wake", "in_seconds": 60, "every_seconds": 0}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mcp_rejects_negative_fire_at() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = mcp_tool(
        app,
        "amesh_schedule_create",
        json!({"to_peer": "worker", "text": "wake", "fire_at": -1}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mcp_rejects_negative_in_seconds() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = mcp_tool(
        app,
        "amesh_schedule_create",
        json!({"to_peer": "worker", "text": "wake", "in_seconds": -1}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mcp_rejects_negative_every_seconds() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = mcp_tool(
        app,
        "amesh_schedule_create",
        json!({"to_peer": "worker", "text": "wake", "in_seconds": 60, "every_seconds": -5}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn schedule_range_rejects_fire_at() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = json_req(
        app,
        "POST",
        "/schedules",
        json!({"to_peer": "worker", "text": "wake", "fire_at": (i64::MAX as u64) + 1}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn schedule_range_rejects_in_seconds() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = json_req(
        app,
        "POST",
        "/schedules",
        json!({"to_peer": "worker", "text": "wake", "in_seconds": u64::MAX}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn schedule_range_rejects_every_seconds() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = json_req(
        app,
        "POST",
        "/schedules",
        json!({"to_peer": "worker", "text": "wake", "every_seconds": (i64::MAX as u64) + 1}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn schedule_range_recurring_next_fire_round_trips() {
    let (app, path) = schedule_worker_app().await;
    let (_, body) = json_req(
        app,
        "POST",
        "/schedules",
        json!({"to_peer": "worker", "text": "wake", "fire_at": 0,
               "every_seconds": i64::MAX as u64}),
    )
    .await;
    let id = body["schedule_id"].as_str().unwrap();
    let app = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    tick_schedules(&app).await;
    drop(app);
    let hub = Hub::open(&path).unwrap();
    assert_eq!(hub.schedules[id].fire_at, i64::MAX as u64);
}

#[tokio::test]
async fn mcp_rejects_non_integer_fire_at() {
    let (app, _state) = schedule_worker_app().await;
    let (st, _) = mcp_tool(
        app,
        "amesh_schedule_create",
        json!({"to_peer": "worker", "text": "wake", "fire_at": 1.5}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_due_schedule_waits_for_an_absent_target() {
    let path = temp_state("sched");
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
        reopened.inbox.count("later") == 0,
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
        let held = &hub.inbox.records("later");
        assert_eq!(
            held.len(),
            1,
            "a second tick must not deliver a consumed schedule again"
        );
        assert_eq!(held[0].event()["type"], "notify");
        assert_eq!(held[0].event()["message"], "wake");
    }
    let settled = Hub::open(&path).unwrap();
    assert!(!settled.schedules.contains_key("s1"));
    assert_eq!(
        settled.inbox.records("later").len(),
        1,
        "the delivery has to be on disk, not only in memory"
    );
    let _ = fs::remove_file(&path);
}

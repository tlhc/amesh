use super::*;
use crate::wire;
use std::future::Future;

fn app() -> (App, tests::TempState) {
    let path = tests::TempState::new("b2");
    let app = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    };
    (app, path)
}

fn question(open: bool) -> Ask {
    serde_json::from_value(json!({
        "correlation_id":"a", "from_peer":"sender", "to_peer":"worker",
        "to_peer_id":"p", "text":"question", "open":open, "reply":null,
        "opened_at":90
    }))
    .unwrap()
}

#[tokio::test]
async fn snapshot_v2_endpoint() {
    let (app, _path) = app();
    let result = snapshot(
        State(app),
        HeaderMap::new(),
        Query(SnapshotQuery {
            circle: None,
            detail: None,
            ask: None,
        }),
    )
    .await
    .unwrap();
    let value = serde_json::to_value(result.0).unwrap();
    assert_eq!(value["schema_version"], 2, "snapshot_v2_endpoint");
    assert!(matches!(
        wire::decode_snapshot(value).unwrap(),
        wire::Decoded::V2(_)
    ));
}

#[tokio::test]
async fn http_open_wait_derived() {
    let (app, _path) = app();
    app.inner
        .lock()
        .await
        .asks
        .insert("a".into(), question(true));
    let result = wait_ask(
        State(app),
        HeaderMap::new(),
        AxumPath("a".into()),
        Json(WaitReq {
            timeout_seconds: Some(0),
        }),
    )
    .await
    .unwrap();
    assert_eq!(result.0["schema_version"], 2, "http_open_wait_derived");
    assert_eq!(
        result.0["state"],
        json!({"kind":"open","progress":{"kind":"gone"}})
    );
}

fn fixture_hub(hub: &mut Hub, fixture: &Value) {
    hub.epoch = fixture["hub_epoch"].as_str().unwrap().into();
    for row in fixture["asks"].as_array().unwrap() {
        let ask: Ask = serde_json::from_value(row.clone()).unwrap();
        hub.asks.insert(ask.correlation_id.clone(), ask);
    }
    for row in fixture["jobs"].as_array().unwrap() {
        let mut raw = row.clone();
        raw["path"] = json!("");
        raw["backend"] = json!("");
        raw["result_summary"] = raw["result"].clone();
        let job: Job = serde_json::from_value(raw).unwrap();
        hub.jobs.insert(job.job_id.clone(), job);
    }
    for row in fixture["peers"]
        .as_array()
        .unwrap()
        .iter()
        .chain(fixture["roster"].as_array().unwrap())
    {
        let mut raw = row.clone();
        raw["path"] = json!("");
        raw["description"] = json!("");
        let peer: Peer = serde_json::from_value(raw).unwrap();
        if hub.peers.contains_key(&peer.peer_id) {
            continue;
        }
        if let Some(activity) = row.get("activity").filter(|a| !a.is_null()) {
            hub.activity.insert(
                peer.peer_id.clone(),
                Activity {
                    state: activity["state"].as_str().unwrap().into(),
                    since: activity["since"].as_u64().unwrap(),
                    observed_at: activity["observed_at"].as_u64().unwrap(),
                    source: activity["source"].as_str().unwrap().into(),
                    reason: activity["reason"].as_str().map(str::to_owned),
                },
            );
        }
        if row["push"] == true {
            hub.sockets
                .insert(peer.peer_id.clone(), (1, mpsc::unbounded_channel().0));
        }
        if row["acks"] == true {
            hub.recv_live.insert(peer.peer_id.clone());
        }
        let count = row["queued"].as_u64().unwrap() as usize;
        if count > 0 {
            /* These timestamps are the queue inputs recorded in each fixture's _note. */
            let queued_at = match peer.peer_id.as_str() {
                "s-nopush" => 990,
                "s-fresh" => 999,
                "s-off" => 900,
                id => panic!("queue input missing for {id}"),
            };
            hub.inbox.enqueue(
                &peer.peer_id,
                (0..count).map(|i| QueuedRecord::new(json!({"fixture_record":i}), queued_at)),
                true,
            );
        }
        hub.peers.insert(peer.peer_id.clone(), peer);
    }
}

async fn check_fixture(text: &str) {
    let expected: Value = serde_json::from_str(text).unwrap();
    let (app, _path) = app();
    let mut hub = app.inner.lock().await;
    fixture_hub(&mut hub, &expected);
    let model = derive::ReadModel::new(&hub, expected["captured_at"].as_u64().unwrap());
    let mut mismatches = Vec::new();
    for row in expected["asks"].as_array().unwrap() {
        let id = row["correlation_id"].as_str().unwrap();
        let actual = json!(model.ask(&hub.asks[id]));
        for field in [
            "from_peer_id",
            "state",
            "actions",
            "closed_just_now",
            "opened_just_now",
        ] {
            if actual[field] != row[field] {
                mismatches.push(format!(
                    "ask {id} {field}: actual={} expected={}",
                    actual[field], row[field]
                ));
            }
        }
    }
    for row in expected["jobs"].as_array().unwrap() {
        let id = row["job_id"].as_str().unwrap();
        let actual = json!(model.job(&hub.jobs[id]));
        for field in [
            "from_peer_id",
            "worker",
            "assignee_id",
            "relation",
            "progress",
            "dispatch_state",
            "actions",
        ] {
            if actual[field] != row[field] {
                mismatches.push(format!(
                    "job {id} {field}: actual={} expected={}",
                    actual[field], row[field]
                ));
            }
        }
    }
    for section in ["peers", "roster"] {
        for row in expected[section].as_array().unwrap() {
            let id = row["peer_id"].as_str().unwrap();
            let actual = json!(model.peer(&hub.peers[id]));
            for field in ["running", "liveness", "delivery"] {
                if actual[field] != row[field] {
                    mismatches.push(format!(
                        "{section} {id} {field}: actual={} expected={}",
                        actual[field], row[field]
                    ));
                }
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "semantic differences:\n{}",
        mismatches.join("\n")
    );
    let view = model.snapshot(&SnapshotQuery {
        circle: Some("c".into()),
        detail: None,
        ask: None,
    });
    let actual = json!(view);
    assert_eq!(
        actual["captured_at"], expected["captured_at"],
        "captured clock"
    );
    for (section, fields) in [
        (
            "asks",
            &[
                "from_peer_id",
                "state",
                "actions",
                "closed_just_now",
                "opened_just_now",
            ][..],
        ),
        (
            "jobs",
            &[
                "from_peer_id",
                "worker",
                "assignee_id",
                "relation",
                "progress",
                "dispatch_state",
                "actions",
            ][..],
        ),
        ("peers", &["running", "liveness", "delivery"][..]),
        ("roster", &["running", "liveness", "delivery"][..]),
    ] {
        for (actual, expected) in actual[section]
            .as_array()
            .unwrap()
            .iter()
            .zip(expected[section].as_array().unwrap())
        {
            for field in fields {
                assert_eq!(
                    actual[*field], expected[*field],
                    "snapshot {section} {field}"
                );
            }
        }
    }
    for (section, id) in [
        ("jobs", "job_id"),
        ("asks", "correlation_id"),
        ("peers", "peer_id"),
        ("roster", "peer_id"),
    ] {
        let ids = |value: &Value| {
            value[section]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row[id].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&actual), ids(&expected), "{section} selected ids");
    }
    assert_eq!(actual["missing"], expected["missing"], "missing ids");
    let serialized = serde_json::to_vec(&view).unwrap();
    let decoded = wire::decode_snapshot(serde_json::from_slice(&serialized).unwrap()).unwrap();
    let wire::Decoded::V2(decoded) = decoded else {
        panic!("producer must decode as V2")
    };
    assert_eq!(decoded, view, "typed roundtrip");
}

macro_rules! fixture_test {
    ($name:ident, $file:literal) => {
        #[tokio::test]
        async fn $name() {
            check_fixture(include_str!(concat!(
                "../../tests/fixtures/wire/",
                $file,
                ".json"
            )))
            .await;
        }
    };
}
fixture_test!(producer_ask_open_progress, "ask_open_progress");
fixture_test!(producer_ask_closed_outcomes, "ask_closed_outcomes");
fixture_test!(producer_job_relations, "job_relations");
fixture_test!(producer_job_dispatch, "job_dispatch");
fixture_test!(producer_peers_delivery, "peers_delivery");

macro_rules! age_test {
    ($name:ident, $progress:expr, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(
                derive::progress_age(&$progress, 100),
                $expected,
                stringify!($name)
            );
        }
    };
}
age_test!(
    age_pending,
    wire::Progress::Pending { since: Some(90) },
    Some(10)
);
age_test!(
    age_pending_absent,
    wire::Progress::Pending { since: None },
    None
);
age_test!(
    age_wait,
    wire::Progress::Wait {
        since: 91,
        reason: None
    },
    Some(9)
);
age_test!(
    age_idle,
    wire::Progress::Idle {
        since: 92,
        why: wire::IdleWhy::TurnEnded
    },
    Some(8)
);
age_test!(age_work, wire::Progress::Work { since: 93 }, Some(7));
age_test!(
    age_future_saturates,
    wire::Progress::Work { since: 101 },
    Some(0)
);
age_test!(age_gone, wire::Progress::Gone, None);
age_test!(age_offline, wire::Progress::Offline, None);
age_test!(age_unknown, wire::Progress::Unknown, None);

fn add_peer(hub: &mut Hub, id: &str, name: &str, status: &str) {
    hub.peers.insert(
        id.into(),
        serde_json::from_value(json!({
            "peer_id":id,"name":name,"path":"","backend":"codex","circle":"c",
            "status":status,"description":"","last_seen":99
        }))
        .unwrap(),
    );
}

#[tokio::test]
async fn final_capture_clock_coherence() {
    let (app, _path) = app();
    let mut hub = app.inner.lock().await;
    add_peer(&mut hub, "p", "worker", "online");
    hub.activity.insert(
        "p".into(),
        Activity {
            state: "idle".into(),
            since: 80,
            observed_at: 80,
            source: "test".into(),
            reason: None,
        },
    );
    let ask = question(true);
    let before = capture_wait(&hub, &ask, None, 99);
    let after = capture_wait(&hub, &ask, None, 100);
    let before = before.derived.unwrap();
    let after = after.derived.unwrap();
    assert_eq!(
        (before.captured_at, before.for_secs, before.state),
        (
            99,
            Some(9),
            wire::AskState::Open {
                progress: wire::Progress::Pending { since: Some(90) }
            }
        )
    );
    assert_eq!(
        (after.captured_at, after.for_secs, after.state),
        (
            100,
            Some(10),
            wire::AskState::Open {
                progress: wire::Progress::Idle {
                    since: 90,
                    why: wire::IdleWhy::NotPickedUp
                }
            }
        )
    );
}

#[tokio::test]
async fn wait_carries_actions_and_the_resolved_sender() {
    let (app, _path) = app();
    let mut hub = app.inner.lock().await;
    add_peer(&mut hub, "sender-id", "sender", "online");
    add_peer(&mut hub, "p", "worker", "online");
    hub.activity.insert(
        "p".into(),
        Activity {
            state: "idle".into(),
            since: 80,
            observed_at: 80,
            source: "test".into(),
            reason: None,
        },
    );
    let ask = question(true);
    let idle = capture_wait(&hub, &ask, None, 100).derived.unwrap();
    assert_eq!(idle.from_peer_id.as_deref(), Some("sender-id"));
    assert_eq!(
        idle.actions,
        vec![wire::AskAction::Nudge { to: "p".into() }]
    );
    hub.peers.remove("p");
    let gone = capture_wait(&hub, &ask, None, 100).derived.unwrap();
    assert_eq!(gone.actions, vec![wire::AskAction::CloseLeft]);
}

#[tokio::test]
async fn capture_survives_close_and_disconnect() {
    let (app, _path) = app();
    let mut hub = app.inner.lock().await;
    add_peer(&mut hub, "sender-id", "sender", "online");
    hub.sockets
        .insert("sender-id".into(), (1, mpsc::unbounded_channel().0));
    hub.asks.insert("a".into(), question(true));
    let captured = capture_wait(&hub, &hub.asks["a"], Some("sender-id"), 100);
    hub.asks.get_mut("a").unwrap().open = false;
    hub.sockets.clear();
    let summary = wait_summary(&captured, 0);
    assert_eq!(
        (
            summary["open"].clone(),
            summary["schema_version"].clone(),
            summary["hint"].clone()
        ),
        (json!(true), json!(2), json!(WAIT_OPEN_HINT))
    );
}

#[tokio::test]
async fn closed_capture_stays_closed_after_reopen() {
    let (app, _path) = app();
    let mut hub = app.inner.lock().await;
    hub.asks.insert("a".into(), question(false));
    let captured = capture_wait(&hub, &hub.asks["a"], None, 100);
    hub.asks.get_mut("a").unwrap().open = true;
    assert_eq!(
        wait_summary(&captured, 0),
        json!({"correlation_id":"a","from_peer":"sender","to_peer":"worker",
        "open":false,"timed_out":false,"reply":null,"failed":false,"timeout_seconds":0})
    );
}

async fn mcp_wait(app: &App, timeout: u64) -> Result<Value, (StatusCode, Json<Value>)> {
    let result = mcp_call(app,json!({"name":"amesh_wait","arguments":{"correlation_id":"a","from_peer":"sender","timeout_seconds":timeout}})).await?;
    Ok(serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap())
}

#[tokio::test]
async fn mcp_open_wait_derived() {
    let (app, _path) = app();
    app.inner
        .lock()
        .await
        .asks
        .insert("a".into(), question(true));
    let result = mcp_wait(&app, 0).await.unwrap();
    assert_eq!(result["schema_version"], 2, "mcp_open_wait_derived");
    assert_eq!(result["for_secs"], Value::Null);
    assert!(result.get("text").is_none());
}

#[tokio::test]
async fn http_closed_wait_bytes() {
    let (app, _path) = app();
    let mut ask = question(false);
    ask.text = "长".repeat(501);
    ask.reply = Some("reply".repeat(501));
    ask.closed_by = Some("hand".into());
    let expected = json!(ask).to_string();
    app.inner.lock().await.asks.insert("a".into(), ask);
    let result = wait_ask(
        State(app),
        HeaderMap::new(),
        AxumPath("a".into()),
        Json(WaitReq {
            timeout_seconds: Some(0),
        }),
    )
    .await
    .unwrap();
    assert_eq!(result.0.to_string(), expected, "http_closed_wait_bytes");
}

#[tokio::test]
async fn mcp_closed_wait_bytes() {
    let (app, _path) = app();
    app.inner
        .lock()
        .await
        .asks
        .insert("a".into(), question(false));
    let result = mcp_call(
        &app,
        json!({"name":"amesh_wait", "arguments":{"correlation_id":"a", "timeout_seconds":0}}),
    )
    .await
    .unwrap();
    let expected = json!({"correlation_id":"a","from_peer":"sender","to_peer":"worker",
        "open":false,"timed_out":false,"reply":null,"failed":false,"timeout_seconds":0});
    assert_eq!(
        result["content"][0]["text"].as_str().unwrap(),
        expected.to_string(),
        "mcp_closed_wait_bytes"
    );
}

#[tokio::test]
async fn ask_blocking_closed_full_text() {
    let (app, _path) = app();
    let mut ask = question(false);
    ask.text = "文".repeat(501);
    let expected = json!(ask);
    app.inner.lock().await.asks.insert("a".into(), ask);
    let result = ask_blocking(
        State(app),
        HeaderMap::new(),
        Json(json!({"prompt":"reuse","correlation_id":"a","timeout_seconds":0})),
    )
    .await
    .unwrap();
    assert_eq!(result.0, expected, "ask_blocking_closed_full_text");
}

async fn started_then_delete<F: std::future::Future>(future: F, app: &App) -> F::Output {
    tokio::pin!(future);
    std::future::poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "wait must have started while ask is open"
        );
        std::task::Poll::Ready(())
    })
    .await;
    app.inner.lock().await.asks.remove("a");
    future.await
}

#[tokio::test]
async fn http_deleted_while_waiting() {
    let (app, _path) = app();
    app.inner
        .lock()
        .await
        .asks
        .insert("a".into(), question(true));
    let future = wait_ask(
        State(app.clone()),
        HeaderMap::new(),
        AxumPath("a".into()),
        Json(WaitReq {
            timeout_seconds: Some(1),
        }),
    );
    let error = started_then_delete(future, &app).await.err().unwrap();
    assert_eq!(
        (error.0, error.1 .0),
        (StatusCode::NOT_FOUND, json!({"error":"unknown ask"})),
        "http_deleted_while_waiting"
    );
}

#[tokio::test]
async fn mcp_deleted_while_waiting() {
    let (app, _path) = app();
    app.inner
        .lock()
        .await
        .asks
        .insert("a".into(), question(true));
    let error = started_then_delete(mcp_wait(&app, 1), &app)
        .await
        .err()
        .unwrap();
    assert_eq!(
        (error.0, error.1 .0),
        (StatusCode::NOT_FOUND, json!({"error":"unknown ask"})),
        "mcp_deleted_while_waiting"
    );
}

#[tokio::test]
async fn http_open_wait_full_text() {
    let (app, _path) = app();
    let mut ask = question(true);
    ask.text = "文".repeat(501);
    app.inner.lock().await.asks.insert("a".into(), ask);
    let result = wait_ask(
        State(app),
        HeaderMap::new(),
        AxumPath("a".into()),
        Json(WaitReq {
            timeout_seconds: Some(0),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        result.0["text"],
        "文".repeat(501),
        "http_open_wait_full_text"
    );
}

async fn sized_mcp_wait(state: &str) -> String {
    let (app, _path) = app();
    let mut hub = app.inner.lock().await;
    let sender = "s".repeat(50);
    let worker = "w".repeat(50);
    add_peer(&mut hub, &sender, &sender, "online");
    add_peer(&mut hub, &worker, &worker, "online");
    hub.sockets
        .insert(sender.clone(), (1, mpsc::unbounded_channel().0));
    let mut ask = question(true);
    ask.correlation_id = "00000000-0000-0000-0000-000000000000".into();
    ask.from_peer = sender.clone();
    ask.to_peer = worker.clone();
    ask.to_peer_id = worker.clone();
    ask.text = "q".repeat(600);
    ask.opened_at = Some(80);
    set_activity(
        &mut hub,
        &worker,
        &ActivityReport {
            state: state.into(),
            source: None,
            reason: Some("\u{20000}".repeat(121)),
            check: false,
            ends_wait: false,
        },
        90,
    );
    let captured = capture_wait(&hub, &ask, Some(&sender), 100);
    let summary = wait_summary(&captured, 0).to_string();
    eprintln!("bounded MCP {state}: {} bytes", summary.len());
    summary
}

#[tokio::test]
async fn mcp_wait_reason_stays_at_the_stored_cap() {
    let summary = sized_mcp_wait("wait").await;
    let value: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(
        value["state"]["progress"]["reason"]
            .as_str()
            .map(|reason| reason.chars().count()),
        Some(120),
        "the wait shows the reason as stored, cut to 120 characters"
    );
    assert!(
        summary.len() <= 1158,
        "wait reason summary grew to {} bytes",
        summary.len()
    );
}

#[tokio::test]
async fn mcp_wait_nudge_size_bound() {
    let summary = sized_mcp_wait("idle").await;
    assert!(
        summary.len() <= 759,
        "nudge summary grew to {} bytes",
        summary.len()
    );
}

#[tokio::test]
async fn mcp_wait_excludes_question_anywhere() {
    let summary = sized_mcp_wait("wait").await;
    assert!(
        !summary.contains(&"q".repeat(600)),
        "question echoed in MCP response"
    );
}

#[tokio::test]
async fn http_timeout_delete_at_final_lock() {
    let (app, _path) = app();
    app.inner
        .lock()
        .await
        .asks
        .insert("a".into(), question(true));
    let held = app.inner.lock().await;
    let response = wait_ask(
        State(app.clone()),
        HeaderMap::new(),
        AxumPath("a".into()),
        Json(WaitReq {
            timeout_seconds: Some(0),
        }),
    );
    tokio::pin!(response);
    std::future::poll_fn(|cx| {
        assert!(response.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let deletion = async {
        app.inner.lock().await.asks.remove("a");
    };
    tokio::pin!(deletion);
    std::future::poll_fn(|cx| {
        assert!(deletion.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(held);
    let (response, ()) = tokio::join!(response, deletion);
    let body = response.unwrap().0;
    assert_eq!(
        (body["correlation_id"].clone(), body["open"].clone()),
        (json!("a"), json!(true)),
        "final capture must precede the queued deletion: {body}"
    );
}

#[tokio::test]
async fn http_timeout_without_deletion_control() {
    let (app, _path) = app();
    app.inner
        .lock()
        .await
        .asks
        .insert("a".into(), question(true));
    let body = wait_ask(
        State(app),
        HeaderMap::new(),
        AxumPath("a".into()),
        Json(WaitReq {
            timeout_seconds: Some(0),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(
        (body["correlation_id"].clone(), body["open"].clone()),
        (json!("a"), json!(true))
    );
}

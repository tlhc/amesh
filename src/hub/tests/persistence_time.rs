use super::*;

#[tokio::test]
async fn schedule_queue_timestamp_starts_after_lock_wait() {
    use std::future::Future;
    use std::task::{Context, Waker};

    let path = temp_state("schedule-queue-time");
    let mut hub = Hub::open(&path).unwrap();
    let _rx = recv_peer(&mut hub, "worker");
    hub.schedules.insert(
        "s1".into(),
        Schedule {
            schedule_id: "s1".into(),
            from_peer: "boss".into(),
            to_peer: "worker".into(),
            text: "wake".into(),
            kind: "notify".into(),
            fire_at: 0,
            every_seconds: Some(60),
            circle: "default".into(),
        },
    );
    let app = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    let held = app.inner.lock().await;
    let mut tick = Box::pin(tick_schedules(&app));
    let mut contender = Box::pin(app.inner.lock());
    let mut context = Context::from_waker(Waker::noop());
    assert!(tick.as_mut().poll(&mut context).is_pending());
    assert!(contender.as_mut().poll(&mut context).is_pending());
    drop(held);
    let cutoff_start = now_unix();
    assert!(tick.as_mut().poll(&mut context).is_pending());
    let held = contender.await;
    let cutoff_end = now_unix();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let released_at = now_unix();
    assert!(released_at.saturating_sub(cutoff_end) >= 2);
    drop(held);
    tick.await;

    let hub = app.inner.lock().await;
    let queued = hub.inbox.records("worker");
    assert_eq!(queued.len(), 1);
    assert!(queued[0].queued_at() >= released_at);
    assert_eq!(hub.inbox.stuck("worker", released_at), None);
    assert!((cutoff_start + 60..=cutoff_end + 60).contains(&hub.schedules["s1"].fire_at));
}

#[test]
fn reopen_preserves_queue_time() {
    let path = temp_state("a2-reopen");
    let mut hub = Hub::open_at(&path, 100).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    queue_inbox(&mut hub, "w", [json!({"type": "notify", "id": "one"})], 100);
    persist(&mut hub).unwrap();
    let expected = hub.inbox.clone();
    drop(hub);
    let hub = Hub::open_at(&path, 500).unwrap();
    assert_eq!(hub.inbox, expected);
    assert_eq!(hub.inbox.stuck("w", 500), Some((1, 100)));
}

#[test]
fn negative_stamp_reads_as_zero() {
    let path = temp_state("a2-negative");
    let mut hub = Hub::open_at(&path, 100).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    queue_inbox(&mut hub, "w", [json!({"type": "notify", "id": "one"})], 100);
    persist(&mut hub).unwrap();
    hub.db
        .execute("UPDATE inbox SET queued_at = -1", [])
        .unwrap();
    drop(hub);
    let hub = Hub::open_at(&path, 200).unwrap();
    assert_eq!(hub.inbox.stuck("w", 200), Some((1, 0)));
}

fn sched_row(id: &str, fire_at: u64, every_seconds: Option<u64>) -> Schedule {
    Schedule {
        schedule_id: id.into(),
        from_peer: "boss".into(),
        to_peer: "w".into(),
        text: "wake".into(),
        kind: "notify".into(),
        fire_at,
        every_seconds,
        circle: "default".into(),
    }
}

#[test]
fn negative_schedule_fire_at_reads_as_zero() {
    let path = temp_state("sched-neg-fire");
    let mut hub = Hub::open_at(&path, 100).unwrap();
    hub.schedules
        .insert("s-past".into(), sched_row("s-past", 50, None));
    persist(&mut hub).unwrap();
    hub.db
        .execute(
            "UPDATE schedules SET fire_at = -1 WHERE schedule_id = 's-past'",
            [],
        )
        .unwrap();
    drop(hub);
    let hub = Hub::open_at(&path, 200).unwrap();
    assert_eq!(hub.schedules["s-past"].fire_at, 0);
}

#[test]
fn negative_schedule_every_seconds_drops_the_row() {
    let path = temp_state("sched-neg-every");
    let mut hub = Hub::open_at(&path, 100).unwrap();
    hub.schedules
        .insert("s-loop".into(), sched_row("s-loop", 100, Some(60)));
    persist(&mut hub).unwrap();
    hub.db
        .execute(
            "UPDATE schedules SET every_seconds = -5 WHERE schedule_id = 's-loop'",
            [],
        )
        .unwrap();
    drop(hub);
    let hub = Hub::open_at(&path, 200).unwrap();
    assert!(
        !hub.schedules.contains_key("s-loop"),
        "negative every_seconds must drop the row, got {:?}",
        hub.schedules.get("s-loop").map(|s| s.every_seconds)
    );
}

#[test]
fn zero_schedule_every_seconds_still_loads() {
    let path = temp_state("sched-zero-every");
    let mut hub = Hub::open_at(&path, 100).unwrap();
    hub.schedules
        .insert("s-zero".into(), sched_row("s-zero", 100, Some(0)));
    persist(&mut hub).unwrap();
    drop(hub);
    let hub = Hub::open_at(&path, 200).unwrap();
    assert_eq!(hub.schedules["s-zero"].every_seconds, Some(0));
}

#[test]
fn legacy_schema_backfills_once() {
    let path = temp_state("a2-legacy");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE inbox(peer_id TEXT, seq INTEGER, payload TEXT); INSERT INTO inbox VALUES ('w', 0, '{}');").unwrap();
    drop(db);
    let hub = Hub::open_at(&path, 777).unwrap();
    let stamp: i64 = hub
        .db
        .query_row("SELECT queued_at FROM inbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stamp, 777);
    drop(hub);
    let hub = Hub::open_at(&path, 900).unwrap();
    let stamp: i64 = hub
        .db
        .query_row("SELECT queued_at FROM inbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stamp, 777);
    hub.db
        .execute_batch("INSERT INTO inbox VALUES ('w', 1, '{}', NULL), ('w', 2, '{}', 0);")
        .unwrap();
    drop(hub);
    let hub = Hub::open_at(&path, 1000).unwrap();
    let mut stmt = hub
        .db
        .prepare("SELECT queued_at FROM inbox ORDER BY seq")
        .unwrap();
    let stamps = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(stamps, vec![777, 1000, 0]);
}

#[test]
fn payload_stays_raw_with_independent_time() {
    let path = temp_state("a2-payload");
    let mut hub = Hub::open_at(&path, 100).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    let event = json!({"event": 1, "queued_at": 5, "type": "notify"});
    hub.inbox
        .put("w", vec![QueuedRecord::new(event.clone(), 100)]);
    persist(&mut hub).unwrap();
    let (payload, stamp): (String, i64) = hub
        .db
        .query_row("SELECT payload, queued_at FROM inbox", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(payload, event.to_string());
    assert_eq!(serde_json::from_str::<Value>(&payload).unwrap(), event);
    assert_eq!(stamp, 100);
    drop(hub);
    let hub = Hub::open_at(&path, 500).unwrap();
    assert_eq!(hub.inbox.records("w"), &[QueuedRecord::new(event, 100)]);
}

#[test]
fn json_import_stamps_import_instant() {
    let path = temp_state("a2-json");
    let event = json!({"event": 1, "queued_at": 5, "type": "notify"});
    fs::write(path.with_extension("json"), json!({
        "peers": {"w": {"peer_id": "w", "name": "w", "path": "/tmp", "backend": "pi", "circle": "default", "status": "online", "description": "", "session_id": "", "last_seen": 0}},
        "asks": {}, "jobs": {}, "schedules": {}, "inbox": {"w": [event.clone()]}
    }).to_string()).unwrap();
    let hub = Hub::open_at(&path, 333).unwrap();
    assert_eq!(
        hub.inbox.records("w"),
        &[QueuedRecord::new(event.clone(), 333)]
    );
    drop(hub);
    let hub = Hub::open_at(&path, 900).unwrap();
    assert_eq!(hub.inbox.records("w"), &[QueuedRecord::new(event, 333)]);
}

#[test]
fn failed_persist_restores_exact_queue_times() {
    let path = temp_state("a2-rollback");
    let mut hub = Hub::open_at(&path, 100).unwrap();
    let _rx = recv_peer(&mut hub, "w");
    hub.inbox.put(
        "w",
        vec![
            QueuedRecord::new(json!({"id": "first"}), 100),
            QueuedRecord::new(json!({"id": "second"}), 70),
        ],
    );
    persist(&mut hub).unwrap();
    let expected = hub.inbox.clone();
    hub.inbox.take("w");
    hub.inbox
        .put("w", vec![QueuedRecord::new(json!({"id": "new"}), 500)]);
    hub.db.execute_batch("PRAGMA query_only = ON").unwrap();
    assert!(persist_ok(&mut hub).is_err());
    assert_eq!(hub.inbox, expected);
}

#[test]
fn migration_failure_rolls_back_column_and_rows() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE inbox(peer_id TEXT, seq INTEGER, payload TEXT); INSERT INTO inbox VALUES ('w', 0, '{}'); CREATE TRIGGER fail_backfill BEFORE UPDATE ON inbox BEGIN SELECT RAISE(ABORT, 'a2 backfill failure'); END;").unwrap();
    let error = migrate_inbox_queued_at(&db, 777).unwrap_err();
    assert!(error.contains("a2 backfill failure"), "{error}");
    let columns: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('inbox') WHERE name = 'queued_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(columns, 0);
    let row: (String, i64, String) = db
        .query_row("SELECT peer_id, seq, payload FROM inbox", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(row, ("w".into(), 0, "{}".into()));
    db.execute_batch("DROP TRIGGER fail_backfill").unwrap();
    migrate_inbox_queued_at(&db, 777).unwrap();
    let stamp: i64 = db
        .query_row("SELECT queued_at FROM inbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stamp, 777);
}

#[test]
fn legacy_send_failure_first_time() {
    let (mut hub, _path) = undelivered_hub();
    let (tx, rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (1, tx));
    drop(rx);
    persist_then_deliver(&mut hub, "worker", json!({"type":"notify"}), 17).unwrap();
    let held = &hub.inbox.records("worker")[0];
    assert_eq!(held.queued_at(), 17);
    assert!(held.event().get("id").is_none());
}

#[test]
fn dropped_link_return_time() {
    let (mut hub, _path) = undelivered_hub();
    let record = QueuedRecord::new(json!({"type":"notify"}), 17);
    assert!(return_undelivered(
        &mut hub,
        "worker",
        vec![Outbound::Record(record.clone())]
    ));
    assert_eq!(hub.inbox.records("worker"), &[record]);
}

#[test]
fn successor_second_failure_time() {
    let (mut hub, _path) = undelivered_hub();
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (2, tx));
    let record = QueuedRecord::new(json!({"type":"notify"}), 17);
    assert!(!return_undelivered(
        &mut hub,
        "worker",
        vec![Outbound::Record(record.clone())]
    ));
    let forwarded = rx.try_recv().unwrap();
    drop(rx);
    assert!(return_undelivered(&mut hub, "worker", vec![forwarded]));
    assert_eq!(hub.inbox.records("worker"), &[record]);
}

#[test]
fn recv_copy_time_equals_original() {
    let (mut hub, _path) = undelivered_hub();
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (1, tx));
    hub.recv_live.insert("worker".into());
    hub.recv_known.insert("worker".into());
    persist_then_deliver(&mut hub, "worker", json!({"type":"notify"}), 17).unwrap();
    let Outbound::Copy(copy) = rx.try_recv().unwrap() else {
        panic!("record expected")
    };
    assert_eq!(&copy, &hub.inbox.records("worker")[0]);
    assert_eq!(copy.queued_at(), 17);
    assert!(copy.event()["id"].as_str().is_some());
}

#[tokio::test]
async fn pending_failure_restores_exact_image() {
    let (mut hub, path) = undelivered_hub();
    hub.inbox.put(
        "worker",
        vec![
            QueuedRecord::new(json!({"type":"ask", "correlation_id":"stale"}), 11),
            QueuedRecord::new(json!({"type":"notify", "id":"valid"}), 17),
        ],
    );
    persist(&mut hub).unwrap();
    let expected = hub.inbox.clone();
    hub.db.execute_batch("PRAGMA query_only = ON").unwrap();
    let app = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    assert!(pending_asks(
        State(app.clone()),
        HeaderMap::new(),
        Query(PendingQuery {
            peer_id: Some("worker".into())
        })
    )
    .await
    .is_err());
    assert_eq!(app.inner.lock().await.inbox, expected);
}

#[tokio::test]
async fn recovery_write_failure_keeps_inflight_memory() {
    let (mut hub, path) = undelivered_hub();
    persist(&mut hub).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (1, tx));
    hub.db.execute_batch("PRAGMA query_only = ON").unwrap();
    let record = QueuedRecord::new(json!({"type":"notify"}), 17);
    let app = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    close_connection(
        &app,
        "worker",
        1,
        false,
        rx,
        vec![Outbound::Record(record.clone())],
    )
    .await;
    assert_eq!(app.inner.lock().await.inbox.records("worker"), &[record]);
}

#[test]
fn transfer_duplicate_keeps_earliest_time() {
    let mut inbox = Inbox::default();
    let payload = json!({"id":"same", "message":"target"});
    inbox.put(
        "target",
        vec![
            QueuedRecord::new(payload.clone(), 90),
            QueuedRecord::new(payload.clone(), 100),
        ],
    );
    inbox.put(
        "old",
        vec![QueuedRecord::new(json!({"id":"same", "message":"old"}), 17)],
    );
    inbox.transfer_owed("old", "target");
    assert_eq!(
        inbox.records("target"),
        &[
            QueuedRecord::new(payload.clone(), 17),
            QueuedRecord::new(payload, 17)
        ]
    );
}

#[tokio::test]
async fn register_copy_uses_merged_time() {
    let (mut hub, path) = undelivered_hub();
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.sockets.insert("worker".into(), (1, tx));
    hub.recv_live.insert("worker".into());
    for id in ["worker", "old"] {
        hub.owed.insert(
            id.into(),
            Owed {
                owner: "session".into(),
                since: now_unix(),
            },
        );
    }
    let event = json!({"id":"same", "type":"notify"});
    hub.inbox
        .put("worker", vec![QueuedRecord::new(event.clone(), 90)]);
    hub.inbox.put("old", vec![QueuedRecord::new(event, 17)]);
    persist(&mut hub).unwrap();
    let app = App {
        inner: Arc::new(Mutex::new(hub)),
        token: None,
        state_path: path.to_path_buf(),
    };
    let (status, _) = json_req(router(app.clone()), "POST", "/peers", json!({"peer_id":"worker", "name":"worker", "backend":"pi", "path":"/tmp", "session_id":"session"})).await;
    assert_eq!(status, StatusCode::OK);
    let copy = loop {
        if let Outbound::Copy(record) = rx.try_recv().unwrap() {
            break record;
        }
    };
    assert_eq!(copy.queued_at(), 17);
    assert_eq!(app.inner.lock().await.inbox.records("worker"), &[copy]);
}

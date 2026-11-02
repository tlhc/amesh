use super::*;

fn fixture() -> (Connection, DiskState) {
    let mut db = open_db(Path::new(":memory:"), 100).unwrap();
    db.execute_batch(
        "INSERT INTO peers(peer_id,name,path,backend,circle,status,description,session_id,last_seen)
         VALUES ('p','p','/repo','codex','c','online','','',100);
         INSERT INTO asks(correlation_id,from_peer,to_peer,to_peer_id,text,open,from_circle,to_circle)
         VALUES ('a','p','p','p','question',1,'c','c');
         INSERT INTO jobs(job_id,title,prompt,path,backend,state,circle)
         VALUES ('j','job','prompt','/repo','codex','queued','c');
         INSERT INTO schedules(schedule_id,from_peer,to_peer,text,kind,fire_at,circle)
         VALUES ('s','p','p','later','notify',200,'c');
         INSERT INTO inbox(peer_id,seq,payload,queued_at)
         VALUES ('p',0,'{\"id\":\"first\"}',100),('p',1,'{\"id\":\"second\"}',101);
         INSERT INTO recv_peers(peer_id,pruned_at,owner) VALUES ('p',99,'session');
         INSERT INTO mcp_servers(peer_id,payload) VALUES ('p','[]');",
    ).unwrap();
    let disk = read_snapshot(&db).unwrap();
    write_snapshot(&mut db, &disk).unwrap();
    (db, disk)
}

macro_rules! unchanged_table {
    ($name:ident, $table:literal) => {
        #[test]
        fn $name() {
            let (mut db, disk) = fixture();
            db.execute_batch(&format!(
                "CREATE TEMP TABLE audit(op TEXT);
                 CREATE TEMP TRIGGER audit_insert AFTER INSERT ON {0} BEGIN INSERT INTO audit VALUES ('insert'); END;
                 CREATE TEMP TRIGGER audit_update AFTER UPDATE ON {0} BEGIN INSERT INTO audit VALUES ('update'); END;
                 CREATE TEMP TRIGGER audit_delete AFTER DELETE ON {0} BEGIN INSERT INTO audit VALUES ('delete'); END;",
                $table
            )).unwrap();
            write_snapshot(&mut db, &disk).unwrap();
            let changes: i64 = db.query_row("SELECT count(*) FROM audit", [], |r| r.get(0)).unwrap();
            assert_eq!(changes, 0, "unchanged {} rows were written", $table);
        }
    };
}

unchanged_table!(unchanged_peers, "peers");
unchanged_table!(unchanged_asks, "asks");
unchanged_table!(unchanged_jobs, "jobs");
unchanged_table!(unchanged_schedules, "schedules");
unchanged_table!(unchanged_inbox, "inbox");
unchanged_table!(unchanged_recv_peers, "recv_peers");
unchanged_table!(unchanged_mcp_servers, "mcp_servers");

#[test]
fn changing_one_peer_writes_one_row() {
    let (mut db, mut disk) = fixture();
    disk.peers.get_mut("p").unwrap().last_seen += 1;
    let before = db.total_changes();
    write_snapshot(&mut db, &disk).unwrap();
    assert_eq!(db.total_changes() - before, 1);
    assert_eq!(read_snapshot(&db).unwrap().peers["p"].last_seen, 101);
}

#[test]
fn removing_one_ask_writes_one_row() {
    let (mut db, mut disk) = fixture();
    disk.asks.remove("a");
    let before = db.total_changes();
    write_snapshot(&mut db, &disk).unwrap();
    assert_eq!(db.total_changes() - before, 1);
    assert!(read_snapshot(&db).unwrap().asks.is_empty());
}

#[test]
fn appending_inbox_writes_one_row() {
    let (mut db, mut disk) = fixture();
    disk.inbox
        .get_mut("p")
        .unwrap()
        .push(QueuedRecord::new(json!({"id":"third"}), 102));
    let before = db.total_changes();
    write_snapshot(&mut db, &disk).unwrap();
    assert_eq!(db.total_changes() - before, 1);
    let stored = read_snapshot(&db).unwrap();
    assert_eq!(stored.inbox["p"][2].event()["id"], "third");
    assert_eq!(stored.inbox["p"][2].queued_at(), 102);
}

#[test]
fn removing_inbox_head_preserves_order_and_age() {
    let (mut db, mut disk) = fixture();
    disk.inbox
        .get_mut("p")
        .unwrap()
        .push(QueuedRecord::new(json!({"id":"third"}), 102));
    write_snapshot(&mut db, &disk).unwrap();
    disk.inbox.get_mut("p").unwrap().remove(0);
    let before = db.total_changes();
    write_snapshot(&mut db, &disk).unwrap();
    assert_eq!(db.total_changes() - before, 1);
    let stored = read_snapshot(&db).unwrap();
    assert_eq!(stored.inbox["p"].len(), 2);
    assert_eq!(stored.inbox["p"][0].event()["id"], "second");
    assert_eq!(stored.inbox["p"][0].queued_at(), 101);
}

macro_rules! changed_table {
    ($name:ident, $sql:literal) => {
        #[test]
        fn $name() {
            let (mut db, original) = fixture();
            let table = $sql.split_whitespace().nth(1).unwrap();
            let rows = |db: &Connection| {
                let mut query = db.prepare(&format!("SELECT * FROM {table}")).unwrap();
                let columns = query.column_count();
                query
                    .query_map([], |row| {
                        (0..columns)
                            .map(|column| row.get::<_, rusqlite::types::Value>(column))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap()
            };
            let original_rows = rows(&db);
            db.execute_batch($sql).unwrap();
            let changed_rows = rows(&db);
            let changed = read_snapshot(&db).unwrap();
            write_snapshot(&mut db, &original).unwrap();
            let before = db.total_changes();
            write_snapshot(&mut db, &changed).unwrap();
            assert_eq!(db.total_changes() - before, 1);
            assert_eq!(rows(&db), changed_rows);
            let before = db.total_changes();
            write_snapshot(&mut db, &original).unwrap();
            assert_eq!(db.total_changes() - before, 1);
            assert_eq!(rows(&db), original_rows);
            let before = db.total_changes();
            write_snapshot(&mut db, &original).unwrap();
            assert_eq!(db.total_changes() - before, 0);
        }
    };
}

changed_table!(changing_peer_fields, "UPDATE peers SET name = 'other', path = '/other', backend = 'pi', circle = 'd', status = 'offline', description = 'changed', last_seen = 200");
changed_table!(changing_nullable_ask, "UPDATE asks SET from_peer = 'other', to_peer = 'other', to_peer_id = 'other', text = 'changed', from_circle = 'd', to_circle = 'd', reply = 'answer', closed_at = 200, opened_at = 100, closed_by = 'p', open = 0, failed = 1");
changed_table!(changing_nullable_job, "UPDATE jobs SET title = 'changed', prompt = 'changed', path = '/other', backend = 'pi', state = 'done', circle = 'd', assigned_peer = 'p', result_summary = 'done', ask_id = 'a', from_peer = 'p', nudge_at = 110, finished_at = 120, created_at = 100, handoff = 'next', depends_on = '[\"other\"]', dispatch = 1");
changed_table!(changing_nullable_schedule, "UPDATE schedules SET from_peer = 'other', to_peer = 'other', text = 'changed', kind = 'ask', fire_at = 300, circle = 'd', every_seconds = 60");
changed_table!(
    changing_owed_owner,
    "UPDATE recv_peers SET owner = 'new-session', pruned_at = 120"
);
changed_table!(
    changing_mcp_servers,
    "UPDATE mcp_servers SET payload = '[{\"name\":\"server\"}]'"
);

#[test]
fn failed_write_rolls_back_deletes_and_updates_before_retry() {
    let (mut db, mut disk) = fixture();
    disk.asks.clear();
    disk.peers.get_mut("p").unwrap().last_seen = 200;
    disk.mcp_servers
        .get_mut("p")
        .unwrap()
        .push(json!({"name":"server"}));
    db.execute_batch("CREATE TEMP TRIGGER fail_write BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(write_snapshot(&mut db, &disk).is_err());
    let stored = read_snapshot(&db).unwrap();
    assert_eq!(stored.peers["p"].last_seen, 100);
    assert!(stored.asks.contains_key("a"));
    assert!(stored.mcp_servers["p"].is_empty());
    db.execute_batch("DROP TRIGGER fail_write").unwrap();
    let before = db.total_changes();
    write_snapshot(&mut db, &disk).unwrap();
    assert_eq!(db.total_changes() - before, 3);
    let stored = read_snapshot(&db).unwrap();
    assert_eq!(stored.peers["p"].last_seen, 200);
    assert!(stored.asks.is_empty());
    assert_eq!(stored.mcp_servers["p"].len(), 1);
}

macro_rules! legacy_map_key {
    ($name:ident, $map:ident, $old:literal, $field:ident, $table:literal, $column:literal) => {
        #[test]
        fn $name() {
            let (mut db, mut disk) = fixture();
            disk.$map.get_mut($old).unwrap().$field = "new-id".into();
            write_snapshot(&mut db, &disk).unwrap();
            let ids = db
                .prepare(concat!("SELECT ", $column, " FROM ", $table))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(ids, vec!["new-id"]);
        }
    };
}

legacy_map_key!(legacy_peer_key, peers, "p", peer_id, "peers", "peer_id");
legacy_map_key!(
    legacy_ask_key,
    asks,
    "a",
    correlation_id,
    "asks",
    "correlation_id"
);
legacy_map_key!(legacy_job_key, jobs, "j", job_id, "jobs", "job_id");
legacy_map_key!(
    legacy_schedule_key,
    schedules,
    "s",
    schedule_id,
    "schedules",
    "schedule_id"
);

#[test]
fn removing_inbox_middle_writes_one_row() {
    let (mut db, mut disk) = fixture();
    disk.inbox
        .get_mut("p")
        .unwrap()
        .push(QueuedRecord::new(json!({"id":"third"}), 102));
    write_snapshot(&mut db, &disk).unwrap();
    disk.inbox.get_mut("p").unwrap().remove(1);
    let before = db.total_changes();
    write_snapshot(&mut db, &disk).unwrap();
    assert_eq!(db.total_changes() - before, 1);
    let stored = read_snapshot(&db).unwrap();
    assert_eq!(stored.inbox["p"][0].event()["id"], "first");
    assert_eq!(stored.inbox["p"][1].event()["id"], "third");
    assert_eq!(stored.inbox["p"][1].queued_at(), 102);
}

#[test]
fn inbox_reordering_and_replacement_preserve_records() {
    for ids in [
        vec![2, 1, 0],
        vec![0, 3, 2],
        vec![0, 0, 2],
        vec![3, 0, 2],
        vec![],
        vec![1, 2, 3],
    ] {
        let (mut db, mut disk) = fixture();
        let records = |ids: &[u64]| {
            ids.iter()
                .map(|id| QueuedRecord::new(json!({"id": id.to_string()}), 100 + id))
                .collect()
        };
        disk.inbox.insert("p".into(), records(&[0, 1, 2]));
        write_snapshot(&mut db, &disk).unwrap();
        disk.inbox.insert("p".into(), records(&ids));
        write_snapshot(&mut db, &disk).unwrap();
        let stored = read_snapshot(&db).unwrap();
        let actual: Vec<_> = stored
            .inbox
            .get("p")
            .into_iter()
            .flatten()
            .map(|record| {
                (
                    record.event()["id"].as_str().unwrap().to_string(),
                    record.queued_at(),
                )
            })
            .collect();
        assert_eq!(
            actual,
            ids.iter()
                .map(|id| (id.to_string(), 100 + id))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn inbox_transfer_preserves_order_and_earliest_age() {
    let (mut db, mut disk) = fixture();
    let mut target = disk.peers["p"].clone();
    target.peer_id = "target".into();
    disk.peers.insert("target".into(), target);
    disk.inbox.insert(
        "target".into(),
        vec![QueuedRecord::new(json!({"id":"first"}), 200)],
    );
    write_snapshot(&mut db, &disk).unwrap();
    let mut inbox = Inbox::from_map(disk.inbox);
    inbox.transfer_owed("p", "target");
    disk.inbox = inbox.to_map();
    write_snapshot(&mut db, &disk).unwrap();
    let stored = read_snapshot(&db).unwrap();
    assert!(!stored.inbox.contains_key("p"));
    assert_eq!(stored.inbox["target"].len(), 2);
    assert_eq!(stored.inbox["target"][0].event()["id"], "first");
    assert_eq!(stored.inbox["target"][0].queued_at(), 100);
    assert_eq!(stored.inbox["target"][1].event()["id"], "second");
    assert_eq!(stored.inbox["target"][1].queued_at(), 101);
}

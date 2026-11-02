use super::*;

fn pragma(hub: &Hub, name: &str) -> i64 {
    hub.db
        .pragma_query_value(None, name, |row| row.get(0))
        .unwrap()
}

#[test]
fn the_state_file_gives_back_what_a_sweep_deletes() {
    let path = temp_state("vacuum");
    let mut hub = Hub::open(&path).unwrap();
    let ended = now_unix() - JOB_KEEP_SECS - 60;
    for n in 0..200 {
        let id = format!("job-{n}");
        let job: Job = serde_json::from_value(json!({"job_id": id, "title": "t",
            "prompt": format!("gone-{n} {}", "x".repeat(4000)), "path": "/tmp",
            "backend": "pi", "state": "done", "finished_at": ended}))
        .unwrap();
        hub.jobs.insert(id, job);
    }
    persist(&mut hub).unwrap();
    hub.db
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let full = pragma(&hub, "page_count");
    let size = fs::metadata(&path).unwrap().len();
    advance_jobs(&mut hub);
    assert!(hub.jobs.is_empty(), "the sweep deleted the ended jobs");
    let left = pragma(&hub, "page_count");
    assert!(
        pragma(&hub, "freelist_count") == 0 && left * 10 < full,
        "{left} of {full} pages kept after the sweep"
    );
    let now = fs::metadata(&path).unwrap().len();
    assert!(
        now * 10 < size,
        "the file itself shrinks: {now} of {size} bytes"
    );
    let text = fs::read(&path).unwrap();
    assert!(
        !text.windows(8).any(|window| window == b"gone-17 "),
        "deleted rows are no longer readable in the file"
    );
    assert_eq!(
        pragma(&hub, "auto_vacuum"),
        0,
        "no page map to slow every write"
    );
    assert_eq!(pragma(&hub, "journal_size_limit"), WAL_LIMIT);
}

#[test]
fn a_file_left_in_auto_vacuum_goes_back_to_plain_pages() {
    let path = temp_state("full");
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA auto_vacuum=FULL; PRAGMA journal_mode=WAL; CREATE TABLE t(x);")
            .unwrap();
    }
    let hub = Hub::open(&path).unwrap();
    assert_eq!(pragma(&hub, "auto_vacuum"), 0);
}

#[test]
fn an_older_state_file_is_compacted_once() {
    let path = temp_state("compact");
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE junk(body TEXT);")
            .unwrap();
        for n in 0..200 {
            let row = format!("gone-{n} {}", "x".repeat(4000));
            db.execute("INSERT INTO junk VALUES (?1)", [row]).unwrap();
        }
        db.execute("DELETE FROM junk", []).unwrap();
    }
    let before = fs::metadata(&path).unwrap().len();
    let hub = Hub::open(&path).unwrap();
    let after = fs::metadata(&path).unwrap().len();
    assert!(after * 10 < before, "{after} of {before} bytes");
    let wal = fs::metadata(path.with_extension("db-wal")).map_or(0, |meta| meta.len());
    assert_eq!(wal, 0, "the log is cut back after the compaction");
    let text = fs::read(&path).unwrap();
    assert!(
        !text.windows(8).any(|window| window == b"gone-17 "),
        "deleted rows are no longer readable in the file"
    );
    assert_eq!(pragma(&hub, "auto_vacuum"), 0, "compacted, not switched");
    drop(hub);
    /* compacting again would need the write lock another connection now holds */
    let other = Connection::open(&path).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let again = open_db(&path, now_unix());
    let took = started.elapsed();
    other.execute_batch("ROLLBACK").unwrap();
    assert!(
        again.is_ok() && took < Duration::from_secs(2),
        "reopened after {took:?}: {:?}",
        again.err()
    );
}

#[test]
fn a_compacted_state_file_opens_beside_another_writer() {
    let path = temp_state("writer");
    drop(Hub::open(&path).unwrap());
    let other = Connection::open(&path).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let opened = open_db(&path, now_unix());
    let took = started.elapsed();
    other.execute_batch("ROLLBACK").unwrap();
    assert!(
        opened.is_ok() && took < Duration::from_secs(2),
        "{:?} after {took:?}",
        opened.err()
    );
}

#[test]
fn a_log_a_reader_still_holds_is_reported_not_cut() {
    let path = temp_state("reader");
    let db = open_db(&path, now_unix()).unwrap();
    db.busy_timeout(Duration::from_millis(50)).unwrap();
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM peers;")
        .unwrap();
    db.execute_batch("CREATE TABLE later(x); INSERT INTO later VALUES (1);")
        .unwrap();
    let held = cut_back_log(&db);
    assert!(
        held.as_ref().is_err_and(|error| error.contains("in use")),
        "{held:?}"
    );
    reader.execute_batch("ROLLBACK").unwrap();
    assert_eq!(cut_back_log(&db), Ok(()));
    let wal = fs::metadata(path.with_extension("db-wal")).map_or(0, |meta| meta.len());
    assert_eq!(wal, 0, "released, the log is cut back");
}

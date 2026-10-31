use super::super::Peer;
use super::{call, purge, SCHEMA};
use axum::http::StatusCode;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 86_400;

fn store() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db
}

fn peer(id: &str, circle: &str) -> Peer {
    Peer {
        peer_id: id.into(),
        name: id.into(),
        path: "/repo".into(),
        backend: "codex".into(),
        circle: circle.into(),
        status: "online".into(),
        description: String::new(),
        session_id: format!("{id}-session"),
        last_seen: NOW,
    }
}

fn ok(db: &mut Connection, action: &str, args: Value, caller: &Peer, now: u64) -> Value {
    call(db, action, args, caller, now)
        .unwrap_or_else(|(status, body)| panic!("{action} failed with {status}: {}", body.0))
}

fn err(db: &mut Connection, action: &str, args: Value, caller: &Peer, now: u64) -> StatusCode {
    match call(db, action, args, caller, now) {
        Ok(value) => panic!("{action} succeeded: {value}"),
        Err((status, _)) => status,
    }
}

fn entry(title: &str) -> Value {
    json!({
        "title": title,
        "body": format!("{title} body"),
        "applies_to": "src/hub",
        "evidence": "cargo test exit 0",
    })
}

fn with(mut args: Value, key: &str, value: Value) -> Value {
    args[key] = value;
    args
}

fn publish(db: &mut Connection, caller: &Peer, now: u64, args: Value) -> Value {
    ok(db, "publish", args, caller, now)["knowledge"].clone()
}

fn get(db: &mut Connection, caller: &Peer, now: u64, id: &Value) -> Value {
    ok(db, "get", json!({"id": id}), caller, now)["knowledge"].clone()
}

fn search(db: &mut Connection, caller: &Peer, now: u64, args: Value) -> Vec<Value> {
    ok(db, "search", args, caller, now)["items"]
        .as_array()
        .unwrap()
        .clone()
}

fn ids(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect()
}

fn rows(db: &Connection) -> i64 {
    db.query_row("SELECT count(*) FROM knowledge", [], |r| r.get(0))
        .unwrap()
}

type Revocation<'a> = (
    Option<i64>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

const UNREVOKED: Revocation<'static> = (None, None, None, None);

fn insert_raw(
    db: &Connection,
    created: i64,
    expires: i64,
    revoked: Revocation,
) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT INTO knowledge VALUES ('kn-raw','c','t','b','a','e','p','s',?1,?2,?3,?4,?5,?6)",
        params![created, expires, revoked.0, revoked.1, revoked.2, revoked.3],
    )
}

#[test]
fn schema_has_the_fourteen_contract_columns() {
    let db = store();
    let columns: Vec<(String, String, bool, bool)> = db
        .prepare(
            "SELECT name, type, \"notnull\", pk FROM pragma_table_info('knowledge') ORDER BY cid",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, i64>(3)? != 0,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected: Vec<(String, String, bool, bool)> = [
        ("id", "TEXT", true, true),
        ("circle", "TEXT", true, false),
        ("title", "TEXT", true, false),
        ("body", "TEXT", true, false),
        ("applies_to", "TEXT", true, false),
        ("evidence", "TEXT", true, false),
        ("publisher_peer", "TEXT", true, false),
        ("publisher_session", "TEXT", true, false),
        ("created_at", "INTEGER", true, false),
        ("expires_at", "INTEGER", true, false),
        ("revoked_at", "INTEGER", false, false),
        ("revoke_reason", "TEXT", false, false),
        ("revoked_by_peer", "TEXT", false, false),
        ("revoked_by_session", "TEXT", false, false),
    ]
    .iter()
    .map(|(name, kind, not_null, pk)| (name.to_string(), kind.to_string(), *not_null, *pk))
    .collect();
    assert_eq!(columns, expected);
}

#[test]
fn schema_indexes_only_circle_then_expiry() {
    let db = store();
    let created: Vec<String> = db
        .prepare("SELECT name FROM pragma_index_list('knowledge') WHERE origin = 'c'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(created.len(), 1, "{created:?}");
    let columns: Vec<String> = db
        .prepare(&format!(
            "SELECT name FROM pragma_index_info('{}') ORDER BY seqno",
            created[0]
        ))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(columns, ["circle", "expires_at"]);
}

#[test]
fn schema_requires_expiry_after_creation() {
    let db = store();
    assert!(insert_raw(&db, 10, 10, UNREVOKED).is_err());
    insert_raw(&db, 10, 11, UNREVOKED).unwrap();
}

#[test]
fn schema_requires_all_revocation_fields_together() {
    let db = store();
    for partial in [
        (Some(10), None, Some("p"), Some("s")),
        (Some(10), Some("r"), None, Some("s")),
        (Some(10), Some("r"), Some("p"), None),
        (None, Some("r"), Some("p"), Some("s")),
    ] {
        assert!(insert_raw(&db, 10, 20, partial).is_err(), "{partial:?}");
    }
    insert_raw(&db, 10, 20, (Some(10), Some("r"), Some("p"), Some("s"))).unwrap();
}

#[test]
fn schema_rejects_revocation_before_creation() {
    let db = store();
    assert!(insert_raw(&db, 10, 20, (Some(9), Some("r"), Some("p"), Some("s"))).is_err());
    insert_raw(&db, 10, 20, (Some(10), Some("r"), Some("p"), Some("s"))).unwrap();
}

#[test]
fn publish_stamps_the_callers_identity_time_and_default_ttl() {
    let mut db = store();
    let author = peer("p1", "c1");
    let knowledge = publish(&mut db, &author, NOW, entry("timeout"));
    assert!(knowledge["id"].as_str().unwrap().starts_with("kn-"));
    assert_eq!(knowledge["circle"], "c1");
    assert_eq!(knowledge["publisher_peer"], "p1");
    assert_eq!(knowledge["publisher_session"], "p1-session");
    assert_eq!(knowledge["created_at"], NOW);
    assert_eq!(knowledge["expires_at"], NOW + 7 * DAY);
    assert_eq!(knowledge["status"], "active");
    for field in [
        "revoked_at",
        "revoke_reason",
        "revoked_by_peer",
        "revoked_by_session",
    ] {
        assert!(knowledge[field].is_null(), "{field}");
    }
    assert_eq!(knowledge.as_object().unwrap().len(), 15);
    assert_ne!(
        publish(&mut db, &author, NOW, entry("timeout"))["id"],
        knowledge["id"]
    );
}

#[test]
fn publish_rejects_authorship_time_or_id_from_the_caller() {
    let mut db = store();
    let author = peer("p1", "c1");
    for (key, value) in [
        ("id", json!("kn-mine")),
        ("publisher_peer", json!("p9")),
        ("publisher_session", json!("s9")),
        ("created_at", json!(1)),
        ("expires_at", json!(2)),
        ("status", json!("active")),
    ] {
        let status = err(
            &mut db,
            "publish",
            with(entry("t"), key, value),
            &author,
            NOW,
        );
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}");
    }
    assert_eq!(rows(&db), 0);
    assert_eq!(
        publish(&mut db, &author, NOW, entry("t"))["publisher_peer"],
        "p1"
    );
}

#[test]
fn publish_trims_content_and_rejects_blank_fields() {
    let mut db = store();
    let author = peer("p1", "c1");
    let knowledge = publish(
        &mut db,
        &author,
        NOW,
        with(entry("t"), "title", json!("  padded  ")),
    );
    assert_eq!(knowledge["title"], "padded");
    for field in ["title", "body", "applies_to", "evidence"] {
        let status = err(
            &mut db,
            "publish",
            with(entry("t"), field, json!(" \n\t ")),
            &author,
            NOW,
        );
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}");
    }
}

#[test]
fn publish_limits_fields_by_utf8_bytes() {
    let mut db = store();
    let author = peer("p1", "c1");
    for (field, max) in [
        ("title", 200),
        ("body", 8192),
        ("applies_to", 2048),
        ("evidence", 2048),
    ] {
        publish(
            &mut db,
            &author,
            NOW,
            with(entry("t"), field, json!("a".repeat(max))),
        );
        let long = err(
            &mut db,
            "publish",
            with(entry("t"), field, json!("a".repeat(max + 1))),
            &author,
            NOW,
        );
        assert_eq!(long, StatusCode::BAD_REQUEST, "{field}");
        /* three bytes a character: under the limit by count, over it by bytes */
        let wide = err(
            &mut db,
            "publish",
            with(entry("t"), field, json!("超".repeat(max / 3 + 1))),
            &author,
            NOW,
        );
        assert_eq!(wide, StatusCode::BAD_REQUEST, "{field} in bytes");
    }
}

#[test]
fn publish_bounds_ttl_to_whole_seconds_up_to_thirty_days() {
    let mut db = store();
    let author = peer("p1", "c1");
    let shortest = publish(
        &mut db,
        &author,
        NOW,
        with(entry("t"), "ttl_seconds", json!(1)),
    );
    assert_eq!(shortest["expires_at"], NOW + 1);
    let longest = publish(
        &mut db,
        &author,
        NOW,
        with(entry("t"), "ttl_seconds", json!(30 * DAY)),
    );
    assert_eq!(longest["expires_at"], NOW + 30 * DAY);
    for ttl in [
        json!(0),
        json!(30 * DAY + 1),
        json!(-5),
        json!(1.5),
        json!("60"),
    ] {
        let status = err(
            &mut db,
            "publish",
            with(entry("t"), "ttl_seconds", ttl.clone()),
            &author,
            NOW,
        );
        assert_eq!(status, StatusCode::BAD_REQUEST, "{ttl}");
    }
}

#[test]
fn search_requires_every_term_and_stays_in_the_callers_circle() {
    let mut db = store();
    let (mine, other) = (peer("p1", "c1"), peer("p2", "c2"));
    let both = publish(
        &mut db,
        &mine,
        NOW,
        with(entry("cargo 超时"), "applies_to", json!("E0502 src/hub")),
    );
    publish(&mut db, &mine, NOW, entry("超时 only"));
    publish(
        &mut db,
        &other,
        NOW,
        with(entry("cargo 超时"), "applies_to", json!("E0502 src/hub")),
    );
    let found = search(&mut db, &mine, NOW, json!({"query": " 超时  E0502 "}));
    assert_eq!(ids(&found), [both["id"].as_str().unwrap()]);
}

#[test]
fn search_leaves_evidence_out_of_the_match() {
    let mut db = store();
    let author = peer("p1", "c1");
    publish(
        &mut db,
        &author,
        NOW,
        with(entry("t"), "evidence", json!("exit 101 E0599")),
    );
    assert!(search(&mut db, &author, NOW, json!({"query": "E0599"})).is_empty());
}

#[test]
fn search_hides_expired_and_revoked_entries() {
    let mut db = store();
    let author = peer("p1", "c1");
    let short = publish(
        &mut db,
        &author,
        NOW,
        with(entry("disk full"), "ttl_seconds", json!(10)),
    );
    let revoked = publish(&mut db, &author, NOW, entry("disk full again"));
    ok(
        &mut db,
        "revoke",
        json!({"id": revoked["id"], "reason": "wrong"}),
        &author,
        NOW,
    );
    let before = search(&mut db, &author, NOW + 9, json!({"query": "disk"}));
    assert_eq!(ids(&before), [short["id"].as_str().unwrap()]);
    assert!(search(&mut db, &author, NOW + 10, json!({"query": "disk"})).is_empty());
}

#[test]
fn search_matches_like_wildcards_literally() {
    let mut db = store();
    let author = peer("p1", "c1");
    let underscore = publish(
        &mut db,
        &author,
        NOW,
        with(
            entry("path"),
            "applies_to",
            json!("src/hub/job_dag_tests.rs"),
        ),
    );
    publish(
        &mut db,
        &author,
        NOW,
        with(entry("path"), "applies_to", json!("src/hub/jobXdag")),
    );
    let percent = publish(
        &mut db,
        &author,
        NOW,
        with(entry("rate"), "body", json!("cpu at 100% load")),
    );
    publish(
        &mut db,
        &author,
        NOW,
        with(entry("rate"), "body", json!("100 requests")),
    );
    let backslash = publish(
        &mut db,
        &author,
        NOW,
        with(entry("win"), "body", json!(r"C:\temp\x")),
    );
    for (query, expected) in [
        ("job_dag", &underscore),
        ("100%", &percent),
        (r"C:\temp", &backslash),
    ] {
        let found = search(&mut db, &author, NOW, json!({"query": query}));
        assert_eq!(ids(&found), [expected["id"].as_str().unwrap()], "{query}");
    }
}

#[test]
fn search_orders_newest_first_then_by_id() {
    let mut db = store();
    let author = peer("p1", "c1");
    let oldest = publish(&mut db, &author, NOW, entry("lock"));
    let mut expected: Vec<String> = (0..2)
        .map(|_| {
            publish(&mut db, &author, NOW + 1, entry("lock"))["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    expected.sort();
    expected.push(oldest["id"].as_str().unwrap().to_string());
    assert_eq!(
        ids(&search(&mut db, &author, NOW + 2, json!({"query": "lock"}))),
        expected
    );
}

#[test]
fn search_returns_five_by_default_and_at_most_five() {
    let mut db = store();
    let author = peer("p1", "c1");
    for _ in 0..7 {
        publish(&mut db, &author, NOW, entry("flaky"));
    }
    assert_eq!(
        search(&mut db, &author, NOW, json!({"query": "flaky"})).len(),
        5
    );
    assert_eq!(
        search(&mut db, &author, NOW, json!({"query": "flaky", "limit": 2})).len(),
        2
    );
    for limit in [json!(0), json!(6), json!(1.5), json!(-1)] {
        let status = err(
            &mut db,
            "search",
            json!({"query": "flaky", "limit": limit.clone()}),
            &author,
            NOW,
        );
        assert_eq!(status, StatusCode::BAD_REQUEST, "{limit}");
    }
}

#[test]
fn search_validates_the_query() {
    let mut db = store();
    let author = peer("p1", "c1");
    publish(&mut db, &author, NOW, entry("a b c d e f g h"));
    assert_eq!(
        search(&mut db, &author, NOW, json!({"query": "a b c d e f g h"})).len(),
        1
    );
    let widest = format!(" {} ", "a".repeat(256));
    assert!(search(&mut db, &author, NOW, json!({"query": widest})).is_empty());
    let too_long = "a".repeat(257);
    for query in ["  ", "a b c d e f g h i", too_long.as_str()] {
        let status = err(&mut db, "search", json!({"query": query}), &author, NOW);
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
}

#[test]
fn search_items_carry_the_contract_fields_and_a_short_excerpt() {
    let mut db = store();
    let author = peer("p1", "c1");
    publish(
        &mut db,
        &author,
        NOW,
        with(
            entry("long"),
            "body",
            json!(format!("long {}", "超".repeat(400))),
        ),
    );
    let items = search(&mut db, &author, NOW, json!({"query": "long"}));
    let item = items[0].as_object().unwrap();
    let mut keys: Vec<&str> = item.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "applies_to",
            "created_at",
            "excerpt",
            "expires_at",
            "id",
            "publisher_peer",
            "publisher_session",
            "status",
            "title"
        ]
    );
    let excerpt = item["excerpt"].as_str().unwrap();
    assert_eq!(excerpt.chars().count(), 300);
    assert!(excerpt.starts_with("long 超"));
    assert_eq!(item["status"], "active");
}

#[test]
fn get_returns_the_full_row_only_inside_the_circle() {
    let mut db = store();
    let (mine, other) = (peer("p1", "c1"), peer("p2", "c2"));
    let published = publish(&mut db, &mine, NOW, entry("t"));
    assert_eq!(get(&mut db, &mine, NOW + 1, &published["id"]), published);
    assert_eq!(
        err(&mut db, "get", json!({"id": published["id"]}), &other, NOW),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        err(&mut db, "get", json!({"id": "kn-missing"}), &mine, NOW),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn get_reports_expired_then_revoked_status() {
    let mut db = store();
    let author = peer("p1", "c1");
    let id = publish(
        &mut db,
        &author,
        NOW,
        with(entry("t"), "ttl_seconds", json!(10)),
    )["id"]
        .clone();
    assert_eq!(get(&mut db, &author, NOW + 9, &id)["status"], "active");
    assert_eq!(get(&mut db, &author, NOW + 10, &id)["status"], "expired");
    ok(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "stale"}),
        &author,
        NOW + 11,
    );
    assert_eq!(get(&mut db, &author, NOW + 11, &id)["status"], "revoked");
}

#[test]
fn revoke_by_any_circle_peer_records_who_and_why() {
    let mut db = store();
    let (author, colleague) = (peer("p1", "c1"), peer("p2", "c1"));
    let id = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    let reply = ok(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "  superseded upstream "}),
        &colleague,
        NOW + 5,
    );
    assert_eq!(reply, json!({"ok": true, "id": id}));
    let revoked = get(&mut db, &author, NOW + 5, &id);
    assert_eq!(revoked["revoked_at"], NOW + 5);
    assert_eq!(revoked["revoke_reason"], "superseded upstream");
    assert_eq!(revoked["revoked_by_peer"], "p2");
    assert_eq!(revoked["revoked_by_session"], "p2-session");
}

#[test]
fn revoke_after_a_session_change_keeps_the_publishers_session() {
    let mut db = store();
    let before = peer("p1", "c1");
    let id = publish(&mut db, &before, NOW, entry("t"))["id"].clone();
    let restarted = Peer {
        session_id: "p1-restarted".into(),
        ..peer("p1", "c1")
    };
    ok(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "stale"}),
        &restarted,
        NOW + 1,
    );
    let revoked = get(&mut db, &restarted, NOW + 1, &id);
    assert_eq!(revoked["publisher_session"], "p1-session");
    assert_eq!(revoked["revoked_by_session"], "p1-restarted");
}

#[test]
fn revoke_twice_conflicts_and_keeps_the_first_audit() {
    let mut db = store();
    let (author, late) = (peer("p1", "c1"), peer("p3", "c1"));
    let id = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    ok(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "first"}),
        &author,
        NOW + 1,
    );
    let again = err(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "second"}),
        &late,
        NOW + 2,
    );
    assert_eq!(again, StatusCode::CONFLICT);
    let revoked = get(&mut db, &author, NOW + 2, &id);
    assert_eq!(revoked["revoke_reason"], "first");
    assert_eq!(revoked["revoked_by_peer"], "p1");
    assert_eq!(revoked["revoked_at"], NOW + 1);
}

#[test]
fn revoke_outside_the_circle_or_of_a_missing_id_is_not_found() {
    let mut db = store();
    let (author, outsider) = (peer("p1", "c1"), peer("p9", "c2"));
    let id = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    let foreign = err(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "x"}),
        &outsider,
        NOW,
    );
    assert_eq!(foreign, StatusCode::NOT_FOUND);
    let missing = err(
        &mut db,
        "revoke",
        json!({"id": "kn-missing", "reason": "x"}),
        &author,
        NOW,
    );
    assert_eq!(missing, StatusCode::NOT_FOUND);
    assert_eq!(get(&mut db, &author, NOW, &id)["status"], "active");
}

#[test]
fn revoke_requires_a_reason_within_1024_bytes() {
    let mut db = store();
    let author = peer("p1", "c1");
    let id = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    for reason in [" ".to_string(), "a".repeat(1025), "超".repeat(342)] {
        let status = err(
            &mut db,
            "revoke",
            json!({"id": id, "reason": reason}),
            &author,
            NOW,
        );
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    ok(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "a".repeat(1024)}),
        &author,
        NOW,
    );
}

#[test]
fn revoke_survives_a_clock_that_moved_back() {
    let mut db = store();
    let author = peer("p1", "c1");
    let id = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    ok(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "clock"}),
        &author,
        NOW - 60,
    );
    assert_eq!(get(&mut db, &author, NOW, &id)["revoked_at"], NOW);
}

#[test]
fn publish_replaces_revokes_the_old_entry_in_the_same_write() {
    let mut db = store();
    let (author, editor) = (peer("p1", "c1"), peer("p2", "c1"));
    let old = publish(&mut db, &author, NOW, entry("retry"))["id"].clone();
    let new = publish(
        &mut db,
        &editor,
        NOW + 1,
        with(entry("retry"), "replaces", old.clone()),
    );
    assert_eq!(new["status"], "active");
    let replaced = get(&mut db, &author, NOW + 1, &old);
    assert_eq!(replaced["status"], "revoked");
    assert_eq!(replaced["revoke_reason"], "replaced");
    assert_eq!(replaced["revoked_by_peer"], "p2");
    assert_eq!(replaced["revoked_by_session"], "p2-session");
    assert_eq!(replaced["revoked_at"], NOW + 1);
    let found = search(&mut db, &author, NOW + 1, json!({"query": "retry"}));
    assert_eq!(ids(&found), [new["id"].as_str().unwrap()]);
}

#[test]
fn publish_replaces_an_expired_entry() {
    let mut db = store();
    let author = peer("p1", "c1");
    let old = publish(
        &mut db,
        &author,
        NOW,
        with(entry("t"), "ttl_seconds", json!(1)),
    )["id"]
        .clone();
    publish(
        &mut db,
        &author,
        NOW + 5,
        with(entry("t"), "replaces", old.clone()),
    );
    assert_eq!(get(&mut db, &author, NOW + 5, &old)["status"], "revoked");
}

#[test]
fn publish_replaces_refuses_revoked_missing_or_foreign_entries() {
    let mut db = store();
    let (author, outsider) = (peer("p1", "c1"), peer("p9", "c2"));
    let revoked = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    ok(
        &mut db,
        "revoke",
        json!({"id": revoked, "reason": "x"}),
        &author,
        NOW,
    );
    let foreign = publish(&mut db, &outsider, NOW, entry("t"))["id"].clone();
    let before = rows(&db);
    for (replaces, expected) in [
        (revoked, StatusCode::CONFLICT),
        (json!("kn-missing"), StatusCode::NOT_FOUND),
        (foreign, StatusCode::NOT_FOUND),
    ] {
        let status = err(
            &mut db,
            "publish",
            with(entry("t"), "replaces", replaces.clone()),
            &author,
            NOW,
        );
        assert_eq!(status, expected, "{replaces}");
    }
    assert_eq!(rows(&db), before);
}

#[test]
fn publish_replaces_rolls_back_the_new_entry_when_revoking_fails() {
    let mut db = store();
    let author = peer("p1", "c1");
    let old = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    db.execute_batch(
        "CREATE TEMP TRIGGER fail_revoke BEFORE UPDATE ON knowledge BEGIN SELECT RAISE(ABORT, 'injected revoke failure'); END;",
    )
    .unwrap();
    let status = err(
        &mut db,
        "publish",
        with(entry("t"), "replaces", old.clone()),
        &author,
        NOW + 1,
    );
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(db.is_autocommit());
    db.execute_batch("DROP TRIGGER fail_revoke;").unwrap();
    assert_eq!(rows(&db), 1);
    assert_eq!(get(&mut db, &author, NOW + 1, &old)["status"], "active");
}

#[test]
fn purge_deletes_entries_expired_or_revoked_more_than_seven_days_ago() {
    let mut db = store();
    let author = peer("p1", "c1");
    let ttl = |seconds: u64| with(entry("t"), "ttl_seconds", json!(seconds));
    let mut kept = vec![
        publish(&mut db, &author, NOW, entry("t"))["id"].clone(),
        publish(&mut db, &author, NOW - 7 * DAY - 10, ttl(10))["id"].clone(),
    ];
    publish(&mut db, &author, NOW - 7 * DAY - 11, ttl(10));
    let recent = publish(&mut db, &author, NOW - DAY, entry("t"))["id"].clone();
    ok(
        &mut db,
        "revoke",
        json!({"id": recent, "reason": "x"}),
        &author,
        NOW - DAY,
    );
    kept.push(recent);
    let old = publish(&mut db, &author, NOW - 8 * DAY, ttl(30 * DAY))["id"].clone();
    ok(
        &mut db,
        "revoke",
        json!({"id": old, "reason": "x"}),
        &author,
        NOW - 7 * DAY - 1,
    );
    assert_eq!(purge(&db, NOW).unwrap(), 2);
    let mut left: Vec<String> = db
        .prepare("SELECT id FROM knowledge")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    left.sort();
    let mut kept: Vec<String> = kept
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    kept.sort();
    assert_eq!(left, kept);
}

#[test]
fn unknown_actions_are_not_found() {
    let mut db = store();
    assert_eq!(
        err(&mut db, "renew", json!({}), &peer("p1", "c1"), NOW),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn every_action_rejects_unknown_fields() {
    let mut db = store();
    let author = peer("p1", "c1");
    let id = publish(&mut db, &author, NOW, entry("t"))["id"].clone();
    for (action, args) in [
        ("publish", with(entry("t"), "verified", json!(true))),
        ("search", json!({"query": "t", "circle": "c2"})),
        ("get", json!({"id": id, "include_revoked": true})),
        (
            "revoke",
            json!({"id": id, "reason": "x", "revoked_by_peer": "p9"}),
        ),
        ("get", json!(["kn-x"])),
    ] {
        assert_eq!(
            err(&mut db, action, args, &author, NOW),
            StatusCode::BAD_REQUEST,
            "{action}"
        );
    }
    assert_eq!(rows(&db), 1);
    assert_eq!(get(&mut db, &author, NOW, &id)["status"], "active");
}

/* a procedure's indentation is content: the body is stored as written, and only the
blank check looks at the trimmed value */
#[test]
fn publish_keeps_leading_indentation_in_the_body() {
    let mut db = store();
    let author = peer("p1", "c1");
    let body = "    cargo test --all\n    cargo clippy --all-targets";
    let knowledge = publish(
        &mut db,
        &author,
        NOW,
        with(entry("indent"), "body", json!(body)),
    );
    assert_eq!(knowledge["body"], body);
    let stored = get(&mut db, &author, NOW, &knowledge["id"]);
    assert_eq!(
        stored["body"], body,
        "get returns the bytes that were published"
    );
}

#[test]
fn publish_keeps_a_trailing_newline_in_the_body() {
    let mut db = store();
    let author = peer("p1", "c1");
    let body = "run the whole suite\n";
    let knowledge = publish(
        &mut db,
        &author,
        NOW,
        with(entry("tail"), "body", json!(body)),
    );
    assert_eq!(knowledge["body"], body);
}

#[test]
fn publish_rejects_a_whitespace_only_body() {
    let mut db = store();
    let author = peer("p1", "c1");
    let status = err(
        &mut db,
        "publish",
        with(entry("blank"), "body", json!("  \n\t \n")),
        &author,
        NOW,
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rows(&db), 0, "a rejected publish writes nothing");
}

#[test]
fn publish_accepts_a_body_at_the_raw_byte_limit() {
    let mut db = store();
    let author = peer("p1", "c1");
    let body = "b".repeat(8192);
    let knowledge = publish(
        &mut db,
        &author,
        NOW,
        with(entry("limit"), "body", json!(body)),
    );
    assert_eq!(knowledge["body"].as_str().unwrap().len(), 8192);
}

/* the limit counts the bytes stored, so trailing whitespace buys no extra room */
#[test]
fn publish_rejects_a_body_over_the_raw_limit_that_trims_under_it() {
    let mut db = store();
    let author = peer("p1", "c1");
    let body = format!("{}\n\n", "b".repeat(8191));
    assert_eq!(body.trim().len(), 8191);
    assert_eq!(body.len(), 8193);
    let status = err(
        &mut db,
        "publish",
        with(entry("over"), "body", json!(body)),
        &author,
        NOW,
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rows(&db), 0, "a rejected publish writes nothing");
}

/* a row that expired long before it was revoked keeps seven days from the stored revocation,
not from its expiry */
#[test]
fn purge_keeps_a_late_revocation_for_seven_days() {
    let mut db = store();
    let author = peer("p1", "c1");
    let id = publish(
        &mut db,
        &author,
        NOW - 10 * DAY,
        with(entry("late"), "ttl_seconds", json!(DAY)),
    )["id"]
        .clone();
    ok(
        &mut db,
        "revoke",
        json!({"id": id, "reason": "stale"}),
        &author,
        NOW - DAY,
    );

    assert_eq!(
        purge(&db, NOW).unwrap(),
        0,
        "a fresh revocation must outlive an old expiry"
    );
    /* exactly seven days after the revocation the row is still there, one second later it goes */
    assert_eq!(purge(&db, NOW + 6 * DAY).unwrap(), 0);
    assert_eq!(purge(&db, NOW + 6 * DAY + 1).unwrap(), 1);
}

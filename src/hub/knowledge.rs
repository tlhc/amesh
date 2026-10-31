use axum::http::StatusCode;
use axum::Json;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::Peer;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS knowledge (
  id TEXT PRIMARY KEY NOT NULL,
  circle TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  applies_to TEXT NOT NULL,
  evidence TEXT NOT NULL,
  publisher_peer TEXT NOT NULL,
  publisher_session TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  revoked_at INTEGER,
  revoke_reason TEXT,
  revoked_by_peer TEXT,
  revoked_by_session TEXT,
  CHECK (expires_at > created_at),
  CHECK ((revoked_at IS NULL) = (revoke_reason IS NULL)
    AND (revoked_at IS NULL) = (revoked_by_peer IS NULL)
    AND (revoked_at IS NULL) = (revoked_by_session IS NULL)),
  CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);
CREATE INDEX IF NOT EXISTS knowledge_circle_expires ON knowledge(circle, expires_at);
";

const COLUMNS: &str = "id,circle,title,body,applies_to,evidence,publisher_peer,publisher_session,created_at,expires_at,revoked_at,revoke_reason,revoked_by_peer,revoked_by_session";
const TTL_DEFAULT: u64 = 7 * 86400;
const TTL_MAX: u64 = 30 * 86400;
/* how long an expired or revoked entry stays readable by id before purge deletes it */
const KEEP_SECS: u64 = 7 * 86400;
const SEARCH_LIMIT: u64 = 5;
const QUERY_BYTES: usize = 256;
const QUERY_TERMS: usize = 8;
const EXCERPT_CHARS: usize = 300;

type Failure = (StatusCode, Json<Value>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishArgs {
    title: String,
    body: String,
    applies_to: String,
    evidence: String,
    ttl_seconds: Option<u64>,
    replaces: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    limit: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeArgs {
    id: String,
    reason: String,
}

pub(super) fn call(
    db: &mut Connection,
    action: &str,
    args: Value,
    peer: &Peer,
    now: u64,
) -> Result<Value, Failure> {
    match action {
        "publish" => publish(db, parse(args)?, peer, now),
        "search" => search(db, parse(args)?, peer, now),
        "get" => {
            let args: GetArgs = parse(args)?;
            Ok(json!({"knowledge": load(db, &args.id, &peer.circle, now)?}))
        }
        "revoke" => {
            let args: RevokeArgs = parse(args)?;
            let reason = text("reason", &args.reason, 1024)?;
            mark_revoked(db, &args.id, &reason, peer, now)?;
            Ok(json!({"ok": true, "id": args.id}))
        }
        _ => Err(fail(StatusCode::NOT_FOUND, "unknown knowledge action")),
    }
}

/* an unrevoked row goes a week after its expiry; a revoked one goes a week after the stored
revocation, even when that came long after the expiry */
pub(super) fn purge(db: &Connection, now: u64) -> Result<usize, String> {
    db.execute(
        "DELETE FROM knowledge WHERE (revoked_at IS NULL AND expires_at < ?1) OR revoked_at < ?1",
        [secs(now.saturating_sub(KEEP_SECS))],
    )
    .map_err(|e| e.to_string())
}

/* the new row goes in first: when the entry it replaces is missing, foreign or already revoked,
the error drops the transaction and takes the insert with it */
fn publish(
    db: &mut Connection,
    args: PublishArgs,
    peer: &Peer,
    now: u64,
) -> Result<Value, Failure> {
    let title = text("title", &args.title, 200)?;
    let body = body_text(&args.body, 8192)?;
    let applies_to = text("applies_to", &args.applies_to, 2048)?;
    let evidence = text("evidence", &args.evidence, 2048)?;
    let ttl = args.ttl_seconds.unwrap_or(TTL_DEFAULT);
    if ttl == 0 || ttl > TTL_MAX {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            format!("ttl_seconds must be 1 to {TTL_MAX}"),
        ));
    }
    let id = format!("kn-{}", Uuid::new_v4().simple());
    let tx = db.transaction().map_err(internal)?;
    tx.execute(
        "INSERT INTO knowledge(id,circle,title,body,applies_to,evidence,publisher_peer,publisher_session,created_at,expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            id,
            peer.circle,
            title,
            body,
            applies_to,
            evidence,
            peer.peer_id,
            peer.session_id,
            secs(now),
            secs(now.saturating_add(ttl))
        ],
    )
    .map_err(internal)?;
    if let Some(old) = &args.replaces {
        mark_revoked(&tx, old, "replaced", peer, now)?;
    }
    let knowledge = load(&tx, &id, &peer.circle, now)?;
    tx.commit().map_err(internal)?;
    Ok(json!({"ok": true, "knowledge": knowledge}))
}

fn search(db: &Connection, args: SearchArgs, peer: &Peer, now: u64) -> Result<Value, Failure> {
    let query = args.query.trim();
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() || terms.len() > QUERY_TERMS || query.len() > QUERY_BYTES {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            format!("query must be 1 to {QUERY_TERMS} terms within {QUERY_BYTES} UTF-8 bytes"),
        ));
    }
    let limit = args.limit.unwrap_or(SEARCH_LIMIT);
    if !(1..=SEARCH_LIMIT).contains(&limit) {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            format!("limit must be 1 to {SEARCH_LIMIT}"),
        ));
    }
    /* every term is literal: LIKE's wildcards and its escape character are escaped, and the
    fields are joined by a newline no term can hold, so a term never matches across two.
    Evidence stays out: it is only checked for shape, never for truth */
    let mut sql = format!(
        "SELECT {COLUMNS} FROM knowledge WHERE circle = ?1 AND revoked_at IS NULL AND expires_at > ?2"
    );
    let mut values: Vec<rusqlite::types::Value> =
        vec![peer.circle.clone().into(), secs(now).into()];
    for term in terms {
        let pattern = term
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.push(format!("%{pattern}%").into());
        sql.push_str(&format!(
            " AND (title || char(10) || body || char(10) || applies_to) LIKE ?{} ESCAPE '\\'",
            values.len()
        ));
    }
    values.push(secs(limit).into());
    sql.push_str(&format!(
        " ORDER BY created_at DESC, id ASC LIMIT ?{}",
        values.len()
    ));
    let mut statement = db.prepare(&sql).map_err(internal)?;
    let items = statement
        .query_map(params_from_iter(values), |row| {
            let entry = entry(row, now)?;
            let excerpt: String = entry["body"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(EXCERPT_CHARS)
                .collect();
            Ok(json!({
                "id": entry["id"],
                "title": entry["title"],
                "excerpt": excerpt,
                "applies_to": entry["applies_to"],
                "publisher_peer": entry["publisher_peer"],
                "publisher_session": entry["publisher_session"],
                "created_at": entry["created_at"],
                "expires_at": entry["expires_at"],
                "status": entry["status"],
            }))
        })
        .map_err(internal)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(internal)?;
    Ok(json!({"items": items}))
}

/* only an unrevoked row changes, so the first revocation's audit stands; MAX keeps revoked_at
from falling before created_at when the clock moved back */
fn mark_revoked(
    db: &Connection,
    id: &str,
    reason: &str,
    peer: &Peer,
    now: u64,
) -> Result<(), Failure> {
    let changed = db
        .execute(
            "UPDATE knowledge SET revoked_at = MAX(?1, created_at), revoke_reason = ?2, revoked_by_peer = ?3, revoked_by_session = ?4 WHERE id = ?5 AND circle = ?6 AND revoked_at IS NULL",
            params![secs(now), reason, peer.peer_id, peer.session_id, id, peer.circle],
        )
        .map_err(internal)?;
    if changed == 1 {
        return Ok(());
    }
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM knowledge WHERE id = ?1 AND circle = ?2)",
            params![id, peer.circle],
            |r| r.get(0),
        )
        .map_err(internal)?;
    Err(if exists {
        fail(StatusCode::CONFLICT, "knowledge already revoked")
    } else {
        not_found()
    })
}

fn load(db: &Connection, id: &str, circle: &str, now: u64) -> Result<Value, Failure> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM knowledge WHERE id = ?1 AND circle = ?2"),
        params![id, circle],
        |row| entry(row, now),
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(not_found)
}

fn entry(row: &Row, now: u64) -> rusqlite::Result<Value> {
    let expires_at: i64 = row.get(9)?;
    let revoked_at: Option<i64> = row.get(10)?;
    let status = if revoked_at.is_some() {
        "revoked"
    } else if expires_at <= secs(now) {
        "expired"
    } else {
        "active"
    };
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "circle": row.get::<_, String>(1)?,
        "title": row.get::<_, String>(2)?,
        "body": row.get::<_, String>(3)?,
        "applies_to": row.get::<_, String>(4)?,
        "evidence": row.get::<_, String>(5)?,
        "publisher_peer": row.get::<_, String>(6)?,
        "publisher_session": row.get::<_, String>(7)?,
        "created_at": row.get::<_, i64>(8)?,
        "expires_at": expires_at,
        "revoked_at": revoked_at,
        "revoke_reason": row.get::<_, Option<String>>(11)?,
        "revoked_by_peer": row.get::<_, Option<String>>(12)?,
        "revoked_by_session": row.get::<_, Option<String>>(13)?,
        "status": status,
    }))
}

/* a derived struct also deserializes from an array, filling fields by position */
fn parse<T: DeserializeOwned>(args: Value) -> Result<T, Failure> {
    if !args.is_object() {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "knowledge arguments must be an object",
        ));
    }
    serde_json::from_value(args).map_err(|e| fail(StatusCode::BAD_REQUEST, e))
}

/* metadata limits count UTF-8 bytes of the trimmed value, which is the value stored */
fn text(field: &str, value: &str, max: usize) -> Result<String, Failure> {
    let value = value.trim();
    if value.is_empty() || value.len() > max {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            format!("{field} must be 1 to {max} UTF-8 bytes after trimming"),
        ));
    }
    Ok(value.to_string())
}

/* the body is stored as written — indentation and a trailing newline are part of a
procedure — so only the blank check trims, and the limit counts the bytes stored */
fn body_text(value: &str, max: usize) -> Result<String, Failure> {
    if value.trim().is_empty() || value.len() > max {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            format!("body must be 1 to {max} UTF-8 bytes as given"),
        ));
    }
    Ok(value.to_string())
}

fn secs(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn fail(status: StatusCode, error: impl std::fmt::Display) -> Failure {
    (status, Json(json!({"error": error.to_string()})))
}

fn internal(error: rusqlite::Error) -> Failure {
    fail(StatusCode::INTERNAL_SERVER_ERROR, error)
}

fn not_found() -> Failure {
    fail(StatusCode::NOT_FOUND, "unknown knowledge")
}

#[cfg(test)]
#[path = "../../tests/support/knowledge_store.rs"]
mod tests;

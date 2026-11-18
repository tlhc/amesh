use super::{
    check_job_circle, database_error, params, Connection, DiskState, Json, StatusCode, Value,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::json;

pub(super) const KEEP_SECS: u64 = 7 * 86400;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS job_history (
    job_id TEXT PRIMARY KEY,
    circle TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at INTEGER,
    state TEXT NOT NULL,
    finished_at INTEGER,
    request_scope TEXT UNIQUE,
    request_hash TEXT
);
CREATE TABLE IF NOT EXISTS job_attempts (
    ask_id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    session_id TEXT,
    opened_at INTEGER,
    closed_at INTEGER,
    closed_by TEXT,
    failed INTEGER
);
CREATE INDEX IF NOT EXISTS job_attempts_job ON job_attempts(job_id);
CREATE INDEX IF NOT EXISTS job_attempts_open ON job_attempts(ask_id) WHERE failed IS NULL;
";

pub(super) fn backfill(db: &mut Connection, disk: &DiskState) -> rusqlite::Result<()> {
    let missing = db.query_row(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE NOT EXISTS
            (SELECT 1 FROM job_history WHERE job_history.job_id=jobs.job_id))",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if missing {
        let tx = db.transaction()?;
        record(&tx, disk, false)?;
        tx.commit()?;
    }
    Ok(())
}

pub(super) fn record(
    tx: &Transaction<'_>,
    disk: &DiskState,
    capture_sessions: bool,
) -> rusqlite::Result<()> {
    let mut job = tx.prepare_cached(
        "INSERT INTO job_history(job_id,circle,title,created_at,state,finished_at)
        VALUES (?1,?2,?3,?4,?5,?6)
        ON CONFLICT(job_id) DO UPDATE
            SET (circle,title,created_at,state,finished_at) =
                (excluded.circle,excluded.title,excluded.created_at,excluded.state,excluded.finished_at)
        WHERE (circle,title,created_at,state,finished_at) IS NOT
            (excluded.circle,excluded.title,excluded.created_at,excluded.state,excluded.finished_at)",
    )?;
    let mut attempt = tx.prepare_cached(
        "INSERT INTO job_attempts
        (ask_id,job_id,peer_id,session_id,opened_at,closed_at,closed_by,failed)
        VALUES (?1,?2,?3,CASE WHEN EXISTS (SELECT 1 FROM asks WHERE correlation_id=?1)
            THEN NULL ELSE ?4 END,?5,?6,?7,?8) ON CONFLICT(ask_id) DO NOTHING",
    )?;
    for row in disk.jobs.values() {
        job.execute(params![
            row.job_id,
            row.circle,
            row.title,
            row.created_at.map(|ts| ts as i64),
            row.state,
            row.finished_at.map(|ts| ts as i64)
        ])?;
        if let Some(ask) = row.ask_id.as_ref().and_then(|id| disk.asks.get(id)) {
            let session = if capture_sessions {
                disk.peers
                    .get(&ask.to_peer_id)
                    .map(|peer| peer.session_id.as_str())
            } else {
                None
            };
            attempt.execute(params![
                ask.correlation_id,
                row.job_id,
                ask.to_peer_id,
                session.filter(|id| !id.is_empty()),
                ask.opened_at.map(|ts| ts as i64),
                ask.closed_at.map(|ts| ts as i64),
                ask.closed_by,
                (!ask.open).then_some(ask.failed)
            ])?;
        }
    }
    Ok(())
}

pub(super) fn record_closures(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute("UPDATE job_attempts AS attempt
        SET (closed_at,closed_by,failed) = (ask.closed_at,ask.closed_by,ask.failed)
        FROM asks AS ask WHERE attempt.ask_id=ask.correlation_id AND attempt.failed IS NULL AND ask.open=0", [])?;
    Ok(())
}

pub(super) fn purge(db: &Connection, now: u64) -> rusqlite::Result<bool> {
    let tx = db.unchecked_transaction()?;
    let cutoff = now.saturating_sub(KEEP_SECS) as i64;
    tx.execute(
        "DELETE FROM job_attempts WHERE job_id IN (SELECT job_id FROM job_history
            WHERE COALESCE(finished_at, created_at, 0) <= ?1 AND NOT EXISTS
            (SELECT 1 FROM jobs WHERE jobs.job_id=job_history.job_id))",
        [cutoff],
    )?;
    let deleted = tx.execute(
        "DELETE FROM job_history WHERE COALESCE(finished_at, created_at, 0) <= ?1
            AND NOT EXISTS (SELECT 1 FROM jobs WHERE jobs.job_id=job_history.job_id)",
        [cutoff],
    )?;
    tx.commit()?;
    Ok(deleted != 0)
}

pub(super) fn forget(tx: &Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM job_attempts WHERE job_id=?1", [id])?;
    tx.execute("DELETE FROM job_history WHERE job_id=?1", [id])?;
    Ok(())
}

pub(super) fn read(
    db: &Connection,
    id: &str,
    circle: Option<&str>,
) -> Result<Value, (StatusCode, Json<Value>)> {
    const SELECT_JOB: &str = "SELECT title,circle,created_at,state,finished_at FROM job_history
        WHERE job_id=?1";
    let (job_circle, mut history) = db
        .query_row(SELECT_JOB, [id], |row| {
            let job_circle = row.get::<_, String>(1)?;
            let value = json!({
                "job_id": id,
                "title": row.get::<_, String>(0)?,
                "circle": job_circle,
                "created_at": row.get::<_, Option<i64>>(2)?,
                "state": row.get::<_, String>(3)?,
                "finished_at": row.get::<_, Option<i64>>(4)?,
            });
            Ok((job_circle, value))
        })
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "unknown job"}))))?;
    check_job_circle(circle, &job_circle)?;
    let mut attempts = db
        .prepare_cached(
            "SELECT ask_id,peer_id,session_id,opened_at,closed_at,closed_by,failed
        FROM job_attempts WHERE job_id=?1 ORDER BY rowid",
        )
        .map_err(database_error)?;
    let attempts = attempts
        .query_map([id], |row| {
            Ok(json!({
                "ask_id": row.get::<_, String>(0)?,
                "peer_id": row.get::<_, String>(1)?,
                "session_id": row.get::<_, Option<String>>(2)?,
                "opened_at": row.get::<_, Option<i64>>(3)?,
                "closed_at": row.get::<_, Option<i64>>(4)?,
                "closed_by": row.get::<_, Option<String>>(5)?,
                "failed": row.get::<_, Option<bool>>(6)?,
            }))
        })
        .map_err(database_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(database_error)?;
    history["attempts"] = json!(attempts);
    Ok(history)
}

pub(super) fn request(db: &Connection, scope: &str) -> rusqlite::Result<Option<(String, String)>> {
    db.query_row(
        "SELECT job_id,request_hash FROM job_history WHERE request_scope=?1",
        [scope],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

pub(super) fn bind_request(
    tx: &Transaction<'_>,
    id: &str,
    scope: &str,
    hash: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE job_history SET request_scope=?1,request_hash=?2 WHERE job_id=?3",
        params![scope, hash, id],
    )?;
    Ok(())
}

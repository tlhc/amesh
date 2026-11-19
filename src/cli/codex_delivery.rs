use super::{
    app_rpc, hook_ws_dir, AppRejected, AppSink, Queued, Result, SinkState, NO_DIRECT_INPUT,
};
use crate::bridge::{inject_method, inject_params, turn_is_over};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_RETRIES: u8 = 3;
const RECENT_TURNS: usize = 8;
const PAGE_BUDGET: usize = 4;
const SEND_BUDGET: usize = 8;

pub(super) struct CodexDelivery {
    peer: String,
    journal: Option<Connection>,
    thread: Option<String>,
    pending: HashMap<String, Pending>,
    retries: HashMap<String, u8>,
    ready: HashSet<String>,
    last_polled: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Anchor {
    turn: Option<String>,
    item: Option<String>,
}

struct Pending {
    record: Queued,
    anchor: Anchor,
    turn: Option<String>,
    progress: HashMap<String, Progress>,
    exhausted: bool,
}

#[derive(Default)]
struct Progress {
    cursor: Option<Value>,
    closed: bool,
}

enum Receipt {
    Found,
    Missing,
    Waiting,
}

impl CodexDelivery {
    pub(super) fn new(peer: &str) -> Self {
        let bind = std::env::var("AMESH_BIND").unwrap_or_else(|_| "127.0.0.1:8378".into());
        Self {
            peer: serde_json::to_string(&(bind, peer)).expect("string pair serializes"),
            journal: None,
            thread: None,
            pending: HashMap::new(),
            retries: HashMap::new(),
            ready: HashSet::new(),
            last_polled: None,
        }
    }

    pub(super) fn bind(&mut self, thread: &str) -> bool {
        if self.thread.as_deref() == Some(thread) {
            return false;
        }
        self.thread = Some(thread.to_string());
        self.pending.clear();
        self.retries.clear();
        self.ready.clear();
        self.last_polled = None;
        true
    }

    fn open_journal(&mut self) -> Result<()> {
        if self.journal.is_none() {
            let path = hook_ws_dir().ok_or("no A Mesh state directory for Codex receipts")?;
            let db = Connection::open(path.join("codex-delivery.db"))?;
            db.busy_timeout(std::time::Duration::from_millis(200))?;
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS attempts (
                    peer TEXT NOT NULL, thread TEXT NOT NULL, id TEXT NOT NULL,
                    anchor TEXT NOT NULL, PRIMARY KEY (peer, thread, id)
                ) WITHOUT ROWID;",
            )?;
            self.journal = Some(db);
        }
        Ok(())
    }

    pub(super) fn retired(&self, thread: &str, id: &str) {
        if let Some(db) = &self.journal {
            if let Err(error) = db.execute(
                "DELETE FROM attempts WHERE peer = ?1 AND thread = ?2 AND id = ?3",
                params![self.peer, thread, id],
            ) {
                eprintln!("amesh hook ws: could not retire Codex receipt anchor: {error}");
            }
        }
    }

    pub(super) fn holds(&self, owner: Option<&str>, id: &str) -> bool {
        owner.is_none_or(|owner| self.thread.as_deref() == Some(owner))
            && self.pending.contains_key(id)
    }

    pub(super) fn awaiting_confirmation(&self, owner: Option<&str>) -> Vec<&str> {
        if self.thread.as_deref() != owner {
            return Vec::new();
        }
        let mut ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, item)| item.turn.is_some() && !item.exhausted)
            .map(|(id, _)| id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    }

    fn checkpoint(&mut self, thread: &str, id: &str) -> Result<Option<Anchor>> {
        self.open_journal()?;
        let db = self.journal.as_ref().unwrap();
        let saved: Option<String> = db
            .query_row(
                "SELECT anchor FROM attempts WHERE peer = ?1 AND thread = ?2 AND id = ?3",
                params![self.peer, thread, id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(saved.as_deref().map(serde_json::from_str).transpose()?)
    }

    fn save_checkpoint(&self, thread: &str, id: &str, anchor: &Anchor) -> Result<bool> {
        Ok(self.journal.as_ref().unwrap().execute(
            "INSERT INTO attempts (peer, thread, id, anchor) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (peer, thread, id) DO NOTHING",
            params![self.peer, thread, id, serde_json::to_string(anchor)?],
        )? != 0)
    }

    fn hold(&mut self, mut record: Queued, anchor: Anchor, turn: Option<String>, thread: &str) {
        let id = record.2.clone().unwrap();
        self.ready.remove(&id);
        record.0 = Some(thread.to_string());
        self.pending.insert(
            id,
            Pending {
                record,
                anchor,
                turn,
                progress: HashMap::new(),
                exhausted: false,
            },
        );
    }

    async fn receipts(
        &mut self,
        sink: &mut AppSink,
        queued: &mut VecDeque<Queued>,
        acked: &mut Vec<String>,
    ) -> Result<()> {
        let mut ids: Vec<_> = self.pending.keys().cloned().collect();
        if ids.is_empty() {
            return Ok(());
        }
        ids.sort_unstable();
        if let Some(last) = &self.last_polled {
            let first = ids.partition_point(|id| id <= last) % ids.len();
            ids.rotate_left(first);
        }
        let snapshot = sink.recent_turns(RECENT_TURNS).await?;
        let (turns, next) = page(&snapshot)?;
        let latest = turns.first().and_then(turn_id);
        if latest != sink.active_turn.as_deref() {
            sink.active_turn = None;
        }
        let can_be_idle = turns.iter().all(terminal_turn);
        let mut budget = PAGE_BUDGET;
        for id in ids {
            if budget == 0 {
                break;
            }
            self.last_polled = Some(id.clone());
            let item = self.pending.get_mut(&id).unwrap();
            let mut remaining = budget;
            let result = item
                .receipt(sink, &id, turns, next.is_none(), &mut remaining)
                .await?;
            budget = remaining.min(budget - 1);
            match result {
                Receipt::Found => {
                    self.pending.remove(&id);
                    self.retries.remove(&id);
                    acked.push(id);
                }
                Receipt::Missing if !item.exhausted => {
                    let interrupted = item.turn.as_deref().is_some_and(|id| {
                        turns.iter().any(|turn| {
                            turn_id(turn) == Some(id)
                                && matches!(turn["status"].as_str(), Some("interrupted" | "failed"))
                        })
                    });
                    if !interrupted {
                        if !can_be_idle || !thread_idle(&sink.read().await?) {
                            continue;
                        }
                        let head = sink.recent_turns(1).await?;
                        let (head, _) = page(&head)?;
                        if head.first().and_then(turn_id) != latest
                            || !head.first().is_none_or(terminal_turn)
                        {
                            continue;
                        }
                    }
                    let retries = self.retries.entry(id.clone()).or_default();
                    if *retries == MAX_RETRIES {
                        item.exhausted = true;
                        eprintln!("amesh hook ws: {id} missing from Codex history after {MAX_RETRIES} retries; left in hub inbox");
                        continue;
                    }
                    *retries += 1;
                    let item = self.pending.remove(&id).unwrap();
                    queued.push_back(item.record);
                    self.ready.insert(id);
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn send(&mut self, sink: &mut AppSink, queued: &mut VecDeque<Queued>) -> Result<bool> {
        for _ in 0..SEND_BUDGET {
            let Some((owner, text, id)) = queued.front() else {
                return Ok(false);
            };
            if !matches_thread(owner, &sink.thread_id) {
                return Ok(false);
            }
            let anchor = if let Some(id) = id {
                let saved = self.checkpoint(&sink.thread_id, id)?;
                if !self.ready.contains(id) {
                    if let Some(anchor) = saved {
                        self.hold(queued.pop_front().unwrap(), anchor, None, &sink.thread_id);
                        continue;
                    }
                    /* Commit the lower bound before the first RPC that can inject this ID. */
                    let anchor = sink.tail_anchor().await?;
                    if !self.save_checkpoint(&sink.thread_id, id, &anchor)? {
                        continue;
                    }
                    self.ready.insert(id.clone());
                    anchor
                } else {
                    saved.ok_or("missing Codex delivery checkpoint")?
                }
            } else {
                Anchor::default()
            };
            match sink.submit(text, id.as_deref()).await {
                Ok(Submission::Busy) => return Ok(false),
                Ok(Submission::Accepted(turn)) => {
                    let record = queued.pop_front().unwrap();
                    if record.2.is_some() {
                        self.hold(record, anchor, turn, &sink.thread_id);
                    }
                }
                Err((error, uncertain)) => {
                    if uncertain && id.is_some() {
                        self.hold(queued.pop_front().unwrap(), anchor, None, &sink.thread_id);
                    }
                    return Err(error);
                }
            }
        }
        Ok(queued
            .front()
            .is_some_and(|(owner, _, _)| matches_thread(owner, &sink.thread_id)))
    }

    pub(super) async fn flush(
        &mut self,
        sink: &mut AppSink,
        queued: &mut VecDeque<Queued>,
        poll: bool,
    ) -> (SinkState, Vec<String>, bool) {
        self.bind(&sink.thread_id);
        queued.retain(|(_, _, id)| id.as_ref().is_none_or(|id| !self.pending.contains_key(id)));
        if (!queued.is_empty() || !self.pending.is_empty()) && sink.paginated_history != Some(true)
        {
            if sink.paginated_history.is_none() {
                match sink.read().await {
                    Ok(read)
                        if read.pointer("/result/thread/canAcceptDirectInput")
                            == Some(&Value::Bool(false)) =>
                    {
                        return (SinkState::NoDirectInput, Vec::new(), false);
                    }
                    Ok(_) if sink.paginated_history != Some(true) => {
                        eprintln!("amesh hook ws: Codex thread has no indexed history; inbound remains queued");
                    }
                    Ok(_) => {}
                    Err(error) => return (failure(error.as_ref()), Vec::new(), false),
                }
            }
            if sink.paginated_history != Some(true) {
                return (SinkState::Usable, Vec::new(), false);
            }
        }
        let more = match self.send(sink, queued).await {
            Ok(more) => more,
            Err(error) => {
                eprintln!("amesh hook ws: App Server inject failed: {error}");
                return (failure(error.as_ref()), Vec::new(), false);
            }
        };
        let mut acked = Vec::new();
        if poll {
            if let Err(error) = self.receipts(sink, queued, &mut acked).await {
                eprintln!("amesh hook ws: Codex receipt check failed: {error}");
                if !error.is::<AppRejected>() {
                    return (failure(error.as_ref()), acked, false);
                }
            }
        }
        (
            SinkState::Usable,
            acked,
            more || (poll
                && queued.front().is_some_and(|(_, _, id)| {
                    id.as_ref()
                        .is_some_and(|id| self.retries.contains_key(id) && self.ready.contains(id))
                })),
        )
    }
}

impl Pending {
    async fn receipt(
        &mut self,
        sink: &mut AppSink,
        id: &str,
        recent: &[Value],
        complete: bool,
        budget: &mut usize,
    ) -> Result<Receipt> {
        let floor = self.turn.as_deref().or(self.anchor.turn.as_deref());
        let mut scope: Vec<_> = recent
            .iter()
            .map(|turn| {
                Ok((
                    turn_id(turn).ok_or("missing Codex turn ID")?,
                    terminal_turn(turn),
                ))
            })
            .collect::<Result<_>>()?;
        let bounded = match floor {
            Some(floor) => match scope.iter().position(|(id, _)| *id == floor) {
                Some(index) => {
                    scope.truncate(index + 1);
                    true
                }
                None => {
                    /* A turn can accept input before it appears in the recent-turn index. */
                    scope.clear();
                    scope.push((floor, false));
                    false
                }
            },
            None => complete,
        };

        let client = format!("amesh:{id}");
        for (turn, terminal) in scope.into_iter().rev() {
            let progress = self
                .progress
                .entry(turn.to_string())
                .or_insert_with(|| Progress {
                    cursor: self
                        .anchor
                        .item
                        .as_ref()
                        .filter(|_| self.anchor.turn.as_deref() == Some(turn))
                        .map(|item| item_cursor(item)),
                    closed: false,
                });
            if progress.closed {
                continue;
            }
            loop {
                if *budget == 0 {
                    return Ok(Receipt::Waiting);
                }
                *budget -= 1;
                let reply = sink.rpc("thread/items/list", json!({
                    "threadId": sink.thread_id, "turnId": turn, "limit": 32, "sortDirection": "asc",
                    "cursor": progress.cursor
                })).await?;
                let (items, next) = page(&reply)?;
                let mut last = None;
                for entry in items {
                    if entry["turnId"] != turn {
                        return Err("unexpected turn in Codex receipt page".into());
                    }
                    let item = &entry["item"];
                    if item["type"] == "userMessage" && item["clientId"] == client {
                        return Ok(Receipt::Found);
                    }
                    last = Some(
                        item["id"]
                            .as_str()
                            .filter(|id| !id.is_empty())
                            .ok_or("missing Codex item ID")?,
                    );
                }
                if let Some(next) = next {
                    let next = json!(next);
                    if progress.cursor.as_ref() == Some(&next) {
                        return Err("Codex item cursor did not advance".into());
                    }
                    progress.cursor = Some(next);
                    continue;
                }
                if let Some(last) = last {
                    progress.cursor = Some(item_cursor(last));
                }
                progress.closed = terminal;
                break;
            }
        }
        Ok(if bounded {
            Receipt::Missing
        } else {
            Receipt::Waiting
        })
    }
}

fn item_cursor(id: &str) -> Value {
    json!({"type": "item", "itemId": id})
}

fn thread_idle(read: &Value) -> bool {
    read.pointer("/result/thread/status/type")
        .and_then(Value::as_str)
        == Some("idle")
}

fn turn_id(turn: &Value) -> Option<&str> {
    turn["id"].as_str().filter(|id| !id.is_empty())
}

fn matches_thread(owner: &Option<String>, thread: &str) -> bool {
    owner.as_deref().is_none_or(|owner| owner == thread)
}

fn terminal_turn(turn: &Value) -> bool {
    matches!(
        turn["status"].as_str(),
        Some("completed" | "interrupted" | "failed")
    )
}

fn page(reply: &Value) -> Result<(&[Value], Option<String>)> {
    let result = &reply["result"];
    let rows = result["data"]
        .as_array()
        .ok_or("invalid Codex history data")?;
    let next = match result.get("nextCursor") {
        Some(Value::Null) => None,
        Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
        _ => return Err("invalid Codex history cursor".into()),
    };
    Ok((rows, next))
}

fn failure(error: &(dyn std::error::Error + 'static)) -> SinkState {
    if error.to_string().contains(NO_DIRECT_INPUT) {
        SinkState::NoDirectInput
    } else {
        SinkState::Failed
    }
}

enum Submission {
    Busy,
    Accepted(Option<String>),
}

impl AppSink {
    pub(super) async fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.rpc_id;
        self.rpc_id += 1;
        app_rpc(&mut self.ws, id, method, params).await
    }

    pub(super) async fn read(&mut self) -> Result<Value> {
        let reply = self
            .rpc("thread/read", json!({"threadId": self.thread_id}))
            .await?;
        self.paginated_history = Some(
            reply
                .pointer("/result/thread/historyMode")
                .and_then(Value::as_str)
                == Some("paginated"),
        );
        Ok(reply)
    }

    async fn recent_turns(&mut self, limit: usize) -> Result<Value> {
        self.rpc(
            "thread/turns/list",
            json!({
                "threadId": self.thread_id, "limit": limit, "itemsView": "notLoaded",
                "sortDirection": "desc"
            }),
        )
        .await
    }

    async fn tail_anchor(&mut self) -> Result<Anchor> {
        let reply = self
            .rpc(
                "thread/items/list",
                json!({
                    "threadId": self.thread_id, "limit": 1, "sortDirection": "desc"
                }),
            )
            .await?;
        let (items, next) = page(&reply)?;
        match items {
            [] if next.is_none() => Ok(Anchor::default()),
            [entry] => Ok(Anchor {
                turn: Some(
                    entry["turnId"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or("missing Codex tail turn ID")?
                        .to_string(),
                ),
                item: Some(
                    entry["item"]["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or("missing Codex tail item ID")?
                        .to_string(),
                ),
            }),
            _ => Err("invalid Codex history tail".into()),
        }
    }

    async fn submit(
        &mut self,
        text: &str,
        record: Option<&str>,
    ) -> std::result::Result<Submission, (Box<dyn std::error::Error>, bool)> {
        for attempt in 0..2 {
            let read = self.read().await.map_err(|error| (error, false))?;
            if read.pointer("/result/thread/canAcceptDirectInput") == Some(&Value::Bool(false)) {
                return Err((NO_DIRECT_INPUT.into(), false));
            }
            if turn_is_over(&read) {
                self.active_turn = None;
            } else if self.active_turn.is_none() {
                let turns = self.recent_turns(1).await.map_err(|error| (error, false))?;
                self.active_turn = turns
                    .pointer("/result/data/0")
                    .filter(|turn| turn["status"] == "inProgress")
                    .and_then(|turn| turn["id"].as_str())
                    .filter(|id| !id.is_empty())
                    .map(str::to_string);
                if self.active_turn.is_none() {
                    return Ok(Submission::Busy);
                }
            }
            let method = inject_method(self.active_turn.as_deref());
            let mut params = inject_params(&self.thread_id, text, self.active_turn.as_deref());
            if let Some(id) = record {
                params["clientUserMessageId"] = json!(format!("amesh:{id}"));
            }
            match self.rpc(method, params).await {
                Ok(reply) => {
                    let turn = reply
                        .pointer("/result/turn/id")
                        .or_else(|| reply.pointer("/result/turnId"))
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string);
                    self.active_turn = turn.clone();
                    return Ok(Submission::Accepted(turn));
                }
                Err(error) => {
                    let Some(rejection) = error.downcast_ref::<AppRejected>() else {
                        return Err((error, true));
                    };
                    let message = rejection.error["message"].as_str().unwrap_or("");
                    if method == "turn/steer"
                        && rejection.error["code"] == -32600
                        && (message == "no active turn to steer" || turn_mismatch(message))
                    {
                        self.active_turn = None;
                        if attempt == 0 {
                            continue;
                        }
                        return Ok(Submission::Busy);
                    }
                    if rejection
                        .error
                        .pointer("/data/codexErrorInfo/activeTurnNotSteerable")
                        .is_some()
                        || matches!(
                            message,
                            "cannot steer a review turn" | "cannot steer a compact turn"
                        )
                    {
                        return Ok(Submission::Busy);
                    }
                    /* An internal error can follow a partial submission. Reconcile by ID. */
                    let uncertain = rejection.error["code"].as_i64() != Some(-32600)
                        && rejection.error["code"].as_i64() != Some(-32602);
                    return Err((error, uncertain));
                }
            }
        }
        unreachable!("submission either returns or refreshes once")
    }
}

fn turn_mismatch(message: &str) -> bool {
    message
        .strip_prefix("expected active turn id `")
        .and_then(|message| message.strip_suffix('`'))
        .and_then(|message| message.split_once("` but found `"))
        .is_some_and(|(expected, actual)| {
            !expected.is_empty()
                && !actual.is_empty()
                && !expected.contains('`')
                && !actual.contains('`')
        })
}

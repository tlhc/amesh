use super::{
    app_rpc, ask_open, env_secs, remember_hook_inbound, AppRejected, AppSink, Queued, Result,
    SinkState, NO_DIRECT_INPUT,
};
use crate::bridge::{inject_method, inject_params, turn_is_over};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const MAX_RETRIES: u8 = 3;
const MAX_REPLAY_REJECTIONS: u8 = 3;
const REPLAY_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_PARKED_RECORDS: usize = 32;
const PARKED_RECHECK_CAP: Duration = Duration::from_secs(600);
const RECENT_TURNS: usize = 8;
const PAGE_BUDGET: usize = 4;
const SEND_BUDGET: usize = 8;

#[derive(Default)]
pub(super) struct CodexDelivery {
    thread: Option<String>,
    pending: HashMap<String, Pending>,
    replay: Option<ReplayLookup>,
    parked_replays: VecDeque<String>,
    rechecks: VecDeque<ParkedReplay>,
    retries: HashMap<String, u8>,
    sequence: u64,
    last_polled: Option<u64>,
}

struct ReplayLookup {
    id: String,
    scan: ReceiptScan,
    rejections: u8,
    retry_at: Instant,
}

struct ParkedReplay {
    record: Queued,
    scan: ReceiptScan,
    retry_at: Instant,
    delay: Duration,
}

impl ParkedReplay {
    fn new(record: Queued) -> Self {
        let delay = Duration::from_secs(3 * env_secs("AMESH_WS_BEAT_SECS", 10).min(10));
        Self {
            record,
            scan: ReceiptScan::default(),
            retry_at: Instant::now() + delay,
            delay,
        }
    }

    fn back_off(&mut self, now: Instant) {
        self.delay = (self.delay * 2).min(PARKED_RECHECK_CAP);
        self.retry_at = now + self.delay;
    }
}

#[derive(Default)]
struct Anchor {
    turn: Option<String>,
    item: Option<String>,
}

struct Pending {
    sequence: u64,
    record: Queued,
    scan: ReceiptScan,
    exhausted: bool,
}

#[derive(Default)]
struct ReceiptScan {
    anchor: Option<Anchor>,
    turn: Option<String>,
    progress: HashMap<String, Progress>,
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
    pub(super) fn bind(&mut self, thread: &str) -> bool {
        if self.thread.as_deref() == Some(thread) {
            return false;
        }
        self.thread = Some(thread.to_string());
        self.pending.clear();
        self.replay = None;
        self.parked_replays.clear();
        self.rechecks.clear();
        self.retries.clear();
        self.sequence = 0;
        self.last_polled = None;
        true
    }

    pub(super) fn holds(&self, owner: Option<&str>, id: &str) -> bool {
        owner.is_none_or(|owner| self.thread.as_deref() == Some(owner))
            && (self.pending.contains_key(id)
                || self.parked_replays.iter().any(|known| known == id)
                || self
                    .rechecks
                    .iter()
                    .any(|item| item.record.id.as_deref() == Some(id)))
    }

    pub(super) fn awaiting_confirmation(&self, owner: Option<&str>) -> Vec<&str> {
        if self.thread.as_deref() != owner {
            return Vec::new();
        }
        let mut ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, item)| item.scan.turn.is_some() && !item.exhausted)
            .map(|(id, _)| id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    }

    fn hold(
        &mut self,
        mut record: Queued,
        anchor: Option<Anchor>,
        turn: Option<String>,
        thread: &str,
    ) {
        let id = record.id.clone().unwrap();
        record.owner = Some(thread.to_string());
        record.replay = false;
        self.replay = None;
        self.sequence += 1;
        self.pending.insert(
            id,
            Pending {
                sequence: self.sequence,
                record,
                scan: ReceiptScan {
                    anchor,
                    turn,
                    ..ReceiptScan::default()
                },
                exhausted: false,
            },
        );
    }

    async fn receipts(
        &mut self,
        sink: &mut AppSink,
        queued: &mut VecDeque<Queued>,
        acked: &mut Vec<String>,
        budget: &mut usize,
    ) -> Result<bool> {
        let mut ids: Vec<_> = self
            .pending
            .iter()
            .map(|(id, item)| (item.sequence, id.clone()))
            .collect();
        if ids.is_empty() || *budget == 0 {
            return Ok(false);
        }
        ids.sort_unstable();
        if let Some(last) = self.last_polled {
            let first = ids.partition_point(|(sequence, _)| *sequence <= last) % ids.len();
            ids.rotate_left(first);
        }
        let snapshot = sink.recent_turns(RECENT_TURNS).await?;
        let (turns, next) = page(&snapshot)?;
        let latest = turns.first().and_then(turn_id);
        if latest != sink.active_turn.as_deref() {
            sink.active_turn = None;
        }
        let can_be_idle = turns.iter().all(terminal_turn);
        let mut ready = false;
        for (sequence, id) in ids {
            if *budget == 0 {
                break;
            }
            self.last_polled = Some(sequence);
            let item = self.pending.get_mut(&id).unwrap();
            let mut remaining = *budget;
            let result = item
                .scan
                .receipt(sink, &id, turns, next.is_none(), &mut remaining)
                .await;
            *budget = remaining.min(*budget - 1);
            match result? {
                Receipt::Found => {
                    self.pending.remove(&id);
                    self.retries.remove(&id);
                    acked.push(id);
                }
                Receipt::Missing if !item.exhausted => {
                    let interrupted = item.scan.turn.as_deref().is_some_and(|id| {
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
                    queued.push_back(self.pending.remove(&id).unwrap().record);
                    ready = true;
                }
                _ => {}
            }
        }
        Ok(ready)
    }

    fn park_replay(&mut self, record: Queued, permanent: bool) {
        remember_hook_inbound(&mut self.parked_replays, record.id.as_deref().unwrap());
        if permanent {
            return;
        }
        if self.rechecks.len() == MAX_PARKED_RECORDS {
            let evicted = self.rechecks.pop_front().unwrap();
            remember_hook_inbound(
                &mut self.parked_replays,
                evicted.record.id.as_deref().unwrap(),
            );
        }
        self.rechecks.push_back(ParkedReplay::new(record));
    }

    async fn send(
        &mut self,
        sink: &mut AppSink,
        queued: &mut VecDeque<Queued>,
        acked: &mut Vec<String>,
        budget: &mut usize,
    ) -> Result<bool> {
        for _ in 0..SEND_BUDGET {
            let Some(record) = queued.front_mut() else {
                return Ok(false);
            };
            if !matches_thread(&record.owner, &sink.thread_id) {
                return Ok(false);
            }
            let mut replay_unindexed = false;
            if let Some(id) = record.id.as_ref().filter(|_| record.replay) {
                if self
                    .replay
                    .as_ref()
                    .is_some_and(|lookup| lookup.id == *id && Instant::now() < lookup.retry_at)
                {
                    return Ok(false);
                }
                if *budget == 0 {
                    return Ok(true);
                }
                /* Keep the replay at the queue head while its lookup is still viable. */
                let result = self.scan_replay(sink, id, budget).await;
                match result {
                    Ok(Receipt::Found) => {
                        acked.push(id.clone());
                        queued.pop_front();
                        self.replay = None;
                        continue;
                    }
                    Ok(Receipt::Missing) => {
                        record.replay = false;
                        self.replay = None;
                    }
                    Ok(Receipt::Waiting) => return Ok(true),
                    Err(error)
                        if history_unindexed(
                            error.as_ref(),
                            "thread/turns/list",
                            &sink.thread_id,
                        ) =>
                    {
                        replay_unindexed = true;
                    }
                    Err(error) => {
                        let Some(rejection) = error.downcast_ref::<AppRejected>() else {
                            return Err(error);
                        };
                        let lookup = self.replay.as_mut().unwrap();
                        lookup.rejections += 1;
                        let permanent = permanent_replay_rejection(rejection, &sink.thread_id);
                        if !permanent && lookup.rejections < MAX_REPLAY_REJECTIONS {
                            /* A burst of new frames must give the history store time to recover. */
                            lookup.retry_at = Instant::now() + REPLAY_RETRY_DELAY;
                            eprintln!("amesh hook ws: Codex replay check failed: {error}");
                            return Ok(false);
                        }
                        /* A rejected lookup leaves prior delivery unknown. Keep the Hub record. */
                        eprintln!("amesh hook ws: Codex replay {id} left in hub inbox: {error}");
                        self.park_replay(queued.pop_front().unwrap(), permanent);
                        self.replay = None;
                        continue;
                    }
                }
            }
            let (anchor, bootstrap) = if record.id.is_some() {
                match sink.tail_anchor().await {
                    /* The index appeared during replay; check for the receipt on the next beat. */
                    Ok(_) if replay_unindexed => return Ok(false),
                    Ok(anchor) => (Some(anchor), false),
                    Err(error)
                        if history_unindexed(
                            error.as_ref(),
                            "thread/items/list",
                            &sink.thread_id,
                        ) =>
                    {
                        if !self.pending.is_empty() {
                            return Ok(false);
                        }
                        if !replay_unindexed {
                            match sink.recent_turns(1).await {
                                Ok(_) => return Ok(false),
                                Err(error)
                                    if history_unindexed(
                                        error.as_ref(),
                                        "thread/turns/list",
                                        &sink.thread_id,
                                    ) => {}
                                Err(error) => return Err(error),
                            }
                        }
                        /* Codex indexes the thread only when its first turn persists. */
                        (Some(Anchor::default()), true)
                    }
                    Err(error) => return Err(error),
                }
            } else {
                (None, false)
            };
            let mode = if bootstrap {
                InputMode::Bootstrap
            } else {
                InputMode::Normal
            };
            if !self.submit_queued(sink, queued, anchor, mode).await? {
                return Ok(false);
            }
        }
        Ok(queued
            .front()
            .is_some_and(|record| matches_thread(&record.owner, &sink.thread_id)))
    }

    async fn submit_queued(
        &mut self,
        sink: &mut AppSink,
        queued: &mut VecDeque<Queued>,
        anchor: Option<Anchor>,
        mode: InputMode<'_>,
    ) -> Result<bool> {
        let record = queued.front().unwrap();
        match sink.submit(&record.text, record.id.as_deref(), mode).await {
            Ok(Submission::Busy) => Ok(false),
            Ok(Submission::Accepted(turn)) => {
                let record = queued.pop_front().unwrap();
                if record.id.is_some() {
                    self.hold(record, anchor, turn, &sink.thread_id);
                }
                Ok(true)
            }
            Err((error, uncertain)) => {
                if uncertain && record.id.is_some() {
                    self.hold(queued.pop_front().unwrap(), anchor, None, &sink.thread_id);
                }
                Err(error)
            }
        }
    }

    async fn scan_replay(
        &mut self,
        sink: &mut AppSink,
        id: &str,
        budget: &mut usize,
    ) -> Result<Receipt> {
        if self.replay.as_ref().is_none_or(|lookup| lookup.id != id) {
            self.replay = Some(ReplayLookup {
                id: id.to_string(),
                scan: ReceiptScan::default(),
                rejections: 0,
                retry_at: Instant::now(),
            });
        }
        self.replay
            .as_mut()
            .unwrap()
            .scan
            .cold_receipt(sink, id, budget)
            .await
    }

    async fn recheck_parked(
        &mut self,
        sink: &mut AppSink,
        acked: &mut Vec<String>,
        budget: &mut usize,
    ) -> Result<()> {
        if *budget == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let Some((index, item)) = self
            .rechecks
            .iter_mut()
            .enumerate()
            .filter(|(_, item)| item.retry_at <= now)
            .min_by_key(|(_, item)| item.retry_at)
        else {
            return Ok(());
        };
        let id = item.record.id.as_ref().unwrap().clone();
        let receipt: Result<_> = async {
            let snapshot = sink.recent_turns(RECENT_TURNS).await?;
            let (turns, next) = page(&snapshot)?;
            if turns.iter().any(|turn| !terminal_turn(turn)) {
                return Ok(None);
            }
            let receipt = item
                .scan
                .receipt(sink, &id, turns, next.is_none(), budget)
                .await?;
            let latest = turns.first().and_then(turn_id).map(str::to_string);
            Ok(Some((receipt, latest)))
        }
        .await;
        if matches!(&receipt, Ok(Some((Receipt::Waiting, _)))) {
            return Ok(());
        }
        item.back_off(Instant::now());
        item.scan = ReceiptScan::default();
        match receipt {
            Ok(None | Some((Receipt::Waiting, _))) => return Ok(()),
            Ok(Some((Receipt::Found, _))) => {}
            Ok(Some((Receipt::Missing, latest))) => {
                let open = if let Some(cid) = item.record.ask_id.clone() {
                    tokio::task::spawn_blocking(move || ask_open(&cid))
                        .await
                        .ok()
                        .flatten()
                } else {
                    Some(true)
                };
                match open {
                    None => return Ok(()),
                    Some(false) => {}
                    Some(true) => {
                        let anchor = sink.tail_anchor().await?;
                        let mut parked = self.rechecks.remove(index).unwrap();
                        /* An unsuccessful recovery cannot become the foreground's next head. */
                        let mut retry = VecDeque::from([parked.record]);
                        let result = self
                            .submit_queued(
                                sink,
                                &mut retry,
                                Some(anchor),
                                InputMode::Recovered(latest.as_deref()),
                            )
                            .await;
                        if let Some(record) = retry.pop_front() {
                            parked.record = record;
                            self.rechecks.insert(index, parked);
                        } else {
                            self.parked_replays.retain(|known| known != &id);
                        }
                        return result.map(|_| ());
                    }
                }
            }
            Err(error) => {
                let Some(rejection) = error.downcast_ref::<AppRejected>() else {
                    return Err(error);
                };
                if permanent_replay_rejection(rejection, &sink.thread_id)
                    && !history_unindexed(rejection, "thread/turns/list", &sink.thread_id)
                {
                    self.rechecks.remove(index);
                    remember_hook_inbound(&mut self.parked_replays, &id);
                    eprintln!(
                        "amesh hook ws: Codex replay {id} permanently left in hub inbox: {error}"
                    );
                }
                return Ok(());
            }
        }
        self.rechecks.remove(index);
        self.parked_replays.retain(|known| known != &id);
        acked.push(id);
        Ok(())
    }

    pub(super) async fn flush(
        &mut self,
        sink: &mut AppSink,
        queued: &mut VecDeque<Queued>,
        poll: bool,
    ) -> (SinkState, Vec<String>, bool) {
        self.bind(&sink.thread_id);
        queued.retain(|record| {
            record
                .id
                .as_ref()
                .is_none_or(|id| !self.pending.contains_key(id))
        });
        if (!queued.is_empty() || !self.pending.is_empty() || (poll && !self.rechecks.is_empty()))
            && sink.paginated_history != Some(true)
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
        let mut acked = Vec::new();
        let mut budget = PAGE_BUDGET;
        let more = match self.send(sink, queued, &mut acked, &mut budget).await {
            Ok(more) => more,
            Err(error) => {
                eprintln!("amesh hook ws: App Server inject failed: {error}");
                return (failure(error.as_ref()), acked, false);
            }
        };
        let mut ready = false;
        if poll {
            match self.receipts(sink, queued, &mut acked, &mut budget).await {
                Ok(requeued) => ready = requeued,
                Err(error) => {
                    eprintln!("amesh hook ws: Codex receipt check failed: {error}");
                    if !error.is::<AppRejected>() {
                        return (failure(error.as_ref()), acked, false);
                    }
                }
            }
            if queued.is_empty() {
                if let Err(error) = self.recheck_parked(sink, &mut acked, &mut budget).await {
                    if !error.is::<AppRejected>() {
                        return (failure(error.as_ref()), acked, false);
                    }
                }
            }
        }
        (SinkState::Usable, acked, more || ready)
    }
}

impl ReceiptScan {
    async fn cold_receipt(
        &mut self,
        sink: &mut AppSink,
        id: &str,
        budget: &mut usize,
    ) -> Result<Receipt> {
        let snapshot = sink.recent_turns(RECENT_TURNS).await?;
        let (turns, next) = page(&snapshot)?;
        self.receipt(sink, id, turns, next.is_none(), budget).await
    }

    async fn receipt(
        &mut self,
        sink: &mut AppSink,
        id: &str,
        recent: &[Value],
        complete: bool,
        budget: &mut usize,
    ) -> Result<Receipt> {
        let replay = self.anchor.is_none();
        if replay {
            self.progress
                .retain(|id, _| recent.iter().any(|turn| turn_id(turn) == Some(id)));
        }
        let floor = self.turn.as_deref().or_else(|| {
            self.anchor
                .as_ref()
                .and_then(|anchor| anchor.turn.as_deref())
        });
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
            None => replay || complete,
        };
        let direction = if replay {
            "desc"
        } else {
            scope.reverse();
            "asc"
        };

        let client = format!("amesh:{id}");
        for (turn, terminal) in scope {
            let progress = self
                .progress
                .entry(turn.to_string())
                .or_insert_with(|| Progress {
                    cursor: self
                        .anchor
                        .as_ref()
                        .filter(|anchor| anchor.turn.as_deref() == Some(turn))
                        .and_then(|anchor| anchor.item.as_deref())
                        .map(item_cursor),
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
                    "threadId": sink.thread_id, "turnId": turn, "limit": 32, "sortDirection": direction,
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
                progress.closed = replay || terminal;
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

fn history_unindexed(
    error: &(dyn std::error::Error + 'static),
    method: &str,
    thread: &str,
) -> bool {
    let Some(rejection) = error.downcast_ref::<AppRejected>() else {
        return false;
    };
    let (code, message) = (&rejection.error["code"], &rejection.error["message"]);
    rejection.method == method
        && match method {
            "thread/items/list" => {
                *code == -32601 && *message == "thread/items/list is not supported yet"
            }
            /* -32601 when the store knows the thread but has no row for it yet, -32600 when
            it finds no rollout and Codex falls back to the legacy reader */
            "thread/turns/list" => {
                *code == -32601 && *message == "list_turns is not supported yet"
                    || *code == -32600
                        && *message
                            == format!(
                                "thread {thread} is not materialized yet; \
                                 thread/turns/list is unavailable before first user message"
                            )
            }
            _ => false,
        }
}

fn permanent_replay_rejection(rejection: &AppRejected, thread: &str) -> bool {
    match rejection.error["code"].as_i64() {
        Some(-32600) => {
            rejection.error["message"] != format!("no rollout found for thread id {thread}")
        }
        Some(-32601) => !history_unindexed(rejection, "thread/items/list", thread),
        Some(-32602) => true,
        _ => false,
    }
}

fn failure(error: &(dyn std::error::Error + 'static)) -> SinkState {
    if error.to_string().contains(NO_DIRECT_INPUT) {
        SinkState::NoDirectInput
    } else {
        SinkState::Failed
    }
}

#[derive(Clone, Copy)]
enum InputMode<'a> {
    Normal,
    Bootstrap,
    Recovered(Option<&'a str>),
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
        mode: InputMode<'_>,
    ) -> std::result::Result<Submission, (Box<dyn std::error::Error>, bool)> {
        for attempt in 0..2 {
            let read = self.read().await.map_err(|error| (error, false))?;
            if read.pointer("/result/thread/canAcceptDirectInput") == Some(&Value::Bool(false)) {
                return Err((NO_DIRECT_INPUT.into(), false));
            }
            match mode {
                InputMode::Normal => {}
                InputMode::Bootstrap => {
                    let thread = &read["result"]["thread"];
                    if self.paginated_history != Some(true)
                        || !thread_idle(&read)
                        || thread["ephemeral"] != false
                        || thread["preview"] != ""
                    {
                        return Ok(Submission::Busy);
                    }
                }
                InputMode::Recovered(latest) => {
                    if self.paginated_history != Some(true) || !thread_idle(&read) {
                        return Ok(Submission::Busy);
                    }
                    let head = self.recent_turns(1).await.map_err(|error| (error, false))?;
                    let (turns, _) = page(&head).map_err(|error| (error, false))?;
                    if turns.first().and_then(turn_id) != latest
                        || !turns.first().is_none_or(terminal_turn)
                    {
                        return Ok(Submission::Busy);
                    }
                }
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

#[cfg(test)]
#[path = "../../tests/cli/codex_recovery_policy.rs"]
mod recovery_tests;

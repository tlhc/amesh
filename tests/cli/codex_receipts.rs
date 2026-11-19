use super::*;
use std::sync::mpsc;

#[derive(Default)]
struct History {
    status: Option<&'static str>,
    client_ids: Vec<String>,
    hidden_page: bool,
    failed: bool,
    long_history: bool,
    legacy: bool,
    hide_floor: bool,
    successor: bool,
    successor_receipt: Option<String>,
    calls: Vec<Value>,
}

struct Probe {
    sandbox: Sandbox,
    home: PathBuf,
    requests: mpsc::Receiver<String>,
    history: Arc<Mutex<History>>,
    connections: Arc<AtomicUsize>,
    _drainer: KillChild,
}

impl Probe {
    fn new(submit: impl Fn(&Value) -> Value + Send + Sync + 'static) -> Self {
        Self::with_history(History::default(), submit)
    }

    fn with_history(
        initial: History,
        submit: impl Fn(&Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        let mut sandbox = Sandbox::new();
        sandbox.start();
        let worker = sandbox.id("worker");
        bind_session(&sandbox, "codex", worker, "th1");
        register_boss(&sandbox);
        let home = sandbox.root.join("codex");
        let history = Arc::new(Mutex::new(initial));
        let state = history.clone();
        let connections = Arc::new(AtomicUsize::new(0));
        let opened = connections.clone();
        let requests = app_server_capture_driver(
            &home,
            &["th1", "th2"],
            Arc::default(),
            None,
            None,
            Some(Arc::new(move |req| {
                let mut history = state.lock().unwrap();
                if matches!(
                    req["method"].as_str(),
                    Some("thread/items/list" | "thread/turns/list")
                ) {
                    history.calls.push(req.clone());
                }
                match req["method"].as_str()? {
                    "initialize" => {
                        opened.fetch_add(1, Ordering::SeqCst);
                        None
                    }
                    "thread/read" => Some(json!({"result":{"thread":{"status":{"type":
                        if history.successor || history.status == Some("inProgress") { "active" } else { "idle" }
                    },"historyMode": if history.legacy { "legacy" } else { "paginated" }}}})),
                    "thread/turns/list" => {
                        let mut data = vec![
                            json!({"id":"user-turn","status":history.status.unwrap_or("inProgress")}),
                        ];
                        if history.hide_floor {
                            data = vec![json!({"id":"unrelated-head","status":"completed"})];
                        } else if history.successor {
                            data.insert(0, json!({"id":"next-turn","status":"inProgress"}));
                        }
                        data.truncate(req["params"]["limit"].as_u64().unwrap() as usize);
                        Some(
                            json!({"result":{"data":data,"nextCursor":if history.long_history { json!("old-turns") } else { Value::Null }}}),
                        )
                    }
                    "thread/items/list" => {
                        if history.failed {
                            return Some(
                                json!({"error":{"code":-32603,"message":"history temporarily unavailable"}}),
                            );
                        }
                        let params = &req["params"];
                        let matches = params["turnId"].is_null() || params["turnId"] == "user-turn";
                        let bounded_tail =
                            params["limit"] == 1 && params["sortDirection"] == "desc";
                        let anchor = json!({"turnId":"user-turn","item":{"id":"tail","type":"agentMessage"}});
                        let mut data = Vec::new();
                        if matches {
                            if history.long_history && params["cursor"].is_null() {
                                data.push(anchor);
                            }
                            data.extend(history.client_ids.iter().map(|id|
                                json!({"turnId":"user-turn","item":{"id":id,"type":"userMessage","clientId":id}})
                            ));
                        }
                        if (params["turnId"].is_null() || params["turnId"] == "next-turn")
                            && history.successor
                        {
                            if let Some(id) = &history.successor_receipt {
                                data.push(json!({"turnId":"next-turn","item":{"id":id,"type":"userMessage","clientId":id}}));
                            }
                        }
                        if bounded_tail {
                            data = data.pop().into_iter().collect();
                        }
                        if history.hidden_page && req["params"]["cursor"] == "more" {
                            return Some(
                                json!({"error":{"code":-32603,"message":"page temporarily unavailable"}}),
                            );
                        }
                        Some(
                            json!({"result":{"data":data,"nextCursor":if history.hidden_page { json!("more") } else { Value::Null }}}),
                        )
                    }
                    "turn/start" | "turn/steer" => Some(submit(req)),
                    _ => None,
                }
            })),
        );
        let drainer = start_codex_drainer(&sandbox, worker, &home);
        Self {
            sandbox,
            home,
            requests,
            history,
            connections,
            _drainer: drainer,
        }
    }

    fn send(&self, kind: &str, text: &str) {
        let mut args = vec!["peer", kind];
        if kind == "notify" {
            args.push(self.sandbox.id("worker"));
        }
        args.extend([text, "--from-peer", "boss"]);
        self.sandbox.json(&args, None);
    }

    fn next(&self) -> Value {
        serde_json::from_str(&self.requests.recv_timeout(Duration::from_secs(8)).unwrap()).unwrap()
    }

    fn quiet(&self) {
        assert!(self
            .requests
            .recv_timeout(Duration::from_millis(1250))
            .is_err());
    }

    fn queued(&self) -> i64 {
        inbox_count(&self.sandbox, self.sandbox.id("worker"))
    }

    fn until_queued(&self, expected: i64) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.queued() != expected {
            assert!(
                Instant::now() < deadline,
                "expected {expected} queued, got {}",
                self.queued()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn accepted(req: &Value) -> Value {
    if req["method"] == "turn/start" {
        json!({"result":{"turn":{"id":"user-turn"}}})
    } else {
        json!({"result":{"turnId":req["params"]["expectedTurnId"]}})
    }
}

fn client_id(req: &Value) -> String {
    let id = req["params"]["clientUserMessageId"].as_str().unwrap_or("");
    assert!(id.starts_with("amesh:") && id.len() > 6, "{req}");
    id.to_string()
}

#[test]
fn start_that_steers_after_idle_stays_queued_until_persisted() {
    let p = Probe::new(accepted);
    p.send("notify", "start races an external turn");
    let start = p.next();
    assert_eq!(start["method"], "turn/start");
    p.history.lock().unwrap().status = Some("inProgress");
    p.quiet();
    assert_eq!(p.queued(), 1, "start RPC alone must not retire delivery");
    p.history.lock().unwrap().client_ids.push(client_id(&start));
    p.until_queued(0);
}

#[test]
fn interrupted_delivery_retries_with_same_id_without_blocking_broadcast() {
    let p = Probe::new(accepted);
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "interrupt me");
    let first = p.next();
    assert_eq!(first["method"], "turn/steer");
    p.send("broadcast", "separate message");
    let second = p.next();
    assert_eq!(second["method"], "turn/steer");
    p.history
        .lock()
        .unwrap()
        .client_ids
        .push(client_id(&second));
    p.until_queued(1);
    p.history.lock().unwrap().status = Some("interrupted");
    let retry = p.next();
    assert_eq!(retry["method"], "turn/start");
    assert_eq!(client_id(&retry), client_id(&first));
    assert_eq!(p.queued(), 1);
    p.history.lock().unwrap().client_ids.push(client_id(&first));
    p.until_queued(0);
    p.quiet();
}

#[test]
fn completed_without_receipt_keeps_hub_record_and_caps_retries() {
    let mut p = Probe::new(accepted);
    p.send("notify", "rejected by submit hook");
    let first = p.next();
    p.history.lock().unwrap().status = Some("completed");
    for _ in 0..3 {
        let retry = p.next();
        assert_eq!(client_id(&retry), client_id(&first));
        assert_eq!(p.queued(), 1);
    }
    p.quiet();
    p.send("broadcast", "still deliver new messages");
    let next = p.next();
    assert_ne!(client_id(&next), client_id(&first));
    p.history.lock().unwrap().client_ids.push(client_id(&next));
    p.until_queued(1);

    let opened = p.connections.load(Ordering::SeqCst);
    let mut hub = p.sandbox.daemon.take().unwrap();
    hub.kill().unwrap();
    hub.wait().unwrap();
    p.sandbox.start();
    let deadline = Instant::now() + Duration::from_secs(8);
    while p.connections.load(Ordering::SeqCst) <= opened {
        assert!(Instant::now() < deadline, "drainer did not reconnect");
        std::thread::sleep(Duration::from_millis(25));
    }
    p.quiet();
    assert_eq!(p.queued(), 1);
}

#[test]
fn receipt_scan_waits_for_a_complete_history_page_before_retry() {
    let p = Probe::new(accepted);
    p.send("notify", "history on another page");
    let first = p.next();
    {
        let mut h = p.history.lock().unwrap();
        h.status = Some("interrupted");
        h.hidden_page = true;
    }
    p.quiet();
    assert_eq!(p.queued(), 1);
    {
        let mut h = p.history.lock().unwrap();
        h.client_ids.push(client_id(&first));
        h.hidden_page = false;
    }
    p.until_queued(0);
    p.quiet();
}

#[test]
fn uncertain_submission_waits_for_persisted_receipt_across_app_reconnect() {
    let p = Probe::new(|_| Value::Null);
    p.send("notify", "connection closed after receiving input");
    let first = p.next();
    p.history.lock().unwrap().status = Some("inProgress");
    p.quiet();
    assert_eq!(p.queued(), 1);
    p.history.lock().unwrap().client_ids.push(client_id(&first));
    p.until_queued(0);
    p.quiet();
}

#[test]
fn persisted_delivery_is_deduplicated_on_drainer_cutover() {
    let mut p = Probe::new(accepted);
    {
        let mut h = p.history.lock().unwrap();
        h.long_history = true;
        h.status = Some("inProgress");
    }
    p.send("notify", "persisted before Hub received recv");
    let first = p.next();
    p._drainer.stop();
    assert_eq!(p.queued(), 1);
    {
        let mut h = p.history.lock().unwrap();
        h.client_ids.push(client_id(&first));
        h.calls.clear();
    }
    p._drainer = start_codex_drainer(&p.sandbox, p.sandbox.id("worker"), &p.home);
    p.until_queued(0);
    p.quiet();
    let h = p.history.lock().unwrap();
    assert!(h
        .calls
        .iter()
        .any(|req| req["method"] == "thread/items/list"
            && req["params"]["turnId"] == "user-turn"
            && req["params"]["cursor"].is_null()));
    assert!(h
        .calls
        .iter()
        .all(|req| req["params"]["cursor"] != "old-turns"));
}

#[test]
fn fresh_submission_and_live_receipts_only_read_the_tail_and_scoped_items() {
    let p = Probe::new(accepted);
    {
        let mut h = p.history.lock().unwrap();
        h.status = Some("inProgress");
        h.long_history = true;
    }
    p.send("broadcast", "new message despite long history");
    let first = p.next();
    for _ in 0..2 {
        p.quiet();
    }
    assert_eq!(p.queued(), 1);
    let mut h = p.history.lock().unwrap();
    let items: Vec<_> = h
        .calls
        .iter()
        .filter(|req| req["method"] == "thread/items/list")
        .collect();
    assert_eq!(items[0]["params"]["limit"], 1);
    assert!(items.len() >= 3, "{items:?}");
    for req in &items[1..] {
        assert_eq!(req["params"]["turnId"], "user-turn");
        assert_eq!(req["params"]["cursor"]["itemId"], "tail");
    }
    h.client_ids.push(client_id(&first));
    drop(h);
    p.until_queued(0);
}

#[test]
fn legacy_history_stays_queued_without_a_history_query() {
    let p = Probe::with_history(
        History {
            legacy: true,
            ..History::default()
        },
        accepted,
    );
    p.send("notify", "legacy history must not be traversed");
    p.quiet();
    assert_eq!(p.queued(), 1);
    assert!(p.history.lock().unwrap().calls.is_empty());
}

#[test]
fn retained_attempt_outside_the_recent_window_stays_queued() {
    let p = Probe::with_history(
        History {
            long_history: true,
            status: Some("inProgress"),
            ..History::default()
        },
        accepted,
    );
    p.send(
        "notify",
        "the originating turn leaves the window during delivery",
    );
    p.next();
    {
        let mut h = p.history.lock().unwrap();
        h.hide_floor = true;
        h.status = Some("completed");
        h.calls.clear();
    }
    p.quiet();
    assert_eq!(p.queued(), 1);
    let h = p.history.lock().unwrap();
    assert!(h
        .calls
        .iter()
        .any(|req| req["params"]["turnId"] == "user-turn"));
    assert!(h
        .calls
        .iter()
        .all(|req| req["params"]["cursor"] != "old-turns"));
}

#[test]
fn replay_without_a_receipt_in_the_recent_window_submits_once() {
    let mut p = Probe::with_history(
        History {
            long_history: true,
            status: Some("inProgress"),
            ..History::default()
        },
        accepted,
    );
    p.send("notify", "unconfirmed before the drainer exited");
    let first = p.next();
    p._drainer.stop();
    {
        let mut h = p.history.lock().unwrap();
        h.hide_floor = true;
        h.status = Some("completed");
        h.hidden_page = true;
        h.client_ids.push(client_id(&first));
        h.calls.clear();
    }
    p._drainer = start_codex_drainer(&p.sandbox, p.sandbox.id("worker"), &p.home);
    p.quiet();
    assert_eq!(p.queued(), 1, "an incomplete replay scan cannot submit");
    p.history.lock().unwrap().hidden_page = false;
    let second = p.next();
    assert_eq!(client_id(&second), client_id(&first));
    {
        let mut h = p.history.lock().unwrap();
        assert!(h
            .calls
            .iter()
            .any(|req| req["params"]["turnId"] == "unrelated-head"));
        assert!(h
            .calls
            .iter()
            .all(|req| req["params"]["cursor"] != "old-turns"));
        h.hide_floor = false;
        h.status = Some("inProgress");
    }
    p.until_queued(0);
    p.quiet();
}

#[test]
fn ambiguous_submission_retries_only_after_idle_and_complete_absence() {
    let attempts = AtomicUsize::new(0);
    let p = Probe::new(move |req| {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Value::Null
        } else {
            accepted(req)
        }
    });
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "transport lost before admission");
    let first = p.next();
    p.quiet();
    {
        let mut h = p.history.lock().unwrap();
        h.status = Some("interrupted");
        h.hidden_page = true;
    }
    p.quiet();
    p.history.lock().unwrap().hidden_page = false;
    let retry = p.next();
    assert_eq!(client_id(&retry), client_id(&first));
    p.history.lock().unwrap().client_ids.push(client_id(&retry));
    p.until_queued(0);
}

#[test]
fn interrupted_turn_can_retry_into_an_active_successor() {
    let p = Probe::new(accepted);
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "retry while the successor is running");
    let first = p.next();
    {
        let mut h = p.history.lock().unwrap();
        h.status = Some("interrupted");
        h.successor = true;
    }
    let retry = p.next();
    assert_eq!(retry["method"], "turn/steer");
    assert_eq!(retry["params"]["expectedTurnId"], "next-turn");
    assert_eq!(client_id(&retry), client_id(&first));
}

#[test]
fn completed_turn_can_be_confirmed_from_a_successor() {
    let p = Probe::new(accepted);
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "input inherited by the following turn");
    let first = p.next();
    {
        let mut h = p.history.lock().unwrap();
        h.status = Some("completed");
        h.successor = true;
        h.successor_receipt = Some(client_id(&first));
    }
    p.until_queued(0);
    p.quiet();
}

#[test]
fn codex_delivery_creates_no_database_file() {
    let p = Probe::new(accepted);
    let databases = || {
        std::fs::read_dir(&p.sandbox.root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| {
                matches!(
                    Path::new(name).extension().and_then(|ext| ext.to_str()),
                    Some("db" | "sqlite" | "sqlite3")
                )
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = databases();
    assert_eq!(before, ["state.db".into()].into_iter().collect());
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("broadcast", "deliver without a local database");
    let first = p.next();
    p.history.lock().unwrap().client_ids.push(client_id(&first));
    p.until_queued(0);
    assert_eq!(databases(), before);
}

#[test]
fn receipt_in_failed_turn_is_not_resubmitted_into_an_active_successor() {
    let p = Probe::new(accepted);
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "terminal status with a stored receipt");
    let first = p.next();
    {
        let mut h = p.history.lock().unwrap();
        h.status = Some("failed");
        h.successor = true;
        h.client_ids.push(client_id(&first));
    }
    p.until_queued(0);
    p.quiet();
}

#[test]
fn wrong_error_code_with_steer_mismatch_text_does_not_refresh_or_resubmit() {
    let p = Probe::new(|_| {
        json!({"error":{
            "code":-32603, "message":"expected active turn id `user-turn` but found `next-turn`"
        }})
    });
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "internal error must reconcile by ID");
    let first = p.next();
    p.quiet();
    assert_eq!(p.queued(), 1);
    p.history.lock().unwrap().client_ids.push(client_id(&first));
    p.until_queued(0);
}

#[test]
fn non_steerable_turn_keeps_the_same_connection_until_idle() {
    let p = Probe::new(|req| {
        if req["method"] == "turn/steer" {
            json!({"error":{"code":-32600,"message":"cannot steer a compact turn"}})
        } else {
            accepted(req)
        }
    });
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "compact is running");
    let steer = p.next();
    assert_eq!(steer["method"], "turn/steer");
    let connected = p.connections.load(Ordering::SeqCst);
    let next = p.next();
    assert_eq!(client_id(&next), client_id(&steer));
    assert_eq!(p.connections.load(Ordering::SeqCst), connected);
    p.history.lock().unwrap().status = Some("completed");
    let start = p.next();
    assert_eq!(start["method"], "turn/start");
    assert_eq!(client_id(&start), client_id(&steer));
    p.history.lock().unwrap().client_ids.push(client_id(&start));
    p.until_queued(0);
}

#[test]
fn unconfirmed_delivery_is_deduplicated_on_hub_reconnect() {
    let mut p = Probe::new(accepted);
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "waiting for a receipt when Hub restarts");
    let first = p.next();
    let mut hub = p.sandbox.daemon.take().unwrap();
    hub.kill().unwrap();
    hub.wait().unwrap();
    p.sandbox.start();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !peer_is_online(&p.sandbox, p.sandbox.id("worker")) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    p.send("broadcast", "new message after reconnect");
    let next = p.next();
    assert_ne!(client_id(&first), client_id(&next));
    p.history
        .lock()
        .unwrap()
        .client_ids
        .extend([client_id(&first), client_id(&next)]);
    p.until_queued(0);
    p.quiet();
}

#[test]
fn a_pending_receipt_does_not_follow_a_replaced_owner() {
    let p = Probe::new(accepted);
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "owned by original thread");
    let first = p.next();
    assert_eq!(first["params"]["threadId"], "th1");
    bind_session(&p.sandbox, "codex", p.sandbox.id("worker"), "th2");
    p.history.lock().unwrap().status = Some("interrupted");
    p.send("notify", "owned by the new thread");
    let next = p.next();
    assert_eq!(next["params"]["threadId"], "th2");
    assert_ne!(client_id(&first), client_id(&next));
    p.history.lock().unwrap().client_ids.push(client_id(&next));
    p.until_queued(0);
    p.quiet();
    p.send(
        "broadcast",
        "an earlier owner's pending receipt cannot block the next message",
    );
    let later = p.next();
    assert_eq!(later["params"]["threadId"], "th2");
    assert_ne!(client_id(&first), client_id(&later));
    p.history.lock().unwrap().client_ids.push(client_id(&later));
    p.until_queued(0);
}

fn worker_status(sandbox: &Sandbox) -> Value {
    sandbox.json(&["status"], None)["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|peer| peer["peer_id"] == sandbox.id("worker"))
        .unwrap()
        .clone()
}

#[test]
fn codex_ping_reports_accepted_but_not_uncertain_deliveries() {
    let p = Probe::new(|req| {
        if req["params"]["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("uncertain")
        {
            Value::Null
        } else {
            accepted(req)
        }
    });
    p.history.lock().unwrap().status = Some("inProgress");
    p.send("notify", "accepted");
    let first = p.next();
    p.send("broadcast", "uncertain");
    p.next();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let peer = worker_status(&p.sandbox);
        let delivery = &peer["delivery"];
        if !delivery["stuck"].is_null() {
            assert_eq!(peer["queued"], 2);
            assert_eq!(delivery["awaiting_confirmation"], 1);
            assert_eq!(delivery["stuck"]["count"], 1);
            break;
        }
        assert!(Instant::now() < deadline, "{peer}");
        std::thread::sleep(Duration::from_millis(50));
    }
    p.history.lock().unwrap().client_ids.push(client_id(&first));
    p.until_queued(1);
    let peer = worker_status(&p.sandbox);
    assert!(peer["delivery"]["awaiting_confirmation"].is_null());
    assert_eq!(peer["delivery"]["stuck"]["count"], 1);
}

#[tokio::test]
async fn codex_pending_report_is_scoped_to_inbox_socket_and_session() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let mut sandbox = Sandbox::new();
    sandbox.start();
    let worker = sandbox.id("worker");
    bind_session(&sandbox, "codex", worker, "th1");
    register_boss(&sandbox);
    sandbox.json(
        &["peer", "notify", worker, "pending", "--from-peer", "boss"],
        None,
    );
    let connect = || async {
        let (mut ws, _) = connect_async(format!("ws://{}/ws", sandbox.bind))
            .await
            .unwrap();
        ws.send(Message::Text(
            json!({"type":"connect","peer_id":worker,"recv":true,"auth_token":"cli-test-token"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let read =
            serde_json::from_str::<Value>(&ws.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap();
        assert_eq!(read["type"], "connected");
        let frame =
            serde_json::from_str::<Value>(&ws.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap();
        (ws, frame["id"].as_str().unwrap().to_string())
    };
    let (mut first, id) = connect().await;
    let ping = |owner: &str| json!({"type":"ping","session_id":owner,"awaiting_confirmation":[id,"foreign",id]});
    macro_rules! send_report {
        ($socket:expr, $data:expr) => {{
            $socket
                .send(Message::Text($data.to_string().into()))
                .await
                .unwrap();
            $socket.send(Message::Ping(vec![7].into())).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(Ok(frame)) = $socket.next().await {
                    if let Message::Text(ref text) = frame {
                        let value: Value = serde_json::from_str(text).unwrap();
                        assert_ne!(value["type"], "recv_confirmed");
                    }
                    if frame == Message::Pong(vec![7].into()) {
                        return;
                    }
                }
                panic!("ping fence lost");
            })
            .await
            .unwrap();
        }};
    }
    let count = || {
        worker_status(&sandbox)["delivery"]["awaiting_confirmation"]
            .as_u64()
            .unwrap_or(0)
    };
    send_report!(first, ping("th2"));
    assert_eq!(count(), 0, "wrong session");
    send_report!(first, ping("th1"));
    assert_eq!(count(), 1, "only the one owed record is held");
    let (mut second, replayed) = connect().await;
    assert_eq!(replayed, id);
    assert_eq!(count(), 0, "new generation clears old report");
    let old = ping("th1");
    let _ = first.send(Message::Text(old.to_string().into())).await;
    send_report!(second, ping("th2"));
    assert_eq!(count(), 0, "old socket cannot report for newer generation");
    send_report!(second, ping("th1"));
    assert_eq!(count(), 1, "current generation");
    send_report!(second, json!({"type":"ping"}));
    assert_eq!(count(), 0, "old ping shape remains valid");
    send_report!(
        second,
        json!({"type":"recv","id":id,"session_id":"th1","confirm":true})
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(250), second.next())
            .await
            .is_err()
    );
    assert_eq!(
        inbox_count(&sandbox, worker),
        0,
        "old recv shape remains valid"
    );
}

fn replay_after_first_beat(
    beat_secs: &str,
    driver: AppDriver,
    run: impl FnOnce(tokio::sync::mpsc::UnboundedSender<Value>, mpsc::Receiver<String>),
) {
    use axum::{
        extract::ws::{Message, WebSocketUpgrade},
        routing::{get, post},
        Router,
    };

    let sandbox = Sandbox::new();
    let worker = sandbox.id("worker");
    let home = sandbox.root.join("replay");
    let submissions =
        app_server_capture_driver(&home, &["th1"], Arc::default(), None, None, Some(driver));
    let (frames, queued) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let queued = Arc::new(Mutex::new(Some(queued)));
    let (ping, first_beat) = mpsc::channel();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let app = Router::new()
            .route(
                "/peers",
                post(move || async move { axum::Json(json!({"peer_id":worker})) }),
            )
            .route(
                "/ws",
                get(move |ws: WebSocketUpgrade| {
                    let queued = queued.lock().unwrap().take().unwrap();
                    let ping = ping.clone();
                    async move {
                        ws.on_upgrade(move |mut socket| async move {
                            let mut queued = queued;
                            let Some(Ok(Message::Text(connect))) = socket.recv().await else {
                                return;
                            };
                            let connect: Value = serde_json::from_str(&connect).unwrap();
                            assert_eq!(connect["mark_replay"], true);
                            let greeting = json!({"type":"connected","session_id":"th1"});
                            socket
                                .send(Message::Text(greeting.to_string().into()))
                                .await
                                .unwrap();
                            while let Some(Ok(Message::Text(raw))) = socket.recv().await {
                                let value: Value = serde_json::from_str(&raw).unwrap();
                                if value["type"] == "ping" {
                                    ping.send(()).unwrap();
                                    break;
                                }
                            }
                            loop {
                                tokio::select! {
                                    frame = queued.recv() => {
                                        let Some(frame) = frame else { break };
                                        if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
                                            break;
                                        }
                                    }
                                    incoming = socket.recv() => {
                                        if !matches!(incoming, Some(Ok(_))) {
                                            break;
                                        }
                                    }
                                }
                            }
                        })
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind(&sandbox.bind).await.unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }).abort_handle()
    });
    let _drainer = KillChild(Some(
        sandbox
            .command()
            .args(["hook", "ws", "--peer-id", worker, "--backend", "codex"])
            .env("CODEX_HOME", home)
            .env("AMESH_WS_BEAT_SECS", beat_secs)
            .spawn()
            .unwrap(),
    ));
    first_beat.recv_timeout(Duration::from_secs(5)).unwrap();
    run(frames, submissions);
    server.abort();
}

fn replay_frame(id: &str, replay: bool) -> Value {
    json!({"type":"notify","id":id,"message":id,"replay":replay})
}

#[test]
fn cold_replay_starts_lookup_after_the_first_beat() {
    let (scoped, lookup) = mpsc::channel();
    let driver = Arc::new(move |req: &Value| {
        if req["method"] == "thread/items/list" && req["params"]["turnId"] == "finished-turn" {
            scoped.send(()).unwrap();
        }
        None
    });
    replay_after_first_beat("60", driver, |frames, submissions| {
        frames.send(replay_frame("older", true)).unwrap();
        lookup
            .recv_timeout(Duration::from_secs(5))
            .expect("replay should look up its receipt before the next beat");
        let raw = submissions.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(turn_text(&raw).contains("older"), "{raw}");
    });
}

#[test]
fn cold_replay_keeps_new_messages_behind_an_incomplete_lookup() {
    let (scoped, lookup) = mpsc::channel();
    let retry = Arc::new(AtomicBool::new(false));
    let state = retry.clone();
    let driver = Arc::new(move |req: &Value| {
        if req["method"] == "thread/items/list"
            && req["params"]["turnId"] == "finished-turn"
            && !state.load(Ordering::SeqCst)
        {
            scoped.send(()).unwrap();
            return Some(json!({
                "error":{"code":-32603,"message":"item page temporarily unavailable"}
            }));
        }
        None
    });
    replay_after_first_beat("1", driver, |frames, submissions| {
        frames.send(replay_frame("older", true)).unwrap();
        frames.send(replay_frame("newer", false)).unwrap();
        assert!(
            submissions
                .recv_timeout(Duration::from_millis(800))
                .is_err(),
            "new frame passed the replay waiting for its item page"
        );
        lookup.recv_timeout(Duration::from_secs(5)).unwrap();
        retry.store(true, Ordering::SeqCst);
        frames.send(replay_frame("last", false)).unwrap();
        for expected in ["older", "newer", "last"] {
            let raw = submissions.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(turn_text(&raw).contains(expected), "{raw}");
        }
    });
}

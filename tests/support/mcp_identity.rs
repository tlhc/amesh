use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use tokio_tungstenite::tungstenite::Message;

fn identity_probe_case(case: &str, target: Option<&str>) {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    sandbox.json(
        &[
            "peer",
            "register",
            "--peer-id",
            "worker",
            "--name",
            "worker",
            "--backend",
            "codex",
            "--path",
            sandbox.root.to_str().unwrap(),
        ],
        None,
    );
    let home = PathBuf::from(format!("/tmp/ai-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(home.join("app-server-control")).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let listener = tokio::net::UnixListener::bind(home.join("app-server-control/app-server-control.sock")).unwrap();
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = calls.clone();
        let scenario = case.to_string();
        /* a sink is reopened once the stream names the session the probe bound */
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (captured, scenario) = (captured.clone(), scenario.clone());
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = ws.next().await {
                        let request: Value = serde_json::from_str(&text).unwrap();
                        if request.get("id").is_none() { continue; }
                        captured.lock().unwrap().push(request.clone());
                        let result = match request["method"].as_str().unwrap() {
                            "initialize" => json!({}),
                            "thread/loaded/list" if request["params"].get("cursor").is_none() => json!({"data":["other"],"nextCursor":"next"}),
                            "thread/loaded/list" => json!({"data":["target"],"nextCursor":null}),
                            "mcpServer/tool/call" => {
                                assert_eq!(request["params"]["server"], "amesh");
                                assert_eq!(request["params"]["tool"], "amesh_whoami");
                                assert_eq!(request["params"]["arguments"], json!({}));
                                let own = request["params"]["threadId"] == "target";
                                if scenario == "timeout" && !own { continue; }
                                if matches!(scenario.as_str(), "error" | "subagent" | "no-server") && !own {
                                    ws.send(Message::Text(json!({"id":request["id"],"error":{"code":if scenario == "subagent" { -32600 } else { -32603 },"message":if scenario == "subagent" { "direct app-server input is not allowed for multi-agent v2 sub-agents" } else if scenario == "no-server" { "unknown MCP server 'amesh'" } else { "MCP unavailable" }}}).to_string().into())).await.unwrap();
                                    continue;
                                }
                                let identity = match scenario.as_str() {
                                    "duplicate" => "worker",
                                    "missing" => "someone-else",
                                    "target-first" if !own => "worker",
                                    "target-first" => "someone-else",
                                    _ if own => "worker",
                                    _ => "worker-2",
                                };
                                json!({"content":[{"type":"text","text":identity}]})
                            }
                            "thread/read" => json!({"thread":{"status":{"type":"idle"}}}),
                            "turn/start" => json!({"turn":{"id":"turn"}}),
                            method => panic!("unexpected method {method}"),
                        };
                        if ws.send(Message::Text(json!({"id":request["id"],"result":result}).to_string().into())).await.is_err() { break; }
                    }
                });
            }
        });
        let log_path = home.join("hook.log");
        let log = fs::File::create(&log_path).unwrap();
        let mut hook = KillChild(Some(sandbox.command().args(["hook", "ws", "--peer-id", "worker", "--backend", "codex"])
            .env("CODEX_HOME", &home).stderr(Stdio::from(log)).spawn().unwrap()));
        sandbox.json(&["peer", "notify", "worker", "identity-probe-marker"], None);
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let peers = sandbox.json(&["peer", "list"], None);
            let peer = peers.as_array().unwrap().iter().find(|peer| peer["peer_id"] == "worker").unwrap();
            if let Some(target) = target {
                if peer["session_id"] == target && calls.lock().unwrap().iter().any(|r| r["method"] == "turn/start") { break; }
                assert!(Instant::now() < deadline, "identity was not bound: {peer}; log={}", fs::read_to_string(&log_path).unwrap());
            } else {
                assert_eq!(peer["session_id"], "", "uncertain identity was bound: {peer}");
                if fs::read_to_string(&log_path).unwrap().contains("no verified App Server thread for inject") { break; }
                assert!(Instant::now() < deadline, "probe did not return to FIFO waiting");
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        let requests = calls.lock().unwrap();
        assert_eq!(requests.iter().filter(|r| r["method"] == "mcpServer/tool/call").count(), 2, "identity probe branch was not exercised");
        if let Some(target) = target {
            let turn = requests.iter().find(|r| r["method"] == "turn/start").unwrap();
            assert_eq!(turn["params"]["threadId"], target);
            assert!(turn["params"]["input"][0]["text"].as_str().unwrap().contains("identity-probe-marker"));
        } else {
            let reason = match case {
                "duplicate" => "ambiguous MCP identity",
                "missing" => "no matching MCP identity",
                "error" => "incomplete MCP identity verification",
                "timeout" => "identity verification timed out",
                _ => unreachable!(),
            };
            assert!(fs::read_to_string(&log_path).unwrap().contains(reason), "expected refusal: {reason}");
            assert!(!requests.iter().any(|r| matches!(r["method"].as_str(),Some("thread/read" | "turn/start" | "turn/steer"))));
        }
        drop(requests);
        if let Some(mut child) = hook.0.take() { child.kill().unwrap(); child.wait().unwrap(); }
        server.abort();
    });
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn mcp_identity_binds_exact_peer_on_later_page_and_flushes() {
    identity_probe_case("normal", Some("target"));
}
#[test]
fn mcp_identity_binds_first_match_after_checking_all_threads() {
    identity_probe_case("target-first", Some("other"));
}
#[test]
fn mcp_identity_duplicate_keeps_queue() {
    identity_probe_case("duplicate", None);
}
#[test]
fn mcp_identity_missing_keeps_queue() {
    identity_probe_case("missing", None);
}
#[test]
fn mcp_identity_error_keeps_queue() {
    identity_probe_case("error", None);
}
#[test]
fn mcp_identity_timeout_keeps_queue() {
    identity_probe_case("timeout", None);
}

#[test]
fn mcp_identity_skips_threads_that_reject_direct_input() {
    identity_probe_case("subagent", Some("target"));
}

#[test]
fn mcp_identity_skips_threads_without_amesh_server() {
    identity_probe_case("no-server", Some("target"));
}

/* the MCP under test, driven over its own stdio the way Codex drives it */
struct Mcp {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Mcp {
    fn start(sandbox: &Sandbox, home: &Path, args: &[&str]) -> Self {
        let mut child = sandbox
            .command()
            .arg("mcp")
            .args(args)
            .env("AMESH_BACKEND", "codex")
            .env("CODEX_HOME", home)
            .stderr(Stdio::from(fs::File::create(home.join("mcp.log")).unwrap()))
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
        }
    }

    fn call(&mut self, request: Value) -> Value {
        writeln!(self.stdin.as_mut().unwrap(), "{request}").unwrap();
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP must answer");
        serde_json::from_str(&line).unwrap()
    }

    /* closing stdin is how Codex stops it, and it takes its drainer down with it */
    fn close(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    /* the App Server relays the probe for "target" to the MCP under test */
    Relayed,
    /* "target" is a sub-agent, so the App Server refuses direct input for it */
    Refused,
    /* "target" is not loaded */
    Hidden,
    /* the probe for "target" is relayed, but thread "other" never answers its own */
    Stalled,
    /* as Stalled, from an App Server that stamps no _meta.threadId */
    StalledNoMeta,
}

/* an App Server with threads "other" and "target"; like the real one it stamps
_meta.threadId on a call it relays to a thread's MCP server, unless it is too old to */
async fn app_server(
    listener: tokio::net::UnixListener,
    mcp: Arc<Mutex<Mcp>>,
    target: Target,
    probes: Arc<Mutex<Vec<(Value, Value)>>>,
) {
    while let Ok((stream, _)) = listener.accept().await {
        let (mcp, probes) = (mcp.clone(), probes.clone());
        tokio::spawn(async move {
            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                let request: Value = serde_json::from_str(&text).unwrap();
                if request.get("id").is_none() {
                    continue;
                }
                let params = &request["params"];
                let own = params["threadId"] == "target";
                let mut reply = match request["method"].as_str().unwrap() {
                    "thread/loaded/list" if target == Target::Hidden => {
                        json!({"result": {"data": ["other"]}})
                    }
                    "thread/loaded/list" => json!({"result": {"data": ["other", "target"]}}),
                    "mcpServer/tool/call" if own && target == Target::Refused => {
                        json!({"error": {"code": -32600, "message": "direct app-server input is not allowed for multi-agent v2 sub-agents"}})
                    }
                    "mcpServer/tool/call" if own => {
                        let mut call = json!({"jsonrpc": "2.0", "id": 900, "method": "tools/call", "params": {
                            "name": params["tool"], "arguments": params["arguments"], "_meta": {"threadId": "target"}}});
                        if target == Target::StalledNoMeta {
                            call["params"].as_object_mut().unwrap().remove("_meta");
                        }
                        let answer = tokio::task::block_in_place(|| mcp.lock().unwrap().call(call));
                        probes
                            .lock()
                            .unwrap()
                            .push((params["arguments"].clone(), answer.clone()));
                        json!({"result": answer["result"]})
                    }
                    "mcpServer/tool/call"
                        if matches!(target, Target::Stalled | Target::StalledNoMeta) =>
                    {
                        continue
                    }
                    "mcpServer/tool/call" => {
                        json!({"result": {"content": [{"type": "text", "text": "someone-else"}]}})
                    }
                    _ => json!({"result": {}}),
                };
                reply["id"] = request["id"].clone();
                if ws
                    .send(Message::Text(reply.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
}

/* the second a Codex spawns the new MCP for thread "target" while the App Server is up and
the directory's name is still held: by the previous run of "target" when it resumes, or by
another thread when "previous" names one */
struct Resume {
    sandbox: Sandbox,
    expected: String,
    home: PathBuf,
    mcp: Arc<Mutex<Mcp>>,
    probes: Arc<Mutex<Vec<(Value, Value)>>>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Resume {
    fn start(target: Target, previous: &str) -> Self {
        let mut sandbox = Sandbox::new();
        sandbox.start();
        sandbox.json(
            &["hook", "session", "--backend", "codex"],
            Some(json!({"session_id": previous, "cwd": sandbox.root})),
        );
        let expected = format!(
            "{}-codex",
            sandbox.root.file_name().unwrap().to_string_lossy()
        );
        let home = PathBuf::from(format!("/tmp/ai-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(home.join("app-server-control")).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let socket = home.join("app-server-control/app-server-control.sock");
        let listener = runtime
            .block_on(async { tokio::net::UnixListener::bind(socket) })
            .unwrap();
        let mcp = Arc::new(Mutex::new(Mcp::start(&sandbox, &home, &[])));
        let probes = Arc::new(Mutex::new(Vec::new()));
        runtime.spawn(app_server(listener, mcp.clone(), target, probes.clone()));
        Self {
            sandbox,
            expected,
            home,
            mcp,
            probes,
            runtime: Some(runtime),
        }
    }

    fn wait_for_drainer(&self, peer_id: &str, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if hook_ws_alive(peer_id) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn rows(&self) -> Vec<(String, String)> {
        self.sandbox
            .json(&["peer", "list"], None)
            .as_array()
            .unwrap()
            .iter()
            .filter(|peer| peer["backend"] == "codex")
            .map(|peer| {
                (
                    peer["peer_id"].as_str().unwrap().into(),
                    peer["session_id"].as_str().unwrap().into(),
                )
            })
            .collect()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.home.join("mcp.log")).unwrap_or_default()
    }
}

impl Drop for Resume {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
        self.mcp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .close();
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn whoami(meta: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "amesh_whoami", "arguments": {}, "_meta": meta}})
}

#[test]
fn codex_mcp_takes_back_its_thread_name_after_resume() {
    let resume = Resume::start(Target::Relayed, "target");
    assert!(
        resume.wait_for_drainer(&resume.expected, 10),
        "the resumed thread must come back as {}; rows={:?} log={}",
        resume.expected,
        resume.rows(),
        resume.log()
    );
    assert_eq!(
        resume.rows(),
        vec![(resume.expected.clone(), "target".to_string())]
    );
    let probes = resume.probes.lock().unwrap().clone();
    let (arguments, answer) = probes
        .first()
        .expect("the App Server must relay the bind probe");
    let nonce = arguments["bind"]
        .as_str()
        .expect("the probe carries its nonce");
    assert_eq!(
        answer["result"]["content"][0]["text"], nonce,
        "before it has a name the MCP answers its own probe with the nonce"
    );
    let who = resume.mcp.lock().unwrap().call(whoami(json!({})));
    assert_eq!(
        who["result"]["content"][0]["text"],
        resume.expected.as_str(),
        "{who}"
    );
}

#[test]
fn codex_mcp_binds_the_thread_named_by_a_tool_call() {
    let resume = Resume::start(Target::Refused, "target");
    let who = resume
        .mcp
        .lock()
        .unwrap()
        .call(whoami(json!({"threadId": "target"})));
    assert_eq!(
        who["result"]["content"][0]["text"],
        resume.expected.as_str(),
        "{who}"
    );
    assert!(
        resume.wait_for_drainer(&resume.expected, 4),
        "log={}",
        resume.log()
    );
    assert_eq!(
        resume.rows(),
        vec![(resume.expected.clone(), "target".to_string())]
    );
    assert!(
        resume.probes.lock().unwrap().is_empty(),
        "a refused thread is never relayed"
    );
}

/* waits out the probe window: past it, a process with no thread would have to register */
fn after_the_probe_window(resume: &Resume) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !resume.log().contains("waiting for the first tool call") {
        assert!(Instant::now() < deadline, "log={}", resume.log());
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn codex_mcp_without_a_visible_thread_waits_for_its_first_tool_call() {
    let resume = Resume::start(Target::Hidden, "target");
    after_the_probe_window(&resume);
    assert_eq!(
        resume.rows(),
        vec![(resume.expected.clone(), "target".to_string())],
        "no probe answered, and nothing may register without the thread"
    );
    let who = resume
        .mcp
        .lock()
        .unwrap()
        .call(whoami(json!({"threadId": "target"})));
    assert_eq!(
        who["result"]["content"][0]["text"],
        resume.expected.as_str(),
        "{who}"
    );
    assert!(
        resume.wait_for_drainer(&resume.expected, 4),
        "log={}",
        resume.log()
    );
    assert_eq!(
        resume.rows(),
        vec![(resume.expected.clone(), "target".to_string())]
    );
}

#[test]
fn codex_mcp_registers_without_a_thread_only_for_a_call_that_names_none() {
    let resume = Resume::start(Target::Hidden, "target");
    let fallback = format!("{}-2", resume.expected);
    /* a Codex too old to stamp _meta: the call must act as a peer of its own, since the
    directory's name belongs to the other row, but only once no probe can still bind */
    let bare = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "amesh_whoami", "arguments": {}}});
    let early = resume.mcp.lock().unwrap().call(bare.clone());
    assert_eq!(early["error"]["code"], -32000, "{early}");
    after_the_probe_window(&resume);
    let who = resume.mcp.lock().unwrap().call(bare);
    assert_eq!(
        who["result"]["content"][0]["text"],
        fallback.as_str(),
        "{who}"
    );
    assert!(
        resume.wait_for_drainer(&fallback, 4),
        "log={}",
        resume.log()
    );
    let mut rows = resume.rows();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (resume.expected.clone(), "target".to_string()),
            (fallback, String::new())
        ]
    );
}

#[test]
fn codex_hook_and_mcp_meet_on_the_session_when_the_probe_cannot_bind() {
    let resume = Resume::start(Target::Hidden, "another");
    let own = format!("{}-2", resume.expected);
    after_the_probe_window(&resume);
    /* the thread's own hook runs first, beside an App Server, and names its session */
    let root = resume.sandbox.root.canonicalize().unwrap();
    let output = resume.sandbox.run_text(
        &["hook", "session", "--backend", "codex"],
        &json!({"session_id": "target", "cwd": root}).to_string(),
        &[("CODEX_HOME", resume.home.to_str().unwrap())],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let start: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = start["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains(&format!("you are {own} in circle")),
        "{context}"
    );
    /* the MCP learns the thread from its first tool call and lands on the hook's row */
    let who = resume
        .mcp
        .lock()
        .unwrap()
        .call(whoami(json!({"threadId": "target"})));
    assert_eq!(who["result"]["content"][0]["text"], own.as_str(), "{who}");
    assert!(resume.wait_for_drainer(&own, 4), "log={}", resume.log());
    let mut rows = resume.rows();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (resume.expected.clone(), "another".to_string()),
            (own, "target".to_string())
        ],
        "one row per thread"
    );
}

#[test]
fn codex_mcp_binds_from_its_own_probe_while_another_thread_stalls() {
    let resume = Resume::start(Target::Stalled, "target");
    /* the relayed probe for "target" carries its thread in _meta and binds through it,
    while "other" never answers; the StalledNoMeta case is what needs the probe's own
    first match */
    assert!(
        resume.wait_for_drainer(&resume.expected, 4),
        "rows={:?} log={}",
        resume.rows(),
        resume.log()
    );
    assert_eq!(
        resume.rows(),
        vec![(resume.expected.clone(), "target".to_string())]
    );
}

#[test]
fn codex_session_start_leaves_another_threads_name_alone() {
    let resume = Resume::start(Target::Refused, "another");
    let own = format!("{}-2", resume.expected);
    /* SessionStart of "target" lands while its MCP has not bound yet, and the only live
    peer in the directory belongs to thread "another" */
    resume.sandbox.json(
        &["hook", "session", "--backend", "codex"],
        Some(json!({"session_id": "target", "cwd": resume.sandbox.root})),
    );
    let mut rows = resume.rows();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (resume.expected.clone(), "another".to_string()),
            (own.clone(), "target".to_string())
        ],
        "another thread's name must stay with it"
    );
    let who = resume
        .mcp
        .lock()
        .unwrap()
        .call(whoami(json!({"threadId": "target"})));
    assert_eq!(who["result"]["content"][0]["text"], own.as_str(), "{who}");
    assert!(resume.wait_for_drainer(&own, 4), "log={}", resume.log());
}

#[test]
fn codex_mcp_binds_by_its_nonce_without_meta_while_another_thread_stalls() {
    let resume = Resume::start(Target::StalledNoMeta, "target");
    /* no _meta to bind from: only the nonce answer names the thread, and it must not wait
    for "other", which never answers */
    assert!(
        resume.wait_for_drainer(&resume.expected, 4),
        "rows={:?} log={}",
        resume.rows(),
        resume.log()
    );
    assert_eq!(
        resume.rows(),
        vec![(resume.expected.clone(), "target".to_string())]
    );
}

/* the previous run of a pinned thread: its row still carries session A and an open ask */
fn pinned_after_a_previous_run() -> (Sandbox, PathBuf) {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    pin_session(&sandbox, "A");
    sandbox.json(&["peer", "ask", "pinned", "for A only"], None);
    let home = PathBuf::from(format!("/tmp/ap-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&home).unwrap();
    (sandbox, home)
}

#[test]
fn codex_pinned_mcp_names_its_thread_before_it_registers_or_drains() {
    let (sandbox, home) = pinned_after_a_previous_run();
    let mut mcp = Mcp::start(&sandbox, &home, &["--peer-id", "pinned"]);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !hook_ws_alive("pinned"),
        "no drainer may pick up A's backlog before this thread is known"
    );
    assert_eq!(row_session(&sandbox, "pinned"), "A");
    let who = mcp.call(whoami(json!({"threadId": "B"})));
    assert_eq!(who["result"]["content"][0]["text"], "pinned", "{who}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !hook_ws_alive("pinned") {
        assert!(
            Instant::now() < deadline,
            "the drainer starts once B registers"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(row_session(&sandbox, "pinned"), "B");
    let asks = sandbox.json(&["peer", "asks", "--peer-id", "pinned"], None);
    assert!(
        asks["asks"].as_array().unwrap().is_empty(),
        "A's ask is closed, not handed to B: {asks}"
    );
    mcp.close();
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn codex_pinned_mcp_without_a_thread_speaks_for_its_pin_unregistered() {
    let (sandbox, home) = pinned_after_a_previous_run();
    /* a second Codex row in the folder, so only the pin can say who is speaking */
    sandbox.json(
        &[
            "peer",
            "register",
            "--peer-id",
            "bystander",
            "--name",
            "bystander",
            "--backend",
            "codex",
            "--path",
            sandbox.root.to_str().unwrap(),
        ],
        None,
    );
    let mut mcp = Mcp::start(&sandbox, &home, &["--peer-id", "pinned"]);
    let who = mcp.call(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "amesh_whoami", "arguments": {}}}));
    assert_eq!(who["result"]["content"][0]["text"], "pinned", "{who}");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !hook_ws_alive("pinned"),
        "a pin with no thread never drains"
    );
    assert_eq!(row_session(&sandbox, "pinned"), "A", "nor moves the row");
    let asks = sandbox.json(&["peer", "asks", "--peer-id", "pinned"], None);
    assert_eq!(asks["asks"][0]["text"], "for A only", "{asks}");
    mcp.close();
    let _ = fs::remove_dir_all(&home);
}

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
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
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

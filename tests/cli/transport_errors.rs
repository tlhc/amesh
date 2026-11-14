use super::*;

#[derive(Clone, Copy)]
enum ResponseLoss {
    Empty,
    Timeout,
    Truncated,
    TruncatedThenReply,
}

struct LostResponse {
    bind: String,
    writes: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LostResponse {
    fn start(loss: ResponseLoss) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let bind = listener.local_addr().unwrap().to_string();
        listener.set_nonblocking(true).unwrap();
        let writes = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let writes = writes.clone();
            let stop = stop.clone();
            move || {
                while !stop.load(Ordering::SeqCst) {
                    let (stream, _) = match listener.accept() {
                        Ok(pair) => pair,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(e) => panic!("{e}"),
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(stream);
                    let mut first = String::new();
                    if reader.read_line(&mut first).unwrap() == 0 {
                        continue;
                    }
                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap() == 0 {
                            break;
                        }
                        if header == "\r\n" {
                            break;
                        }
                        if let Some((key, value)) = header.split_once(':') {
                            if key.eq_ignore_ascii_case("content-length") {
                                length = value.trim().parse().unwrap();
                            }
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    if first.starts_with("GET /health ") {
                        let body = r#"{"ok":true,"name":"amesh"}"#;
                        write!(
                            reader.get_mut(),
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .unwrap();
                    } else {
                        let count = writes.fetch_add(1, Ordering::SeqCst) + 1;
                        match loss {
                            ResponseLoss::Timeout => {
                                while !stop.load(Ordering::SeqCst) {
                                    std::thread::sleep(Duration::from_millis(10));
                                }
                            }
                            ResponseLoss::TruncatedThenReply if count > 1 => {
                                let body =
                                    r#"{"correlation_id":"ask-1","open":false,"reply":"pong"}"#;
                                write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                            }
                            ResponseLoss::Truncated | ResponseLoss::TruncatedThenReply => {
                                write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{}}").unwrap();
                            }
                            ResponseLoss::Empty => {}
                        }
                    }
                }
            }
        });
        Self {
            bind,
            writes,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for LostResponse {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn lost_ack(loss: ResponseLoss) {
    let sandbox = Sandbox::new();
    let hub = LostResponse::start(loss);
    let output = sandbox.run_text(
        &["peer", "ack", "ask-1", "--message", "answer"],
        "",
        &[("AMESH_BIND", &hub.bind)],
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{output:?}");
    assert!(error.contains("amesh hub response incomplete"), "{error}");
    assert!(error.contains("outcome unknown"), "{error}");
    assert!(error.contains("check state before retrying"), "{error}");
    assert!(error.contains("POST /ack"), "{error}");
    assert!(
        error.contains(match loss {
            ResponseLoss::Empty => "curl: (52)",
            ResponseLoss::Timeout => "curl: (28)",
            _ => "curl: (18)",
        }),
        "{error}"
    );
    assert_eq!(
        hub.writes.load(Ordering::SeqCst),
        1,
        "write is never automatically retried"
    );
}
#[test]
fn completed_write_with_lost_response_is_unknown() {
    lost_ack(ResponseLoss::Empty);
}
#[test]
fn completed_write_with_timeout_is_unknown() {
    lost_ack(ResponseLoss::Timeout);
}

#[test]
fn hook_transport_loss_stays_successful() {
    let sandbox = Sandbox::new();
    let hub = LostResponse::start(ResponseLoss::Empty);
    let output = sandbox.run_text(
        &["hook", "prompt", "--backend", "pi"],
        &json!({"session_id":"test-session", "cwd":sandbox.root}).to_string(),
        &[("AMESH_BIND", &hub.bind)],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(hub.writes.load(Ordering::SeqCst), 1);
}

#[test]
fn session_registration_lost_response_explains_unknown_outcome() {
    let sandbox = Sandbox::new();
    let hub = LostResponse::start(ResponseLoss::Empty);
    let output = sandbox.run_text(
        &["hook", "session", "--backend", "claude-code"],
        &json!({"session_id":"test-session", "cwd":sandbox.root}).to_string(),
        &[("AMESH_BIND", &hub.bind)],
    );
    assert!(output.status.success(), "{output:?}");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = body["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("may already be registered"), "{text}");
    assert!(text.contains("amesh peer list"), "{text}");
    assert_eq!(hub.writes.load(Ordering::SeqCst), 1);
}

macro_rules! curl_exit_case {
    ($name:ident, $code:literal, $prefix:literal) => {
        #[test]
        fn $name() {
            use std::os::unix::fs::PermissionsExt;
            let sandbox = Sandbox::new();
            let curl = sandbox.root.join("curl");
            fs::write(
                &curl,
                format!(
                    "#!/bin/sh\nwhile IFS= read -r line; do :; done\nprintf 'curl: ({}) injected detail\\n' >&2\nexit {}\n",
                    $code, $code
                ),
            )
            .unwrap();
            fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
            let output = sandbox.run_text(
                &["peer", "ack", "ask-1", "--message", "answer"],
                "",
                &[("PATH", sandbox.root.to_str().unwrap())],
            );
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success());
            assert!(error.contains($prefix), "{error}");
            assert!(
                error.contains(&format!("curl: ({}) injected detail", $code)),
                "{error}"
            );
            assert!(error.contains("POST /ack"), "{error}");
            if matches!($code, 6 | 7) {
                assert!(error.contains("amesh doctor"), "{error}");
                assert!(!error.contains("outcome unknown"), "{error}");
            }
        }
    };
}
curl_exit_case!(
    curl_dns_failure_keeps_connect_diagnostic,
    6,
    "amesh daemon unreachable"
);
curl_exit_case!(
    curl_refusal_keeps_connect_diagnostic,
    7,
    "amesh daemon unreachable"
);
curl_exit_case!(curl_timeout_is_unknown, 28, "amesh hub response incomplete");
curl_exit_case!(
    curl_empty_response_is_unknown,
    52,
    "amesh hub response incomplete"
);
curl_exit_case!(
    curl_send_failure_is_unknown,
    55,
    "amesh hub response incomplete"
);
curl_exit_case!(
    curl_receive_failure_is_unknown,
    56,
    "amesh hub response incomplete"
);

#[test]
fn completed_write_with_truncated_response_is_unknown() {
    lost_ack(ResponseLoss::Truncated);
}

#[test]
fn hook_truncated_response_stays_successful() {
    let sandbox = Sandbox::new();
    let hub = LostResponse::start(ResponseLoss::Truncated);
    let output = sandbox.run_text(
        &["hook", "prompt", "--backend", "pi"],
        &json!({"session_id":"test-session", "cwd":sandbox.root}).to_string(),
        &[("AMESH_BIND", &hub.bind)],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(hub.writes.load(Ordering::SeqCst), 1);
}

#[test]
fn wait_recovers_after_truncated_response() {
    let sandbox = Sandbox::new();
    let hub = LostResponse::start(ResponseLoss::TruncatedThenReply);
    let output = sandbox.run_text(
        &["peer", "wait", "ask-1", "--timeout-seconds", "8"],
        "",
        &[("AMESH_BIND", &hub.bind)],
    );
    assert!(output.status.success(), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["reply"], "pong");
    assert_eq!(
        hub.writes.load(Ordering::SeqCst),
        2,
        "one truncated request followed by one successful wait"
    );
}

curl_exit_case!(
    curl_partial_response_is_unknown,
    18,
    "amesh hub response incomplete"
);

use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{protocol::Role, Message};

async fn select(pages: Vec<Value>, trusted: Option<&str>) -> (Result<String>, Vec<Value>) {
    let (a, b) = tokio::net::UnixStream::pair().unwrap();
    let mut client = AppWs::from_raw_socket(a, Role::Client, None).await;
    let mut server = AppWs::from_raw_socket(b, Role::Server, None).await;
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut pages = pages.into_iter();
        while let Some(Ok(Message::Text(text))) = server.next().await {
            let request: Value = serde_json::from_str(&text).unwrap();
            let result = if request["method"] == "thread/loaded/list" {
                pages
                    .next()
                    .unwrap_or(json!({"error":{"message":"unexpected page"}}))
            } else {
                json!({"result":{"thread":{"cwd":"/same", "threadSource":"user", "updatedAt":10}}})
            };
            let mut response = result;
            response["id"] = request["id"].clone();
            requests.push(request);
            if server
                .send(Message::Text(response.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
        requests
    });
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        select_app_thread(&mut client, &mut 1, trusted),
    )
    .await;
    drop(client);
    let requests = task.await.unwrap();
    (result.expect("routing must terminate"), requests)
}

#[tokio::test]
async fn routing_target_on_second_page_without_reading_other_threads() {
    let (result, requests) = select(
        vec![
            json!({"result":{"data":["other"],"nextCursor":"p2"}}),
            json!({"result":{"data":["target"],"nextCursor":null}}),
        ],
        Some("target"),
    )
    .await;
    assert_eq!(result.unwrap(), "target");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["params"]["cursor"], "p2");
    assert!(requests.iter().all(|r| r["method"] == "thread/loaded/list"));
}

#[tokio::test]
async fn routing_missing_trusted_id_does_not_select_same_cwd() {
    let (result, _) = select(vec![json!({"result":{"data":["other"]}})], Some("target")).await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("trusted thread target is not loaded"));
}

#[tokio::test]
async fn routing_sessionless_makes_no_app_requests() {
    let (result, requests) = select(vec![json!({"result":{"data":["other"]}})], None).await;
    assert!(result.is_err(), "unbound peer must wait for identity");
    assert!(
        requests.is_empty(),
        "unbound peer must not query App Server: {requests:?}"
    );
}

#[tokio::test]
async fn routing_bad_second_page_rejects_partial_match() {
    let (result, _) = select(
        vec![
            json!({"result":{"data":["target"],"nextCursor":"p2"}}),
            json!({"result":{"data":[42]}}),
        ],
        Some("target"),
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("invalid thread id"));
}

#[tokio::test]
async fn routing_repeated_cursor_terminates() {
    let (result, requests) = select(
        vec![
            json!({"result":{"data":["target"],"nextCursor":"p2"}}),
            json!({"result":{"data":[],"nextCursor":"p2"}}),
        ],
        Some("target"),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("repeated cursor"));
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn routing_failed_page_retries_with_target_later_present() {
    let (first, _) = select(
        vec![json!({"error":{"message":"unavailable"}})],
        Some("target"),
    )
    .await;
    assert!(first.is_err());
    let (second, _) = select(vec![json!({"result":{"data":["target"]}})], Some("target")).await;
    assert_eq!(second.unwrap(), "target");
}

#[test]
fn routing_peer_session_requires_an_exact_valid_row() {
    assert_eq!(
        session_from_peers(&json!([{"peer_id":"own","session_id":""}]), "own").unwrap(),
        None
    );
    assert_eq!(
        session_from_peers(&json!([{"peer_id":"own","session_id":"target"}]), "own")
            .unwrap()
            .as_deref(),
        Some("target")
    );
    for value in [
        json!([]),
        json!({}),
        json!([{"peer_id":"other","session_id":""}]),
        json!([{"peer_id":"own"}]),
        json!([{"peer_id":"own","session_id":42}]),
        json!([{"peer_id":"own","session_id":null}]),
    ] {
        assert!(
            session_from_peers(&value, "own").is_err(),
            "invalid identity: {value}"
        );
    }
}

#[test]
fn routing_session_lookup_subprocess() {
    let Ok(case) = std::env::var("AMESH_ROUTING_CASE") else {
        return;
    };
    let root = PathBuf::from("/tmp").join(format!("ar-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("app-server-control")).unwrap();
    std::env::set_var("CODEX_HOME", &root);
    std::env::remove_var("CODEX_THREAD_ID");
    std::env::remove_var("AMESH_TOKEN");
    let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let bind = http.local_addr().unwrap();
    std::env::set_var("AMESH_BIND", bind.to_string());
    if case == "row-over-env" {
        std::env::set_var("CODEX_THREAD_ID", "other");
    }
    let body = match case.as_str() {
        "missing" => json!([]),
        "invalid" => json!([{"peer_id":"own","session_id":42}]),
        "empty" => json!([{"peer_id":"own","session_id":""}]),
        _ => json!([{"peer_id":"own","session_id":"target"}]),
    };
    let http_task = if case == "env" || case == "stream" {
        std::env::set_var("AMESH_BIND", "invalid-bind");
        if case == "env" {
            std::env::set_var("CODEX_THREAD_ID", "target");
        }
        None
    } else {
        let status = if case == "query-failure" {
            "503 Unavailable"
        } else {
            "200 OK"
        };
        Some(std::thread::spawn(move || {
            let (mut socket, _) = http.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer).unwrap();
            let body = body.to_string();
            write!(
                socket,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }))
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let outcome = runtime.block_on(async {
        let listener = tokio::net::UnixListener::bind(crate::bridge::app_server_socket()).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                let request: Value = serde_json::from_str(&text).unwrap();
                if request.get("id").is_none() {
                    continue;
                }
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => json!({}),
                    "thread/loaded/list" => json!({"data":["other","target"]}),
                    "thread/read" => {
                        json!({"thread":{"cwd":"/same","threadSource":"user","updatedAt":1}})
                    }
                    method => panic!("unexpected method {method}"),
                };
                if ws
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let stream = (case == "stream").then_some("target");
        let outcome = tokio::time::timeout(Duration::from_secs(3), app_connect("own", stream))
            .await
            .unwrap();
        server.abort();
        outcome.map(|sink| sink.thread_id)
    });
    if let Some(task) = http_task {
        task.join().unwrap();
    }
    fs::remove_dir_all(root).unwrap();
    match case.as_str() {
        "empty" => assert!(
            outcome.is_none(),
            "confirmed empty session must wait for binding"
        ),
        "env" | "bound" | "stream" | "row-over-env" => {
            assert_eq!(outcome.as_deref(), Some("target"))
        }
        _ => assert!(
            outcome.is_none(),
            "identity failure must not select by cwd: {outcome:?}"
        ),
    }
}

fn identity_case(case: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli::thread_routing_tests::routing_session_lookup_subprocess",
            "--nocapture",
        ])
        .env("AMESH_ROUTING_CASE", case)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("identity case {case} timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "identity case {case}");
}
#[test]
fn routing_query_failure_never_falls_back() {
    identity_case("query-failure");
}
#[test]
fn routing_missing_peer_never_falls_back() {
    identity_case("missing");
}
#[test]
fn routing_invalid_session_never_falls_back() {
    identity_case("invalid");
}
#[test]
fn routing_confirmed_empty_session_waits_for_binding() {
    identity_case("empty");
}
#[test]
fn routing_env_identity_bypasses_failed_hub() {
    identity_case("env");
}
#[test]
fn routing_row_session_outranks_the_thread_env() {
    identity_case("row-over-env");
}

#[test]
fn routing_stream_session_needs_no_hub() {
    identity_case("stream");
}

#[test]
fn routing_hub_identity_selects_exact_target() {
    identity_case("bound");
}

#[tokio::test]
async fn routing_invalid_data_shape_is_rejected() {
    let (result, _) = select(vec![json!({"result":{"data":{}}})], Some("target")).await;
    assert!(result.unwrap_err().to_string().contains("invalid data"));
}

#[tokio::test]
async fn routing_invalid_cursor_type_is_rejected() {
    let (result, _) = select(
        vec![json!({"result":{"data":["target"],"nextCursor":42}})],
        Some("target"),
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("invalid or repeated cursor"));
}

#[tokio::test]
async fn routing_cursor_cycle_terminates() {
    let (result, requests) = select(
        vec![
            json!({"result":{"data":[],"nextCursor":"a"}}),
            json!({"result":{"data":[],"nextCursor":"b"}}),
            json!({"result":{"data":[],"nextCursor":"a"}}),
        ],
        Some("target"),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("repeated cursor"));
    assert_eq!(requests.len(), 3);
}

use super::*;
use axum::{
    extract::{ws::Message, State, WebSocketUpgrade},
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio_tungstenite::tungstenite::Message as AppMessage;

async fn next<T>(rx: &mut UnboundedReceiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(25), rx.recv())
        .await
        .expect("routing progress timed out")
        .unwrap()
}

#[test]
fn hook_ws_waits_for_identity_then_flushes_fifo_and_restarts_with_new_binding() {
    let mut sandbox = Sandbox::new();
    let root = PathBuf::from(format!("/tmp/amesh-cli-{}", uuid::Uuid::new_v4().simple()));
    fs::rename(&sandbox.root, &root).unwrap();
    sandbox.root = root;
    let home = sandbox.root.join("cx");
    fs::create_dir_all(home.join("app-server-control")).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(45), async {
        let binding = Arc::new(Mutex::new(String::new()));
        let generation = Arc::new(AtomicUsize::new(0));
        let connections = Arc::new(AtomicUsize::new(0));
        let (receipt_tx, mut receipts) = unbounded_channel();
        let state = (binding.clone(), generation, receipt_tx);
        let hub = Router::new()
            .route("/health", get(|| async { axum::Json(json!({"ok":true,"name":"amesh"})) }))
            .route("/peers", post(|| async { axum::Json(json!({"peer_id":"worker"})) })
                .get(|State((binding, _, _)): State<(Arc<Mutex<String>>, Arc<AtomicUsize>, tokio::sync::mpsc::UnboundedSender<Value>)>| async move {
                    axum::Json(json!([{"peer_id":"worker","session_id":binding.lock().unwrap().clone()}]))
                }))
            .route("/ws", get(|ws: WebSocketUpgrade, State((_, generation, receipts)): State<(Arc<Mutex<String>>, Arc<AtomicUsize>, tokio::sync::mpsc::UnboundedSender<Value>)>| async move {
                ws.on_upgrade(move |mut socket| async move {
                    let Some(Ok(_)) = socket.recv().await else { return; };
                    let index = generation.fetch_add(1, Ordering::SeqCst);
                    let messages = if index == 0 { vec!["q0", "q1"] } else { vec!["q2"] };
                    for text in messages {
                        socket.send(Message::Text(json!({"type":"notify","id":text,"from_peer":"boss","text":text}).to_string().into())).await.unwrap();
                    }
                    while let Some(Ok(Message::Text(text))) = socket.recv().await {
                        let value: Value = serde_json::from_str(&text).unwrap();
                        if value["type"] == "recv" { let _ = receipts.send(value); }
                    }
                })
            })).with_state(state);
        let listener = tokio::net::TcpListener::bind(&sandbox.bind).await.unwrap();
        let hub_task = tokio::spawn(async move { axum::serve(listener, hub).await.unwrap(); });
        let listener = tokio::net::UnixListener::bind(home.join("app-server-control/app-server-control.sock")).unwrap();
        let (request_tx, mut requests) = unbounded_channel();
        let app_connections = connections.clone();
        let cwd = sandbox.root.to_string_lossy().into_owned();
        let app_task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    stream = listener.accept() => {
                        let (stream, _) = stream.unwrap();
                        app_connections.fetch_add(1, Ordering::SeqCst);
                        let requests = request_tx.clone();
                        let cwd = cwd.clone();
                        clients.spawn(async move {
                            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                            while let Some(Ok(AppMessage::Text(text))) = ws.next().await {
                                let request: Value = serde_json::from_str(&text).unwrap();
                                if request.get("id").is_none() { continue; }
                                let result = match request["method"].as_str().unwrap() {
                                    "initialize" => json!({}),
                                    "thread/loaded/list" => json!({"data":["target","newer"]}),
                                    "thread/read" => json!({"thread":{"cwd":cwd,"threadSource":"user","updatedAt":if request["params"]["threadId"] == "newer" { 20 } else { 10 },"status":{"type":"idle"}}}),
                                    "turn/start" | "turn/steer" => json!({"turn":{"id":"turn"}}),
                                    method => panic!("unexpected method {method}"),
                                };
                                requests.send(request.clone()).unwrap();
                                if ws.send(AppMessage::Text(json!({"id":request["id"],"result":result}).to_string().into())).await.is_err() { break; }
                            }
                        });
                    }
                    Some(result) = clients.join_next() => { result.unwrap(); }
                }
            }
        });
        let (log_tx, mut logs) = unbounded_channel();
        let mut readers = Vec::new();
        let mut spawn = || {
            let mut child = sandbox.command().args(["hook","ws","--peer-id","worker","--backend","codex"])
                .env("CODEX_HOME", "cx").spawn().unwrap();
            let stderr = child.stderr.take().unwrap();
            let tx = log_tx.clone();
            readers.push(std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines() { let _ = tx.send(line.unwrap()); }
            }));
            KillChild(Some(child))
        };
        let mut first = spawn();
        assert_eq!(next(&mut receipts).await["id"], "q0");
        assert_eq!(next(&mut receipts).await["id"], "q1");
        // The initial connection attempt completes before the hook acknowledges either frame.
        assert_eq!(connections.load(Ordering::SeqCst), 0, "unbound peer connected to App Server");
        assert!(requests.try_recv().is_err(), "unbound peer made an App Server request");
        *binding.lock().unwrap() = "target".into();
        let mut turns = Vec::new();
        while turns.len() < 2 {
            let request = next(&mut requests).await;
            if request["method"] == "thread/read" {
                assert_eq!(request["params"]["threadId"], "target", "read an unrelated thread");
            }
            if matches!(request["method"].as_str(), Some("turn/start" | "turn/steer")) { turns.push(request); }
        }
        for (turn, text) in turns.iter().zip(["q0", "q1"]) {
            assert_eq!(turn["params"]["threadId"], "target");
            assert!(turn["params"]["input"][0]["text"].as_str().unwrap().contains(&format!("\n{text}\n")), "FIFO order: {turn}");
        }
        while !next(&mut logs).await.contains("flushed, queued=0") {}
        let mut child = first.0.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        *binding.lock().unwrap() = "newer".into();
        let mut second = spawn();
        assert_eq!(next(&mut receipts).await["id"], "q2");
        loop {
            let request = next(&mut requests).await;
            if matches!(request["method"].as_str(), Some("turn/start" | "turn/steer")) {
                assert_eq!(request["params"]["threadId"], "newer", "replacement reused the old sink");
                assert!(request["params"]["input"][0]["text"].as_str().unwrap().contains("\nq2\n"));
                break;
            }
        }
        while !next(&mut logs).await.contains("flushed, queued=0") {}
        let mut child = second.0.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        hub_task.abort();
        app_task.abort();
        for reader in readers { reader.join().unwrap(); }
        }).await.expect("hook identity lifecycle timed out");
    });
}

use super::*;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

mod ack_delivery;
mod asks_peers;
mod delivery;
mod dispatch;
mod event_visibility;
mod events_ring;
mod handoff;
mod ids;
mod incremental_persistence;
mod job_dag;
mod knowledge_api;
mod mcp_schedule;
mod mcp_wait_events;
mod ownership;
mod persistence_time;
mod read_model;
mod state_file;
mod sweep;
mod watch;

/* config.toml and attachments sit next to the state file, so each hub gets its own
directory, removed with everything in it once the test lets go */
pub(super) struct TempState(PathBuf);

impl TempState {
    pub(super) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("amesh-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir.join("state.db"))
    }
}

impl std::ops::Deref for TempState {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl AsRef<Path> for TempState {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

fn temp_state(name: &str) -> TempState {
    TempState::new(name)
}

/* wakes on each commit until the check holds; the bound only fails a test whose commit
never comes */
async fn until(state: &App, check: impl Fn(&Hub) -> bool, failure: &str) {
    let bound = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let mut changes = {
            let hub = state.inner.lock().await;
            if check(&hub) {
                return;
            }
            hub.changes.subscribe()
        };
        assert!(
            tokio::time::timeout_at(bound, changes.changed())
                .await
                .is_ok(),
            "{failure}"
        );
    }
}

/* the router and the directory its state lives in, which goes when the test drops it */
fn test_app() -> (Router, TempState) {
    let path = temp_state("test");
    let app = router(App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.to_path_buf(),
    });
    (app, path)
}

async fn json_req(app: Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/* a waker that counts wakeups: a manual poll can then show the future was signalled, not just
re-read. A noop waker would let a wait that never subscribes still pass */
struct WakeCount(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl std::task::Wake for WakeCount {
    fn wake(self: std::sync::Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn mcp_tool(app: Router, name: &str, args: Value) -> (StatusCode, Value) {
    json_req(
        app,
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": name, "arguments": args}}),
    )
    .await
}

fn undelivered_hub() -> (Hub, TempState) {
    let path = temp_state("undeliv");
    let mut hub = Hub::open(&path).unwrap();
    hub.peers.insert(
        "worker".into(),
        Peer {
            peer_id: "worker".into(),
            name: "worker".into(),
            path: "/tmp".into(),
            backend: "pi".into(),
            circle: "default".into(),
            status: "online".into(),
            description: String::new(),
            session_id: String::new(),
            last_seen: now_unix(),
        },
    );
    (hub, path)
}

fn recv_peer(hub: &mut Hub, id: &str) -> mpsc::UnboundedReceiver<Outbound> {
    hub.peers.insert(
        id.into(),
        Peer {
            peer_id: id.into(),
            name: id.into(),
            path: "/tmp".into(),
            backend: "pi".into(),
            circle: "default".into(),
            status: "online".into(),
            description: String::new(),
            session_id: String::new(),
            last_seen: now_unix(),
        },
    );
    let (tx, rx) = mpsc::unbounded_channel();
    hub.conn_gen += 1;
    let gen = hub.conn_gen;
    hub.sockets.insert(id.into(), (gen, tx));
    hub.recv_live.insert(id.into());
    hub.recv_known.insert(id.into());
    rx
}

fn test_app_with_hub() -> (Router, Arc<Mutex<Hub>>, TempState) {
    let path = temp_state("test");
    let inner = Arc::new(Mutex::new(Hub::open(&path).unwrap()));
    let app = router(App {
        inner: inner.clone(),
        token: None,
        state_path: path.to_path_buf(),
    });
    (app, inner, path)
}

async fn register(app: Router, id: &str, backend: &str, circle: &str) {
    let _ = json_req(
        app,
        "POST",
        "/peers",
        json!({"name": id, "peer_id": id, "backend": backend, "circle": circle}),
    )
    .await;
}

fn tool_json(body: &Value) -> Value {
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

async fn running(hub: &Arc<Mutex<Hub>>, jid: &str) -> String {
    let mut hub = hub.lock().await;
    advance_jobs(&mut hub);
    assert_eq!(hub.jobs[jid].state, "running");
    hub.jobs[jid].ask_id.clone().unwrap()
}

fn test_outbound(event: Value) -> Outbound {
    if matches!(event["type"].as_str(), Some(DISPLACED | REPLACED | "bound")) {
        Outbound::Control(event)
    } else {
        Outbound::Record(QueuedRecord::new(event, 0))
    }
}

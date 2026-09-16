use super::*;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

struct Fixture(App);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("amesh-events-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state_path = root.join("state.db");
        Self(App {
            inner: Arc::new(Mutex::new(Hub::open(&state_path).unwrap())),
            token: None,
            state_path,
        })
    }

    async fn request(&self, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let response = router(self.0.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn peer(&self, id: &str, circle: &str, backend: &str) {
        let (status, _) = self
            .request(
                "POST",
                "/peers",
                json!({
                    "peer_id": id, "name": id, "circle": circle, "backend": backend,
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    async fn events(&self, uri: &str) -> Value {
        let (status, body) = self.request("GET", uri, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    async fn mcp(&self, args: Value) -> Result<Value, (StatusCode, Json<Value>)> {
        let response =
            mcp_call(&self.0, json!({"name": "amesh_events", "arguments": args})).await?;
        Ok(serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.state_path.parent().unwrap()).unwrap();
    }
}

#[tokio::test]
async fn events_broadcast_labels_are_visible_to_both_circles() {
    let f = Fixture::new();
    for (id, circle) in [("a", "one"), ("b", "one"), ("c", "two"), ("d", "three")] {
        f.peer(id, circle, "pi").await;
    }
    f.0.inner.lock().await.recv_known.insert("c".into());
    for body in [
        json!({"from_peer":"a", "message":"local"}),
        json!({"from_peer":"a", "circle":"two", "cross_circle":true, "message":"cross"}),
    ] {
        assert_eq!(
            f.request("POST", "/broadcast", body).await.0,
            StatusCode::OK
        );
    }
    let one = f.events("/events?circle=one").await;
    let two = f.events("/events?circle=two").await;
    assert_eq!(one.as_array().unwrap().len(), 2);
    assert_eq!(two.as_array().unwrap().len(), 1);
    assert_eq!(two[0]["message"], "cross");
    assert_eq!(two[0]["from_circle"], "one");
    assert_eq!(two[0]["to_circle"], "two");
    assert_eq!(f.events("/events?circle=three").await, json!([]));
    assert_eq!(f.events("/events").await.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn events_chat_labels_follow_peer_at_write_time() {
    let f = Fixture::new();
    f.peer("writer", "one", "codex").await;
    for route in ["/events/chat", "/events/chat_delta"] {
        assert_eq!(
            f.request(
                "POST",
                route,
                json!({
                    "peer":"writer", "role":"assistant", "text":"hello",
                    "from_circle":"forged", "to_circle":"forged",
                })
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let events = f.events("/events?circle=one").await;
    for kind in ["chat", "chat_turn_delta"] {
        let event = events
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == kind)
            .unwrap();
        assert_eq!(event["from_circle"], "one");
        assert_eq!(event["to_circle"], "one");
    }
    assert_eq!(f.events("/events?circle=forged").await, json!([]));
}

#[tokio::test]
async fn events_keep_labels_after_peer_disappears_and_name_is_reused() {
    let f = Fixture::new();
    f.peer("old", "one", "pi").await;
    {
        let mut hub = f.0.inner.lock().await;
        push_event(
            &mut hub,
            json!({"id":"before", "from_peer":"old", "to_peer":"old"}),
        );
        hub.peers.remove("old");
    }
    assert_eq!(f.events("/events?circle=one").await[0]["id"], "before");
    f.peer("old", "two", "pi").await;
    assert_eq!(f.events("/events?circle=one").await[0]["id"], "before");
    assert_eq!(f.events("/events?circle=two").await, json!([]));
}

#[tokio::test]
async fn events_partial_labels_are_visible_and_forged_labels_are_removed() {
    let f = Fixture::new();
    f.peer("known", "one", "pi").await;
    {
        let mut hub = f.0.inner.lock().await;
        for (id, from, to) in [
            ("in", "gone", "known"),
            ("out", "known", "gone"),
            ("unknown", "gone", "gone"),
        ] {
            push_event(
                &mut hub,
                json!({"id":id, "type":"ack", "from_peer":from, "to_peer":to,
                "from_circle":"forged", "to_circle":"forged"}),
            );
        }
        hub.events
            .push(json!({"id":"legacy", "from_peer":"known", "to_peer":"known"}));
    }
    let scoped = f.events("/events?circle=one").await;
    assert_eq!(scoped.as_array().unwrap().len(), 2);
    assert_eq!(scoped[0]["id"], "in");
    assert!(scoped[0].get("from_circle").is_none());
    assert_eq!(scoped[0]["to_circle"], "one");
    assert_eq!(scoped[1]["id"], "out");
    assert!(scoped[1].get("to_circle").is_none());
    assert_eq!(scoped[1]["from_circle"], "one");
    assert_eq!(f.events("/events?circle=forged").await, json!([]));
    assert_eq!(f.events("/events").await.as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn events_since_uses_global_cursor_before_circle_filter() {
    let f = Fixture::new();
    f.peer("a", "one", "pi").await;
    f.peer("b", "two", "pi").await;
    {
        let mut hub = f.0.inner.lock().await;
        for i in 0..501 {
            let peer = if i == 498 || i == 499 { "b" } else { "a" };
            push_event(&mut hub, json!({"id":format!("e{i}"), "from_peer":peer}));
        }
    }
    let all = f.events("/events").await;
    assert_eq!(all.as_array().unwrap().len(), 500);
    assert_eq!(all[0]["id"], "e1");
    let after = f.events("/events?since=e498&circle=one").await;
    assert_eq!(after.as_array().unwrap().len(), 1);
    assert_eq!(after[0]["id"], "e500");
    assert_eq!(f.events("/events?since=e0&circle=one").await, json!([]));
}

#[tokio::test]
async fn events_mcp_scope_matrix_and_literal_all_circle() {
    let f = Fixture::new();
    for (id, circle) in [("a", "one"), ("b", "two"), ("c", "all")] {
        f.peer(id, circle, "pi").await;
        push_event(
            &mut *f.0.inner.lock().await,
            json!({"id":id, "from_peer":id}),
        );
    }
    for args in [
        json!({"from_peer":"a"}),
        json!({"from_peer":"a", "circle":"one"}),
    ] {
        let events = f.mcp(args).await.unwrap();
        assert_eq!(events.as_array().unwrap().len(), 1);
        assert_eq!(events[0]["id"], "a");
    }
    assert_eq!(
        f.mcp(json!({"from_peer":"a", "circle":"two"}))
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    let all = f
        .mcp(json!({"from_peer":"a", "cross_circle":true}))
        .await
        .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 3);
    for (circle, id) in [("two", "b"), ("all", "c")] {
        let events = f
            .mcp(json!({"from_peer":"a", "circle":circle, "cross_circle":true}))
            .await
            .unwrap();
        assert_eq!(events.as_array().unwrap().len(), 1);
        assert_eq!(events[0]["id"], id);
    }
    for args in [
        json!({}),
        json!({"from_peer":"gone"}),
        json!({"from_peer":"gone", "cross_circle":true}),
    ] {
        assert_eq!(f.mcp(args).await.unwrap_err().0, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn events_legacy_online_success_remains_outside_the_ring() {
    let f = Fixture::new();
    f.peer("legacy", "one", "pi").await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut hub = f.0.inner.lock().await;
    hub.sockets.insert("legacy".into(), (1, tx));
    persist_then_deliver(&mut hub, "legacy", json!({"id":"live", "to_peer":"legacy"})).unwrap();
    assert_eq!(rx.try_recv().unwrap()["id"], "live");
    assert!(hub.events.is_empty());
}

#[test]
fn events_schema_exposes_circle_and_cross_circle() {
    let tools = mcp_tools();
    let tool = tools.iter().find(|t| t["name"] == "amesh_events").unwrap();
    assert_eq!(
        tool["inputSchema"]["properties"]["circle"]["type"],
        "string"
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["cross_circle"]["type"],
        "boolean"
    );
}

async fn caller_identity(backend: &str) {
    let f = Fixture::new();
    f.peer("caller", "one", backend).await;
    f.peer("other", "two", backend).await;
    {
        let mut hub = f.0.inner.lock().await;
        push_event(&mut hub, json!({"id":"local", "from_peer":"caller"}));
        push_event(&mut hub, json!({"id":"foreign", "from_peer":"other"}));
    }
    let events = f.mcp(json!({"from_peer":"caller"})).await.unwrap();
    assert_eq!(events.as_array().unwrap().len(), 1, "{backend}");
    assert_eq!(events[0]["id"], "local", "{backend}");
    for args in [
        json!({}),
        json!({"from_peer":"missing", "cross_circle":true}),
    ] {
        let (status, body) = f.mcp(args).await.unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND, "{backend}");
        assert_eq!(body.0["error"], "unknown caller", "{backend}");
    }
}

#[tokio::test]
async fn events_pi_identity_is_required() {
    caller_identity("pi").await;
}

#[tokio::test]
async fn events_codex_identity_is_required() {
    caller_identity("codex").await;
}

#[tokio::test]
async fn events_claude_identity_is_required() {
    caller_identity("claude-code").await;
}

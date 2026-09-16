use super::*;

fn events_through_stdio(backend: &str) {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    for (id, circle) in [("caller", "one"), ("other", "two")] {
        sandbox.json(
            &[
                "peer",
                "register",
                "--peer-id",
                id,
                "--name",
                id,
                "--backend",
                backend,
                "--circle",
                circle,
            ],
            None,
        );
        sandbox.json(&["peer", "events", "--peer-id", id, id], None);
    }
    let input = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{
        "name":"amesh_events", "arguments":{"from_peer":"other"}
    }})
    .to_string();
    let output = sandbox.run_text(
        &["mcp", "--peer-id", "caller"],
        &input,
        &[
            ("AMESH_BACKEND", backend),
            ("AMESH_CIRCLE", "one"),
            ("AMESH_SKIP_WS", "1"),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let events: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let events = events.as_array().unwrap();
    assert!(!events.is_empty(), "the caller's events must be visible");
    assert!(
        events
            .iter()
            .all(|event| event["from_circle"] == "one" || event["to_circle"] == "one"),
        "stdio must use its own caller, not model-supplied from_peer: {events:?}"
    );
    assert!(events
        .iter()
        .any(|event| event["type"] == "chat" && event["text"] == "caller"));
}

#[test]
fn events_pi_stdio_binds_caller() {
    events_through_stdio("pi");
}

#[test]
fn events_codex_stdio_binds_caller() {
    events_through_stdio("codex");
}

#[test]
fn events_claude_stdio_binds_caller() {
    events_through_stdio("claude-code");
}

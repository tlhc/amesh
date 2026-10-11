fn check_peer_handoff(backend: &str) {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let result = sandbox.json(
        &["hook", "session", "--backend", backend],
        Some(json!({"session_id": format!("handoff-{backend}"), "cwd": sandbox.root})),
    );
    let context = if backend == "pi" {
        &result["context"]
    } else {
        &result["hookSpecificOutput"]["additionalContext"]
    }
    .as_str()
    .unwrap();
    for clause in [
        "amesh_list_peers(detail=true)",
        "include only the relevant scope, file ownership, acceptance checks",
        "input revision or frozen diff path/hash",
        "artifact paths in the ask text or job prompt",
        "room for independent findings",
    ] {
        assert!(
            context.contains(clause),
            "{backend} missing {clause}: {context}"
        );
    }
}

#[test]
fn peer_handoff_reaches_pi() {
    check_peer_handoff("pi");
}
#[test]
fn peer_handoff_reaches_codex() {
    check_peer_handoff("codex");
}
#[test]
fn peer_handoff_reaches_claude() {
    check_peer_handoff("claude-code");
}

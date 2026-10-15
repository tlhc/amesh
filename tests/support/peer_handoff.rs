fn check_peer_handoff(backend: &str) {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let result = sandbox.json(
        &["hook", "session", "--backend", backend],
        Some(json!({"session_id": format!("handoff-{backend}"), "cwd": sandbox.root})),
    );
    let context = if matches!(backend, "pi" | "opencode") {
        &result["context"]
    } else {
        &result["hookSpecificOutput"]["additionalContext"]
    }
    .as_str()
    .unwrap();
    let missing: Vec<_> = [
        "Fan out same-circle work as one job per peer",
        "Same-circle by default; set cross_circle=true for questions and notifications to another circle",
        "including job asks from jobs you created",
        "Requeuing a running job emits a synthetic failed ack for the old ask",
        "Track the replacement ask_id or the job's current state for the new attempt's result",
        "amesh_job_create(assigned_peer=...) for same-circle work with its own acceptance criteria",
        "even for one worker with no dependencies",
        "amesh_ask() for questions, clarifications, decisions or status",
        "Cross-circle work uses amesh_ask(cross_circle=true)",
        "depends_on only for real prerequisites",
        "scope or acceptance changes and reassignment use amesh_job_update(state=queued",
        "closes the old ask and dispatches a new one",
        "coordinator alone for a handoff",
        "Workers ack the job's ask with the actual result",
        "the hub settles the job",
        "Explicit user instructions take precedence",
        "amesh_list_peers(detail=true)",
        "include only the relevant scope, file ownership, acceptance checks",
        "input revision or frozen diff path/hash",
        "artifact paths in the ask text or job prompt",
        "room for independent findings",
    ]
    .into_iter()
    .filter(|clause| !context.contains(clause))
    .collect();
    assert!(
        missing.is_empty(),
        "{backend} missing {missing:?}: {context}"
    );
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

#[test]
fn peer_handoff_reaches_opencode() {
    check_peer_handoff("opencode");
}

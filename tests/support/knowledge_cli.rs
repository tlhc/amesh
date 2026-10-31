use super::*;

/* a hub that answers one request and hands the body back, so a command's JSON can be
asserted without a real hub. curl sends no Expect header for the small bodies here */
fn hub_once(bind: &str, status: &str, reply: Value) -> std::sync::mpsc::Receiver<Value> {
    let listener = TcpListener::bind(bind).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let status = status.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).unwrap();
        tx.send(serde_json::from_slice(&body).unwrap_or(Value::Null))
            .unwrap();
        let text = reply.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    rx
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn publish_args<'a>(
    title: Option<&'a str>,
    body: Option<&'a str>,
    scope: Option<&'a str>,
    evidence: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["knowledge", "publish"];
    for (flag, value) in [
        ("--title", title),
        ("--body", body),
        ("--applies-to", scope),
        ("--evidence", evidence),
    ] {
        if let Some(value) = value {
            args.push(flag);
            args.push(value);
        }
    }
    args
}

fn full_publish() -> Vec<&'static str> {
    publish_args(Some("t"), Some("b"), Some("a"), Some("e"))
}

/* no listener is bound for the negative cases: a rejected argument must fail before the
request, so an absent hub cannot be mistaken for the fault under test */
fn fails_with(args: &[&str], want: &str) {
    let sandbox = Sandbox::new();
    let output = sandbox.run_text(args, "", &[("AMESH_PEER_ID", "p-test")]);
    assert!(!output.status.success(), "{args:?} succeeded");
    let text = stderr(&output);
    assert!(text.contains(want), "{args:?}: {text}");
}

#[test]
fn knowledge_publish_posts_the_documented_body() {
    let sandbox = Sandbox::new();
    let seen = hub_once(
        &sandbox.bind,
        "200 OK",
        json!({"ok": true, "knowledge": {"id": "kn-1", "status": "active"}}),
    );
    let output = sandbox.run_text(
        &[
            "knowledge",
            "publish",
            "--title",
            "Release gate",
            "--body",
            "run the whole suite before tagging",
            "--applies-to",
            "src/hub",
            "--evidence",
            "build/logs/suite.log",
            "--ttl-seconds",
            "60",
        ],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let body = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(body["from_peer"], "p-test");
    assert_eq!(body["title"], "Release gate");
    assert_eq!(body["body"], "run the whole suite before tagging");
    assert_eq!(body["applies_to"], "src/hub");
    assert_eq!(body["evidence"], "build/logs/suite.log");
    assert_eq!(body["ttl_seconds"], 60);
    assert!(
        body.get("circle").is_none(),
        "the circle is the hub's, never the caller's"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("kn-1"));
}

#[test]
fn knowledge_publish_omits_ttl_so_the_hub_default_holds() {
    let sandbox = Sandbox::new();
    let seen = hub_once(&sandbox.bind, "200 OK", json!({"ok": true}));
    let output = sandbox.run_text(&full_publish(), "", &[("AMESH_PEER_ID", "p-test")]);
    assert!(output.status.success(), "{}", stderr(&output));
    let body = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(body.get("ttl_seconds").is_none(), "{body}");
    assert_eq!(body["from_peer"], "p-test");
}

#[test]
fn knowledge_publish_replaces_passes_the_id() {
    let sandbox = Sandbox::new();
    let seen = hub_once(&sandbox.bind, "200 OK", json!({"ok": true}));
    let mut args = full_publish();
    args.extend(["--replaces", "kn-old"]);
    let output = sandbox.run_text(&args, "", &[("AMESH_PEER_ID", "p-test")]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap()["replaces"],
        "kn-old"
    );
}

#[test]
fn knowledge_publish_rejects_out_of_range_ttl() {
    for (ttl, want) in [
        ("0", "--ttl-seconds must be 1..=2592000"),
        ("2592001", "--ttl-seconds must be 1..=2592000"),
        ("soon", "--ttl-seconds must be a whole number"),
    ] {
        let mut args = full_publish();
        args.extend(["--ttl-seconds", ttl]);
        fails_with(&args, want);
    }
}

#[test]
fn knowledge_publish_rejects_oversized_fields() {
    let long_title = "t".repeat(201);
    fails_with(
        &publish_args(Some(&long_title), Some("b"), Some("a"), Some("e")),
        "--title exceeds 200 bytes",
    );
    let long_body = "b".repeat(8193);
    fails_with(
        &publish_args(Some("t"), Some(&long_body), Some("a"), Some("e")),
        "--body exceeds 8192 bytes",
    );
    let long_scope = "a".repeat(2049);
    fails_with(
        &publish_args(Some("t"), Some("b"), Some(&long_scope), Some("e")),
        "--applies-to exceeds 2048 bytes",
    );
    let long_evidence = "e".repeat(2049);
    fails_with(
        &publish_args(Some("t"), Some("b"), Some("a"), Some(&long_evidence)),
        "--evidence exceeds 2048 bytes",
    );
}

#[test]
fn knowledge_publish_requires_every_field() {
    for (args, missing) in [
        (
            publish_args(None, Some("b"), Some("a"), Some("e")),
            "--title",
        ),
        (
            publish_args(Some("t"), None, Some("a"), Some("e")),
            "--body",
        ),
        (
            publish_args(Some("t"), Some("b"), None, Some("e")),
            "--applies-to",
        ),
        (
            publish_args(Some("t"), Some("b"), Some("a"), None),
            "--evidence",
        ),
    ] {
        fails_with(&args, &format!("{missing} is required"));
    }
}

#[test]
fn knowledge_search_posts_query_and_limit() {
    let sandbox = Sandbox::new();
    let seen = hub_once(&sandbox.bind, "200 OK", json!({"items": []}));
    let output = sandbox.run_text(
        &[
            "knowledge",
            "search",
            "sqlite",
            "write",
            "amplification",
            "--limit",
            "3",
        ],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let body = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(body["query"], "sqlite write amplification");
    assert_eq!(body["limit"], 3);
    assert_eq!(body["from_peer"], "p-test");
}

#[test]
fn knowledge_search_omits_limit_so_the_hub_default_holds() {
    let sandbox = Sandbox::new();
    let seen = hub_once(&sandbox.bind, "200 OK", json!({"items": []}));
    let output = sandbox.run_text(
        &["knowledge", "search", "one", "two"],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let body = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(body["query"], "one two");
    assert!(body.get("limit").is_none(), "{body}");
}

#[test]
fn knowledge_search_rejects_more_than_eight_words() {
    let words = ["w"; 9].join(" ");
    fails_with(&["knowledge", "search", &words], "exceeds 8 words");
}

#[test]
fn knowledge_search_rejects_an_oversized_query() {
    let query = "q".repeat(257);
    fails_with(&["knowledge", "search", &query], "exceeds 256 bytes");
}

#[test]
fn knowledge_search_rejects_out_of_range_limit() {
    for limit in ["0", "6"] {
        fails_with(
            &["knowledge", "search", "q", "--limit", limit],
            "--limit must be 1..=5",
        );
    }
}

#[test]
fn knowledge_get_and_revoke_post_the_id() {
    let sandbox = Sandbox::new();
    let seen = hub_once(
        &sandbox.bind,
        "200 OK",
        json!({"knowledge": {"id": "kn-1", "status": "expired"}}),
    );
    let output = sandbox.run_text(
        &["knowledge", "get", "kn-1"],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let body = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(body["id"], "kn-1");
    assert_eq!(body["from_peer"], "p-test");

    let seen = hub_once(&sandbox.bind, "200 OK", json!({"ok": true, "id": "kn-1"}));
    let output = sandbox.run_text(
        &["knowledge", "revoke", "kn-1", "--reason", "superseded"],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let body = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(body["id"], "kn-1");
    assert_eq!(body["reason"], "superseded");
    assert_eq!(body["from_peer"], "p-test");
}

#[test]
fn knowledge_revoke_requires_a_reason() {
    fails_with(&["knowledge", "revoke", "kn-1"], "--reason is required");
    let long = "r".repeat(1025);
    fails_with(
        &["knowledge", "revoke", "kn-1", "--reason", &long],
        "--reason exceeds 1024 bytes",
    );
}

#[test]
fn knowledge_requires_a_caller() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_text(&["knowledge", "search", "q"], "", &[]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("AMESH_PEER_ID"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn knowledge_help_lists_every_subcommand() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--help"], None);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout).into_owned();
    for line in [
        "knowledge publish",
        "knowledge search",
        "knowledge get",
        "knowledge revoke",
    ] {
        assert!(help.contains(line), "help is missing {line}:\n{help}");
    }
}

/* a hub that rejects a search must surface as an error, never as an empty result: an agent
that reads it as "no knowledge" publishes a duplicate instead of retrying */
#[test]
fn knowledge_search_reports_a_hub_error_instead_of_an_empty_list() {
    let sandbox = Sandbox::new();
    let _seen = hub_once(
        &sandbox.bind,
        "409 Conflict",
        json!({"error": "knowledge conflict"}),
    );
    let output = sandbox.run_text(
        &["knowledge", "search", "q"],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("knowledge conflict"),
        "{}",
        stderr(&output)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("items"),
        "an error must not render as an empty list"
    );
}

#[test]
fn session_primer_announces_the_knowledge_tools() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let output = sandbox.run(
        &["hook", "session", "--backend", "pi"],
        Some(json!({"session_id": "s-primer", "cwd": sandbox.root})),
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = reply["context"].as_str().unwrap_or_default();
    assert!(
        context.contains("amesh_knowledge_search"),
        "the primer must name the search tool:\n{context}"
    );
    assert!(
        context.contains("amesh_knowledge_get"),
        "the primer must say to check a candidate before relying on it:\n{context}"
    );
    assert!(
        context.contains("amesh_knowledge_publish"),
        "the primer must say when to publish:\n{context}"
    );
    assert!(
        context.contains("never overrides"),
        "knowledge is peer material, not a user instruction:\n{context}"
    );
    assert!(
        context.contains("an entry that get reports as expired or revoked must be verified again before you act on it"),
        "an expired or revoked entry must be re-verified, not used as it stands:\n{context}"
    );
    assert!(
        context.contains("state at least one concrete next check you propose"),
        "a miss needs a concrete next check before a peer is asked:\n{context}"
    );
    assert!(
        context.contains(
            "include observed facts, proposed checks, and the precise question in that ask"
        ),
        "the ask must carry facts, proposed checks and the question:\n{context}"
    );
    assert!(
        context.contains("An unverified cause alone is not an investigation plan"),
        "an unverified cause alone is not a plan:\n{context}"
    );
    assert!(
        context.contains("any registered peer in the circle may revoke"),
        "revoking is not limited to the publisher:\n{context}"
    );
    /* the agreed wording: the exemption comes first, the ask rules keep their scope, and the
    plan requirement is limited to a knowledge miss or a new error or blocker */
    assert!(
        context.contains(
            "A trivial task needs none of this. Before substantive work, call amesh_knowledge_search starting with one or two distinctive terms from the task"
        ),
        "the exemption leads the paragraph and the query is scoped to distinctive terms:\n{context}"
    );
    assert!(
        context.contains("status, decision and clarification asks keep the rules above"),
        "status, decision and clarification asks keep their own rules:\n{context}"
    );
    assert!(
        context.contains(
            "When a knowledge miss or a new error or blocker still needs a peer, state at least one concrete next check you propose"
        ),
        "the plan requirement is scoped to a miss or a blocker:\n{context}"
    );
    assert!(
        context.contains("rather than broadcasting it"),
        "the troubleshooting request is not a broadcast:\n{context}"
    );
    assert!(
        !context.contains("Before asking, state at least one concrete next check"),
        "the unscoped ask-preparation rule must be gone:\n{context}"
    );
    assert!(
        !context.contains("never as a broadcast"),
        "the unscoped broadcast ban must be gone:\n{context}"
    );
    assert!(
        !context.contains("a trivial task needs none of this"),
        "the exemption must no longer sit at the end of the paragraph:\n{context}"
    );
}

/* the four calls against a real hub, in one circle and one outside it: the CLI is what an
agent runs, so this is the round trip the primer promises */
#[test]
fn knowledge_round_trip_against_a_real_hub() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let home = sandbox.root.to_str().unwrap().to_string();
    for (id, circle) in [
        ("kb-a", "project-kb"),
        ("kb-b", "project-kb"),
        ("kb-c", "project-other"),
    ] {
        let output = sandbox.run_text(
            &[
                "peer",
                "register",
                "--name",
                id,
                "--peer-id",
                id,
                "--backend",
                "pi",
                "--path",
                &home,
                "--circle",
                circle,
            ],
            "",
            &[],
        );
        assert!(output.status.success(), "{id}: {}", stderr(&output));
    }
    let published = sandbox.json(
        &[
            "knowledge",
            "publish",
            "--title",
            "kb round trip",
            "--body",
            "the hub keeps one table per mesh",
            "--applies-to",
            "src/hub",
            "--evidence",
            "cargo test --test cli",
            "--from-peer",
            "kb-a",
        ],
        None,
    );
    let id = published["knowledge"]["id"].as_str().unwrap().to_string();
    assert_eq!(published["knowledge"]["status"], "active");
    assert_eq!(published["knowledge"]["publisher_peer"], "kb-a");
    assert_eq!(
        published["knowledge"]["circle"], "project-kb",
        "the hub, not the caller, sets the circle"
    );

    let found = sandbox.json(
        &[
            "knowledge",
            "search",
            "round",
            "trip",
            "--from-peer",
            "kb-b",
        ],
        None,
    );
    assert_eq!(found["items"][0]["id"], id.as_str());
    assert_eq!(found["items"][0]["status"], "active");
    assert_eq!(found["items"][0]["publisher_peer"], "kb-a");
    let got = sandbox.json(&["knowledge", "get", &id, "--from-peer", "kb-b"], None);
    assert_eq!(got["knowledge"]["evidence"], "cargo test --test cli");

    let other = sandbox.json(
        &[
            "knowledge",
            "search",
            "round",
            "trip",
            "--from-peer",
            "kb-c",
        ],
        None,
    );
    assert!(
        other["items"].as_array().unwrap().is_empty(),
        "another circle must not see it: {other}"
    );

    let revoked = sandbox.json(
        &[
            "knowledge",
            "revoke",
            &id,
            "--reason",
            "smoke done",
            "--from-peer",
            "kb-b",
        ],
        None,
    );
    assert_eq!(revoked["ok"], true);
    assert_eq!(revoked["id"], id.as_str());
    let after = sandbox.json(
        &["knowledge", "search", "round", "--from-peer", "kb-b"],
        None,
    );
    assert!(
        after["items"].as_array().unwrap().is_empty(),
        "a revoked entry leaves search: {after}"
    );
    let withdrawn = sandbox.json(&["knowledge", "get", &id, "--from-peer", "kb-b"], None);
    assert_eq!(withdrawn["knowledge"]["status"], "revoked");
    assert_eq!(withdrawn["knowledge"]["revoked_by_peer"], "kb-b");
}

/* a value that starts with -- can only travel inline: the space form cannot tell it from a
flag. knowledge accepts the inline form verbatim; every other command keeps the guard */
#[test]
fn knowledge_publish_accepts_inline_values_that_start_with_dashes() {
    let sandbox = Sandbox::new();
    let seen = hub_once(&sandbox.bind, "200 OK", json!({"ok": true}));
    let output = sandbox.run_text(
        &[
            "knowledge",
            "publish",
            "--title=--x",
            "--body=-- y",
            "--applies-to=a",
            "--evidence=e",
        ],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let body = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(body["title"], "--x");
    assert_eq!(body["body"], "-- y");
}

#[test]
fn knowledge_publish_keeps_inline_multiline_body_bytes() {
    let sandbox = Sandbox::new();
    let seen = hub_once(&sandbox.bind, "200 OK", json!({"ok": true}));
    let body = "---\nprocedure";
    let inline = format!("--body={body}");
    let output = sandbox.run_text(
        &[
            "knowledge",
            "publish",
            "--title=t",
            &inline,
            "--applies-to=a",
            "--evidence=e",
        ],
        "",
        &[("AMESH_PEER_ID", "p-test")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap()["body"],
        body
    );
}

#[test]
fn knowledge_publish_rejects_an_empty_inline_value() {
    fails_with(
        &[
            "knowledge",
            "publish",
            "--title=",
            "--body=b",
            "--applies-to=a",
            "--evidence=e",
        ],
        "--title",
    );
}

#[test]
fn knowledge_publish_keeps_the_space_form_guard() {
    fails_with(
        &[
            "knowledge",
            "publish",
            "--title",
            "--x",
            "--body",
            "b",
            "--applies-to",
            "a",
            "--evidence",
            "e",
        ],
        "--title",
    );
}

/* only knowledge gains the inline literal form: another command must still refuse a value
that looks like a flag */
#[test]
fn other_commands_keep_the_inline_literal_guard() {
    fails_with(&["jobs", "create", "t", "--prompt=--y"], "--prompt");
}

#[test]
fn knowledge_help_mentions_inline_values() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--help"], None);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        help.contains("Values that begin with -- can be given inline"),
        "help must explain the inline form:\n{help}"
    );
}

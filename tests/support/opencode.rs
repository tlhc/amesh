use super::*;

#[test]
fn opencode_setup_default_and_owned_uninstall() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let args = ["setup", "--home", sandbox.root.to_str().unwrap()];
    let output = sandbox.run(&args, None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = sandbox
        .root
        .join(".config/opencode/plugins/amesh/server.js");
    assert!(path.is_file(), "default setup must install OpenCode");
    let source = fs::read_to_string(&path).unwrap();
    assert!(source.contains("amesh-plugin:opencode:v1"));
    assert!(source.contains("amesh_whoami"));
    assert!(
        !source.contains("cli-test-token"),
        "token must remain runtime-only"
    );
    assert!(sandbox.run(&args, None).status.success());
    assert_eq!(source, fs::read_to_string(&path).unwrap());
    let doctor = sandbox.json(&["doctor", "--home", sandbox.root.to_str().unwrap()], None);
    assert_eq!(doctor["runtimes"]["opencode"]["hooks_installed"], true);
    let tui = path.with_file_name("tui.js");
    let tui_source = fs::read_to_string(&tui).unwrap();
    fs::remove_file(&tui).unwrap();
    let incomplete = sandbox.json(&["doctor", "--home", sandbox.root.to_str().unwrap()], None);
    assert_eq!(incomplete["runtimes"]["opencode"]["hooks_installed"], false);
    fs::write(&tui, tui_source).unwrap();
    let dry = sandbox.json(
        &["uninstall", "--home", sandbox.root.to_str().unwrap()],
        None,
    );
    assert!(dry["would_remove"].to_string().contains("server.js"));
    assert!(path.exists());
    sandbox.json(
        &[
            "uninstall",
            "--home",
            sandbox.root.to_str().unwrap(),
            "--apply",
            "true",
        ],
        None,
    );
    assert!(!path.exists());
    assert!(
        !path.parent().unwrap().exists(),
        "empty plugin directory must be removed"
    );
}

#[test]
fn opencode_setup_refuses_foreign_file_and_fixed_identity() {
    let sandbox = Sandbox::new();
    let args = [
        "setup",
        "opencode",
        "--home",
        sandbox.root.to_str().unwrap(),
    ];
    let path = sandbox
        .root
        .join(".config/opencode/plugins/amesh/server.js");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "export default {id:'foreign'};").unwrap();
    let output = sandbox.run(&args, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ownership"));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "export default {id:'foreign'};"
    );
    let output = sandbox.run(
        &[
            "setup",
            "opencode",
            "--peer-id",
            "fixed",
            "--home",
            sandbox.root.to_str().unwrap(),
        ],
        None,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--peer-id"));
}

#[test]
fn opencode_hook_registers_without_consuming_inbox_and_reports_activity() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let payload = json!({"session_id":"opencode:ses_test", "cwd":sandbox.root});
    let registered = sandbox.json(
        &["hook", "session", "--backend", "opencode"],
        Some(payload.clone()),
    );
    let id = registered["peer_id"].as_str().unwrap();
    assert!(registered["circle"].is_string());
    assert!(registered["context"].as_str().unwrap().contains(id));
    sandbox.json(
        &["peer", "notify", id, "retain-inbox", "--from-peer", id],
        None,
    );
    sandbox.json(&["hook", "session", "--backend", "opencode"], Some(payload));
    let pending = sandbox.json(&["peer", "asks", "--peer-id", id], None);
    assert!(pending["inbox"].to_string().contains("retain-inbox"));
    for state in ["work", "idle", "wait"] {
        let output = sandbox.run(
            &["hook", state, "--backend", "opencode"],
            Some(json!({"session_id":"opencode:ses_test","reason":"approval","check":false})),
        );
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        let reply = sandbox.json(&["mcp", "--peer-id", id], Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"amesh_list_peers","arguments":{"detail":true}}})));
        let peers: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let peer = peers
            .as_array()
            .unwrap()
            .iter()
            .find(|peer| peer["peer_id"] == id)
            .unwrap();
        assert_eq!(peer["activity"]["state"], state);
        assert_eq!(peer["activity"]["reason"], "approval");
    }
}

#[test]
fn opencode_plugin_contracts() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let output = sandbox.run(
        &[
            "setup",
            "opencode",
            "--home",
            sandbox.root.to_str().unwrap(),
        ],
        None,
    );
    assert!(output.status.success());
    let mut node = Command::new("node");
    let result = isolate_session_env(&mut node, &sandbox.root)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/opencode_hook.mjs"
        ))
        .arg(
            sandbox
                .root
                .join(".config/opencode/plugins/amesh/server.js"),
        )
        .arg(env!("CARGO_BIN_EXE_amesh"))
        .arg(&sandbox.root)
        .env("AMESH_BIND", &sandbox.bind)
        .env("AMESH_TOKEN", "cli-test-token")
        .env("AMESH_STATE", sandbox.root.join("state.json"))
        .env("AMESH_CIRCLE", "inherited-wrong-circle")
        .env("AMESH_PEER_ID", "inherited-wrong-peer")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn opencode_config_path_precedence_and_home_isolation() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let home = sandbox.root.to_str().unwrap();
    let configured = sandbox.root.join("configured");
    let xdg = sandbox.root.join("xdg");
    let vars = [
        ("HOME", home),
        ("OPENCODE_CONFIG_DIR", configured.to_str().unwrap()),
        ("XDG_CONFIG_HOME", xdg.to_str().unwrap()),
    ];
    let output = sandbox.run_text(&["setup", "opencode", "--home", home], "", &vars);
    assert!(output.status.success());
    assert!(sandbox
        .root
        .join(".config/opencode/plugins/amesh/server.js")
        .is_file());
    assert!(!configured.exists());
    assert!(!xdg.exists());
    assert!(sandbox
        .run_text(&["setup", "opencode"], "", &vars)
        .status
        .success());
    assert!(configured.join("plugins/amesh/server.js").is_file());
    assert!(!xdg.exists());
    let doctor = sandbox.run_text(&["doctor"], "", &vars);
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(
        report["runtimes"]["opencode"]["path"],
        json!(configured.join("plugins/amesh/server.js"))
    );
    let dry = sandbox.run_text(&["uninstall", "opencode"], "", &vars);
    assert!(dry.status.success());
    assert!(String::from_utf8_lossy(&dry.stdout).contains(configured.to_str().unwrap()));
    assert!(configured.join("plugins/amesh/server.js").exists());
    let remove = sandbox.run_text(&["uninstall", "opencode", "--apply", "true"], "", &vars);
    assert!(remove.status.success());
    assert!(!configured.join("plugins/amesh/server.js").exists());
    assert!(sandbox
        .root
        .join(".config/opencode/plugins/amesh/server.js")
        .exists());
    assert!(sandbox
        .run_text(
            &["setup", "opencode"],
            "",
            &[
                ("HOME", home),
                ("OPENCODE_CONFIG_DIR", ""),
                ("XDG_CONFIG_HOME", xdg.to_str().unwrap())
            ]
        )
        .status
        .success());
    assert!(xdg.join("opencode/plugins/amesh/server.js").is_file());
    for key in ["OPENCODE_CONFIG_DIR", "XDG_CONFIG_HOME"] {
        let output = sandbox.run_text(
            &["setup", "opencode"],
            "",
            &[("HOME", home), (key, "relative-config")],
        );
        assert!(!output.status.success(), "relative {key} must fail");
        assert!(String::from_utf8_lossy(&output.stderr).contains("absolute"));
        assert!(!sandbox.root.join("relative-config").exists());
    }
}

#[test]
fn opencode_check_report_yields_to_recent_activity() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let registered = sandbox.json(
        &["hook", "session", "--backend", "opencode"],
        Some(json!({"session_id":"opencode:ses_check","cwd":sandbox.root})),
    );
    for (state, check) in [("work", false), ("idle", true)] {
        assert!(sandbox
            .run(
                &["hook", state, "--backend", "opencode"],
                Some(json!({"session_id":"opencode:ses_check","check":check}))
            )
            .status
            .success());
    }
    let id = registered["peer_id"].as_str().unwrap();
    let reply=sandbox.json(&["mcp","--peer-id",id],Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"amesh_list_peers","arguments":{"detail":true}}})));
    let peers: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(peers[0]["activity"]["state"], "work");
}

#[cfg(unix)]
#[test]
fn opencode_doctor_bounds_version_probe() {
    use std::os::unix::fs::PermissionsExt;
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let binary = sandbox.root.join("opencode");
    fs::write(&binary, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        sandbox.root.display(),
        std::env::var("PATH").unwrap()
    );
    let started = Instant::now();
    let output = sandbox.run_text(
        &["doctor", "--home", sandbox.root.to_str().unwrap()],
        "",
        &[("PATH", &path)],
    );
    assert!(output.status.success());
    assert!(started.elapsed() < Duration::from_secs(4));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["runtimes"]["opencode"]["available"], true);
    assert_eq!(report["runtimes"]["opencode"]["version"], Value::Null);
}

#[test]
fn opencode_subprocess_faults() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(
        &[
            "setup",
            "opencode",
            "--home",
            sandbox.root.to_str().unwrap(),
        ],
        None,
    );
    assert!(output.status.success());
    let mut node = Command::new("node");
    let result = isolate_session_env(&mut node, &sandbox.root)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/opencode_faults.mjs"
        ))
        .arg(
            sandbox
                .root
                .join(".config/opencode/plugins/amesh/server.js"),
        )
        .arg(&sandbox.root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn opencode_tui_open_contracts() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(
        &[
            "setup",
            "opencode",
            "--home",
            sandbox.root.to_str().unwrap(),
        ],
        None,
    );
    assert!(output.status.success());
    let mut node = Command::new("node");
    let result = isolate_session_env(&mut node, &sandbox.root)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/opencode_tui.mjs"
        ))
        .arg(sandbox.root.join(".config/opencode/plugins/amesh/tui.js"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn opencode_setup_migrates_owned_single_file_and_preserves_foreign_companion() {
    let sandbox = Sandbox::new();
    let dir = sandbox.root.join(".config/opencode/plugins");
    fs::create_dir_all(dir.join("amesh")).unwrap();
    let legacy = dir.join("amesh.js");
    let companion = dir.join("amesh/tui.js");
    fs::write(&legacy, "/* amesh-plugin:opencode:v1 */\nold plugin").unwrap();
    fs::write(&companion, "foreign plugin").unwrap();
    let args = [
        "setup",
        "opencode",
        "--home",
        sandbox.root.to_str().unwrap(),
    ];
    assert!(!sandbox.run(&args, None).status.success());
    assert!(legacy.exists());
    assert!(!dir.join("amesh/server.js").exists());
    assert_eq!(fs::read_to_string(&companion).unwrap(), "foreign plugin");
    fs::write(&companion, "/* amesh-plugin:opencode:v1 */\nold companion").unwrap();
    assert!(sandbox.run(&args, None).status.success());
    assert!(!legacy.exists());
    assert!(dir.join("amesh/server.js").is_file());
    assert!(fs::read_to_string(&companion)
        .unwrap()
        .contains("router.current"));
}

#[test]
#[cfg(unix)]
fn opencode_setup_refuses_foreign_directory_and_symlink() {
    let sandbox = Sandbox::new();
    let directory = sandbox.root.join(".config/opencode/plugins/amesh");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("index.js"), "foreign plugin").unwrap();
    let args = [
        "setup",
        "opencode",
        "--home",
        sandbox.root.to_str().unwrap(),
    ];
    assert!(
        !sandbox.run(&args, None).status.success(),
        "foreign directory must be rejected"
    );
    assert_eq!(
        fs::read_to_string(directory.join("index.js")).unwrap(),
        "foreign plugin"
    );
    assert!(!directory.join("server.js").exists());
    let target = sandbox.root.join("foreign-target");
    fs::rename(&directory, &target).unwrap();
    std::os::unix::fs::symlink(&target, &directory).unwrap();
    assert!(
        !sandbox.run(&args, None).status.success(),
        "symlink must be rejected"
    );
    assert!(!target.join("server.js").exists());
}

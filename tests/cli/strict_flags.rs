use super::*;
use std::sync::Mutex;

struct Capture {
    bodies: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Capture {
    async fn start(bind: &str) -> Self {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let sink = bodies.clone();
        let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
        let router = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let sink = sink.clone();
            async move {
                let bytes = axum::body::to_bytes(request.into_body(), 65536)
                    .await
                    .unwrap();
                sink.lock()
                    .unwrap()
                    .push(serde_json::from_slice(&bytes).unwrap_or(Value::Null));
                axum::Json(json!([]))
            }
        });
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { bodies, task }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn rejected(args: &[&str], expected: &str) {
    let sandbox = Sandbox::new();
    let capture = Capture::start(&sandbox.bind).await;
    let home = sandbox.root.join("home");
    let extension = home.join(".pi/agent/extensions/amesh.ts");
    fs::create_dir_all(extension.parent().unwrap()).unwrap();
    let content = "export default function AmeshHooks() {}";
    fs::write(&extension, content).unwrap();
    let state = home.join(".amesh/state.json");
    let output = sandbox.run_text(
        args,
        "",
        &[
            ("HOME", home.to_str().unwrap()),
            ("AMESH_STATE", state.to_str().unwrap()),
        ],
    );
    let requests = capture.bodies.lock().unwrap();
    assert!(requests.is_empty(), "{args:?} sent requests: {requests:?}");
    assert!(
        !home.join(".amesh").exists(),
        "{args:?} created runtime files"
    );
    assert_eq!(fs::read_to_string(extension).unwrap(), content);
    assert!(!output.status.success(), "{args:?}: {output:?}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains(expected), "{args:?}: {error}");
}

macro_rules! rejects {
    ($name:ident, $args:expr, $error:expr) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            rejected($args, $error).await;
        }
    };
}

rejects!(
    ask_cross_circle,
    &["peer", "ask", "worker", "q", "--cross-circle", "tru"],
    "--cross-circle requires true or false"
);
rejects!(
    broadcast_cross_circle,
    &["peer", "broadcast", "q", "--cross-circle", "TRUE"],
    "--cross-circle requires true or false"
);
rejects!(
    ask_many_cross_circle,
    &["peer", "ask-many", "worker", "q", "--cross-circle", "1"],
    "--cross-circle requires true or false"
);
rejects!(
    ack_failed,
    &["peer", "ack", "ask-1", "--failed", "tru"],
    "--failed requires true or false"
);
rejects!(
    schedule_cross_circle,
    &[
        "schedule",
        "create",
        "worker",
        "q",
        "--in-seconds",
        "5",
        "--cross-circle",
        "TRUE"
    ],
    "--cross-circle requires true or false"
);
rejects!(
    gc_apply,
    &["gc", "--apply", "yes"],
    "--apply requires true or false"
);
rejects!(
    gc_apply_before_home_creation,
    &["gc", "--home", "home", "--apply", "tru"],
    "--apply requires true or false"
);
rejects!(
    uninstall_apply,
    &["uninstall", "pi", "--apply", "1"],
    "--apply requires true or false"
);
rejects!(
    attachment_days_text,
    &["gc", "--attachments-days", "abc"],
    "--attachments-days must be a whole number"
);
rejects!(
    attachment_days_before_home_creation,
    &["gc", "--home", "home", "--attachments-days", "abc"],
    "--attachments-days must be a whole number"
);
rejects!(
    dependencies_commas,
    &["jobs", "create", "j", "--depends-on", ",,"],
    "--depends-on requires at least one job id"
);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boolean_wire_values() {
    let sandbox = Sandbox::new();
    let capture = Capture::start(&sandbox.bind).await;
    for (command, flag, field, explicit_false) in [
        (
            vec!["peer", "ask", "worker", "q"],
            "--cross-circle",
            "cross_circle",
            false,
        ),
        (
            vec!["peer", "broadcast", "q"],
            "--cross-circle",
            "cross_circle",
            false,
        ),
        (
            vec!["peer", "ask-many", "worker", "q"],
            "--cross-circle",
            "cross_circle",
            false,
        ),
        (vec!["peer", "ack", "ask-1"], "--failed", "failed", true),
        (
            vec!["schedule", "create", "worker", "q", "--in-seconds", "5"],
            "--cross-circle",
            "cross_circle",
            true,
        ),
    ] {
        for value in [None, Some("false"), Some("true")] {
            let mut args = command.clone();
            if let Some(value) = value {
                args.extend([flag, value]);
            }
            let output = sandbox.run(&args, None);
            assert!(output.status.success(), "{args:?}: {output:?}");
            let bodies = capture.bodies.lock().unwrap();
            let body = bodies.last().unwrap();
            let expected = if value == Some("true") {
                Some(json!(true))
            } else if explicit_false {
                Some(json!(false))
            } else {
                None
            };
            assert_eq!(body.get(field), expected.as_ref(), "{args:?}: {body}");
            assert!(body.get("watch").is_none(), "{args:?}: {body}");
        }
    }
}

#[test]
fn attachment_days_keep_u64_semantics() {
    let sandbox = Sandbox::new();
    for (input, expected) in [
        ("0", 0),
        ("+5", 5),
        ("05", 5),
        ("18446744073709551615", u64::MAX),
    ] {
        let output = sandbox.json(
            &[
                "gc",
                "--home",
                sandbox.root.to_str().unwrap(),
                "--attachments-days",
                input,
            ],
            None,
        );
        assert_eq!(output["attachments_days"], expected);
        assert_eq!(output["dry_run"], true);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dependencies_keep_partial_empty_items_and_defaults() {
    let sandbox = Sandbox::new();
    let capture = Capture::start(&sandbox.bind).await;
    for input in [None, Some(" job-a, ,job-b,job-a,")] {
        let mut args = vec!["jobs", "create", "j"];
        if let Some(input) = input {
            args.extend(["--depends-on", input]);
        }
        let output = sandbox.run(&args, None);
        assert!(output.status.success(), "{output:?}");
        let bodies = capture.bodies.lock().unwrap();
        assert_eq!(
            bodies.last().unwrap()["depends_on"],
            if input.is_some() {
                json!(["job-a", "job-b", "job-a"])
            } else {
                json!([])
            }
        );
    }
}

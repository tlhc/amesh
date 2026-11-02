use super::*;

#[test]
fn the_footer_offers_copy_only_when_the_card_has_a_command() {
    let mut app = app_with(proto_s5());
    app.sel = Some("job-58851116".into());
    let footer = screen_text(&mut app, 100, 40).last().unwrap().clone();
    assert!(
        footer.contains("y copy"),
        "a failed job can be retried: {footer}"
    );
    app.sel = Some("deploy".into());
    let footer = screen_text(&mut app, 100, 40).last().unwrap().clone();
    assert!(
        !footer.contains("y copy"),
        "a queued job has nothing to copy: {footer}"
    );
}

#[test]
fn copy_keys_leave_navigation_bindings_intact() {
    let mut input = Input::new();
    for (key, action) in [
        ('y', "Copy(0)"),
        ('Y', "Copy(1)"),
        ('t', "Timeline"),
        ('n', "Find(1)"),
        ('r', "Refresh"),
        ('g', "Edge(-1)"),
        ('e', "Events"),
    ] {
        assert_eq!(
            format!("{:?}", input.key(KeyCode::Char(key), 1)),
            action,
            "{key}"
        );
    }
}

#[test]
fn osc52_copies_the_card_commands_and_handles_missing_actions() {
    let mut a = copy_app();
    let mut bytes = Vec::new();
    a.copy_command(0, false, &mut bytes);
    assert_eq!(
        bytes,
        b"\x1b]52;c;YW1lc2ggcGVlciBub3RpZnkgY29kZXggJ3Jldmlldz8n\x07"
    );
    bytes.clear();
    a.copy_command(1, false, &mut bytes);
    assert_eq!(bytes, b"\x1b]52;c;YW1lc2ggam9icyB1cGRhdGUgaiAtLXN0YXRlIHF1ZXVlZCAtLWFzc2lnbmVkLXBlZXIgUEVFUg==\x07");
    bytes.clear();
    a.copy_command(2, false, &mut bytes);
    assert!(bytes.is_empty());
    a.asks = true;
    a.copy_command(0, false, &mut bytes);
    assert!(bytes.is_empty());
    a.asks = false;
    a.events = true;
    a.copy_command(0, false, &mut bytes);
    assert!(bytes.is_empty());
    /* the knowledge page has no commands to copy, so y must emit nothing there */
    a.events = false;
    a.knowledge = true;
    a.copy_command(0, false, &mut bytes);
    assert!(bytes.is_empty());
}

#[test]
fn copy_footer_expires_and_failed_writes_do_not_claim_success() {
    let mut a = copy_app();
    a.copy_command(0, false, &mut Vec::new());
    let text = screen_text(&mut a, 100, 30).last().unwrap().clone();
    assert!(
        text.starts_with("sent to clipboard (OSC 52): amesh peer notify codex 'review?'"),
        "{text}"
    );
    a.copied.as_mut().unwrap().0 = std::time::Instant::now() - std::time::Duration::from_secs(2);
    let text = screen_text(&mut a, 100, 30).last().unwrap().clone();
    assert!(!text.contains("sent to clipboard"), "{text}");
    a.copied = None;
    a.copy_command(0, false, &mut std::io::Cursor::new(&mut [0u8; 0][..]));
    assert!(a.copy_notice().unwrap().starts_with("copy failed:"));
}

#[test]
fn base64_encodes_padding_and_utf8() {
    for (plain, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("中文", "5Lit5paH"),
    ] {
        assert_eq!(super::base64(plain.as_bytes()), encoded, "{plain}");
    }
}

fn retry_app() -> App {
    let mut app = app_with(Snapshot {
        jobs: vec![model::Job {
            job_id: "j".into(),
            state: "failed".into(),
            actions: Some(vec![crate::wire::JobAction::Retry {
                needs_assignee: false,
            }]),
            ..Default::default()
        }],
        ..Default::default()
    });
    app.sel = Some("j".into());
    app
}

#[test]
fn a_clipboard_flush_failure_replaces_the_success_notice() {
    struct FailsFlush;
    impl std::io::Write for FailsFlush {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("flush failed"))
        }
    }
    let mut app = retry_app();
    app.copy_command(0, false, &mut Vec::new());
    app.copy_command(0, false, &mut FailsFlush);
    assert_eq!(
        app.copy_notice().as_deref(),
        Some("copy failed: flush failed")
    );
}

#[test]
fn a_tmux_clipboard_write_sends_plain_before_passthrough() {
    let mut app = retry_app();
    let mut bytes = Vec::new();
    app.copy_command(0, true, &mut bytes);
    assert_eq!(
        bytes,
        b"\x1b]52;c;YW1lc2ggam9icyB1cGRhdGUgaiAtLXN0YXRlIHF1ZXVlZA==\x07\x1bPtmux;\x1b\x1b]52;c;YW1lc2ggam9icyB1cGRhdGUgaiAtLXN0YXRlIHF1ZXVlZA==\x07\x1b\\"
    );
}

#[test]
fn clipboard_notices_claim_the_write_and_explain_tmux() {
    let mut app = retry_app();
    app.copy_command(0, false, &mut Vec::new());
    assert_eq!(
        app.copy_notice().as_deref(),
        Some("sent to clipboard (OSC 52): amesh jobs update j --state queued")
    );
    app.copy_command(0, true, &mut Vec::new());
    assert_eq!(app.copy_notice().as_deref(), Some("tmux: set-clipboard on · sent to clipboard (OSC 52): amesh jobs update j --state queued"));
}

fn preserves_input_prompt(mode: Mode, prefix: &str) {
    let mut app = retry_app();
    app.copy_command(0, false, &mut Vec::new());
    app.input.mode = mode;
    let footer = screen_text(&mut app, 100, 30).last().unwrap().clone();
    assert!(footer.starts_with(prefix), "{footer}");
}

#[test]
fn a_clipboard_notice_preserves_search() {
    preserves_input_prompt(Mode::Search("review".into()), "/review_");
}

#[test]
fn a_clipboard_notice_preserves_jump() {
    preserves_input_prompt(Mode::Jump("1".into()), "jump 1_");
}

#[test]
fn a_clipboard_notice_preserves_hints() {
    preserves_input_prompt(Mode::Hint, "hint:");
}

fn command_text_drops_controls(field: &str, action: crate::wire::JobAction) {
    let mut job = model::Job {
        job_id: "j".into(),
        title: "review".into(),
        assigned_peer: Some("peer".into()),
        actions: Some(vec![action]),
        ..Default::default()
    };
    match field {
        "id" => job.job_id = "j\u{1}\n".into(),
        "assignee" => job.assigned_peer = Some("pe\u{85}er\t".into()),
        "title" => job.title = "x\u{1}echo PWNED #\n".into(),
        _ => {}
    }
    let command = &layout::job_commands(&job)[0].1;
    assert!(
        !command.chars().any(char::is_control),
        "{field}: {command:?}"
    );
}

#[test]
fn clipboard_payload_uses_the_sanitized_command() {
    let mut app = retry_app();
    let frame = app.snap.as_mut().unwrap();
    frame.view.jobs[0].title = "x\u{1}echo PWNED #\n".into();
    frame.view.jobs[0].actions = Some(vec![crate::wire::JobAction::Nudge { to: "codex".into() }]);
    let mut bytes = Vec::new();
    app.copy_command(0, false, &mut bytes);
    assert_eq!(
        bytes,
        b"\x1b]52;c;YW1lc2ggcGVlciBub3RpZnkgY29kZXggJ3hlY2hvIFBXTkVEICM/Jw==\x07"
    );
}

#[test]
fn command_id_drops_controls() {
    command_text_drops_controls(
        "id",
        crate::wire::JobAction::Retry {
            needs_assignee: false,
        },
    );
}

#[test]
fn command_assignee_drops_controls() {
    command_text_drops_controls("assignee", crate::wire::JobAction::Send);
}

#[test]
fn command_recipient_drops_controls() {
    command_text_drops_controls(
        "to",
        crate::wire::JobAction::Nudge {
            to: "pe\u{1}er\n".into(),
        },
    );
}

#[test]
fn clipboard_emission_refuses_a_remaining_control() {
    let mut bytes = Vec::new();
    let result = super::send_clipboard("echo\u{1}danger\n", false, &mut bytes);
    assert!(result.is_err(), "control-bearing command was accepted");
    assert!(bytes.is_empty(), "refusal wrote {bytes:?}");
}

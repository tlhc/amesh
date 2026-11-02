use super::*;

#[test]
fn b3c_worker_queued() {
    let text = b3c_card(b3c_job("queued"), vec![]);
    assert!(text.contains("worker codex (when ready)"), "{text}");
}

#[test]
fn b3c_worker_gone() {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Gone),
        busy: false,
    });
    let ask = Ask {
        correlation_id: "ask-1".into(),
        to_peer_id: "pi".into(),
        open: true,
        ..Default::default()
    };
    let text = b3c_card(job, vec![ask]);
    assert!(text.contains("worker pi left the hub"), "{text}");
}

#[test]
fn b3c_worker_wait() {
    let mut job = b3c_job("running");
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Wait {
            since: 960,
            reason: Some("Bash".into()),
        }),
        busy: false,
    });
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("worker codex WAIT! 40s") && text.contains("Bash"),
        "{text}"
    );
}

#[test]
fn b3c_worker_idle() {
    let mut job = b3c_job("running");
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Idle {
            since: 940,
            why: crate::wire::IdleWhy::TurnEnded,
        }),
        busy: false,
    });
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("why codex ended its turn 1m ago worker codex IDLE"),
        "{text}"
    );
}

#[test]
fn b3c_worker_busy() {
    let mut job = b3c_job("running");
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Work { since: 880 }),
        busy: true,
    });
    let text = b3c_card(job, vec![]);
    assert!(text.contains("worker codex WORK · turn 2m"), "{text}");
}

#[test]
fn b3c_worker_missing() {
    let mut job = b3c_job("running");
    job.worker = None;
    job.relation = Some(crate::wire::JobRelation::Missing { will_fail: true });
    job.ask_id = Some("ask-1".into());
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("worker codex")
            && text.contains("ask-1 is missing: the hub fails this job at its next check")
            && !text.contains("IDLE!"),
        "{text}"
    );
}

#[test]
fn b3c_missing_settlement_disabled() {
    let mut job = b3c_job("running");
    job.relation = Some(crate::wire::JobRelation::Missing { will_fail: false });
    job.ask_id = Some("ask-1".into());
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("ask-1 is missing; automatic settlement is disabled"),
        "{text}"
    );
}

#[test]
fn b3c_cleaned_up() {
    let mut job = b3c_job("done");
    job.relation = Some(crate::wire::JobRelation::CleanedUp);
    job.ask_id = Some("ask-1".into());
    let text = b3c_card(job, vec![]);
    assert!(text.contains("ask-1 was cleaned up"), "{text}");
}

#[test]
fn b3c_settling() {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.relation = Some(crate::wire::JobRelation::Settling);
    let ask = Ask {
        correlation_id: "ask-1".into(),
        open: false,
        state: Some(crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        }),
        ..Default::default()
    };
    let text = b3c_card(job, vec![ask]);
    assert!(text.contains("acked ok, settling"), "{text}");
}

#[test]
fn b3c_settlement_disabled() {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.relation = Some(crate::wire::JobRelation::Closed);
    let ask = Ask {
        correlation_id: "ask-1".into(),
        open: false,
        state: Some(crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        }),
        ..Default::default()
    };
    let text = b3c_card(job, vec![ask]);
    assert!(
        text.contains("acked ok; automatic settlement is disabled"),
        "{text}"
    );
}

#[test]
fn b3c_dispatch_held() {
    let mut job = b3c_job("queued");
    job.dispatch_state = Some(crate::wire::DispatchState::Held);
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("held: set its assignee again to send it"),
        "{text}"
    );
}

#[test]
fn b3c_dispatch_ready() {
    let mut job = b3c_job("queued");
    job.dispatch_state = Some(crate::wire::DispatchState::Ready {
        peer_id: "codex".into(),
    });
    let text = b3c_card(job, vec![]);
    assert!(text.contains("nothing: will be sent next tick"), "{text}");
}

#[test]
fn b3c_command_nudge() {
    let mut job = b3c_job("running");
    job.actions = Some(vec![crate::wire::JobAction::Nudge { to: "codex".into() }]);
    let text = b3c_card(job, vec![]);
    assert!(text.contains("amesh peer notify codex 'title?'"), "{text}");
}

#[test]
fn b3c_command_resend() {
    let mut job = b3c_job("running");
    job.actions = Some(vec![crate::wire::JobAction::Resend]);
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("amesh jobs update job --state queued --assigned-peer PEER"),
        "{text}"
    );
}

fn b3c_tone(grid: &layout::Grid, needle: char) -> layout::Tone {
    grid.rows
        .values()
        .flat_map(|row| row.values())
        .find(|cell| cell.ch == needle)
        .unwrap_or_else(|| panic!("no {needle}"))
        .tone
}

#[test]
fn b3c_offline_beats_a_remembered_wait() {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.worker = Some("codex".into());
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Offline),
        busy: true,
    });
    job.actions = Some(vec![]);
    let mut peer = doing("codex", "wait", 900, Some("Bash"));
    peer.liveness = Some(crate::wire::Liveness::Wait {
        since: 900,
        reason: Some("Bash".into()),
    });
    let (text, grid) = {
        let (snap, grid) = b3c_scene(job, vec![b3c_ask("cc", "codex", "codex")], vec![peer]);
        let _ = snap;
        (flat(&grid.text()), grid)
    };
    assert!(text.contains("worker codex · offline"), "{text}");
    assert!(
        !text.contains("WAIT!") && !text.contains('⣿') && !text.contains("nudge"),
        "{text}"
    );
    assert_ne!(b3c_tone(&grid, '▸'), layout::Tone::Flow);
}

#[test]
fn b3c_blocked_names_the_dependency() {
    let mut job = b3c_job("queued");
    job.depends_on = vec!["dep".into()];
    job.dispatch_state = Some(crate::wire::DispatchState::Blocked {
        dependencies: vec!["dep".into()],
        dependency: "dep".into(),
        reason: crate::wire::BlockReason::Failed,
    });
    let mut dep = b3c_job("failed");
    dep.job_id = "dep".into();
    dep.title = "sec".into();
    let snap = Snapshot {
        captured_at: 1000,
        jobs: vec![dep, job],
        ..Default::default()
    };
    let text = card(&snap, "job");
    assert!(text.contains("sec failed: retry it first"), "{text}");
}

#[test]
fn b3c_blocked_deleted_dependency() {
    let mut job = b3c_job("queued");
    job.dispatch_state = Some(crate::wire::DispatchState::Blocked {
        dependencies: vec!["gone".into()],
        dependency: "gone".into(),
        reason: crate::wire::BlockReason::Deleted,
    });
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("gone was deleted: this job cannot start"),
        "{text}"
    );
}

fn b3c_gone(to_id: &str, to_name: &str, assignee: Option<&str>) -> String {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.assigned_peer = assignee.map(str::to_string);
    job.worker = None;
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Gone),
        busy: false,
    });
    b3c_text(job, vec![b3c_ask("cc", to_id, to_name)], vec![])
}

#[test]
fn b3c_gone_label_to_peer_id() {
    let text = b3c_gone("pi", "display", Some("codex"));
    assert!(
        text.contains("why pi left the hub worker pi left the hub"),
        "{text}"
    );
}

#[test]
fn b3c_gone_label_to_peer() {
    let text = b3c_gone("", "display", Some("codex"));
    assert!(text.contains("worker display left the hub"), "{text}");
}

#[test]
fn b3c_gone_label_assignee() {
    let text = b3c_gone("", "", Some("codex"));
    assert!(text.contains("worker codex left the hub"), "{text}");
}

#[test]
fn b3c_gone_label_dash() {
    let text = b3c_gone("", "", None);
    assert!(text.contains("worker - left the hub"), "{text}");
}

#[test]
fn b3c_worker_set_renders_doing() {
    let mut job = b3c_job("done");
    job.worker = Some("w9".into());
    job.assigned_peer = Some("raw".into());
    let mut peer = doing("w9", "idle", 1, None);
    peer.name = "Ada".into();
    peer.liveness = Some(crate::wire::Liveness::Idle { since: 1 });
    let text = b3c_text(job, vec![], vec![peer]);
    assert!(
        text.contains("worker Ada · now IDLE") && !text.contains("raw"),
        "{text}"
    );
}

#[test]
fn b3c_worker_null_renders_assignee() {
    let mut job = b3c_job("done");
    job.worker = None;
    job.assigned_peer = Some("raw".into());
    let text = b3c_text(job, vec![], vec![]);
    assert!(
        text.contains("worker raw") && !text.contains("· now"),
        "{text}"
    );
}

#[test]
fn b3c_unassigned_dispatch_off() {
    let mut job = b3c_job("queued");
    job.dispatch = false;
    job.dispatch_state = Some(crate::wire::DispatchState::Unassigned);
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("not dispatched: name an assignee to send it"),
        "{text}"
    );
}

#[test]
fn b3c_no_peer() {
    let mut job = b3c_job("queued");
    job.dispatch_state = Some(crate::wire::DispatchState::NoPeer {
        name: "ghost".into(),
    });
    let text = b3c_card(job, vec![]);
    assert!(text.contains("no peer named ghost: not sent"), "{text}");
}

#[test]
fn b3c_other_circle() {
    let mut job = b3c_job("queued");
    job.dispatch_state = Some(crate::wire::DispatchState::OtherCircle {
        peer_id: "codex".into(),
    });
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("codex is in another circle: not sent"),
        "{text}"
    );
}

#[test]
fn b3c_retry_without_assignee() {
    let mut job = b3c_job("failed");
    job.actions = Some(vec![crate::wire::JobAction::Retry {
        needs_assignee: false,
    }]);
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("amesh jobs update job --state queued") && !text.contains("assigned-peer"),
        "{text}"
    );
}

#[test]
fn b3c_retry_with_assignee() {
    let mut job = b3c_job("failed");
    job.actions = Some(vec![crate::wire::JobAction::Retry {
        needs_assignee: true,
    }]);
    let text = b3c_card(job, vec![]);
    assert!(text.contains("--assigned-peer codex"), "{text}");
}

#[test]
fn b3c_command_send() {
    let mut job = b3c_job("queued");
    job.actions = Some(vec![crate::wire::JobAction::Send]);
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("amesh jobs update job --state queued --assigned-peer codex"),
        "{text}"
    );
}

#[test]
fn b3c_work_not_busy() {
    let mut job = b3c_job("running");
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Work { since: 880 }),
        busy: false,
    });
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("· now WORK") && !text.contains("WORK · turn"),
        "{text}"
    );
}

#[test]
fn b3c_pending_idle_word() {
    let mut job = b3c_job("running");
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Pending { since: Some(820) }),
        busy: false,
    });
    let mut peer = doing("codex", "idle", 1, None);
    peer.liveness = Some(crate::wire::Liveness::Idle { since: 1 });
    let text = b3c_text(job, vec![], vec![peer]);
    assert!(
        text.contains("· now IDLE") && !text.contains("IDLE!"),
        "{text}"
    );
}

#[test]
fn b3c_pending_no_word() {
    let mut job = b3c_job("running");
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Pending { since: Some(820) }),
        busy: false,
    });
    let mut peer = doing("codex", "idle", 1, None);
    peer.liveness = Some(crate::wire::Liveness::Online);
    let text = b3c_text(job, vec![], vec![peer]);
    assert!(
        text.contains("worker codex") && !text.contains("· now"),
        "{text}"
    );
}

#[test]
fn b3c_unacked_label_is_dim_when_idle() {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.relation = Some(crate::wire::JobRelation::Open);
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Idle {
            since: 940,
            why: crate::wire::IdleWhy::TurnEnded,
        }),
        busy: false,
    });
    let (_, grid) = b3c_scene(job, vec![b3c_ask("cc", "codex", "codex")], vec![]);
    assert!(grid.text().contains("unacked"));
    assert_eq!(b3c_word_tone(&grid, "unacked"), layout::Tone::Dim);
}

#[test]
fn b3c_unacked_label_is_dim_while_working() {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.relation = Some(crate::wire::JobRelation::Open);
    job.progress = Some(crate::wire::JobProgress {
        state: Some(crate::wire::Progress::Work { since: 900 }),
        busy: false,
    });
    let (_, grid) = b3c_scene(job, vec![b3c_ask("cc", "codex", "codex")], vec![]);
    assert!(grid.text().contains("unacked"));
    assert_eq!(b3c_word_tone(&grid, "unacked"), layout::Tone::Dim);
}

#[test]
fn b3c_previous_attempt() {
    let mut job = b3c_job("queued");
    job.ask_id = Some("ask-1".into());
    let text = b3c_text(job, vec![b3c_ask("cc", "codex", "codex")], vec![]);
    assert!(text.contains("previous attempt"), "{text}");
}

#[test]
fn b3c_times_queued() {
    let text = b3c_card(b3c_job("queued"), vec![]);
    assert!(text.contains("not sent yet"), "{text}");
}

#[test]
fn b3c_times_running() {
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    let text = b3c_text(job, vec![b3c_ask("cc", "codex", "codex")], vec![]);
    assert!(
        text.contains("sent 00:13") && text.contains("running 3m"),
        "{text}"
    );
}

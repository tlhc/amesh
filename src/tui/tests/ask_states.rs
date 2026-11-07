use super::*;

fn snap(ask: Ask, peers: Vec<Peer>) -> Snapshot {
    Snapshot {
        captured_at: 1000,
        asks: vec![ask],
        peers: peers.clone(),
        roster: peers,
        ..Default::default()
    }
}

fn row(ask: Ask, fresh: bool) -> Vec<(String, layout::Tone)> {
    let snap = snap(ask, vec![]);
    let mut frame = as_frame(&snap);
    frame.fresh = fresh;
    let order = super::asks::order(&snap);
    super::asks::rows(&frame, &order, 80).remove(0)
}

fn card(ask: Ask, peers: Vec<Peer>) -> String {
    let snap = snap(ask, peers);
    super::asks::card(&as_frame(&snap), &snap.asks[0], 1, 90, None, Fit::Whole).text()
}

#[test]
fn glyph_open_is_run() {
    let row = row(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Pending { since: Some(820) },
            },
            Some(820),
        ),
        false,
    );
    assert_eq!(cell(&row, "◆"), layout::Tone::Run);
}

#[test]
fn glyph_failed_is_fail() {
    let row = row(
        ask_with(
            crate::wire::AskState::Closed {
                outcome: crate::wire::AskOutcome::Hub,
                failed_effective: true,
            },
            Some(820),
        ),
        false,
    );
    assert_eq!(cell(&row, "×"), layout::Tone::Fail);
}

#[test]
fn glyph_closed_ok_is_done() {
    let row = row(
        ask_with(
            crate::wire::AskState::Closed {
                outcome: crate::wire::AskOutcome::AckedOk,
                failed_effective: false,
            },
            Some(820),
        ),
        false,
    );
    assert_eq!(cell(&row, "●"), layout::Tone::Done);
}

#[test]
fn arrow_work_is_flow() {
    let row = row(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Work { since: 990 },
            },
            Some(820),
        ),
        true,
    );
    assert_eq!(cell(&row, "─▸─"), layout::Tone::Flow);
}

#[test]
fn arrow_pending_is_dim() {
    let row = row(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Pending { since: Some(820) },
            },
            Some(820),
        ),
        true,
    );
    assert_eq!(cell(&row, "─▸─"), layout::Tone::Dim);
}

#[test]
fn arrow_stale_close_is_plain() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        },
        Some(820),
    );
    ask.closed_just_now = Some(true);
    let row = row(ask, false);
    assert_eq!(cell(&row, ">"), layout::Tone::Plain);
}

#[test]
fn arrow_just_failed_is_fail() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::Hub,
            failed_effective: true,
        },
        Some(820),
    );
    ask.closed_just_now = Some(true);
    let row = row(ask, true);
    assert_eq!(cell(&row, "─×─"), layout::Tone::Fail);
}

#[test]
fn arrow_just_ok_is_back() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        },
        Some(820),
    );
    ask.closed_just_now = Some(true);
    let row = row(ask, true);
    assert_eq!(cell(&row, "─◂─"), layout::Tone::Back);
}

#[test]
fn card_wait() {
    let text = card(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Wait {
                    since: 960,
                    reason: None,
                },
            },
            Some(820),
        ),
        vec![],
    );
    assert!(
        text.lines().next().unwrap().ends_with("─ WAIT ┐")
            && text.contains("│ why     waiting for permission "),
        "{text}"
    );
}

fn idle(why: crate::wire::IdleWhy) -> String {
    card(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Idle { since: 940, why },
            },
            Some(820),
        ),
        vec![],
    )
}

#[test]
fn card_idle_not_picked_up() {
    let text = idle(crate::wire::IdleWhy::NotPickedUp);
    assert!(
        text.contains("│ why     amesh-pi: not picked up · 1m "),
        "{text}"
    );
}

#[test]
fn card_idle_still_open() {
    let text = idle(crate::wire::IdleWhy::AskStillOpen);
    assert!(
        text.contains("│ why     amesh-pi: ask still open · 1m "),
        "{text}"
    );
}

#[test]
fn card_idle_turn_ended() {
    let text = idle(crate::wire::IdleWhy::TurnEnded);
    assert!(
        text.contains("│ why     amesh-pi ended its turn 1m ago "),
        "{text}"
    );
}

#[test]
fn card_gone() {
    let mut from = doing("amesh-claude-code", "work", 1, None);
    from.liveness = Some(crate::wire::Liveness::Online);
    let text = card(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Gone,
            },
            Some(820),
        ),
        vec![from],
    );
    assert!(
        text.contains("│ why     amesh-pi left the hub")
            && !text.contains("amesh-pi · left the hub"),
        "the why says it and the to row does not repeat it:\n{text}"
    );
}

#[test]
fn card_offline() {
    let text = card(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Offline,
            },
            Some(820),
        ),
        vec![],
    );
    assert!(
        text.lines().next().unwrap().ends_with("─ OFFLINE ┐")
            && text.contains("│ why     amesh-pi is offline "),
        "{text}"
    );
}

fn pending_to(kind: crate::wire::Liveness) -> String {
    let mut peer = doing("amesh-pi", "idle", 1, None);
    peer.liveness = Some(kind);
    card(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Pending { since: Some(820) },
            },
            Some(820),
        ),
        vec![peer],
    )
}

#[test]
fn card_now_work() {
    let text = pending_to(crate::wire::Liveness::Work { since: 1 });
    assert!(text.contains("· now WORK"), "{text}");
}

#[test]
fn card_now_idle() {
    let text = pending_to(crate::wire::Liveness::Idle { since: 1 });
    assert!(text.contains("· now IDLE"), "{text}");
}

#[test]
fn card_now_wait() {
    let text = pending_to(crate::wire::Liveness::Wait {
        since: 1,
        reason: None,
    });
    assert!(text.contains("· now WAIT"), "{text}");
}

#[test]
fn card_now_offline() {
    let text = pending_to(crate::wire::Liveness::Offline);
    assert!(text.contains("· offline"), "{text}");
}

#[test]
fn card_online_is_empty() {
    let text = pending_to(crate::wire::Liveness::Online);
    assert!(!text.contains("· now"), "{text}");
}

#[test]
fn times_open() {
    let text = card(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Pending { since: Some(820) },
            },
            Some(820),
        ),
        vec![],
    );
    assert!(text.contains("sent 00:13 · waiting 3m"), "{text}");
}

#[test]
fn times_unknown() {
    let text = card(
        ask_with(
            crate::wire::AskState::Open {
                progress: crate::wire::Progress::Pending { since: None },
            },
            None,
        ),
        vec![],
    );
    assert!(text.contains("sent before the hub kept the time"), "{text}");
}

#[test]
fn times_closed() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        },
        Some(820),
    );
    ask.closed_at = Some(950);
    let text = card(ask, vec![]);
    assert!(
        text.contains("sent 00:13 · closed 00:15 · took 2m"),
        "{text}"
    );
}

#[test]
fn command_nudge() {
    let mut ask = ask_with(
        crate::wire::AskState::Open {
            progress: crate::wire::Progress::Idle {
                since: 940,
                why: crate::wire::IdleWhy::TurnEnded,
            },
        },
        Some(820),
    );
    ask.actions = Some(vec![crate::wire::AskAction::Nudge {
        to: "amesh-pi".into(),
    }]);
    let text = card(ask, vec![]);
    assert!(
        text.contains("amesh peer notify amesh-pi 'ask row waits for your ack'"),
        "{text}"
    );
}

#[test]
fn command_close_left() {
    let mut ask = ask_with(
        crate::wire::AskState::Open {
            progress: crate::wire::Progress::Gone,
        },
        Some(820),
    );
    ask.actions = Some(vec![crate::wire::AskAction::CloseLeft]);
    let text = card(ask, vec![]);
    assert!(
        text.contains("amesh peer ack ask-row --failed true --message 'recipient left'"),
        "{text}"
    );
}

#[test]
fn header_counts_hub_as_failed() {
    use crate::wire::{AskOutcome, AskState};
    let snap = Snapshot {
        asks: vec![
            ask_with(
                AskState::Open {
                    progress: crate::wire::Progress::Pending { since: Some(1) },
                },
                Some(1),
            ),
            ask_with(
                AskState::Closed {
                    outcome: AskOutcome::Hub,
                    failed_effective: true,
                },
                Some(1),
            ),
            ask_with(
                AskState::Closed {
                    outcome: AskOutcome::AckedOk,
                    failed_effective: false,
                },
                Some(1),
            ),
        ],
        ..Default::default()
    };
    let order = super::asks::order(&snap);
    assert_eq!(
        super::asks::header(&order, false),
        "asks · 1 open · 1 answered · 1 failed"
    );
}

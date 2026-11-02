use super::*;

/* one view with each thing the attention line lists: w3 at a prompt, w4 idle on two asks
and listed once from the one that stalled first, w5's queue stuck without push, w9's queue
stuck under push, and s1's failed job sec. w6 has no push and works on the ask it holds,
w7's ask is still in its pickup grace, w8 has no push and no ask, and w10 is offline, so
none of those four is listed. Each state is one the hub derives from the peer's activity */
fn attention_view() -> Snapshot {
    use crate::wire::{AskState, DeliveryCondition, IdleWhy, Liveness, Progress};
    let mut w3 = peer("w3", "online", Some("wait"));
    w3.liveness = Some(Liveness::Wait {
        since: 880,
        reason: Some("Claude needs your permission to use Bash".into()),
    });
    let mut w4 = peer("w4", "online", Some("idle"));
    w4.liveness = Some(Liveness::Idle { since: 940 });
    let mut w6 = marked("w6", DeliveryCondition::NoPush, None);
    w6.liveness = Some(Liveness::Work { since: 990 });
    let mut w10 = marked("w10", DeliveryCondition::Offline, Some((3, 900)));
    w10.status = "offline".into();
    w10.liveness = Some(Liveness::Offline);
    let mut snap = delivering(
        s1(),
        vec![
            w3,
            w4,
            marked("w5", DeliveryCondition::NoPush, Some((2, 970))),
            w6,
            peer("w7", "online", Some("idle")),
            marked("w8", DeliveryCondition::NoPush, None),
            marked("w9", DeliveryCondition::Push, Some((1, 990))),
            w10,
        ],
        1000,
    );
    snap.capabilities.ask_list = true;
    for (i, (to, opened, progress)) in [
        (
            "w4",
            950,
            Progress::Idle {
                since: 950,
                why: IdleWhy::NotPickedUp,
            },
        ),
        (
            "w4",
            930,
            Progress::Idle {
                since: 940,
                why: IdleWhy::TurnEnded,
            },
        ),
        ("w6", 930, Progress::Work { since: 990 }),
        ("w7", 995, Progress::Pending { since: Some(995) }),
    ]
    .into_iter()
    .enumerate()
    {
        snap.asks.push(Ask {
            correlation_id: format!("ask-{i}-{to}"),
            to_peer: to.into(),
            to_peer_id: to.into(),
            open: true,
            opened_at: Some(opened),
            state: Some(AskState::Open { progress }),
            ..Default::default()
        });
    }
    snap
}

/* what attention_view's row under the rule reads */
const ATTENTION: &str = "attention  WAIT w3 2m  Claude needs your permission to use Bash  IDLE w4 1m  turn ended  ✉2 w5  no push  ✉1 w9   × sec";

/* the first row that starts with `prefix` */
fn first_row(lines: &[String], prefix: &str) -> usize {
    lines
        .iter()
        .position(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no row starting {prefix:?}:\n{}", lines.join("\n")))
}

fn two_minutes_ago() -> Option<std::time::Instant> {
    std::time::Instant::now().checked_sub(std::time::Duration::from_secs(120))
}

#[test]
fn the_attention_line_lists_what_needs_a_person() {
    let mut app = app_with(attention_view());
    let lines = screen_text(&mut app, 160, 40);
    assert_eq!(
        lines[rule_row(&lines) + 1],
        ATTENTION,
        "{}",
        lines.join("\n")
    );
    let heads: Vec<layout::Tone> = layout::attention(&attention_view())
        .iter()
        .map(|item| item[0].1)
        .collect();
    assert_eq!(
        heads,
        [
            layout::Tone::Badge,
            layout::Tone::PillWait,
            layout::Tone::PillWait,
            layout::Tone::PillWait,
            layout::Tone::PillFail
        ]
    );
    app.opts.ascii = true;
    let lines = screen_text(&mut app, 160, 40);
    assert_eq!(
        lines[rule_row(&lines) + 1],
        "attention  WAIT w3 2m  Claude needs your permission to use Bash  IDLE w4 1m  turn ended  +2 w5  no push  +1 w9   x sec"
    );
    let mut app = app_with(s4());
    let lines = screen_text(&mut app, 100, 40);
    assert_eq!(
        lines[rule_row(&lines) + 1],
        "",
        "nothing needs a person:\n{}",
        lines.join("\n")
    );
}

#[test]
fn a_stale_snapshot_leads_the_attention_line_and_the_edge_cuts_it() {
    let mut app = app_with(attention_view());
    let lines = screen_text(&mut app, 46, 40);
    assert_eq!(
        lines[rule_row(&lines) + 1],
        "attention  WAIT w3 2m   IDLE w4 1m   ✉2 w5   …",
        "every head before any reason, the row cut at the edge"
    );
    app.apply(Err("connection refused".into()));
    /* the error leads in a few words, leaving the items room; the rule shows the age */
    let lines = screen_text(&mut app, 46, 40);
    assert_eq!(
        lines[rule_row(&lines) + 1],
        "! connection refused ·  WAIT w3 2m   IDLE w4 …"
    );
    let lines = screen_text(&mut app, 200, 40);
    let row = &lines[rule_row(&lines) + 1];
    assert!(row.ends_with("   × sec"), "{row}");
}

fn attention_snapshot() -> Snapshot {
    let mut w3 = super::model::Peer {
        peer_id: "w3".into(),
        name: "w3".into(),
        status: "online".into(),
        ..Default::default()
    };
    w3.liveness = Some(Liveness::Wait {
        since: 880,
        reason: Some("Claude needs your permission to use Bash".into()),
    });
    let mut w4 = w3.clone();
    w4.peer_id = "w4".into();
    w4.name = "w4".into();
    w4.liveness = Some(Liveness::Idle { since: 940 });
    let mut snap = Snapshot {
        captured_at: 1000,
        jobs: vec![job("sec", "failed", &[], 1)],
        roster: vec![w3, w4],
        ..Default::default()
    };
    snap.asks.push(super::model::Ask {
        correlation_id: "ask-w4".into(),
        to_peer: "w4".into(),
        to_peer_id: "w4".into(),
        open: true,
        opened_at: Some(930),
        state: Some(crate::wire::AskState::Open {
            progress: Progress::Idle {
                since: 940,
                why: crate::wire::IdleWhy::TurnEnded,
            },
        }),
        ..Default::default()
    });
    snap
}

fn attention_row(app: &mut App, cols: usize) -> String {
    let lines = screen_text(app, cols, 40);
    lines[rule_row(&lines) + 1].clone()
}

#[test]
fn reasons_follow_their_heads_while_room_remains() {
    let mut app = app_with(attention_snapshot());
    assert_eq!(
        attention_row(&mut app, 90),
        "attention  WAIT w3 2m  Claude needs your permission to use Bash  IDLE w4 1m   × sec",
        "the first reason fits after every head, the second does not"
    );
}

#[test]
fn the_attention_line_packs_six_thousand_heads_within_a_second() {
    let snap = Snapshot {
        jobs: (0..6000)
            .map(|i| job(&format!("failed-{i}"), "failed", &[], i))
            .collect(),
        ..Default::default()
    };
    let a = app_with(snap);
    let mut g = layout::Grid::default();
    let start = std::time::Instant::now();
    a.under_rule(&mut g, 0, 80, None);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "6000 heads: {elapsed:?}"
    );
}

#[test]
fn the_rule_ends_in_how_old_the_snapshot_is() {
    let mut snap = attention_view();
    snap.capabilities.event_count = true;
    snap.event_count = 14;
    let mut app = app_with(snap);
    let rule = |app: &mut App, cols: usize| {
        let lines = screen_text(app, cols, 40);
        lines[rule_row(&lines)].clone()
    };
    let fresh = rule(&mut app, 100);
    assert!(
        fresh.ends_with("─ 4 asks open · 14 events · ● ─"),
        "{fresh}"
    );
    assert_eq!(app.freshness().1, layout::Tone::Done);
    /* past the fresh two seconds the age rounds up */
    app.last_ok = std::time::Instant::now().checked_sub(std::time::Duration::from_millis(2100));
    let just = rule(&mut app, 100);
    assert!(
        just.ends_with("─ 4 asks open · 14 events · ○ 3s ─"),
        "{just}"
    );
    assert_eq!(app.freshness().1, layout::Tone::Warn);
    app.last_ok = two_minutes_ago();
    let old = rule(&mut app, 100);
    assert!(aged(&old, "─ 4 asks open · 14 events · ○ ", " ─"), "{old}");
    /* a narrow pane drops the tags before the mark */
    let narrow = rule(&mut app, 30);
    assert!(aged(&narrow, "─ 4 asks open · ○ ", " ─"), "{narrow}");
    app.opts.ascii = true;
    let ascii = rule(&mut app, 100);
    assert!(
        aged(&ascii, "- 4 asks open | 14 events | o ", " -"),
        "{ascii}"
    );
    app.last_ok = Some(std::time::Instant::now());
    let ascii = rule(&mut app, 100);
    assert!(
        ascii.ends_with("- 4 asks open | 14 events | * -"),
        "{ascii}"
    );
}

#[test]
fn the_asks_and_events_screens_carry_the_attention_line_and_the_mark() {
    let mut app = app_with(attention_view());
    app.key(KeyCode::Char('a'));
    let lines = screen_text(&mut app, 160, 40);
    let at = rule_row(&lines);
    assert!(lines[at].ends_with("──── ● ─"), "{}", lines.join("\n"));
    assert_eq!(lines[at + 1], ATTENTION, "{}", lines.join("\n"));
    app.key(KeyCode::Char('e'));
    let lines = screen_text(&mut app, 160, 40);
    let at = rule_row(&lines);
    assert!(
        lines[at].ends_with("─ 4 asks open · ● ─"),
        "{}",
        lines.join("\n")
    );
    assert_eq!(lines[at + 1], ATTENTION, "{}", lines.join("\n"));
    /* on the asks screen a long tag gives way to the mark, and an error leads the row */
    let mut snap = attention_view();
    snap.capabilities.event_count = true;
    snap.event_count = 123_456_789_012;
    let mut app = app_with(snap);
    app.key(KeyCode::Char('a'));
    app.last_ok = two_minutes_ago();
    let lines = screen_text(&mut app, 30, 40);
    let rule = &lines[rule_row(&lines)];
    assert!(aged(rule, "──── ○ ", " ─"), "{}", lines.join("\n"));
    app.apply(Err("connection refused".into()));
    let lines = screen_text(&mut app, 160, 40);
    let row = &lines[rule_row(&lines) + 1];
    assert!(
        row.starts_with("! connection refused ·  WAIT w3 2m"),
        "{row}"
    );
    app.key(KeyCode::Char('e'));
    let lines = screen_text(&mut app, 160, 40);
    let row = &lines[rule_row(&lines) + 1];
    assert!(
        row.starts_with("! connection refused ·  WAIT w3 2m"),
        "{row}"
    );
}

#[test]
fn screens_with_nothing_listed_name_what_needs_a_person() {
    let mut snap = attention_view();
    snap.jobs.clear();
    let mut app = app_with(snap);
    let items = ATTENTION.strip_suffix("   × sec").unwrap();
    let lines = screen_text(&mut app, 200, 40);
    let at = first_row(&lines, "no jobs here yet");
    assert!(lines[at].ends_with(" 4 asks open · ●"), "{}", lines[at]);
    assert_eq!(lines[at + 1], items, "{}", lines.join("\n"));
    app.last_ok = two_minutes_ago();
    let lines = screen_text(&mut app, 200, 40);
    assert!(aged(&lines[at], " 4 asks open · ○ ", ""), "{}", lines[at]);
    app.apply(Err("connection refused".into()));
    let lines = screen_text(&mut app, 200, 40);
    assert_eq!(lines[at + 1], items, "{}", lines.join("\n"));
    /* a wrapped error keeps its first line whole and takes the mark on its last */
    let lines = screen_text(&mut app, 46, 40);
    let at = first_row(&lines, "! connection refused");
    assert_eq!(lines[at], "! connection refused · showing the snapshot");
    assert!(aged(&lines[at + 1], "  ○ ", ""), "{}", lines[at + 1]);
    /* with nothing to list, the hint follows the message */
    let mut app = app_with(Snapshot {
        captured_at: 1000,
        ..Default::default()
    });
    let lines = screen_text(&mut app, 100, 20);
    let at = first_row(&lines, "no jobs here yet");
    assert!(
        lines[at + 1].starts_with("jobs appear once created"),
        "{}",
        lines.join("\n")
    );
    let mut quiet = attention_view();
    quiet.asks.clear();
    quiet.capabilities.event_count = true;
    quiet.event_count = 14;
    let mut app = app_with(quiet);
    app.key(KeyCode::Char('a'));
    let lines = screen_text(&mut app, 200, 40);
    let at = first_row(&lines, "no asks here yet");
    assert!(lines[at].ends_with("  14 events · ●"), "{}", lines[at]);
    assert_eq!(
        lines[at + 1],
        "attention  WAIT w3 2m  Claude needs your permission to use Bash  ✉2 w5  no push  ✉1 w9   × sec",
        "{}",
        lines.join("\n")
    );
    app.last_ok = two_minutes_ago();
    let lines = screen_text(&mut app, 200, 40);
    assert!(aged(&lines[at], "  14 events · ○ ", ""), "{}", lines[at]);
}

#[test]
fn the_events_rule_has_no_mark_without_a_snapshot() {
    let mut app = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    app.key(KeyCode::Char('e'));
    let lines = screen_text(&mut app, 80, 20);
    assert_eq!(lines[rule_row(&lines)], "─".repeat(80));
    let mut app = app_with(attention_view());
    app.apply(Ok(Snapshot {
        schema_version: crate::wire::SNAPSHOT_VERSION + 1,
        ..Default::default()
    }));
    app.key(KeyCode::Char('e'));
    let lines = screen_text(&mut app, 80, 20);
    assert_eq!(lines[rule_row(&lines)], "─".repeat(80));
}

/* an ask sent out of the circle: its recipient comes in peers, not the roster, under the
name it goes by now; a peer in both is listed once, and a peer the view only refers to is
not listed */
#[test]
fn a_recipient_outside_the_circle_is_listed_once() {
    use crate::wire::{AskState, DeliveryCondition, IdleWhy, Liveness, Progress};
    let mut w3 = peer("w3", "online", Some("wait"));
    w3.liveness = Some(Liveness::Wait {
        since: 940,
        reason: None,
    });
    let mut x1 = marked("x1", DeliveryCondition::NoPush, Some((2, 970)));
    x1.liveness = Some(Liveness::Wait {
        since: 970,
        reason: None,
    });
    let mut x2 = peer("x2", "online", Some("idle"));
    x2.name = "x2-now".into();
    let mut y = peer("y", "online", Some("wait"));
    y.liveness = Some(Liveness::Wait {
        since: 900,
        reason: None,
    });
    let mut snap = delivering(Snapshot::default(), vec![w3.clone()], 1000);
    snap.peers = vec![w3, x1, x2, y];
    for (to, called, progress) in [
        (
            "x1",
            "x1",
            Progress::Wait {
                since: 970,
                reason: None,
            },
        ),
        (
            "x2",
            "x2-then",
            Progress::Idle {
                since: 990,
                why: IdleWhy::Unknown,
            },
        ),
    ] {
        snap.asks.push(Ask {
            correlation_id: format!("ask-{to}"),
            to_peer: called.into(),
            to_peer_id: to.into(),
            open: true,
            state: Some(AskState::Open { progress }),
            ..Default::default()
        });
    }
    let items: Vec<String> = layout::attention(&snap)
        .iter()
        .map(|item| joined(item))
        .collect();
    assert_eq!(
        items,
        [
            "WAIT w3 1m",
            "WAIT x1 30s",
            "IDLE x2-now 10s",
            "✉2 x1 no push"
        ]
    );
}

#[test]
fn a_row_cut_where_a_piece_ends_still_ends_in_an_ellipsis() {
    let row = |reason: &str| {
        let mut snap = attention_view();
        snap.roster[0].liveness = Some(crate::wire::Liveness::Wait {
            since: 880,
            reason: Some(reason.into()),
        });
        let mut app = app_with(snap);
        let lines = screen_text(&mut app, 30, 40);
        lines[rule_row(&lines) + 1].clone()
    };
    /* the heads alone overflow: the IDLE item is cut where the edge falls and ends in … */
    assert_eq!(row("123456789"), "attention  WAIT w3 2m   IDLE …");
    /* a wide name at the edge still leaves a row no wider than the pane, ending in … */
    let mut snap = attention_view();
    snap.roster[0].name = "审查者审查者".into();
    let mut app = app_with(snap);
    for cols in 30..60 {
        let lines = screen_text(&mut app, cols, 40);
        let cut = &lines[rule_row(&lines) + 1];
        assert!(cut.ends_with('…') && width(cut) <= cols, "{cols}: {cut}");
    }
    /* a row that fills the pane exactly is not cut; the last pill keeps its padding */
    let mut app = app_with(attention_view());
    let lines = screen_text(&mut app, layout::width(ATTENTION) + 1, 40);
    assert_eq!(lines[rule_row(&lines) + 1], ATTENTION);
}

/* a piece too long for the row is cut in one pass, so a long title or reason cannot stall
a frame */
#[test]
fn a_long_piece_is_cut_in_one_pass() {
    let long = "a".repeat(40_000);
    let started = std::time::Instant::now();
    let cut = layout::ellipsis(&long, 80);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(cut, format!("{}…", "a".repeat(79)));
}

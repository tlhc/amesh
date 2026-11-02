use super::*;

fn row_text(row: &[(String, layout::Tone)]) -> String {
    row.iter().map(|(t, _)| t.as_str()).collect()
}

#[test]
fn the_asks_of_jobs_stay_on_their_cards() {
    let mut snap = s1();
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .ask_id = Some("ask-job".into());
    snap.asks.push(lone("ask-job", "cc", "cc", Some(900)));
    snap.asks.push(lone("ask-lone", "cc", "cc", Some(900)));
    let ids: Vec<&str> = snap
        .asks_outside_jobs()
        .iter()
        .map(|a| a.correlation_id.as_str())
        .collect();
    assert_eq!(ids, ["ask-lone"]);
}

#[test]
fn the_asks_are_listed_by_what_they_need_then_by_age() {
    use crate::wire::{AskOutcome, AskState, IdleWhy, Progress};
    let open = |cid, opened, progress| {
        stamp(
            lone(cid, CC, PI, opened),
            AskState::Open { progress },
            vec![],
            None,
        )
    };
    let wait = |since| Progress::Wait {
        since,
        reason: None,
    };
    let idle = |since| Progress::Idle {
        since,
        why: IdleWhy::TurnEnded,
    };
    let shut = |cid, at| {
        let mut ask = closed(lone(cid, CC, PI, Some(10)), 0, "recipient", false, "r");
        ask.closed_at = at;
        ask.state = Some(AskState::Closed {
            outcome: AskOutcome::AckedOk,
            failed_effective: false,
        });
        ask
    };
    let snap = with_asks(
        Snapshot::default(),
        vec![
            lone("ask-b", CC, PI, Some(900)),
            closed(
                lone("ask-c1", CC, PI, Some(10)),
                950,
                "recipient",
                false,
                "r",
            ),
            lone("ask-legacy", CC, PI, None),
            lone("ask-a", CC, PI, Some(800)),
            closed(
                lone("ask-c2", CC, PI, Some(10)),
                990,
                "recipient",
                false,
                "r",
            ),
            shut("ask-c0", None),
            shut("ask-c3", Some(970)),
            open("ask-work", Some(850), Progress::Work { since: 990 }),
            open("ask-pending", Some(820), Progress::Pending { since: None }),
            open("ask-unknown", None, Progress::Unknown),
            open("ask-left-x", None, Progress::Gone),
            open("ask-offline", Some(500), Progress::Offline),
            open("ask-left", Some(400), Progress::Gone),
            open("ask-idle-new", Some(100), idle(980)),
            open("ask-idle-old", Some(900), idle(950)),
            open("ask-wait-y", Some(900), wait(960)),
            open("ask-wait-new", Some(100), wait(990)),
            open("ask-wait-x", Some(950), wait(960)),
        ],
        vec![],
    );
    let ids: Vec<&str> = super::asks::order(&snap)
        .iter()
        .map(|a| a.correlation_id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "ask-wait-x",
            "ask-wait-y",
            "ask-wait-new",
            "ask-idle-old",
            "ask-idle-new",
            "ask-left",
            "ask-offline",
            "ask-left-x",
            "ask-a",
            "ask-pending",
            "ask-work",
            "ask-b",
            "ask-legacy",
            "ask-unknown",
            "ask-c2",
            "ask-c3",
            "ask-c1",
            "ask-c0",
        ]
    );
}

#[test]
fn each_ask_reads_in_the_words_of_the_job_card() {
    use crate::wire::{AskOutcome, AskState, IdleWhy, Progress};
    use layout::Tone;
    let snap = Snapshot {
        captured_at: 1000,
        ..Default::default()
    };
    let line = |state| {
        super::asks::state(
            &snap,
            &stamp(lone("ask-x", CC, PI, Some(820)), state, vec![], None),
        )
    };
    assert_eq!(
        line(AskState::Open {
            progress: Progress::Work { since: 900 },
        }),
        ("working 3m".into(), Tone::Run)
    );
    assert_eq!(
        line(AskState::Open {
            progress: Progress::Idle {
                since: 940,
                why: IdleWhy::TurnEnded,
            },
        }),
        ("IDLE 1m".into(), Tone::PillWait)
    );
    assert_eq!(
        line(AskState::Open {
            progress: Progress::Wait {
                since: 960,
                reason: None,
            },
        }),
        ("WAIT 40s".into(), Tone::Badge)
    );
    assert_eq!(
        line(AskState::Open {
            progress: Progress::Offline,
        }),
        ("OFFLINE".into(), Tone::PillFail)
    );
    assert_eq!(
        line(AskState::Open {
            progress: Progress::Gone,
        }),
        ("LEFT".into(), Tone::PillFail)
    );
    assert_eq!(
        line(AskState::Open {
            progress: Progress::Unknown,
        }),
        ("waiting 3m".into(), Tone::Run)
    );
    assert_eq!(
        super::asks::state(&snap, &lone("ask-x", CC, PI, Some(820))),
        ("waiting 3m".into(), Tone::Run),
        "a v1 open ask"
    );
    let pending = stamp(
        lone("ask-x", CC, PI, Some(820)),
        AskState::Open {
            progress: Progress::Pending { since: Some(820) },
        },
        vec![],
        None,
    );
    let mut quiet = with_asks(
        snap.clone(),
        vec![pending],
        vec![doing(PI, "idle", 940, None)],
    );
    quiet.capabilities.peer_activity = false;
    assert_eq!(
        super::asks::state(&quiet, &quiet.asks[0]),
        ("waiting 3m".into(), Tone::Run)
    );
    let shut = |outcome, failed| {
        super::asks::state(
            &snap,
            &stamp(
                closed(lone("ask-x", CC, PI, Some(820)), 950, "", failed, "r"),
                AskState::Closed {
                    outcome,
                    failed_effective: failed,
                },
                vec![],
                None,
            ),
        )
    };
    assert_eq!(
        shut(AskOutcome::AckedOk, false),
        ("acked ok".into(), Tone::Done)
    );
    assert_eq!(
        shut(AskOutcome::AckedFailed, true),
        ("acked failed".into(), Tone::Fail)
    );
    assert_eq!(
        shut(AskOutcome::HandOk, false),
        ("closed by hand".into(), Tone::Done)
    );
    assert_eq!(
        shut(AskOutcome::Hub, true),
        ("closed by hub".into(), Tone::Fail)
    );
    assert_eq!(
        shut(AskOutcome::ClosedOk, false),
        ("closed ok".into(), Tone::Done)
    );
    assert_eq!(
        shut(AskOutcome::ClosedFailed, true),
        ("closed failed".into(), Tone::Fail)
    );
}

#[test]
fn ask_rows_leave_out_the_shared_prefix_and_keep_the_state_whole() {
    let snap = fan_out();
    let order = super::asks::order(&snap);
    let text = |cols: usize| -> Vec<String> {
        super::asks::rows(&as_frame(&snap), &order, cols)
            .iter()
            .map(|row| row_text(row))
            .collect()
    };
    let wide = text(100);
    assert!(
        wide[1].starts_with("2 ◆ 1223 claude-code─▸─pi-2  "),
        "{}",
        wide[1]
    );
    assert!(
        wide[1].contains("working 3m") && wide[1].contains("question ask-1223c9e0"),
        "{}",
        wide[1]
    );
    assert!(wide[0].contains("IDLE 1m"), "{}", wide[0]);
    assert!(
        wide[2].starts_with("3 ● a904 claude-code>codex")
            && wide[2].contains("acked ok")
            && wide[2].contains("→ 2 findings, both fixed"),
        "{}",
        wide[2]
    );
    assert!(wide.iter().all(|row| width(row) <= 100));
    let narrow = text(46);
    assert!(
        narrow[1].trim_end().ends_with("working 3m"),
        "no room for a preview: {}",
        narrow[1]
    );
    let mut far = fan_out();
    far.asks.push(stamp(
        lone(
            "ask-0594aa3c",
            "crypto-software-agile-pi-2",
            "crypto-software-agile-pi-3",
            None,
        ),
        crate::wire::AskState::Open {
            progress: crate::wire::Progress::Gone,
        },
        vec![],
        None,
    ));
    let order = super::asks::order(&far);
    let rows: Vec<String> = super::asks::rows(&as_frame(&far), &order, 46)
        .iter()
        .map(|row| row_text(row))
        .collect();
    let gone = rows.iter().find(|r| r.contains("0594")).unwrap();
    assert!(
        gone.trim_end().ends_with("LEFT") && gone.contains('…') && width(gone) <= 46,
        "the route gives way: {gone}"
    );
    let mut anon = fan_out();
    anon.asks
        .push(lone("ask-anon0001", "anonymous", PI, Some(900)));
    let order = super::asks::order(&anon);
    let rows = super::asks::rows(&as_frame(&anon), &order, 100);
    assert!(
        rows.iter()
            .any(|row| row_text(row).contains(" anonymous─▸─pi ")),
        "anonymous keeps its name: {:?}",
        rows.iter().map(|r| row_text(r)).collect::<Vec<_>>()
    );
    let mut across = fan_out();
    across.asks.push(lone("ask-1167aa00", "far", PI, Some(900)));
    across.peers.push(doing("far", "idle", 900, None));
    let order = super::asks::order(&across);
    let rows: Vec<String> = super::asks::rows(&as_frame(&across), &order, 46)
        .iter()
        .map(|row| row_text(row))
        .collect();
    assert!(
        rows.iter().any(|r| r.contains("far─▸─pi "))
            && rows.iter().any(|r| r.contains("claude-code─▸─pi-2")),
        "a sender from another circle keeps its name and leaves the prefix to the circle's own: {rows:?}"
    );
    let mut all = fan_out();
    all.roster.clear();
    let order = super::asks::order(&all);
    let rows: Vec<String> = super::asks::rows(&as_frame(&all), &order, 46)
        .iter()
        .map(|row| row_text(row))
        .collect();
    assert!(
        [PI, PI2, CODEX]
            .iter()
            .zip(&rows)
            .all(|(to, row)| row.contains(" …") && row.contains(&format!(">{to}  "))),
        "a route that does not fit keeps its end, the recipient: {rows:?}"
    );
    let mut kin = fan_out();
    let foreign = "openhitls-sm2-opt-external";
    kin.peers.push(doing(foreign, "work", 900, None));
    kin.peers.push(doing("anonymous", "work", 900, None));
    kin.roster.push(doing("anonymous", "work", 900, None));
    kin.asks.push(lone("ask-f0000001", foreign, PI, Some(900)));
    kin.asks.push(lone(
        "ask-f0000002",
        CC,
        "openhitls-sm2-opt-pi-3",
        Some(900),
    ));
    kin.asks
        .push(lone("ask-f0000003", "anonymous", PI2, Some(900)));
    let order = super::asks::order(&kin);
    let rows: Vec<String> = super::asks::rows(&as_frame(&kin), &order, 200)
        .iter()
        .map(|row| row_text(row))
        .collect();
    assert!(
        rows.iter().any(|r| r.contains(" anonymous─▸─pi-2 ")),
        "anonymous keeps its name and leaves the prefix alone: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains(" external─▸─pi ")),
        "a name from the same folder loses the prefix wherever it comes from: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains(" claude-code─▸─pi-3 ")),
        "a peer that left loses it too: {rows:?}"
    );
}

#[test]
fn the_view_prefix_comes_off_every_name_that_starts_with_it() {
    let snap = fan_out();
    let prefix = layout::prefix(&snap);
    assert_eq!(prefix, "openhitls-sm2-opt-");
    for (name, short) in [
        (CC, "claude-code"),
        ("openhitls-sm2-opt-pi-3", "pi-3"),
        ("crypto-software-agile-pi-2", "crypto-software-agile-pi-2"),
        ("anonymous", "anonymous"),
        (prefix, prefix),
    ] {
        assert_eq!(layout::brief(name, prefix), short, "{name}");
    }
    let alone = with_roster(Snapshot::default(), vec![peer(CC, "online", None)]);
    assert_eq!(layout::prefix(&alone), "", "one peer shares nothing");
    let mixed = with_roster(
        Snapshot::default(),
        vec![
            peer(CC, "online", None),
            peer("crypto-software-agile-pi-2", "online", None),
        ],
    );
    assert_eq!(layout::prefix(&mixed), "", "two folders share no prefix");
    let mut anon = fan_out();
    anon.roster.push(doing("anonymous", "work", 900, None));
    assert_eq!(
        layout::prefix(&anon),
        prefix,
        "anonymous is no folder's peer"
    );
    let all = with_roster(
        Snapshot::default(),
        vec![
            peer("folder-", "online", None),
            peer("folder-two", "online", None),
        ],
    );
    assert_eq!(layout::prefix(&all), "", "no name gives all of itself away");
}

#[test]
fn the_top_line_and_attention_name_peers_without_the_shared_prefix() {
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    let lines = screen_text(&mut app, 46, 24);
    assert_eq!(
        lines[..2],
        [
            "4 online · ○ claude-code  ○ codex",
            "           ○ pi           ⠋ pi-2",
        ],
        "{}",
        lines.join("\n")
    );
    assert!(
        lines.iter().any(|l| l.starts_with("attention  IDLE pi 1m")),
        "{}",
        lines.join("\n")
    );
    assert_eq!(
        screen_text(&mut app, 100, 24)[0],
        "4 online · ○ claude-code  ○ codex  ○ pi  ⠋ pi-2",
        "one line once it fits"
    );
    let stuck = delivering(
        Snapshot::default(),
        vec![
            marked(
                "folder-pi",
                crate::wire::DeliveryCondition::NoPush,
                Some((2, 150)),
            ),
            marked("folder-codex", crate::wire::DeliveryCondition::Push, None),
        ],
        200,
    );
    assert_eq!(
        layout::attention(&stuck)
            .iter()
            .map(|item| joined(item))
            .collect::<Vec<_>>(),
        ["✉2 pi no push"],
        "a stuck queue names its peer short"
    );
}

#[test]
fn the_ask_card_puts_the_reply_first_and_gives_a_command_where_one_is_due() {
    let snap = fan_out();
    let find = |cid: &str| snap.asks.iter().find(|a| a.correlation_id == cid).unwrap();
    let text = |cid: &str, cols: usize| {
        super::asks::card(&as_frame(&snap), find(cid), 1, cols, None, Fit::Whole).text()
    };
    let mut long = fan_out();
    long.asks[2].reply = Some("word ".repeat(80));
    let codex =
        super::asks::card(&as_frame(&long), &long.asks[2], 3, 46, None, Fit::Rows(30)).text();
    let lines: Vec<&str> = codex.lines().collect();
    assert!(
        lines[0].starts_with("┌● 3 ask a904 · acked ok · took 2m"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("│ reply   word"),
        "the reply comes first: {}",
        lines[1]
    );
    assert!(
        lines[4].contains('…') && !lines[5].contains("word"),
        "four lines of it under the list:\n{codex}"
    );
    let whole = super::asks::card(&as_frame(&long), &long.asks[2], 3, 46, None, Fit::Whole).text();
    assert!(
        whole.lines().filter(|l| l.contains("word")).count() > 4,
        "all of it in the full card:\n{whole}"
    );
    let pi = text("ask-a3a07b51", 46);
    assert!(
        pi.contains("│ from    openhitls-sm2-opt-claude-code"),
        "{pi}"
    );
    assert!(
        pi.contains("│         · now IDLE"),
        "a status that does not fit takes its own line:\n{pi}"
    );
    assert!(
        pi.starts_with("┌◆ 1 ask a3a0 · open 3m ─")
            && pi.lines().next().unwrap().ends_with("─ IDLE ┐")
            && pi.contains("│ why     pi ended its turn 1m ago "),
        "the badge, and the why naming the recipient short:\n{pi}"
    );
    assert!(
        pi.contains(
            "│ to      openhitls-sm2-opt-pi               │\n│ times   sent 00:13 · waiting 3m "
        ),
        "the to row leaves the stall to the badge and the why:\n{pi}"
    );
    let joined: String = pi
        .lines()
        .map(|l| l.trim_matches(|c| c == '│' || c == ' '))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        pi.lines()
            .any(|l| l.starts_with("│ nudge   amesh peer notify"))
            && joined.contains("notify openhitls-sm2-opt-pi 'ask a3a0 waits for your ack'"),
        "{pi}"
    );
    assert!(
        !text("ask-1223c9e0", 46).contains("nudge"),
        "a recipient at work needs no nudge"
    );
    let mut gone = fan_out();
    gone.asks[1] = stamp(
        gone.asks[1].clone(),
        crate::wire::AskState::Open {
            progress: crate::wire::Progress::Gone,
        },
        vec![crate::wire::AskAction::CloseLeft],
        None,
    );
    let left = super::asks::card(&as_frame(&gone), &gone.asks[1], 2, 90, None, Fit::Whole).text();
    assert!(
        left.contains("│ why     pi left the hub ")
            && left.contains(
                "│ close   amesh peer ack ask-a3a07b51 --failed true --message 'recipient left'"
            ),
        "{left}"
    );
    let mut old = fan_out();
    old.asks[0].opened_at = None;
    let legacy = super::asks::card(&as_frame(&old), &old.asks[0], 1, 46, None, Fit::Whole).text();
    assert!(
        legacy.contains("open, age unknown")
            && legacy.contains("sent before the hub kept the time"),
        "{legacy}"
    );
}

/* the card under the list: a badge and why while the ask needs a person, the reply once it
is answered, one route in brief names, and the question; parties, times and commands are the
full card's */
#[test]
fn the_compact_ask_card_leads_with_why_and_names_the_route() {
    use layout::Tone;
    let busy = busy_asks();
    let mut off = busy_asks();
    let gone = off
        .asks
        .iter_mut()
        .find(|a| a.correlation_id == "ask-0c9e4a18");
    gone.unwrap().state = Some(crate::wire::AskState::Open {
        progress: crate::wire::Progress::Offline,
    });
    let card = |snap: &Snapshot, cid: &str, cols: usize| {
        let ask = snap.asks.iter().find(|a| a.correlation_id == cid).unwrap();
        super::asks::card(&as_frame(snap), ask, 1, cols, None, Fit::Rows(10))
    };
    for (snap, cid, top, badge, lead) in [
        (
            &busy,
            "ask-5d0c11e2",
            &[
                "┌◆ 1 ask 5d0c · open 40s ────────────── WAIT ┐",
                "│ why     Claude needs your permission to    │",
                "│         use Bash                           │",
                "│ from    claude-code▸codex · 00:16          │",
            ][..],
            Tone::Badge,
            Tone::Wait,
        ),
        (
            &busy,
            "ask-a3a07b51",
            &[
                "┌◆ 1 ask a3a0 · open 3m ─────────────── IDLE ┐",
                "│ why     pi ended its turn 1m ago           │",
                "│ from    claude-code▸pi · 00:13             │",
            ],
            Tone::PillWait,
            Tone::Wait,
        ),
        (
            &busy,
            "ask-77aa0d93",
            &[
                "┌◆ 1 ask 77aa · open 25s ────────────── IDLE ┐",
                "│ why     claude-code: not picked up · 25s   │",
                "│ from    pi-2▸claude-code · 00:16           │",
            ],
            Tone::PillWait,
            Tone::Wait,
        ),
        (
            &busy,
            "ask-0c9e4a18",
            &[
                "┌◆ 1 ask 0c9e · open 5m ─────────────── LEFT ┐",
                "│ why     gemini left the hub                │",
                "│ from    claude-code▸gemini · 00:11         │",
            ],
            Tone::PillFail,
            Tone::Warn,
        ),
        (
            &off,
            "ask-0c9e4a18",
            &[
                "┌◆ 1 ask 0c9e · open 5m ──────────── OFFLINE ┐",
                "│ why     gemini is offline                  │",
                "│ from    claude-code▸gemini · 00:11         │",
            ],
            Tone::PillFail,
            Tone::Warn,
        ),
        (
            &busy,
            "ask-1223c9e0",
            &[
                "┌◆ 1 ask 1223 · open 3m ─────────────────────┐",
                "│ from    claude-code▸pi-2 · 00:13           │",
            ],
            Tone::Line,
            Tone::Soft,
        ),
        (
            &busy,
            "ask-a904f2d6",
            &[
                "┌● 1 ask a904 · acked ok · took 2m ──────────┐",
                "│ reply   2 findings, both fixed             │",
                "│ from    claude-code▸codex · 00:13          │",
            ],
            Tone::Line,
            Tone::Done,
        ),
        (
            &busy,
            "ask-e81f3b07",
            &[
                "┌× 1 ask e81f · acked failed · took 5m ──────┐",
                "│ reply   build broke: sm2_sign undefined    │",
                "│         after the rebase                   │",
                "│ from    claude-code▸pi-2 · 00:10           │",
            ],
            Tone::Line,
            Tone::Fail,
        ),
    ] {
        let g = card(snap, cid, 46);
        let text = g.text();
        let tone = |r: usize, c: usize| g.rows[&r][&c].tone;
        assert_eq!(text.lines().take(top.len()).collect::<Vec<_>>(), top);
        assert_eq!(
            (tone(0, 44), tone(1, 10), tone(top.len() - 1, 10)),
            (badge, lead, Tone::Soft),
            "badge, lead and route tones:\n{text}"
        );
        assert!(
            ["│ nudge ", "│ close ", "times", "│ to "]
                .iter()
                .all(|word| !text.contains(word)),
            "no parties, times or commands:\n{text}"
        );
        assert_eq!(g.height(), 10, "{text}");
    }
    for cols in 30..=60 {
        let g = card(&busy, "ask-5d0c11e2", cols);
        assert_eq!(
            g.rows[&0][&(cols - 8)].ch,
            '─',
            "the head leaves a rule before the badge at {cols}:\n{}",
            g.text()
        );
    }
    let mut odd = busy_asks();
    odd.asks[0].opened_at = None;
    odd.asks[0].text = "word ".repeat(80);
    odd.asks.push(lone(
        "ask-f00d0001",
        "crypto-software-agile-pi-2",
        "crypto-software-agile-pi-3",
        Some(900),
    ));
    let lines: Vec<String> = card(&odd, "ask-1223c9e0", 46)
        .text()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines[1], "│ from    claude-code▸pi-2                   │");
    assert!(
        lines[2].starts_with("│ text    word")
            && lines[7].starts_with("│         word")
            && lines[7].ends_with("… │")
            && lines[8].starts_with("│ ⏎ full card ")
            && lines[9].starts_with('└'),
        "the question takes every row left above the key row:\n{}",
        lines.join("\n")
    );
    let wide = card(&odd, "ask-f00d0001", 46).text();
    assert!(
        wide.contains(
            "│ from    crypto-software-agile-pi-2▸crypto- │\n│         software-agile-pi-3 · 00:15        │"
        ),
        "a route wider than the card wraps:\n{wide}"
    );
}

/* the design's busy view at 46x24: the ask at a permission prompt is listed and selected
first; the full card behind it says the rest */
#[test]
fn the_busy_asks_screen_lists_the_wait_first_as_designed() {
    use layout::Tone;
    let mut app = app_with(busy_asks());
    app.key(KeyCode::Char('a'));
    assert_eq!(
        screen_text(&mut app, 46, 24),
        [
            "4 online · ○ claude-code  ! codex",
            "           ○ pi           ⠋ pi-2",
            "asks · all circles · 5 open · 1 answered · 1 …",
            "────────────────────────────────────────── ● ─",
            "attention  WAIT codex 15s   IDLE pi 1m   IDLE…",
            "1 ◆ 5d0c claude-code─▸─codex   WAIT 15s",
            "2 ◆ a3a0 claude-code─▸─pi      IDLE 1m",
            "3 ◆ 77aa pi-2─▸─claude-code    IDLE 25s",
            "4 ◆ 0c9e claude-code─▸─gemini  LEFT",
            "5 ◆ 1223 claude-code─▸─pi-2    working 3m",
            "6 ● a904 claude-code>codex     acked ok",
            "7 × e81f claude-code>pi-2      acked failed",
            "",
            "┌◆ 1 ask 5d0c · open 40s ────────────── WAIT ┐",
            "│ why     Claude needs your permission to    │",
            "│         use Bash                           │",
            "│ from    claude-code▸codex · 00:16          │",
            "│ text    run the full sm2 bench on the      │",
            "│         arm64 box and post the table       │",
            "│                                            │",
            "│                                            │",
            "│ ⏎ full card                                │",
            "└────────────────────────────────────────────┘",
            "j/k move  1-9 jump  enter card  c jobs",
        ]
    );
    assert_eq!(app.ask_sel.as_deref(), Some("ask-5d0c11e2"));
    let snap = busy_asks();
    let rows = super::asks::rows(&as_frame(&snap), &super::asks::order(&snap), 46);
    for (row, (word, tone)) in rows.iter().zip([
        ("WAIT 15s", Tone::Badge),
        ("IDLE 1m", Tone::PillWait),
        ("IDLE 25s", Tone::PillWait),
        ("LEFT", Tone::PillFail),
        ("working 3m", Tone::Run),
        ("acked ok", Tone::Done),
        ("acked failed", Tone::Fail),
    ]) {
        let at = row.iter().position(|(t, _)| t == word).unwrap();
        assert_eq!(
            (row[at].1, &row[at + 1]),
            (tone, &(" ".repeat(12 - word.len()), Tone::Plain)),
            "{word} in its tone, the column padded plain"
        );
    }
    assert!(
        screen_text(&mut app, 100, 24).contains(
            &"│ why     Claude needs your permission to use     │ text    run the full sm2 bench on the arm64 box│"
                .to_string()
        ),
        "a wide card puts the question beside the why"
    );
    app.key(KeyCode::Enter);
    let full = screen_text(&mut app, 46, 24);
    assert_eq!(
        full[5..14],
        [
            "┌◆ 1 ask 5d0c · open 40s ────────────── WAIT ┐",
            "│ why     Claude needs your permission to    │",
            "│         use Bash                           │",
            "│ from    openhitls-sm2-opt-claude-code      │",
            "│         · now IDLE                         │",
            "│ to      openhitls-sm2-opt-codex            │",
            "│ times   sent 00:16 · waiting 40s           │",
            "│ text    run the full sm2 bench on the      │",
            "│         arm64 box and post the table       │",
        ],
        "{}",
        full.join("\n")
    );
}

#[test]
fn a_opens_the_asks_screen_even_without_jobs() {
    let mut app = app_with(fan_out());
    app.opts.circle = Some("project-42990b2f3ebd".into());
    assert!(screen_text(&mut app, 100, 30)
        .iter()
        .any(|l| l.starts_with("no jobs here yet")));
    app.key(KeyCode::Char('a'));
    let lines = screen_text(&mut app, 100, 30);
    let at = lines
        .iter()
        .position(|l| l.starts_with("asks · 2 open · 1 answered"))
        .expect("the asks header");
    assert!(
        lines[at + 1].ends_with('─'),
        "the rule under it: {}",
        lines[at + 1]
    );
    assert!(lines[at + 3].starts_with("1 ◆ a3a0"), "{}", lines[at + 3]);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("┌◆ 1 ask a3a0 · open 3m")),
        "the first ask's card"
    );
    assert!(lines.last().unwrap().ends_with("c jobs"));
    app.key(KeyCode::Char('c'));
    assert!(
        screen_text(&mut app, 100, 30)
            .iter()
            .any(|l| l.starts_with("no jobs here yet")),
        "c goes back"
    );
}

#[test]
fn the_asks_screen_moves_jumps_finds_and_opens_the_full_card() {
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    screen_text(&mut app, 100, 30);
    app.key(KeyCode::Char('j'));
    assert_eq!(app.ask_sel.as_deref(), Some("ask-1223c9e0"));
    app.key(KeyCode::Char('3'));
    assert_eq!(app.ask_sel.as_deref(), Some("ask-a904f2d6"));
    app.key(KeyCode::Char('/'));
    for c in "pi-2".chars() {
        app.key(KeyCode::Char(c));
    }
    app.key(KeyCode::Enter);
    assert_eq!(
        app.ask_sel.as_deref(),
        Some("ask-1223c9e0"),
        "found by the recipient's name"
    );
    app.key(KeyCode::Char('f'));
    app.key(KeyCode::Char('a'));
    assert!(!app.asks, "f gives no hints here, so a still goes back");
    app.key(KeyCode::Char('a'));
    app.key(KeyCode::Enter);
    let full = screen_text(&mut app, 100, 30);
    assert!(
        full.iter().all(|l| !l.starts_with("1 ◆")),
        "the full card takes the list's place"
    );
    assert!(full.last().unwrap().contains("esc back"));
}

#[test]
fn the_ask_selection_follows_its_id_when_the_list_reorders() {
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    screen_text(&mut app, 100, 30);
    app.key(KeyCode::Char('2'));
    assert_eq!(app.ask_sel.as_deref(), Some("ask-1223c9e0"));
    let mut next = fan_out();
    next.asks[1] = closed(next.asks[1].clone(), 990, "recipient", true, "no toolchain");
    app.apply(Ok(next));
    let lines = screen_text(&mut app, 100, 30);
    assert_eq!(
        app.ask_sel.as_deref(),
        Some("ask-1223c9e0"),
        "the same ask, now first"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("┌◆ 1 ask 1223")),
        "{lines:?}"
    );
    let mut many = fan_out();
    for i in 0..9 {
        many.asks
            .push(lone(&format!("ask-m{i:03}"), CC, PI2, Some(900 + i)));
    }
    let mut app = app_with(many.clone());
    app.key(KeyCode::Char('a'));
    screen_text(&mut app, 100, 40);
    app.key(KeyCode::Char('1'));
    many.asks.retain(|a| a.correlation_id != "ask-1223c9e0");
    app.apply(Ok(many));
    let before = app.ask_sel.clone();
    app.key(KeyCode::Char('0'));
    assert_eq!(
        app.ask_sel, before,
        "a half-typed jump is dropped once the order changed"
    );
}

#[test]
fn the_selected_ask_row_is_marked_across_the_pane() {
    use ratatui::style::Modifier;
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    let lines = app.screen(100, 30, 0);
    let text = |l: &ratatui::text::Line| {
        l.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    };
    let row = lines
        .iter()
        .position(|l| text(l).starts_with("1 ◆ a3a0"))
        .unwrap();
    assert!(lines[row]
        .spans
        .iter()
        .all(|s| s.style.add_modifier.contains(Modifier::REVERSED)));
    assert!(lines[row + 1]
        .spans
        .iter()
        .all(|s| !s.style.add_modifier.contains(Modifier::REVERSED)));
    app.opts.color = true;
    let lines = app.screen(100, 30, 0);
    let working = lines[row + 1]
        .spans
        .iter()
        .find(|s| s.content.contains("working"))
        .unwrap();
    assert_eq!(
        working.style.fg,
        Some(app.opts.theme.run),
        "an unselected row keeps its colours"
    );
    let sel = &lines[row];
    assert!(sel
        .spans
        .iter()
        .all(|s| s.style.bg == Some(app.opts.theme.select_bg)));
    let idle = sel
        .spans
        .iter()
        .find(|s| s.content.contains("IDLE"))
        .unwrap();
    assert_eq!(
        idle.style.fg,
        Some(app.opts.theme.wait),
        "the selected row keeps its colours too"
    );
}

#[test]
fn the_asks_screen_speaks_for_old_hubs_and_empty_views() {
    let mut app = app_with(s1());
    app.key(KeyCode::Char('a'));
    let lines = screen_text(&mut app, 100, 30);
    assert!(
        lines
            .iter()
            .any(|l| l
                .contains("this hub lists only the asks of jobs; restart it on the current amesh")),
        "{lines:?}"
    );
    let mut empty = s1();
    empty.capabilities.ask_list = true;
    let mut app = app_with(empty);
    app.key(KeyCode::Char('a'));
    let lines = screen_text(&mut app, 100, 30);
    assert!(
        lines.iter().any(|l| l.starts_with("no asks here yet")),
        "{lines:?}"
    );
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    app.opts.ascii = true;
    let lines = screen_text(&mut app, 100, 30);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("asks | all circles | 2 open | 1 answered")),
        "without a circle the header says so: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("1 > a3a0 claude-code->-pi ")),
        "ASCII glyphs: {lines:?}"
    );
}

#[test]
fn the_rule_counts_the_open_asks_and_gives_way_after_the_events() {
    let mut app = app_with(with_events(with_lone_asks(s1()), 1234567));
    let rule = |app: &mut App, cols: usize| -> String {
        screen_text(app, cols, 40)
            .into_iter()
            .find(|l| l.starts_with("────"))
            .unwrap()
    };
    let wide = rule(&mut app, 100);
    assert!(
        wide.ends_with(" 2 asks open · 1234567 events · ● ─"),
        "{wide}"
    );
    let both = " 2 asks open ·".width() + " 1234567 events ".width() + "· ● ".width() + 1 + 4;
    let narrow = rule(&mut app, both - 1);
    assert!(
        narrow.ends_with(" 2 asks open · ● ─"),
        "the events go first: {narrow}"
    );
    app.apply(Err("connection refused".into()));
    let stale = rule(&mut app, 100);
    assert!(
        !stale.contains("asks open"),
        "as old as the snapshot: {stale}"
    );
}

#[test]
fn the_no_jobs_row_counts_the_asks_and_points_to_their_screen() {
    let mut app = app_with(with_events(fan_out(), 3));
    let lines = screen_text(&mut app, 100, 30);
    let row = lines
        .iter()
        .find(|l| l.starts_with("no jobs here yet"))
        .unwrap();
    assert!(row.ends_with("2 asks open · 3 events · ●"), "{row}");
    assert!(
        lines
            .iter()
            .any(|l| l == "asks have a screen of their own: press a"),
        "{lines:?}"
    );
    let lines = screen_text(&mut app, 34, 30);
    let row = lines
        .iter()
        .find(|l| l.starts_with("no jobs here yet"))
        .unwrap();
    assert!(
        row.ends_with("2 asks open · ●"),
        "the events go first: {row}"
    );
    let mut quiet = with_events(fan_out(), 3);
    quiet.asks.clear();
    let mut app = app_with(quiet);
    let lines = screen_text(&mut app, 100, 30);
    let row = lines
        .iter()
        .find(|l| l.starts_with("no jobs here yet"))
        .unwrap();
    assert!(
        row.ends_with("  3 events · ●") && !lines.iter().any(|l| l.contains("press a")),
        "no asks, no count and no pointer: {lines:?}"
    );
}

#[test]
fn the_footer_names_a_while_the_view_holds_asks() {
    let mut app = app_with(s1());
    assert_eq!(
        screen_text(&mut app, 100, 40).last().unwrap(),
        "j/k ↑↓ move  h/l ←→ stage  tab chain  t timeline  enter card  / find  f hints  digits jump  esc back"
    );
    assert!(
        !screen_text(&mut app, 46, 40)
            .last()
            .unwrap()
            .contains("a asks"),
        "no asks, no asks key"
    );
    let mut app = app_with(with_lone_asks(s1()));
    assert_eq!(
        screen_text(&mut app, 100, 40).last().unwrap(),
        "j/k ↑↓ move  h/l ←→ stage  tab chain  t timeline  enter card  a asks  / find  f hints  digits jump"
    );
    assert_eq!(
        screen_text(&mut app, 46, 40).last().unwrap(),
        "j/k move  h/l stage  t timeline  a asks",
        "a narrow pane keeps the asks key and the view toggle"
    );
}

#[test]
fn the_fetch_asks_for_the_text_the_card_shows() {
    let mut app = app_with(with_lone_asks(s1()));
    screen_text(&mut app, 100, 40);
    assert_eq!(app.wanted().detail.as_deref(), Some("detail=perf"));
    app.key(KeyCode::Char('a'));
    assert_eq!(
        app.wanted().detail.as_deref(),
        Some("ask=ask-1223c9e0"),
        "the asks screen starts on its first ask"
    );
    app.key(KeyCode::Char('j'));
    assert_eq!(app.wanted().detail.as_deref(), Some("ask=ask-a3a07b51"));
}

#[test]
fn the_full_ask_card_shows_the_whole_text_once_it_arrives() {
    let mut snap = fan_out();
    snap.asks[1].text = "x ".repeat(200);
    let mut app = app_with(snap.clone());
    app.key(KeyCode::Char('a'));
    app.key(KeyCode::Enter);
    assert!(
        !screen_text(&mut app, 100, 30)
            .iter()
            .any(|l| l.contains("END")),
        "the preview until the detail comes"
    );
    snap.ask_detail = Some(crate::wire::AskDetail {
        correlation_id: "ask-a3a07b51".into(),
        text: format!("{}END", "x ".repeat(600)),
        reply: None,
    });
    app.apply(Ok(snap));
    app.key(KeyCode::Char('G'));
    let lines = screen_text(&mut app, 100, 30);
    assert!(lines.iter().any(|l| l.contains("END")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("└─ lines")),
        "the full card scrolls"
    );
}

#[test]
fn the_smallest_pane_still_lists_the_selected_ask() {
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    for cols in [30, 46] {
        let lines = screen_text(&mut app, cols, 8);
        assert!(
            lines.iter().any(|l| l.starts_with("1 ◆ a3a0")),
            "the blank row gives way to the list at {cols}x8: {lines:?}"
        );
    }
    app.key(KeyCode::Char('j'));
    let lines = screen_text(&mut app, 46, 8);
    assert!(
        lines.iter().any(|l| l.starts_with("2 ◆ 1223")),
        "the one row there is the selected ask: {lines:?}"
    );
}

/* y and Y copy the selected ask's first and second command over the jobs screen's OSC 52 path
and footer notice, controls stripped; an ask without one copies nothing, and neither does the
events screen over the asks */
#[test]
fn y_and_shift_y_copy_the_selected_asks_commands() {
    let mut snap = fan_out();
    snap.asks[1]
        .actions
        .as_mut()
        .unwrap()
        .push(crate::wire::AskAction::CloseLeft);
    let mut app = app_with(snap);
    app.key(KeyCode::Char('a'));
    screen_text(&mut app, 100, 30);
    let mut bytes = Vec::new();
    assert_eq!(app.key(KeyCode::Char('y')), Act::Copy(0));
    app.copy_command(0, false, &mut bytes);
    assert_eq!(
        bytes,
        b"\x1b]52;c;YW1lc2ggcGVlciBub3RpZnkgb3BlbmhpdGxzLXNtMi1vcHQtcGkgJ2FzayBhM2EwIHdhaXRzIGZvciB5b3VyIGFjayc=\x07"
    );
    assert_eq!(
        screen_text(&mut app, 100, 30).last().unwrap(),
        "sent to clipboard (OSC 52): amesh peer notify openhitls-sm2-opt-pi 'ask a3a0 waits for your ack'"
    );
    bytes.clear();
    assert_eq!(app.key(KeyCode::Char('Y')), Act::Copy(1));
    app.copy_command(1, false, &mut bytes);
    assert_eq!(
        bytes,
        b"\x1b]52;c;YW1lc2ggcGVlciBhY2sgYXNrLWEzYTA3YjUxIC0tZmFpbGVkIHRydWUgLS1tZXNzYWdlICdyZWNpcGllbnQgbGVmdCc=\x07"
    );
    bytes.clear();
    app.key(KeyCode::Char('e'));
    app.copy_command(0, false, &mut bytes);
    assert!(bytes.is_empty(), "the events screen copies nothing");
    app.key(KeyCode::Char('a'));
    app.key(KeyCode::Char('j'));
    app.copy_command(0, false, &mut bytes);
    assert!(bytes.is_empty(), "a recipient at work needs no command");
    let mut prompt = app_with(fan_out());
    prompt.key(KeyCode::Char('a'));
    prompt.copy_command(0, true, &mut Vec::new());
    prompt.key(KeyCode::Char('/'));
    assert_eq!(
        screen_text(&mut prompt, 100, 30).last().map(String::as_str),
        Some("/_"),
        "a search prompt keeps the footer from the copy notice"
    );
    let mut odd = fan_out();
    odd.asks[1].actions = Some(vec![crate::wire::AskAction::Nudge {
        to: "p\u{1}i\n".into(),
    }]);
    let mut app = app_with(odd);
    app.key(KeyCode::Char('a'));
    app.copy_command(0, false, &mut bytes);
    assert_eq!(
        bytes,
        b"\x1b]52;c;YW1lc2ggcGVlciBub3RpZnkgJ3BpJyAnYXNrIGEzYTAgd2FpdHMgZm9yIHlvdXIgYWNrJw==\x07"
    );
}

/* the ask card ends in its key row: y names the command it copies, then enter opens the full
card or esc leaves it; a compact card short of room keeps only the keys, and the row sits
below both columns and within the rows the list leaves the card */
#[test]
fn the_ask_card_ends_in_its_key_row() {
    let mut short = app_with(fan_out());
    short.key(KeyCode::Char('a'));
    let lines = screen_text(&mut short, 46, 15);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("│ y copy nudge · ⏎ full card")),
        "a short pane keeps the keys:\n{}",
        lines.join("\n")
    );
    let mut snap = fan_out();
    snap.asks[1].text = "word ".repeat(40);
    let card = |snap: &Snapshot, cid: &str, cols: usize, size: Fit| {
        let ask = snap.asks.iter().find(|a| a.correlation_id == cid).unwrap();
        super::asks::card(&as_frame(snap), ask, 1, cols, None, size)
    };
    let keys = |g: &Grid| g.text().lines().rev().nth(1).unwrap().to_string();
    for (cid, cols, size, row) in [
        (
            "ask-a3a07b51",
            46,
            Fit::Rows(10),
            "│ y copy nudge · ⏎ full card                 │",
        ),
        (
            "ask-a3a07b51",
            46,
            Fit::Whole,
            "│ y copy nudge · esc back                    │",
        ),
        (
            "ask-1223c9e0",
            46,
            Fit::Rows(10),
            "│ ⏎ full card                                │",
        ),
        (
            "ask-1223c9e0",
            46,
            Fit::Whole,
            "│ esc back                                   │",
        ),
        (
            "ask-a3a07b51",
            100,
            Fit::Rows(10),
            "│ y copy nudge · ⏎ full card                      │                                                │",
        ),
        (
            "ask-a3a07b51",
            100,
            Fit::Whole,
            "│ y copy nudge · esc back                         │                                                │",
        ),
    ] {
        let g = card(&snap, cid, cols, size);
        assert_eq!(keys(&g), row, "{cols} {size:?}:\n{}", g.text());
    }
    snap.asks[1]
        .actions
        .as_mut()
        .unwrap()
        .push(crate::wire::AskAction::CloseLeft);
    let g = card(&snap, "ask-a3a07b51", 46, Fit::Rows(10));
    assert_eq!(keys(&g), "│ y copy nudge · Y copy close · ⏎ full card  │");
    let g = card(&snap, "ask-a3a07b51", 44, Fit::Rows(10));
    assert_eq!(keys(&g), "│ y nudge · Y close · ⏎                    │");
    let (r, c) = find(&g, "y nudge · Y close · ⏎");
    assert_eq!(
        [0, 2, 10, 12, 20].map(|i| tone_at(&g, r, c + i)),
        [Tone::Near, Tone::Dim, Tone::Near, Tone::Dim, Tone::Near]
    );
    let mut app = app_with(busy_asks());
    app.key(KeyCode::Char('a'));
    let lines = screen_text(&mut app, 46, 19);
    assert_eq!(
        lines[lines.len() - 3],
        "│ ⏎ full card                                │",
        "the list gives way first:\n{}",
        lines.join("\n")
    );
    let busy = busy_asks();
    for ask in &busy.asks {
        for cols in [30, 46, 60, 100] {
            for h in 1..=12 {
                let g = super::asks::card(&as_frame(&busy), ask, 1, cols, None, Fit::Rows(h));
                assert!(
                    g.height() <= h.max(3) && keys(&g).contains('⏎'),
                    "{} at {cols}x{h}:\n{}",
                    ask.correlation_id,
                    g.text()
                );
            }
        }
    }
}

/* / finds an ask by its whole correlation id, from another row */
#[test]
fn find_matches_the_ask_id() {
    let mut app = app_with(busy_asks());
    app.key(KeyCode::Char('a'));
    screen_text(&mut app, 46, 24);
    for query in ["5d0c", "11e2"] {
        app.key(KeyCode::Char('j'));
        app.key(KeyCode::Char('/'));
        for c in query.chars() {
            app.key(KeyCode::Char(c));
        }
        app.key(KeyCode::Enter);
        assert_eq!(app.ask_sel.as_deref(), Some("ask-5d0c11e2"), "/{query}");
    }
    let mut quiet = busy_asks();
    for ask in quiet.asks.iter_mut() {
        ask.text = "no match here".into();
    }
    let mut app = app_with(quiet);
    app.key(KeyCode::Char('a'));
    screen_text(&mut app, 46, 24);
    let first = app.ask_sel.clone();
    app.key(KeyCode::Char('/'));
    for c in "ask".chars() {
        app.key(KeyCode::Char(c));
    }
    app.key(KeyCode::Enter);
    app.key(KeyCode::Char('n'));
    assert_eq!(app.ask_sel, first, "the ids' ask- prefix is no match");
}

/* the asks rows' sender and recipient, whole or cut to fit, and the full card's parties read
in the worker role; the arrows keep their tones */
#[test]
fn ask_names_read_in_the_worker_role() {
    let snap = fan_out();
    let rows = super::asks::rows(&as_frame(&snap), &super::asks::order(&snap), 100);
    let route = |row: &Vec<(String, Tone)>| (row[3].1, row[4].clone(), row[5].1);
    assert_eq!(
        route(&rows[0]),
        (Tone::Soft, ("─▸─".to_string(), Tone::Dim), Tone::Soft)
    );
    assert_eq!(
        route(&rows[2]),
        (Tone::Soft, (">".to_string(), Tone::Plain), Tone::Soft)
    );
    let mut all = fan_out();
    all.roster.clear();
    let rows = super::asks::rows(&as_frame(&all), &super::asks::order(&all), 46);
    assert_eq!(
        (rows[0][3].0.starts_with('…'), rows[0][3].1),
        (true, Tone::Soft),
        "{:?}",
        rows[0]
    );
    let g = super::asks::card(&as_frame(&snap), &snap.asks[1], 1, 46, None, Fit::Whole);
    for k in ["│ from ", "│ to "] {
        let (r, _) = find(&g, k);
        assert_eq!(tone_at(&g, r, 10), Tone::Soft, "{k}:\n{}", g.text());
    }
}

#[test]
fn an_open_asks_arrow_walks_to_a_working_recipient_and_rests_otherwise() {
    let mut app = app_with(proto_s1());
    app.full = true;
    app.sel = Some("upg".into());
    let at = |app: &mut App, tick| line_with(app, 100, tick, "7c2 ");
    assert!(
        at(&mut app, 0).contains("7c2 cc─▸─pi-3 "),
        "the first frame holds it in the middle"
    );
    assert!(
        at(&mut app, 2).contains("7c2 cc──▸pi-3 "),
        "it steps toward the recipient"
    );
    assert!(
        at(&mut app, 4).contains("7c2 cc▸──pi-3 "),
        "and comes round again"
    );
    app.sel = Some("perf".into());
    assert!(
        line_with(&mut app, 100, 2, "9a1 ").contains("9a1 cc─▸─codex "),
        "a recipient that is not working holds it still"
    );
    app.sel = Some("upg".into());
    app.opts.anim = false;
    assert!(
        at(&mut app, 2).contains("7c2 cc─▸─pi-3 "),
        "--no-anim holds it still"
    );
}

#[test]
fn an_ack_the_hub_just_took_walks_the_arrow_back_to_the_sender() {
    /* the hub says whether the ack is just now; the arrow only reads it */
    let closed = |ago: u64| {
        let mut snap = proto_s1();
        let now = snap.captured_at;
        let ask = snap.asks.iter_mut().find(|a| a.correlation_id == "ask-5b1");
        let ask = ask.unwrap();
        ask.closed_at = Some(now - ago);
        ask.state = Some(crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        });
        ask.closed_just_now = Some(ago <= 1);
        let mut app = app_with(snap);
        app.full = true;
        app.sel = Some("func".into());
        app
    };
    let mut app = closed(0);
    let at = |app: &mut App, tick| line_with(app, 100, tick, "5b1 ");
    assert!(
        at(&mut app, 0).contains("5b1 cc─◂─codex "),
        "{}",
        at(&mut app, 0)
    );
    assert!(
        at(&mut app, 2).contains("5b1 cc◂──codex "),
        "it steps toward the sender"
    );
    assert!(
        at(&mut app, 4).contains("5b1 cc──◂codex "),
        "and comes round again"
    );
    assert!(
        at(&mut closed(2), 2).contains("5b1 cc>codex "),
        "an ack from before the last second reads >"
    );
}

#[test]
fn the_asks_screen_walks_the_arrow_of_an_ask_whose_recipient_works() {
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    assert!(line_with(&mut app, 100, 0, " 1223 ").contains("claude-code─▸─pi-2 "));
    assert!(
        line_with(&mut app, 100, 2, " 1223 ").contains("claude-code──▸pi-2 "),
        "the recipient works on it: the arrow steps"
    );
    assert!(
        line_with(&mut app, 100, 2, " a3a0 ").contains("claude-code─▸─pi "),
        "an idle recipient holds it still"
    );
}

use super::*;

#[test]
fn a_card_route_too_wide_for_its_column_cuts_the_sender_and_keeps_the_recipient() {
    let mut snap = proto_s1();
    let ask = snap.asks.iter_mut().find(|a| a.correlation_id == "ask-7c2");
    let ask = ask.unwrap();
    ask.from_peer = "amesh-claude-code-22".into();
    ask.to_peer = "amesh-codex-2".into();
    let mut app = app_with(snap);
    app.full = true;
    app.sel = Some("upg".into());
    let line = line_with(&mut app, 100, 0, "7c2 ");
    assert!(
        line.contains("7c2 amesh-claude-code-…─▸─amesh-codex-2 "),
        "{line}"
    );
    let mut snap = proto_s1();
    let ask = snap.asks.iter_mut().find(|a| a.correlation_id == "ask-7c2");
    let ask = ask.unwrap();
    ask.from_peer = "amesh-claude-code-2".into();
    ask.to_peer = "amesh-pi-2".into();
    let mut app = app_with(snap);
    app.full = true;
    app.sel = Some("upg".into());
    let line = line_with(&mut app, 46, 0, "7c2 ");
    assert!(
        line.contains("7c2 amesh-claude-cod…─▸─amesh-pi-2 "),
        "a one-column card too: {line}"
    );
    let mut snap = proto_s1();
    let ask = snap.asks.iter_mut().find(|a| a.correlation_id == "ask-7c2");
    ask.unwrap().to_peer = "amesh-recipient-name-123456".into();
    let mut app = app_with(snap);
    app.full = true;
    app.sel = Some("upg".into());
    let line = line_with(&mut app, 46, 0, "7c2 ");
    assert!(
        line.contains("7c2 ─▸─amesh-recipient-name-123456") && !line.contains('…'),
        "no room left for the sender: it goes, not the end of the recipient: {line}"
    );
}

fn card_grid(s: Snapshot, size: Fit, cols: usize) -> Grid {
    let chain = model::chains(&s)
        .into_iter()
        .find(|c| c.topo().iter().any(|id| id == "j"))
        .unwrap();
    let num = Numbers::default()
        .assign(&chain)
        .into_iter()
        .map(|(n, id)| (id, n))
        .collect();
    layout::card(
        &View {
            snap: model::Frame {
                view: s,
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "j",
        },
        "j",
        cols,
        None,
        size,
    )
}

#[test]
fn idle_badge_and_why() {
    let g = card_grid(sample(), Fit::Rows(0), 80);
    badge(&g, " IDLE ", Tone::PillWait, 80);
    assert!(
        g.text()
            .lines()
            .nth(1)
            .unwrap()
            .contains("why     codex ended its turn 1m ago"),
        "{}",
        g.text()
    );
}

#[test]
fn wait_badge_and_why() {
    let mut s = sample();
    s.jobs[0].progress.as_mut().unwrap().state = Some(Progress::Wait {
        since: 940,
        reason: Some("approve shell".into()),
    });
    let g = card_grid(s, Fit::Rows(0), 80);
    badge(&g, " WAIT ", Tone::Badge, 80);
    assert!(
        g.text()
            .lines()
            .nth(1)
            .unwrap()
            .contains("why     approve shell"),
        "{}",
        g.text()
    );
}

#[test]
fn failed_badge_and_why() {
    let mut s = sample();
    s.jobs[0].state = "failed".into();
    s.jobs[0].progress = None;
    s.jobs[0].result = Some("build failed".into());
    let g = card_grid(s, Fit::Whole, 46);
    badge(&g, " FAILED ", Tone::PillFail, 46);
    assert!(
        g.text()
            .lines()
            .nth(1)
            .unwrap()
            .contains("why     build failed"),
        "{}",
        g.text()
    );
}

#[test]
fn blocked_badge_and_why() {
    let mut s = sample();
    s.jobs[0].state = "queued".into();
    s.jobs[0].progress = None;
    s.jobs[0].depends_on = vec!["dep".into()];
    s.jobs[0].dispatch_state = Some(DispatchState::Blocked {
        dependencies: vec!["dep".into()],
        dependency: "dep".into(),
        reason: BlockReason::Failed,
    });
    s.jobs.push(Job {
        job_id: "dep".into(),
        title: "compile".into(),
        state: "failed".into(),
        ..Default::default()
    });
    let g = card_grid(s, Fit::Rows(0), 80);
    badge(&g, " BLOCKED ", Tone::PillText, 80);
    assert!(
        g.text()
            .lines()
            .nth(1)
            .unwrap()
            .contains("why     1 compile failed: retry it first"),
        "{}",
        g.text()
    );
}

#[test]
fn compact_card_keeps_commands_discoverable_and_details_in_full() {
    let s = sample();
    let compact = card_grid(s.clone(), Fit::Rows(12), 80).text();
    assert!(
        compact.contains("y copy nudge · Y copy resend · ⏎ full card"),
        "{compact}"
    );
    assert!(compact
        .lines()
        .rev()
        .nth(1)
        .unwrap()
        .contains("y copy nudge"));
    assert!(!compact.contains("times"), "{compact}");
    assert!(compact.contains("codex · ask 1 boss▸codex"), "{compact}");
    assert!(card_grid(s, Fit::Whole, 80).text().contains("times"));
}

#[test]
fn unacked_age_replaces_the_bar() {
    let g = card_grid(sample(), Fit::Whole, 80);
    assert!(g.text().contains("unacked 3m"), "{}", g.text());
    assert_eq!(g.text().matches("unacked").count(), 1, "{}", g.text());
    assert!(!g.text().contains('⣿'));
    let row = g
        .text()
        .lines()
        .position(|l| l.contains("unacked 3m"))
        .unwrap();
    let age = &g.rows[&row];
    let chars: Vec<_> = age.values().collect();
    let at = chars
        .windows(2)
        .position(|c| c[0].ch == '3' && c[1].ch == 'm')
        .unwrap();
    assert_eq!(chars[at].tone, Tone::Wait);
}

#[test]
fn chain_median_counts_only_valid_done_durations() {
    let mut s = with_done(
        sample(),
        &[
            ("d1", "done", 60),
            ("d2", "done", 300),
            ("bad", "failed", 900),
        ],
    );
    let text = card_grid(s.clone(), Fit::Whole, 80).text();
    assert!(text.contains("chain median 3m"), "{text}");
    s.jobs[2].finished_at = Some(99);
    assert!(!card_grid(s, Fit::Whole, 80).text().contains("chain median"));
}

#[test]
fn compact_text_limits_and_badge_absence() {
    let mut s = sample();
    s.jobs[0].state = "done".into();
    s.jobs[0].progress = None;
    s.jobs[0].result = Some("word ".repeat(100));
    s.jobs[0].prompt = "promptword ".repeat(100);
    for cols in [46, 80, 100] {
        let g = card_grid(s.clone(), Fit::Rows(12), cols);
        assert!(g.rows[&0].values().all(|c| !matches!(
            c.tone,
            Tone::Badge | Tone::PillWait | Tone::PillFail | Tone::PillText
        )));
        let text = g.text();
        assert_eq!(
            text.lines().filter(|l| l.contains("word word")).count(),
            if cols >= 96 { 8 } else { 7 },
            "{text}"
        );
        assert_eq!(
            text.lines().filter(|l| l.contains("promptword")).count(),
            if cols >= 96 { 9 } else { 1 },
            "{text}"
        );
        assert!(text.lines().all(|l| layout::width(l) <= cols));
    }
    s.jobs[0].state = "running".into();
    s.jobs[0].progress = Some(JobProgress {
        state: Some(Progress::Work { since: 950 }),
        busy: true,
    });
    assert!(!card_grid(s, Fit::Rows(0), 46).text().contains(" IDLE "));
}

#[test]
fn median_is_sorted_and_scoped_to_the_selected_chain() {
    let mut s = with_done(
        sample(),
        &[
            ("a", "done", 300),
            ("b", "done", 60),
            ("c", "done", 120),
            ("outside", "done", 900),
        ],
    );
    s.jobs[0].depends_on.retain(|d| d != "outside");
    let text = card_grid(s, Fit::Whole, 80).text();
    assert!(text.contains("chain median 2m"), "{text}");
}

/* jobs j depends on, each sent at 100 and ended the given seconds later */
fn with_done(mut s: Snapshot, jobs: &[(&str, &str, u64)]) -> Snapshot {
    for &(id, state, secs) in jobs {
        s.jobs[0].depends_on.push(id.into());
        s.jobs.push(Job {
            job_id: id.into(),
            state: state.into(),
            ask_id: Some(id.into()),
            finished_at: Some(100 + secs),
            ..Default::default()
        });
        s.asks.push(Ask {
            correlation_id: id.into(),
            opened_at: Some(100),
            ..Default::default()
        });
    }
    s
}

fn check_open_comparison(state: Progress, size: Fit) {
    let mut s = with_done(sample(), &[("d1", "done", 60), ("d2", "done", 180)]);
    s.jobs[0].progress.as_mut().unwrap().state = Some(state);
    let text = card_grid(s, size, 80).text();
    assert!(
        text.contains("unacked 3m · chain median 2m"),
        "{size:?}: {text}"
    );
    assert_eq!(text.matches("unacked").count(), 1, "{size:?}: {text}");
}

#[test]
fn open_wait_compact_comparison() {
    check_open_comparison(
        Progress::Wait {
            since: 940,
            reason: Some("permission".into()),
        },
        Fit::Rows(12),
    );
}

#[test]
fn open_wait_full_comparison() {
    check_open_comparison(
        Progress::Wait {
            since: 940,
            reason: Some("permission".into()),
        },
        Fit::Whole,
    );
}

#[test]
fn open_work_compact_comparison() {
    check_open_comparison(Progress::Work { since: 940 }, Fit::Rows(12));
}

#[test]
fn open_work_full_comparison() {
    check_open_comparison(Progress::Work { since: 940 }, Fit::Whole);
}

#[test]
fn compact_fit_is_a_bound_and_keeps_keys() {
    for cols in [30, 46, 80, 100] {
        for height in 1..=12 {
            let g = card_grid(sample(), Fit::Rows(height), cols);
            assert!(
                g.height() <= height,
                "{cols}x{height}: {} rows\n{}",
                g.height(),
                g.text()
            );
            assert!(g.text().contains("y "), "{cols}x{height}: {}", g.text());
        }
    }
}

#[test]
fn tiny_frame_has_no_overflow() {
    for cols in [0, 1] {
        let g = layout::framed(
            ('x', Tone::Plain),
            "title",
            None,
            &[vec![("body".into(), Tone::Plain)]],
            &[],
            cols,
        );
        assert!(g.rows.values().all(|row| row.keys().all(|c| *c < cols)));
    }
}

#[test]
fn compact_values_share_the_key_column() {
    let g = card_grid(sample(), Fit::Rows(10), 80);
    for value in ["codex · ask", "unacked 3m"] {
        let row = g
            .text()
            .lines()
            .find(|l| l.contains(value))
            .unwrap()
            .to_string();
        assert_eq!(
            row.chars().collect::<String>().find(value),
            Some(3 + 1 + layout::KEY),
            "{row}"
        );
    }
    assert!(g.text().contains("worker  codex · ask"), "{}", g.text());
}

#[test]
fn spare_compact_rows_go_to_prompt() {
    let mut s = sample();
    s.jobs[0].prompt = "promptword ".repeat(100);
    let text = card_grid(s, Fit::Rows(12), 46).text();
    assert!(
        text.lines().filter(|l| l.contains("promptword")).count() > 1,
        "{text}"
    );
    assert!(
        !text.lines().any(|l| l.trim_matches('│').trim().is_empty()),
        "{text}"
    );
}

#[test]
fn smallest_jobs_pane_keeps_copy_keys() {
    let mut a = copy_app();
    let text = screen_text(&mut a, 30, 8).join("\n");
    assert!(text.contains("y nudge"), "{text}");
}

#[test]
fn tiny_card_has_no_overflow() {
    for cols in [0, 1] {
        let g = card_grid(sample(), Fit::Rows(8), cols);
        assert!(g.rows.values().all(|row| row.keys().all(|c| *c < cols)));
    }
}

fn idle_lead(why: IdleWhy, since: u64) -> String {
    let mut s = sample();
    s.jobs[0].progress.as_mut().unwrap().state = Some(Progress::Idle { since, why });
    let g = card_grid(s, Fit::Rows(0), 80);
    badge(&g, " IDLE ", Tone::PillWait, 80);
    g.text()
        .lines()
        .nth(1)
        .unwrap()
        .trim_matches('│')
        .trim()
        .to_string()
}

#[test]
fn not_picked_up_shows_idle_age() {
    assert_eq!(
        idle_lead(IdleWhy::NotPickedUp, 940),
        "why     codex: not picked up · 1m"
    );
}

#[test]
fn ask_still_open_shows_idle_age() {
    assert_eq!(
        idle_lead(IdleWhy::AskStillOpen, 940),
        "why     codex: ask still open · 1m"
    );
}

#[test]
fn unknown_idle_gives_no_cause() {
    assert_eq!(idle_lead(IdleWhy::Unknown, 940), "why     codex idle · 1m");
}

#[test]
fn future_idle_stamp_omits_age() {
    for (why, text) in [
        (IdleWhy::TurnEnded, "codex ended its turn"),
        (IdleWhy::NotPickedUp, "codex: not picked up"),
        (IdleWhy::AskStillOpen, "codex: ask still open"),
        (IdleWhy::Unknown, "codex idle"),
    ] {
        assert_eq!(idle_lead(why, 1001), format!("why     {text}"));
    }
}

fn explanation(s: Snapshot, expected: &str) {
    let text = card_grid(s, Fit::Rows(12), 80).text();
    assert!(text.contains("why"), "{text}");
    assert!(text.contains(expected), "{text}");
}

#[test]
fn compact_gone_explains_departure() {
    let mut s = sample();
    s.jobs[0].progress.as_mut().unwrap().state = Some(Progress::Gone);
    explanation(s, "left the hub");
}

fn queued_explanation(state: DispatchState, expected: &str) {
    let mut s = sample();
    s.jobs[0].state = "queued".into();
    s.jobs[0].progress = None;
    s.jobs[0].dispatch_state = Some(state);
    explanation(s, expected);
}

#[test]
fn compact_held_explains_dispatch() {
    queued_explanation(DispatchState::Held, "set its assignee again");
}

#[test]
fn compact_unassigned_explains_dispatch() {
    queued_explanation(DispatchState::Unassigned, "name an assignee");
}

#[test]
fn compact_no_peer_explains_dispatch() {
    queued_explanation(
        DispatchState::NoPeer {
            name: "lost".into(),
        },
        "no peer named lost",
    );
}

#[test]
fn compact_other_circle_explains_dispatch() {
    queued_explanation(
        DispatchState::OtherCircle {
            peer_id: "far".into(),
        },
        "another circle",
    );
}

#[test]
fn compact_missing_explains_settlement() {
    let mut s = sample();
    s.asks.clear();
    s.jobs[0].progress = None;
    s.jobs[0].relation = Some(JobRelation::Missing { will_fail: true });
    explanation(s, "the hub fails this job");
}

#[test]
fn compact_blocked_preserves_instruction() {
    let mut s = sample();
    s.jobs[0].state = "queued".into();
    s.jobs[0].progress = None;
    s.jobs[0].depends_on = vec!["dep".into()];
    s.jobs[0].dispatch_state = Some(DispatchState::Blocked {
        dependencies: vec!["dep".into()],
        dependency: "dep".into(),
        reason: BlockReason::Failed,
    });
    s.jobs[0].prompt = "promptword ".repeat(100);
    s.jobs.push(Job {
        job_id: "dep".into(),
        title: "dependency ".repeat(100),
        state: "failed".into(),
        ..Default::default()
    });
    for cols in [30, 46, 80, 100] {
        let text = card_grid(s.clone(), Fit::Rows(0), cols).text();
        let words = text
            .lines()
            .map(|l| l.split('│').nth(1).unwrap_or(l).trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(words.contains("failed: retry it first"), "{text}");
    }
}

#[test]
fn empty_done_result_has_no_field() {
    let mut s = sample();
    s.jobs[0].state = "done".into();
    s.jobs[0].result = Some(String::new());
    s.jobs[0].progress = None;
    for size in [Fit::Whole, Fit::Rows(12)] {
        let text = card_grid(s.clone(), size, 80).text();
        assert!(!text.contains("result"), "{text}");
    }
}

#[test]
fn full_idle_is_wait_tone_without_repeated_age() {
    let g = card_grid(sample(), Fit::Whole, 80);
    let text = g.text();
    let (r, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("worker"))
        .unwrap();
    assert!(line.contains("IDLE"), "{text}");
    assert!(!line.contains("1m"), "{text}");
    assert!(
        g.rows[&r]
            .values()
            .any(|c| c.ch == 'I' && c.tone == Tone::Wait),
        "{text}"
    );
    assert!(!text.contains("waiting 3m"), "{text}");
}

#[test]
fn independent_jobs_have_no_chain_median() {
    let mut s = with_done(sample(), &[("d1", "done", 60), ("d2", "done", 180)]);
    s.jobs[0].depends_on.clear();
    let text = card_grid(s, Fit::Rows(12), 80).text();
    assert!(!text.contains("chain median"), "{text}");
}

#[test]
fn header_leaves_separator_before_badge() {
    for cols in 30..=130 {
        let mut s = sample();
        s.jobs[0].title = "longtitle ".repeat(100);
        let g = card_grid(s, Fit::Rows(12), cols);
        assert_eq!(
            g.rows[&0][&(cols - 8)].tone,
            Tone::Line,
            "cols={cols}: {}",
            g.text()
        );
    }
}

/* sample's job without progress, its worker and its ask's route set */
fn route_card(worker: &str, from: &str, to: &str, cols: usize, size: Fit) -> Grid {
    let mut s = sample();
    s.jobs[0].worker = Some(worker.into());
    s.jobs[0].progress = None;
    s.asks[0].from_peer = from.into();
    s.asks[0].to_peer = to.into();
    s.asks[0].to_peer_id = "receiver".into();
    card_grid(s, size, cols)
}

fn worker_value(g: &Grid) -> String {
    let text = g.text();
    let row = text.lines().find(|l| l.contains("│ worker  ")).unwrap();
    let value = row.split("│ worker  ").nth(1).unwrap();
    value.split('│').next().unwrap().trim_end().to_string()
}

#[test]
fn a_long_sender_gives_way_before_the_recipient_and_time() {
    for cols in [46, 80, 100] {
        let value = worker_value(&route_card(
            "id",
            "very-long-sender-name",
            "receiver",
            cols,
            Fit::Rows(10),
        ));
        assert!(
            value.starts_with("id · ") && value.contains("receiver") && value.ends_with("00:13"),
            "{cols}: {value}"
        );
    }
}

#[test]
fn the_worker_row_leads_with_the_worker() {
    for cols in 30..=130 {
        let value = worker_value(&route_card(
            "worker-peer-name",
            "sender-peer-name",
            "recipient-peer",
            cols,
            Fit::Rows(10),
        ));
        assert!(value.starts_with("worker-peer-name"), "{cols}: {value}");
        assert!(!value.contains('…'), "{cols}: {value}");
    }
}

#[test]
fn a_route_without_names_uses_id_without_an_arrow() {
    let g = route_card("id", "", "", 80, Fit::Rows(10));
    let text = g.text();
    let worker = text.lines().find(|l| l.contains("worker")).unwrap();
    assert!(worker.contains("receiver"), "{worker}");
    assert!(!worker.contains('▸'), "{worker}");
}

#[test]
fn the_full_card_names_its_back_key() {
    let text = route_card("id", "from", "to", 80, Fit::Whole).text();
    assert!(text.contains("esc back"), "{text}");
    assert!(!text.contains("full card"), "{text}");
}

fn key_letters_use_the_near_tone(size: Fit) {
    for cols in [30, 80] {
        let g = route_card("id", "from", "to", cols, size);
        for ch in ['y', 'Y'] {
            let cell = g
                .rows
                .values()
                .rev()
                .find_map(|r| r.values().find(|c| c.ch == ch))
                .unwrap();
            assert_eq!(cell.tone, Tone::Near, "{cols} {size:?} key {ch}");
        }
        let key = if matches!(size, Fit::Whole) {
            'e'
        } else {
            '⏎'
        };
        assert!(
            g.rows
                .values()
                .any(|r| r.values().any(|c| c.ch == key && c.tone == Tone::Near)),
            "{cols} {size:?} key {key}"
        );
    }
}

#[test]
fn full_card_keys_use_the_near_tone() {
    key_letters_use_the_near_tone(Fit::Whole);
}

#[test]
fn one_row_card_keys_use_the_near_tone() {
    key_letters_use_the_near_tone(Fit::Rows(1));
}

#[test]
fn compact_card_keys_use_the_near_tone() {
    key_letters_use_the_near_tone(Fit::Rows(10));
}

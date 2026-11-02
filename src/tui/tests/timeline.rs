use super::*;

fn titled_job(id: &str, title: &str, state: &str, deps: &[&str], created: u64, peer: &str) -> Job {
    Job {
        job_id: id.into(),
        title: title.into(),
        state: state.into(),
        assigned_peer: Some(peer.into()),
        worker: Some(peer.into()),
        assignee_id: Some(peer.into()),
        depends_on: deps.iter().map(|d| (*d).into()).collect(),
        dispatch: true,
        created_at: Some(created),
        from_peer: "lead".into(),
        circle: "c".into(),
        ..Default::default()
    }
}

fn timeline_ask(id: &str, to: &str, opened: u64, open: bool, failed: bool) -> Ask {
    Ask {
        correlation_id: id.into(),
        from_peer: "lead".into(),
        to_peer: to.into(),
        to_peer_id: to.into(),
        open,
        failed,
        opened_at: Some(opened),
        ..Default::default()
    }
}

/* one chain: done, running, idle holder, failed, blocked, queued */
fn timeline_chain() -> Snapshot {
    let mut scope = titled_job("scope", "scope the work", "done", &[], 100, "cc");
    scope.ask_id = Some("ask-scope".into());
    scope.finished_at = Some(900);
    let mut runit = titled_job("runit", "run the suite", "running", &["scope"], 101, "pi");
    runit.ask_id = Some("ask-runit".into());
    runit.progress = Some(crate::wire::JobProgress {
        state: Some(Progress::Work { since: 850 }),
        busy: true,
    });
    let mut idleh = titled_job("idleh", "hold the ask", "running", &["scope"], 102, "p2");
    idleh.ask_id = Some("ask-idleh".into());
    idleh.progress = Some(crate::wire::JobProgress {
        state: Some(Progress::Idle {
            since: 980,
            why: IdleWhy::TurnEnded,
        }),
        busy: false,
    });
    let mut failj = titled_job("failj", "sec review", "failed", &["scope"], 103, "w3");
    failj.ask_id = Some("ask-failj".into());
    failj.finished_at = Some(950);
    let mut block = titled_job("block", "blocked child", "queued", &["failj"], 104, "w4");
    block.dispatch_state = Some(DispatchState::Blocked {
        dependencies: vec!["failj".into()],
        dependency: "failj".into(),
        reason: BlockReason::Failed,
    });
    let queue = titled_job("queue", "queued last", "queued", &["block"], 105, "w5");
    Snapshot {
        schema_version: 2,
        captured_at: 1000,
        jobs: vec![scope, runit, idleh, failj, block, queue],
        asks: vec![
            timeline_ask("ask-scope", "cc", 800, false, false),
            timeline_ask("ask-runit", "pi", 850, true, false),
            timeline_ask("ask-idleh", "p2", 860, true, false),
            timeline_ask("ask-failj", "w3", 870, false, true),
        ],
        ..Default::default()
    }
}

fn timeline_of(snap: Snapshot, sel: &str, cols: usize) -> layout::Grid {
    let (chain, num) = numbered(&snap);
    let view = View {
        snap: model::Frame {
            view: snap,
            fresh: true,
        },
        chain: &chain,
        num: &num,
        base: 0,
        sel,
    };
    layout::timeline(&view, cols)
}

fn line_of(g: &layout::Grid, row: usize) -> String {
    let w = g.width().max(1);
    g.line(row, w)
        .iter()
        .filter(|cell| cell.ch != layout::SPACER)
        .map(|cell| cell.ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn t_toggles_the_jobs_body_between_flow_and_timeline() {
    let mut app = app_with(timeline_chain());
    let flow = screen_text(&mut app, 80, 40);
    let flow_text = flow.join("\n");
    assert!(
        flow.iter()
            .any(|l| l.contains('●') || l.contains('◆') || l.contains('○')),
        "the flow is up first:\n{flow_text}"
    );
    assert!(
        flow.last().is_some_and(|l| l.contains("t timeline")),
        "footer names the toggle:\n{flow_text}"
    );
    assert!(
        !flow_text.contains("now") || !flow.iter().any(|l| l.contains('━')),
        "the timeline is not drawn yet:\n{flow_text}"
    );

    app.key(KeyCode::Char('t'));
    let time = screen_text(&mut app, 80, 40);
    let time_text = time.join("\n");
    assert!(
        time.iter().any(|l| l.contains("now")),
        "the axis says now:\n{time_text}"
    );
    assert!(
        time.iter().any(|l| l.contains('━') || l.contains('╌')),
        "a bar is drawn:\n{time_text}"
    );
    assert!(
        time.last().is_some_and(|l| l.contains("t flow")),
        "footer names the way back:\n{time_text}"
    );
    assert!(
        !time_text.contains("t timeline"),
        "the flow hint is gone:\n{time_text}"
    );

    app.key(KeyCode::Char('t'));
    let back = screen_text(&mut app, 80, 40);
    assert!(
        back.last().is_some_and(|l| l.contains("t timeline")),
        "t returns to the flow:\n{}",
        back.join("\n")
    );
    assert!(
        back.iter()
            .any(|l| l.contains('●') || l.contains('◆') || l.contains('○')),
        "the flow body is back:\n{}",
        back.join("\n")
    );
}

#[test]
fn timeline_rows_follow_flow_order_with_state_words() {
    let g = timeline_of(timeline_chain(), "runit", 80);
    let text = g.text();
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows.iter().any(|l| l.contains("now")),
        "shared axis:\n{text}"
    );
    let body: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|l| !l.contains("now"))
        .collect();
    let find = |needle: &str| {
        body.iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle} in\n{text}"))
    };
    let order = [
        find("scope the"),
        find("run the"),
        find("hold the"),
        find("sec review"),
        find("blocked chi"),
        find("queued last"),
    ];
    let mut sorted = order;
    sorted.sort();
    assert_eq!(order, sorted, "flow order:\n{text}");
    assert!(text.contains("done"), "{text}");
    assert!(text.contains("run "), "{text}");
    assert!(text.contains("IDLE "), "{text}");
    assert!(text.contains("failed"), "{text}");
    assert!(text.contains("blocked"), "{text}");
    assert!(text.contains("queued"), "{text}");
    assert!(
        text.contains("1 ") && text.contains("6 "),
        "numbers:\n{text}"
    );
}

#[test]
fn timeline_bars_use_the_state_glyphs_and_tones() {
    let g = timeline_of(timeline_chain(), "runit", 80);
    let text = g.text();
    let mut saw_done = false;
    let mut saw_run = false;
    let mut saw_idle = false;
    let mut saw_fail = false;
    let mut fail_ends_x = false;
    for r in 0..g.height() {
        let line = line_of(&g, r);
        let cells = g.line(r, g.width().max(1));
        let bars: Vec<_> = cells
            .iter()
            .filter(|c| c.ch == '━' || c.ch == '╌' || c.ch == '×')
            .collect();
        if line.contains("done") && !line.contains("now") {
            assert!(
                bars.iter().any(|c| c.ch == '━' && c.tone == Tone::Dim),
                "done bar:\n{text}"
            );
            saw_done = true;
        }
        if line.contains("run ") {
            assert!(
                bars.iter().any(|c| c.ch == '━' && c.tone == Tone::Run),
                "run bar:\n{text}"
            );
            saw_run = true;
        }
        if line.contains("IDLE ") {
            assert!(
                bars.iter().any(|c| c.ch == '╌' && c.tone == Tone::Wait),
                "idle dash:\n{text}"
            );
            saw_idle = true;
        }
        if line.contains("failed") {
            assert!(
                bars.iter().any(|c| c.ch == '━' && c.tone == Tone::Fail),
                "failed bar:\n{text}"
            );
            let last = bars.last().copied();
            fail_ends_x = last.is_some_and(|c| c.ch == '×' && c.tone == Tone::Fail);
            saw_fail = true;
        }
        if line.contains("blocked") || line.contains("queued") && !line.contains("blocked") {
            assert!(
                bars.is_empty(),
                "queued/blocked have no bar: {line}\n{text}"
            );
        }
    }
    assert!(saw_done && saw_run && saw_idle && saw_fail, "{text}");
    assert!(fail_ends_x, "failed ends in ×:\n{text}");
}

#[test]
fn timeline_worker_is_soft() {
    let g = timeline_of(timeline_chain(), "runit", 80);
    let mut saw = false;
    for r in 0..g.height() {
        let line = line_of(&g, r);
        if !line.contains("run the") {
            continue;
        }
        for cell in g.line(r, g.width().max(1)) {
            if cell.ch != 'p' && cell.ch != 'i' {
                continue;
            }
            if cell.tone == Tone::Soft {
                saw = true;
            }
        }
    }
    assert!(saw, "worker Tone::Soft:\n{}", g.text());
}

#[test]
fn timeline_fits_46_80_and_100() {
    for cols in [46, 80, 100] {
        let g = timeline_of(timeline_chain(), "runit", cols);
        let text = g.text();
        assert!(
            g.width() <= cols,
            "{cols}: width {} > cols\n{text}",
            g.width()
        );
        assert!(text.contains("now"), "{cols}: axis\n{text}");
        assert!(text.contains("done"), "{cols}: done\n{text}");
        assert!(
            text.contains('━') || text.contains('╌'),
            "{cols}: bar\n{text}"
        );
        let mut app = app_with(timeline_chain());
        app.key(KeyCode::Char('t'));
        let lines = screen_text(&mut app, cols, 40);
        assert!(
            lines.iter().any(|l| l.contains("now")),
            "{cols}: screen axis\n{}",
            lines.join("\n")
        );
        for line in &lines {
            assert!(layout::width(line) <= cols, "{cols}: row overflows: {line}");
        }
    }
}

#[test]
fn ascii_maps_timeline_bars() {
    let mut app = app_with(timeline_chain());
    app.opts.ascii = true;
    app.key(KeyCode::Char('t'));
    let text = screen_text(&mut app, 80, 40).join("\n");
    assert!(text.contains('='), "━ → =\n{text}");
    assert!(text.contains('~'), "╌ → ~\n{text}");
    assert!(!text.contains('━') && !text.contains('╌'), "{text}");
}

#[test]
fn the_timeline_scrolls_to_the_selected_row() {
    let mut snap = Snapshot {
        captured_at: 1000,
        ..Default::default()
    };
    for n in 0..30 {
        let id = format!("j{n:02}");
        let prev = format!("j{:02}", n - 1);
        let deps = [prev.as_str()];
        snap.jobs.push(titled_job(
            &id,
            &id,
            "queued",
            if n == 0 { &[] } else { &deps },
            n as u64,
            "cc",
        ));
    }
    let mut app = app_with(snap);
    app.sel = Some("j29".into());
    app.key(KeyCode::Char('t'));
    let screen = screen_text(&mut app, 80, 20);
    let body: Vec<_> = screen.iter().take_while(|l| !l.starts_with('┌')).collect();
    assert!(
        body.iter().any(|l| l.trim_start().starts_with("30 j29")),
        "selected row absent from timeline:\n{}",
        screen.join("\n")
    );
}

#[test]
fn the_timeline_names_the_actual_worker() {
    let mut snap = timeline_chain();
    snap.jobs[1].assigned_peer = Some("old".into());
    snap.jobs[1].worker = Some("new-id".into());
    snap.jobs[1].assignee_id = None;
    snap.asks[1].to_peer_id = "new-id".into();
    snap.asks[1].to_peer = "old".into();
    snap.peers.push(model::Peer {
        peer_id: "new-id".into(),
        name: "new".into(),
        ..Default::default()
    });
    let grid = timeline_of(snap, "runit", 80);
    let line = grid
        .text()
        .lines()
        .find(|l| l.contains("run the"))
        .unwrap()
        .to_string();
    assert!(line.contains("new") && !line.contains("old"), "{line}");
}

#[test]
fn a_cancelled_interval_takes_no_run_colour() {
    let mut snap = timeline_chain();
    snap.jobs[1].state = "cancelled".into();
    snap.jobs[1].finished_at = Some(950);
    snap.jobs[1].progress = None;
    let grid = timeline_of(snap, "runit", 80);
    let r = grid
        .text()
        .lines()
        .position(|l| l.contains("cancelled"))
        .unwrap();
    assert!(
        grid.line(r, grid.width())
            .iter()
            .filter(|c| c.ch == '━')
            .all(|c| c.tone != Tone::Run),
        "cancelled interval has Run tone:\n{}",
        grid.text()
    );
}

#[test]
fn a_finished_job_without_an_end_draws_no_bar() {
    let mut snap = timeline_chain();
    snap.jobs[0].finished_at = None;
    let grid = timeline_of(snap, "runit", 80);
    let r = grid
        .text()
        .lines()
        .position(|l| l.contains("scope the"))
        .unwrap();
    assert!(
        grid.line(r, grid.width()).iter().all(|c| c.ch != '━'),
        "done without finished_at is painted through now:\n{}",
        grid.text()
    );
}

#[test]
fn extreme_idle_stamps_keep_the_timeline_inside_the_pane() {
    for cols in [30, 46, 80, 100] {
        for since in [0, 850, 980, 1000, 1100, u64::MAX] {
            let mut snap = timeline_chain();
            snap.jobs[2].progress.as_mut().unwrap().state = Some(Progress::Idle {
                since,
                why: IdleWhy::TurnEnded,
            });
            let g = timeline_of(snap, "idleh", cols);
            assert!(g.width() <= cols, "{cols}, {since}: {}", g.text());
        }
    }
}

#[test]
fn the_idle_dash_starts_where_the_worker_went_idle() {
    for cols in [46, 80, 100] {
        let g = timeline_of(timeline_chain(), "idleh", cols);
        let axis = line_of(&g, 0);
        assert!(
            axis.contains(&layout::clock(Some(800)))
                && axis.contains(&layout::clock(Some(900)))
                && axis.contains("now"),
            "{cols}: {axis}"
        );
        let row = g.text().lines().position(|l| l.contains("IDLE")).unwrap();
        let cells = g.line(row, g.width());
        let bars: Vec<_> = cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.ch == '━' || c.ch == '╌')
            .collect();
        let bar_start = axis.find(&layout::clock(Some(800))).unwrap();
        let bar_end = axis.find("now").unwrap() + 3;
        let expected = bar_start + ((980 - 800) * (bar_end - bar_start) / (1000 - 800));
        let idle_start = bars.iter().find(|(_, c)| c.ch == '╌').unwrap().0;
        assert_eq!(idle_start, expected, "{cols}: {}", g.text());
    }
}

#[test]
fn idle_segment_clamps_to_the_bar() {
    for (since, last_only) in [(850, false), (1000, true), (1100, true)] {
        let mut snap = timeline_chain();
        snap.jobs[2].progress.as_mut().unwrap().state = Some(Progress::Idle {
            since,
            why: IdleWhy::TurnEnded,
        });
        let g = timeline_of(snap, "idleh", 80);
        let r = g.text().lines().position(|l| l.contains("IDLE")).unwrap();
        let bar: Vec<char> = g
            .line(r, g.width())
            .iter()
            .map(|c| c.ch)
            .filter(|&ch| ch == '━' || ch == '╌')
            .collect();
        let idle = bar.iter().filter(|&&ch| ch == '╌').count();
        if last_only {
            assert_eq!(idle, 1, "idle from {since}, at or after now:\n{}", g.text());
            assert_eq!(bar.last(), Some(&'╌'), "{}", g.text());
        } else {
            assert_eq!(
                idle,
                bar.len(),
                "idle from {since}, before the send:\n{}",
                g.text()
            );
        }
    }
}

#[test]
fn an_inverted_finished_interval_draws_no_bar() {
    let mut snap = timeline_chain();
    snap.jobs[0].finished_at = Some(700);
    let g = timeline_of(snap, "runit", 80);
    let r = g
        .text()
        .lines()
        .position(|l| l.contains("scope the"))
        .unwrap();
    assert!(
        g.line(r, g.width()).iter().all(|c| c.ch != '━'),
        "finished before it was sent:\n{}",
        g.text()
    );
}

#[test]
fn timeline_hints_sit_where_the_row_starts() {
    let mut app = app_with(timeline_chain());
    app.key(KeyCode::Char('t'));
    app.key(KeyCode::Char('f'));
    let screen = screen_text(&mut app, 80, 40);
    let axis = screen.iter().position(|l| l.ends_with("now")).unwrap();
    let mut letters = Vec::new();
    for title in ["scope", "run t", "hold ", "sec r", "block", "queued l"] {
        let row = screen[axis + 1..]
            .iter()
            .find(|l| l.contains(title))
            .unwrap_or_else(|| panic!("no row for {title}:\n{}", screen.join("\n")));
        let first = row.chars().next().unwrap();
        assert!(
            super::input::HINT_KEYS.contains(first),
            "the letter replaces the number at the row start: {row}"
        );
        letters.push(first);
    }
    letters.dedup();
    assert_eq!(letters.len(), 6, "one letter per job: {letters:?}");
}

#[test]
fn future_starts_leave_the_axis_at_the_snapshot() {
    let mut snap = timeline_chain();
    for ask in &mut snap.asks {
        ask.opened_at = Some(1100);
    }
    let g = timeline_of(snap, "runit", 80);
    let axis = line_of(&g, 0);
    assert!(
        !axis.contains(&layout::clock(Some(1100))) && axis.contains("now"),
        "the axis ends at captured_at: {axis}"
    );
    let r = g
        .text()
        .lines()
        .position(|l| l.contains("run the"))
        .unwrap();
    assert!(
        g.line(r, g.width()).iter().all(|c| c.ch != '━'),
        "a start after captured_at is no elapsed time:\n{}",
        g.text()
    );
}

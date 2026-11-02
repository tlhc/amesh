use super::*;

fn s5() -> Snapshot {
    let mut jobs = vec![job("scope", "done", &[], 100)];
    for i in 0..10 {
        jobs.push(job(
            &format!("r{i:02}"),
            if i % 3 == 0 { "running" } else { "done" },
            &["scope"],
            101 + i,
        ));
    }
    jobs.push(job(
        "synth",
        "queued",
        &[
            "r00", "r01", "r02", "r03", "r04", "r05", "r06", "r07", "r08", "r09",
        ],
        120,
    ));
    jobs.push(job("fix", "queued", &["synth"], 121));
    jobs.push(job("deploy", "queued", &["fix"], 122));
    Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }
}

#[test]
fn chains_stage_and_number_in_topological_order() {
    let snap = s1();
    let chains = model::chains(&snap);
    assert_eq!(chains.len(), 1);
    let (chain, num) = numbered(&snap);
    assert_eq!(chain.name, "deploy");
    assert_eq!(
        chain.stages.iter().map(Vec::len).collect::<Vec<_>>(),
        [1, 5, 1, 1, 1]
    );
    let order: Vec<&str> = [
        "scope", "func", "perf", "deliv", "sec", "upg", "synth", "fix", "deploy",
    ]
    .to_vec();
    for (i, id) in order.iter().enumerate() {
        assert_eq!(num[*id], i + 1, "{id}");
    }
    let mut two = s1();
    two.jobs.push(job("lone", "queued", &[], 50));
    assert_eq!(
        model::chains(&two).len(),
        2,
        "an unconnected job is its own chain"
    );
}

#[test]
fn numbers_stay_put_when_a_job_is_added() {
    let mut numbers = Numbers::default();
    let snap = s4();
    let first: HashMap<String, usize> = numbers
        .assign(&model::chains(&snap)[0])
        .into_iter()
        .map(|(n, id)| (id, n))
        .collect();
    let mut grown = s4();
    grown.jobs.push(job("hotfix", "queued", &["audit"], 200));
    let second: HashMap<String, usize> = numbers
        .assign(&model::chains(&grown)[0])
        .into_iter()
        .map(|(n, id)| (id, n))
        .collect();
    for (id, n) in &first {
        assert_eq!(second[id], *n, "{id} kept its number");
    }
    assert_eq!(second["hotfix"], 7, "the new job takes the next number");
    let mut gone = grown.clone();
    gone.jobs.retain(|j| j.job_id != "hotfix");
    numbers.forget_gone(&gone);
    assert_eq!(numbers.len(), 6, "a job the hub dropped is forgotten");
}

#[test]
fn independent_jobs_share_one_block_and_numbers_stay_dense() {
    let snap = mixed();
    let chains = model::chains(&snap);
    assert_eq!(chains.len(), 2);
    assert!(
        chains[0].loose && chains[0].topo() == ["lint", "docs"],
        "{:?}",
        chains[0]
    );
    assert!(!chains[1].loose);
    let mut numbers = Numbers::default();
    let to_map = |pairs: Vec<(usize, String)>| {
        pairs
            .into_iter()
            .map(|(n, id)| (id, n))
            .collect::<HashMap<_, _>>()
    };
    let three = Snapshot {
        jobs: vec![
            job("a", "done", &[], 1),
            job("b", "done", &[], 2),
            job("c", "done", &[], 3),
        ],
        ..Default::default()
    };
    let loose = to_map(numbers.assign(&model::chains(&three)[0]));
    assert_eq!((loose["a"], loose["b"], loose["c"]), (1, 2, 3));
    let mut grown = three.clone();
    grown.jobs.push(job("d", "queued", &["b"], 4));
    let chains = model::chains(&grown);
    let (chain, rest) = if chains[0].loose {
        (&chains[1], &chains[0])
    } else {
        (&chains[0], &chains[1])
    };
    let chain = to_map(numbers.assign(chain));
    let rest = to_map(numbers.assign(rest));
    assert_eq!(
        (chain["b"], chain["d"]),
        (1, 2),
        "b left the loose block, so its chain starts at 1"
    );
    assert_eq!(
        (rest["a"], rest["c"]),
        (1, 2),
        "the loose block closes the gap b left"
    );
}

#[test]
fn vertical_flow_draws_brackets_rails_and_boxes() {
    let snap = s1();
    let (chain, num) = numbered(&snap);
    let text = layout::vertical(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "perf",
        },
        46,
    )
    .text();
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines
            .iter()
            .any(|l| l.contains('┌') && l.contains('┼') && l.contains('┐')),
        "fan-out:\n{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains('└') && l.contains('┼') && l.contains('┘')),
        "fan-in:\n{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("2 func") && l.contains("6 upg")),
        "stage on one row:\n{text}"
    );
    assert!(
        lines.iter().all(|l| width(l) <= 46),
        "fits 46 columns:\n{text}"
    );

    let snap = s4();
    let (chain, num) = numbered(&snap);
    let text = layout::vertical(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "deploy",
        },
        46,
    )
    .text();
    let audit = text.lines().find(|l| l.contains("1 audit")).unwrap();
    let verify = text.lines().find(|l| l.contains("6 verify")).unwrap();
    assert!(
        audit.trim_end().ends_with('┐'),
        "rail leaves audit:\n{text}"
    );
    assert!(
        verify.contains('◀') && verify.trim_end().ends_with('┘'),
        "rail enters verify:\n{text}"
    );

    let snap = s5();
    let (chain, num) = numbered(&snap);
    let text = layout::vertical(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "r03",
        },
        46,
    )
    .text();
    assert!(
        text.contains("2-11 · 10 jobs"),
        "wide stage folds into a box:\n{text}"
    );
    assert!(
        text.lines().all(|l| width(l) <= 46),
        "fits 46 columns:\n{text}"
    );
}

#[test]
fn horizontal_flow_annotates_skip_level_edges() {
    let snap = s4();
    let (chain, num) = numbered(&snap);
    let text = layout::horizontal(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "audit",
        },
        100,
    )
    .unwrap()
    .text();
    assert!(text.contains("─┬─"), "{text}");
    assert!(text.contains("↑ also needs 1 audit"), "{text}");
}

#[test]
fn prefix_jump_hints_and_search() {
    let mut input = Input::new();
    assert_eq!(
        input.key(KeyCode::Char('5'), 14),
        Act::Pick(5),
        "a unique prefix jumps at once"
    );
    assert_eq!(input.key(KeyCode::Char('1'), 14), Act::None);
    assert_eq!(input.mode, Mode::Jump("1".into()));
    assert_eq!(input.candidates(14), [1, 10, 11, 12, 13, 14]);
    assert_eq!(input.key(KeyCode::Char('2'), 14), Act::Pick(12));
    input.key(KeyCode::Char('1'), 14);
    assert_eq!(
        input.key(KeyCode::Enter, 14),
        Act::Pick(1),
        "enter takes the prefix itself"
    );
    input.key(KeyCode::Char('1'), 14);
    assert_eq!(
        input.key(KeyCode::Char('9'), 14),
        Act::None,
        "no number starts with 19"
    );
    assert_eq!(input.mode, Mode::Normal);
    input.key(KeyCode::Char('1'), 14);
    assert_eq!(input.key(KeyCode::Esc, 14), Act::None);
    assert_eq!(input.mode, Mode::Normal);
    assert_eq!(
        input.key(KeyCode::Char('1'), 9),
        Act::Pick(1),
        "with nine jobs every digit is unique"
    );
    input.key(KeyCode::Char('f'), 14);
    assert_eq!(
        input.key(KeyCode::Char('d'), 14),
        Act::Hint(2),
        "home-row hints"
    );
    input.key(KeyCode::Char('/'), 14);
    for c in "upg".chars() {
        input.key(KeyCode::Char(c), 14);
    }
    assert_eq!(input.key(KeyCode::Enter, 14), Act::Find(0));
    assert_eq!(input.query, "upg");
}

#[test]
fn keys_move_through_the_chain() {
    let mut app = app_with(s1());
    app.screen(46, 40, 0);
    assert_eq!(
        app.sel.as_deref(),
        Some("perf"),
        "the first running job is selected"
    );
    app.key(KeyCode::Char('j'));
    assert_eq!(app.sel.as_deref(), Some("deliv"));
    app.key(KeyCode::Char('l'));
    assert_eq!(app.sel.as_deref(), Some("sec"), "l stays in the stage");
    app.key(KeyCode::Char('9'));
    assert_eq!(app.sel.as_deref(), Some("deploy"));
    app.key(KeyCode::Char('j'));
    assert_eq!(app.sel.as_deref(), Some("scope"), "j wraps to the top");
    app.key(KeyCode::Char('/'));
    for c in "sec".chars() {
        app.key(KeyCode::Char(c));
    }
    app.key(KeyCode::Enter);
    assert_eq!(app.sel.as_deref(), Some("sec"));
}

#[test]
fn the_box_counts_a_cancelled_job() {
    let mut snap = mixed();
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "lint")
        .unwrap()
        .state = "cancelled".into();
    let mut app = app_with(snap);
    app.sel = Some("docs".into());
    let text = screen_text(&mut app, 46, 60).join("\n");
    assert!(text.contains("1-2 · 2 jobs · 1● 0◆ 0× 0○ 1–"), "{text}");
    let narrow = screen_text(&mut app, 30, 60).join("\n");
    assert!(
        narrow.contains("│ 2 jobs · 1● 0◆ 0× 0○ 1– "),
        "a narrow pane keeps the counts:\n{narrow}"
    );
}

#[test]
fn one_chain_per_screen_and_tab_crosses_them() {
    let mut snap = mixed();
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "lint")
        .unwrap()
        .title = "lint the tree".into();
    let mut app = app_with(snap);
    let text = screen_text(&mut app, 46, 60).join("\n");
    assert_eq!(app.sel.as_deref(), Some("lint"), "the first running job");
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    assert_eq!(
        lines[1..3],
        [
            "  deploy            ●─×─○─○─○  3/9  × sec",
            "▸ independent jobs  ◆          1/2  1 run"
        ],
        "a row per block, the selected one marked and the one with a failure first:\n{text}"
    );
    assert!(
        lines[3].starts_with("───") && lines[3].trim_end().ends_with("─ ● ─"),
        "the strip says what runs elsewhere, so the rule does not:\n{text}"
    );
    assert!(
        text.contains("1-2 · 2 jobs · 1● 1◆ 0× 0○")
            && text
                .lines()
                .any(|l| l.contains("1 lint the tree") && l.contains("2 docs")),
        "independent jobs share the box:\n{text}"
    );
    assert!(
        !text.contains("7 synth"),
        "one block's flow at a time:\n{text}"
    );
    app.key(KeyCode::Char('j'));
    assert_eq!(app.sel.as_deref(), Some("docs"));
    app.key(KeyCode::Char('j'));
    assert_eq!(app.sel.as_deref(), Some("lint"), "j wraps within the block");
    app.key(KeyCode::Char('k'));
    assert_eq!(app.sel.as_deref(), Some("docs"));
    app.key(KeyCode::Tab);
    assert_eq!(
        app.sel.as_deref(),
        Some("scope"),
        "tab goes to the next block"
    );
    let text = screen_text(&mut app, 46, 60).join("\n");
    assert!(
        text.lines().nth(1) == Some("▸ deploy            ●─×─○─○─○  3/9  × sec")
            && text.lines().nth(2) == Some("  independent jobs  ◆          1/2  1 run")
            && text.lines().nth(3).unwrap().ends_with("─ ● ─")
            && !text.contains("lint"),
        "{text}"
    );
    app.key(KeyCode::Char('9'));
    assert_eq!(
        app.sel.as_deref(),
        Some("deploy"),
        "digits count in the current block"
    );
    app.key(KeyCode::Char('f'));
    let hinted = screen_text(&mut app, 46, 60);
    let at = hinted
        .iter()
        .position(|l| l.contains("3 perf"))
        .expect("stage row");
    assert!(
        !hinted[at - 1].contains('◆') && !hinted[at - 1].contains('●'),
        "letters replace glyphs:\n{}",
        hinted.join("\n")
    );
    assert!(
        hinted.iter().any(|l| l.starts_with("┌○ 9 deploy")),
        "the card keeps its glyph:\n{}",
        hinted.join("\n")
    );
    app.key(KeyCode::Char('d'));
    assert_eq!(app.sel.as_deref(), Some("perf"), "a scope, s func, d perf");
    app.key(KeyCode::Tab);
    app.key(KeyCode::Char('2'));
    assert_eq!(app.sel.as_deref(), Some("docs"), "two jobs: 2 is unique");
}

/* the design's S1 chain, perf's worker idle on it and upg's at a prompt, beside two loose jobs */
fn two_blocks() -> Snapshot {
    let mut snap = mixed();
    set_progress(
        &mut snap,
        "perf",
        Progress::Idle {
            since: 900,
            why: IdleWhy::TurnEnded,
        },
        false,
    );
    set_progress(
        &mut snap,
        "upg",
        Progress::Wait {
            since: 900,
            reason: None,
        },
        false,
    );
    snap
}

#[test]
fn the_strip_gives_each_block_its_stations_count_and_what_needs_a_person() {
    let mut app = app_with(two_blocks());
    let lines = screen_text(&mut app, 100, 40);
    assert_eq!(
        lines[1],
        "  deploy            ●─×─○─○─○  3/9  × sec   WAIT upg   IDLE perf",
        "the block with a failure first, then its pills in order:\n{}",
        lines.join("\n")
    );
}

#[test]
fn the_strip_draws_from_the_theme_roles() {
    let mut app = app_with(two_blocks());
    app.opts.color = true;
    let theme = super::theme::Theme::default();
    let lines = app.screen(100, 40, 0);
    let span = |row: usize, piece: &str| {
        lines[row]
            .spans
            .iter()
            .find(|s| s.content.contains(piece))
            .unwrap_or_else(|| panic!("no {piece:?} on row {row}"))
            .style
    };
    assert_eq!(
        (span(1, "× sec").fg, span(1, "× sec").bg),
        (Some(theme.fail), Some(theme.select_bg)),
        "a failure is a fail pill"
    );
    let wait = span(1, "WAIT upg");
    assert!(
        wait.fg == Some(theme.wait) && wait.add_modifier.contains(Modifier::REVERSED),
        "a prompt is the wait badge: {wait:?}"
    );
    assert_eq!(
        (span(1, "IDLE perf").fg, span(1, "IDLE perf").bg),
        (Some(theme.wait), Some(theme.select_bg)),
        "an idle holder is a wait pill"
    );
    assert_eq!(
        span(2, "1 run").fg,
        Some(theme.run),
        "nothing to answer: the run count"
    );
    assert_eq!(
        span(1, "×").fg,
        Some(theme.fail),
        "the station of the stage that failed"
    );
    assert_eq!(
        span(2, "▸").fg,
        Some(theme.near),
        "the selected block's mark"
    );
    assert_eq!(span(2, "independent jobs").fg, Some(theme.title));
    assert_eq!(span(1, "deploy").fg, Some(theme.text));
    assert_eq!(span(1, "3").fg, Some(theme.done), "the done count");
}

/* five chains of two jobs; c and e failed */
fn five_chains() -> Snapshot {
    let mut jobs = Vec::new();
    for (i, name) in ["a", "b", "c", "d", "e"].iter().enumerate() {
        let last = if matches!(*name, "c" | "e") {
            "failed"
        } else {
            "queued"
        };
        jobs.push(job(&format!("{name}1"), "done", &[], 10 * i as u64));
        jobs.push(job(
            &format!("{name}2"),
            last,
            &[&format!("{name}1")],
            10 * i as u64 + 1,
        ));
    }
    Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }
}

#[test]
fn blocks_that_need_a_person_come_first_and_the_selected_one_always_shows() {
    let mut app = app_with(five_chains());
    app.sel = Some("d1".into());
    let lines = screen_text(&mut app, 60, 40);
    let names: Vec<&str> = lines[1..5]
        .iter()
        .map(|l| l.split_whitespace().find(|w| *w != "▸").unwrap())
        .collect();
    assert_eq!(
        names,
        ["c2", "e2", "d2", "+2"],
        "the failed blocks first, then the selected one, then a count of the rest:\n{}",
        lines.join("\n")
    );
    assert!(lines[3].starts_with("▸ d2"), "{}", lines.join("\n"));
    assert_eq!(lines[4], "  +2 more");
    assert_eq!(rule_row(&lines), 5, "{}", lines.join("\n"));
}

#[test]
fn a_short_pane_folds_the_rest_into_the_selected_row() {
    let mut snap = five_chains();
    for job in &mut snap.jobs {
        job.title = format!("{}-long-chain-name", job.job_id);
    }
    let mut app = app_with(snap);
    app.sel = Some("a1".into());
    for cols in [30, 46, 60, 80, 100] {
        let lines = screen_text(&mut app, cols, 8);
        assert_eq!(
            rule_row(&lines),
            2,
            "one strip row in a pane of 8:\n{}",
            lines.join("\n")
        );
        assert!(
            lines[1].starts_with("▸ a2") && lines[1].ends_with("+4 more"),
            "{cols}: {}",
            lines[1]
        );
    }
}

#[test]
fn strip_rows_never_pass_the_pane_and_pills_give_way_first() {
    let mut snap = two_blocks();
    for job in &mut snap.jobs {
        job.title = format!("{} with a long title", job.job_id);
    }
    let mut app = app_with(snap);
    /* the screen clips a row at the pane, so the strip's own rows are measured */
    let blocks = app.blocks();
    let (b, _) = app.settle(&blocks).expect("a job is selected");
    let frame = app.snap.clone().expect("a snapshot");
    for cols in 30..=140 {
        for row in super::strip(&blocks, Some(b), &frame, cols, 4, false) {
            assert!(super::row_width(&row) <= cols, "{cols}: {row:?}");
        }
    }
    for cols in 30..=140 {
        let lines = screen_text(&mut app, cols, 40);
        let rule = rule_row(&lines);
        for row in &lines[1..rule] {
            assert!(width(row) <= cols, "{cols}: {row}");
        }
        let deploy = lines[1..rule]
            .iter()
            .find(|l| l.contains("deploy with"))
            .expect("the chain's row");
        if deploy.contains("WAIT") {
            assert!(
                deploy.contains("●─×─○─○─○"),
                "stations before pills go: {deploy}"
            );
        }
    }
    let narrow = screen_text(&mut app, 34, 40);
    assert!(
        narrow[1].contains('…') && !narrow[1].contains("WAIT"),
        "at 34 columns the pills are gone and the name or stations are cut: {}",
        narrow[1]
    );
}

#[test]
fn a_narrow_strip_keeps_the_whole_done_count() {
    let snap = Snapshot {
        jobs: (0..1000)
            .map(|i| job(&format!("j{i}"), "done", &[], i))
            .collect(),
        ..Default::default()
    };
    let mut a = app_with(snap);
    let blocks = a.blocks();
    let frame = a.snap.clone().unwrap();
    let rows = super::strip(&blocks, Some(0), &frame, 30, 4, false);
    let row: String = rows[0].iter().map(|(s, _)| s.as_str()).collect();
    assert!(row.contains("1000/1000"), "{row}");
}

#[test]
fn a_tall_flow_scrolls_to_the_selection_and_keeps_the_footer() {
    let mut app = app_with(s5());
    app.sel = Some("deploy".into());
    let lines = screen_text(&mut app, 46, 24);
    assert_eq!(lines.len(), 24, "{}", lines.join("\n"));
    assert!(
        lines.iter().any(|l| l.contains("○ 14 deploy cc")),
        "the flow row, not just the card:\n{}",
        lines.join("\n")
    );
    assert_eq!(
        lines[23],
        "j/k move  h/l stage  tab chain  t timeline",
        "the keys that fit, none cut, on the last row:\n{}",
        lines.join("\n")
    );
    assert!(
        lines.iter().any(|l| l.contains("└──")) && !lines.iter().any(|l| l.contains("more lines")),
        "the flow gives way first:\n{}",
        lines.join("\n")
    );
    let deploy = app
        .snap
        .as_mut()
        .unwrap()
        .jobs
        .iter_mut()
        .find(|j| j.job_id == "deploy")
        .unwrap();
    deploy.prompt = "roll the fix out to every host, one region at a time, and stop at the first failed health check. ".repeat(3);
    let short = screen_text(&mut app, 46, 16);
    assert!(
        short.iter().any(|l| l.contains("○ 14 deploy cc")),
        "{}",
        short.join("\n")
    );
    assert!(
        short
            .iter()
            .any(|l| l.starts_with("│ prompt") && l.contains('…'))
            && short[14].starts_with('└'),
        "the card fits the rows left, its prompt cut to a line:\n{}",
        short.join("\n")
    );
    let tiny = screen_text(&mut app, 46, 12);
    assert!(
        tiny.iter().any(|l| l.contains("⏎ full card")),
        "a short card keeps its full-card key:\n{}",
        tiny.join("\n")
    );
    app.key(KeyCode::Enter);
    let full = screen_text(&mut app, 46, 24);
    assert!(
        full.iter().all(|l| !l.contains("r05")),
        "the full card hides the flow:\n{}",
        full.join("\n")
    );
    assert!(
        full.iter()
            .any(|l| l.contains("14 deploy · queued · stage 5 of 5")),
        "{}",
        full.join("\n")
    );
}

#[test]
fn errors_say_what_went_wrong() {
    let mut app = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    app.home = false;
    app.apply(Err(format!(
        "GET /snapshot?circle=project-404abc: {}",
        crate::cli::DAEMON_UNREACHABLE
    )));
    assert_eq!(app.error.as_deref(), Some("hub unreachable"));
    app.apply(Err(
        "GET /snapshot: curl: (22) The requested URL returned error: 404".into(),
    ));
    assert!(app.error.as_deref().unwrap().contains("no /snapshot"));
    let text = screen_text(&mut app, 46, 20).join("\n");
    assert!(
        flat(&text).contains("this hub has no /snapshot; restart it on the current amesh"),
        "the whole message:\n{text}"
    );
    assert!(
        !text.contains("jobs appear"),
        "no empty-hub hint for an old hub:\n{text}"
    );
    app.apply(Err(
        "GET /snapshot: curl: (22) The requested URL returned error: 401 {\"error\":\"unauthorized\"}".into(),
    ));
    assert_eq!(
        app.error.as_deref(),
        Some("the hub refused the token; check AMESH_TOKEN")
    );
    app.apply(Err(
        "GET /snapshot: curl: (22) The requested URL returned error: 500".into(),
    ));
    assert!(
        app.error.as_deref().unwrap().contains("500"),
        "other failures are shown as they are"
    );
    app.apply(Ok(s1()));
    app.apply(Err(crate::cli::DAEMON_UNREACHABLE.to_string()));
    let text = screen_text(&mut app, 46, 40).join("\n");
    assert!(flat(&text).contains("! hub unreachable · × sec"), "{text}");
}

/* the rows a terminal shows: a wide character hides the cell after it */
fn rendered(app: &mut App, cols: u16, rows: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    terminal.draw(|frame| super::draw(frame, app, 0)).unwrap();
    let buf = terminal.backend().buffer();
    (0..rows)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < cols {
                let symbol = buf[(x, y)].symbol();
                line.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect()
}

#[test]
fn the_terminal_shows_the_same_screen() {
    let mut snap = mixed();
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "docs")
        .unwrap()
        .title = "审查 hub 数据".into();
    /* the box in a narrow pane, one row a job in a wide one */
    for (cols, flow) in [(46u16, "│ ◆ 1 lint"), (100, "◆ 1 lint")] {
        let mut app = app_with(snap.clone());
        let rows = rendered(&mut app, cols, 50);
        let text = rows.join("\n");
        assert_eq!(
            rows[1..3],
            [
                "  deploy            ●─×─○─○─○  3/9  × sec",
                "▸ independent jobs  ◆          1/2  1 run"
            ],
            "{text}"
        );
        assert!(
            text.contains("2 审查 hub 数据"),
            "wide characters keep their order:\n{text}"
        );
        assert!(text.contains(flow), "{cols} columns:\n{text}");
        assert!(rows.iter().all(|r| r.width() <= cols as usize), "{text}");
    }
}

#[test]
fn control_characters_in_job_text_never_reach_the_terminal() {
    let hostile =
        |text: &str| format!("{text}\u{1b}[2J\u{1b}]52;c;cHduZWQ=\u{7}\u{9b}31m\r\u{8} end");
    for (cols, sel, full) in [
        (46, "perf", false),
        (46, "sec", true),
        (100, "perf", true),
        (100, "sec", false),
    ] {
        let mut snap = s1();
        for job in &mut snap.jobs {
            job.title = hostile(&job.title);
            job.prompt = hostile(&job.prompt);
            job.result = Some(hostile("result"));
        }
        let mut app = app_with(snap);
        app.sel = Some(sel.into());
        if full {
            app.key(KeyCode::Enter);
        }
        let rows = rendered(&mut app, cols, 40);
        let text = rows.join("\n");
        assert!(
            !rows.concat().contains(char::is_control),
            "{cols} columns: {rows:?}"
        );
        /* the layout gives them no column, as ratatui drops them: the card keeps its frame */
        let card: Vec<&String> = rows
            .iter()
            .filter(|row| row.starts_with(['┌', '│', '└']))
            .collect();
        assert!(
            card.len() > 3 && card.iter().all(|row| row.width() == cols as usize),
            "{cols} columns, {sel}, full card {full}:\n{text}"
        );
    }
}

#[test]
fn cards_lead_with_what_matters() {
    let mut snap = s1();
    let sec = snap.jobs.iter_mut().find(|j| j.job_id == "sec").unwrap();
    sec.result = Some("no fixture".into());
    sec.actions = Some(vec![crate::wire::JobAction::Retry {
        needs_assignee: false,
    }]);
    set_dispatch(
        &mut snap,
        "synth",
        crate::wire::DispatchState::Blocked {
            dependencies: vec!["perf".into(), "sec".into(), "upg".into()],
            dependency: "sec".into(),
            reason: crate::wire::BlockReason::Failed,
        },
    );
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .ask_id = Some("ask-p".into());
    snap.asks.push(Ask {
        correlation_id: "ask-p".into(),
        open: false,
        opened_at: Some(900),
        state: Some(crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::ClosedOk,
            failed_effective: false,
        }),
        ..Default::default()
    });
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .relation = Some(crate::wire::JobRelation::Settling);
    let synth = snap.jobs.iter_mut().find(|j| j.job_id == "synth").unwrap();
    synth.ask_id = Some("ask-old".into());
    snap.asks.push(Ask {
        correlation_id: "ask-old".into(),
        open: false,
        opened_at: Some(800),
        ..Default::default()
    });
    let (chain, num) = numbered(&snap);
    let card = |id: &str| {
        let v = View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: id,
        };
        layout::card(&v, id, 46, None, Fit::Whole).text()
    };
    let sec = card("sec");
    assert!(
        sec.lines().nth(1).unwrap().contains("why") && sec.contains("no fixture"),
        "{sec}"
    );
    assert!(
        flat(&sec).contains("retry amesh jobs update sec --state queued"),
        "{sec}"
    );
    let wide = layout::card(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "sec",
        },
        "sec",
        100,
        None,
        Fit::Whole,
    )
    .text();
    let retry = wide
        .lines()
        .find(|l| l.contains("retry   amesh jobs update sec --state queued"))
        .unwrap_or_else(|| panic!("one line when there is room:\n{wide}"));
    assert!(
        retry.find("retry").unwrap() > 50,
        "commands sit under the prompt on the right:\n{wide}"
    );
    assert!(
        !wide.contains('┬') && !wide.contains('├') && !wide.contains('┴'),
        "the split stays inside the frame, as in the design:\n{wide}"
    );
    let perf = card("perf");
    assert!(flat(&perf).contains("· closed ok, settling"), "{perf}");
    let synth = card("synth");
    assert!(synth.contains("previous attempt"), "{synth}");
    assert!(synth.contains("5 sec failed: retry it first"), "{synth}");
    for id in ["scope", "perf", "sec", "synth", "deploy"] {
        assert!(
            card(id).lines().all(|l| width(l) <= 46),
            "{id} card fits:\n{}",
            card(id)
        );
    }
    let scope = card("scope");
    assert!(
        scope
            .lines()
            .any(|l| l.contains("blocks  2 func ● · 3 perf ◆ · 4 deliv ●") && !l.contains("5 sec")),
        "one line of up to three, as in the design:\n{scope}"
    );
}

#[test]
fn queued_cards_follow_the_hubs_blocking_rule() {
    let mut snap = s4();
    snap.peers.push(Peer {
        peer_id: "cc".into(),
        name: "cc".into(),
        status: "online".into(),
        ..Default::default()
    });
    set_dispatch(
        &mut snap,
        "verify",
        crate::wire::DispatchState::Waiting {
            dependencies: vec!["deploy".into()],
        },
    );
    assert!(
        card(&snap, "verify").contains("waits 5 deploy ◆"),
        "{}",
        card(&snap, "verify")
    );
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "deploy")
        .unwrap()
        .state = "done".into();
    set_dispatch(
        &mut snap,
        "verify",
        crate::wire::DispatchState::Ready {
            peer_id: "cc".into(),
        },
    );
    assert!(
        card(&snap, "verify").contains("waits nothing: will be sent next tick"),
        "{}",
        card(&snap, "verify")
    );
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "verify")
        .unwrap()
        .assigned_peer = Some("ghost".into());
    set_dispatch(
        &mut snap,
        "verify",
        crate::wire::DispatchState::NoPeer {
            name: "ghost".into(),
        },
    );
    assert!(card(&snap, "verify").contains("no peer named ghost: not sent"));
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "verify")
        .unwrap()
        .dispatch = false;
    set_dispatch(&mut snap, "verify", crate::wire::DispatchState::Held);
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "verify")
        .unwrap()
        .actions = Some(vec![crate::wire::JobAction::Send]);
    let held = card(&snap, "verify");
    assert!(
        held.contains("held: set its assignee again to send it")
            && held.contains("send amesh jobs update verify --state queued --assigned-peer ghost"),
        "{held}"
    );
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "verify")
        .unwrap()
        .assigned_peer = None;
    set_dispatch(&mut snap, "verify", crate::wire::DispatchState::Unassigned);
    let bare = card(&snap, "verify");
    assert!(
        bare.contains("not dispatched: name an assignee to send it")
            && bare.contains("--assigned-peer PEER"),
        "{bare}"
    );
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "deploy")
        .unwrap()
        .state = "cancelled".into();
    set_dispatch(
        &mut snap,
        "verify",
        crate::wire::DispatchState::Blocked {
            dependencies: vec!["deploy".into()],
            dependency: "deploy".into(),
            reason: crate::wire::BlockReason::Cancelled,
        },
    );
    assert!(
        card(&snap, "verify").contains("blocked 5 deploy cancelled: retry it first"),
        "{}",
        card(&snap, "verify")
    );
    snap.jobs.retain(|j| j.job_id != "deploy");
    snap.missing.jobs = vec!["deploy".into()];
    set_dispatch(
        &mut snap,
        "verify",
        crate::wire::DispatchState::Blocked {
            dependencies: vec!["deploy".into()],
            dependency: "deploy".into(),
            reason: crate::wire::BlockReason::Deleted,
        },
    );
    let text = card(&snap, "verify");
    assert!(
        text.contains("waits deploy deleted")
            && text.contains("blocked deploy was deleted: this job cannot start"),
        "{text}"
    );
}

#[test]
fn an_absent_dependency_is_deleted_only_when_the_hub_says_so() {
    let mut snap = s4();
    snap.jobs.retain(|j| j.job_id != "deploy");
    set_dispatch(
        &mut snap,
        "verify",
        crate::wire::DispatchState::Waiting {
            dependencies: vec!["deploy".into()],
        },
    );
    let text = card(&snap, "verify");
    assert!(
        text.contains("waits deploy") && !text.contains("deploy deleted"),
        "outside the view is not deleted: {text}"
    );
    snap.missing.jobs = vec!["deploy".into()];
    let text = card(&snap, "verify");
    assert!(text.contains("waits deploy deleted"), "{text}");
}

#[test]
fn commands_quote_what_they_copy() {
    assert_eq!(layout::quote("job-3d617c2d"), "job-3d617c2d");
    assert_eq!(layout::quote("x\"; rm -rf ~; \""), "'x\"; rm -rf ~; \"'");
    assert_eq!(layout::quote("it's"), r"'it'\''s'");
    let mut snap = s1();
    let perf = snap.jobs.iter_mut().find(|j| j.job_id == "perf").unwrap();
    perf.ask_id = Some("ask-1".into());
    perf.title = "x\"; rm -rf ~; \"".into();
    perf.relation = Some(crate::wire::JobRelation::Open);
    perf.actions = Some(vec![crate::wire::JobAction::Nudge { to: "cc".into() }]);
    snap.asks.push(Ask {
        correlation_id: "ask-1".into(),
        to_peer_id: "cc".into(),
        open: true,
        ..Default::default()
    });
    /* the nudge is offered to a worker that went idle with the ask open */
    snap.capabilities.peer_activity = true;
    snap.peers.push(doing("cc", "idle", 900, None));
    let (chain, num) = numbered(&snap);
    let text = flat(
        &layout::card(
            &View {
                snap: model::Frame {
                    view: snap.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "perf",
            },
            "perf",
            46,
            None,
            Fit::Whole,
        )
        .text(),
    );
    assert!(
        text.contains("nudge amesh peer notify cc 'x\"; rm -rf ~; \"?'"),
        "{text}"
    );
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "sec")
        .unwrap()
        .dispatch = false;
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "sec")
        .unwrap()
        .actions = Some(vec![crate::wire::JobAction::Retry {
        needs_assignee: true,
    }]);
    let text = flat(
        &layout::card(
            &View {
                snap: model::Frame {
                    view: snap.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "sec",
            },
            "sec",
            46,
            None,
            Fit::Whole,
        )
        .text(),
    );
    assert!(
        text.contains("amesh jobs update sec --state queued --assigned-peer cc"),
        "{text}"
    );
}

#[test]
fn every_screen_fits_and_ascii_stays_ascii() {
    for snap in [s1(), s4(), s5()] {
        for cols in [46, 100] {
            let mut app = app_with(snap.clone());
            let ids: Vec<String> = snap.jobs.iter().map(|j| j.job_id.clone()).collect();
            for id in &ids {
                app.sel = Some(id.clone());
                for line in app.screen(cols, 60, 0) {
                    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                    assert!(width(&text) <= cols, "{cols} cols, {id}:\n{text}");
                }
            }
        }
    }
    let mut app = App::new(Opts {
        circle: None,
        ascii: true,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    app.home = false;
    app.apply(Ok(s4()));
    for line in app.screen(46, 60, 0) {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.is_ascii(), "{text}");
    }
    /* HOME's boxes and each kind of card it shows as j walks the asks, chains and events */
    let mut home = home_app(home_busy());
    home.opts.ascii = true;
    for step in 0..8 {
        for cols in [46, 100] {
            for line in home.screen(cols, 24, 0) {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(text.is_ascii(), "{cols} cols after {step} j: {text}");
            }
        }
        home.key(KeyCode::Char('j'));
    }
}

/* the scratch-hub scenario the fixture names were too short to catch: real title lengths
and a skip-level edge from verify back to scope */
fn real() -> Snapshot {
    let jobs = vec![
        job("scope", "done", &[], 100),
        job("func review", "done", &["scope"], 101),
        job("upgrade review", "running", &["scope"], 102),
        job("perf review", "running", &["scope"], 103),
        job("security review", "failed", &["scope"], 104),
        job(
            "synth",
            "queued",
            &[
                "func review",
                "upgrade review",
                "perf review",
                "security review",
            ],
            105,
        ),
        job("deploy", "queued", &["synth"], 106),
        job("verify", "queued", &["deploy", "scope"], 107),
    ];
    Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }
}

#[test]
fn real_titles_fit_the_pane() {
    let snap = real();
    let (chain, num) = numbered(&snap);
    for cols in [46, 60, 80] {
        let text = layout::vertical(
            &View {
                snap: model::Frame {
                    view: snap.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "synth",
            },
            cols,
        )
        .text();
        assert!(
            text.lines().all(|l| width(l) <= cols),
            "{cols} cols:\n{text}"
        );
        for n in 2..=5 {
            assert!(
                text.lines()
                    .any(|l| l.contains(&format!(" {n} ")) || l.starts_with(&format!("{n} "))),
                "job {n} shown at {cols}:\n{text}"
            );
        }
        let verify = text
            .lines()
            .find(|l| l.contains("8 verify"))
            .expect("verify row");
        assert!(
            verify.contains('◀'),
            "the rail reaches verify at {cols}:\n{text}"
        );
    }
    let mut ended = snap.clone();
    ended
        .jobs
        .iter_mut()
        .find(|j| j.job_id == "security review")
        .unwrap()
        .finished_at = Some(900);
    let card = |cols| {
        layout::card(
            &View {
                snap: model::Frame {
                    view: ended.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "security review",
            },
            "security review",
            cols,
            None,
            Fit::Whole,
        )
        .text()
    };
    let head = card(46).lines().next().unwrap().to_string();
    assert!(
        head.contains("failed 1m ago") && head.contains(" FAILED ") && !head.contains("stage"),
        "the stage gives way first:\n{head}"
    );
    assert!(
        card(100).lines().next().unwrap().contains("stage 2 of 5"),
        "{}",
        card(100)
    );
    let wide = layout::horizontal(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "synth",
        },
        96,
    )
    .expect("fits 96");
    assert!(wide.width() <= 96, "{}", wide.text());
    assert!(
        layout::horizontal(
            &View {
                snap: model::Frame {
                    view: snap.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "synth"
            },
            40
        )
        .is_none(),
        "too narrow falls back"
    );
}

#[test]
fn skip_edges_beyond_the_rail_lanes_become_notes() {
    let mut jobs = vec![
        job("root", "done", &[], 100),
        job("s1", "done", &["root"], 101),
    ];
    for i in 2..11 {
        let prev = format!("s{}", i - 1);
        jobs.push(job(
            &format!("s{i}"),
            "queued",
            &[prev.as_str(), "root"],
            100 + i,
        ));
    }
    let snap = Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    };
    let (chain, num) = numbered(&snap);
    let text = layout::vertical(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "root",
        },
        46,
    )
    .text();
    assert!(text.lines().all(|l| width(l) <= 46), "{text}");
    assert!(
        !text.contains('◀') && text.contains("11 s10 also needs 1 root"),
        "nine skip edges leave no room for lanes:\n{text}"
    );
}

#[test]
fn a_cycle_or_self_dependency_still_draws() {
    let jobs = vec![
        job("a", "queued", &["b"], 1),
        job("b", "queued", &["a"], 2),
        job("me", "queued", &["me"], 3),
    ];
    let snap = Snapshot {
        captured_at: 10,
        jobs,
        ..Default::default()
    };
    for cols in [46, 100] {
        let mut app = app_with(snap.clone());
        for (id, block) in [("a", ["a", "b"]), ("b", ["a", "b"]), ("me", ["me", "me"])] {
            app.sel = Some(id.into());
            let text = screen_text(&mut app, cols, 30).join("\n");
            assert!(
                block.iter().all(|other| text
                    .lines()
                    .take(8)
                    .any(|l| l.contains(&format!(" {other}")))),
                "the flow draws the whole block at {cols}:\n{text}"
            );
            assert!(text.contains(&format!(" {id} · queued")), "{cols}:\n{text}");
        }
    }
    let chains = model::chains(&snap);
    assert!(
        chains
            .iter()
            .all(|c| c.stages.iter().all(|s| !s.is_empty())),
        "no empty stage: {chains:?}"
    );
    let me = chains.iter().find(|c| c.stage.contains_key("me")).unwrap();
    assert!(
        me.loose,
        "a job that only waits on itself is drawn with the independent jobs"
    );
}

#[test]
fn a_renumbering_cancels_a_half_typed_jump() {
    let jobs: Vec<Job> = (1..=12)
        .map(|i| job(&format!("n{i:02}"), "queued", &[], i))
        .collect();
    let mut app = app_with(Snapshot {
        captured_at: 100,
        jobs: jobs.clone(),
        ..Default::default()
    });
    app.screen(46, 40, 0);
    app.sel = Some("n05".into());
    app.key(KeyCode::Char('1'));
    assert!(
        matches!(app.input.mode, Mode::Jump(_)),
        "1 is ambiguous with 10..12"
    );
    let mut fewer = jobs;
    fewer.retain(|j| j.job_id != "n02");
    app.apply(Ok(Snapshot {
        captured_at: 101,
        jobs: fewer,
        ..Default::default()
    }));
    app.screen(46, 40, 0);
    assert_eq!(app.key(KeyCode::Char('0')), Act::None);
    assert_eq!(
        app.sel.as_deref(),
        Some("n05"),
        "10 now names another job, so nothing moves"
    );
    assert_eq!(app.input.mode, Mode::Normal);
    app.key(KeyCode::Char('1'));
    assert_eq!(
        app.key(KeyCode::Char('0')),
        Act::Pick(10),
        "a fresh jump works"
    );
    assert_eq!(app.sel.as_deref(), Some("n11"));
}

#[test]
fn a_missing_ask_is_named() {
    let mut snap = s1();
    let perf = snap.jobs.iter_mut().find(|j| j.job_id == "perf").unwrap();
    perf.ask_id = Some("ask-gone".into());
    perf.relation = Some(crate::wire::JobRelation::Missing { will_fail: true });
    let func = snap.jobs.iter_mut().find(|j| j.job_id == "func").unwrap();
    func.ask_id = Some("ask-old".into());
    func.relation = Some(crate::wire::JobRelation::CleanedUp);
    let (chain, num) = numbered(&snap);
    let card = |id: &str| {
        flat(
            &layout::card(
                &View {
                    snap: model::Frame {
                        view: snap.clone(),
                        ..Default::default()
                    },
                    chain: &chain,
                    num: &num,
                    base: 0,
                    sel: id,
                },
                id,
                46,
                None,
                Fit::Whole,
            )
            .text(),
        )
    };
    assert!(
        card("perf").contains("ask ask-gone is missing: the hub fails this job at its next check"),
        "{}",
        card("perf")
    );
    assert!(
        card("func").contains("ask ask-old was cleaned up"),
        "{}",
        card("func")
    );
}

fn glyph_of(app: &mut App, label: &str, tick: usize) -> char {
    let lines: Vec<String> = app
        .screen(100, 40, tick)
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    let line = lines
        .iter()
        .find(|l| l.contains(label))
        .unwrap_or_else(|| panic!("{label}:\n{}", lines.join("\n")));
    let at = line.find(label).unwrap();
    line[..at].trim_end().chars().last().unwrap()
}

#[test]
fn a_worker_busy_with_one_job_spins_it() {
    let mut app = app_with(busy());
    assert_eq!(glyph_of(&mut app, "3 perf", 0), '⠋');
    assert_eq!(
        glyph_of(&mut app, "3 perf", 1),
        '⠙',
        "the spinner turns every tick"
    );
    assert_eq!(
        glyph_of(&mut app, "6 upg", 0),
        '◆',
        "w2 runs two jobs, so neither spins"
    );
    assert_eq!(
        glyph_of(&mut app, "7 synth", 1),
        '◆',
        "an idle worker shows in the card; the flow keeps ◆"
    );
    assert_eq!(glyph_of(&mut app, "8 fix", 4), '◆', "so does a waiting one");
    assert!(
        row_of(&mut app, "8 fix", 0).contains("8 fix w3 1m WAIT"),
        "WAIT follows the worker"
    );
    assert!(
        row_of(&mut app, "8 fix", 4).contains("8 fix w3 1m WAIT"),
        "and holds steady on the odd half second"
    );
    let mut still = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: false,
        theme: Default::default(),
    });
    still.home = false;
    still.apply(Ok(busy()));
    assert_eq!(glyph_of(&mut still, "3 perf", 1), '⠋', "--no-anim");
    assert!(
        row_of(&mut still, "8 fix", 4).contains("WAIT"),
        "--no-anim keeps the WAIT mark"
    );
    let mut elsewhere = busy();
    elsewhere
        .jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .progress
        .as_mut()
        .unwrap()
        .busy = false;
    let mut app = app_with(elsewhere);
    assert_eq!(
        glyph_of(&mut app, "3 perf", 1),
        '◆',
        "w1 runs another job in a circle this pane filters out"
    );
    let mut old = busy();
    old.capabilities.peer_activity = false;
    for job in &mut old.jobs {
        job.progress = None;
    }
    for peer in &mut old.peers {
        peer.liveness = None;
    }
    let mut app = app_with(old);
    assert_eq!(
        glyph_of(&mut app, "3 perf", 1),
        '◆',
        "no spinner from a hub without activity"
    );
    assert!(
        !row_of(&mut app, "8 fix", 0).contains("WAIT"),
        "nor a WAIT mark"
    );
    let mut ascii = App::new(Opts {
        circle: None,
        ascii: true,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    ascii.home = false;
    ascii.apply(Ok(busy()));
    assert_eq!(glyph_of(&mut ascii, "3 perf", 1), '/', "ASCII turns too");
}

#[test]
fn cards_say_what_the_worker_is_doing() {
    let snap = busy();
    let (chain, num) = numbered(&snap);
    let card = |id: &str| {
        flat(
            &layout::card(
                &View {
                    snap: model::Frame {
                        view: snap.clone(),
                        ..Default::default()
                    },
                    chain: &chain,
                    num: &num,
                    base: 0,
                    sel: id,
                },
                id,
                46,
                None,
                Fit::Whole,
            )
            .text(),
        )
    };
    assert!(
        card("perf").contains("worker w1 WORK · turn 2m"),
        "{}",
        card("perf")
    );
    assert!(
        card("fix").contains("worker w3 WAIT! 2m Claude needs your permission to use Bash ask"),
        "{}",
        card("fix")
    );
    let synth = card("synth");
    assert!(
        synth.contains("why w4: not picked up · 1m worker w4 IDLE ask synt w4 · 00:15 unacked 1m")
            && synth.contains("nudge amesh peer notify w4 'synth?'"),
        "{synth}"
    );
    assert!(
        !card("upg").contains("⠋"),
        "w2 works on two jobs, so upg's card says what w2 does: {}",
        card("upg")
    );
    let text = layout::vertical(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "perf",
        },
        46,
    )
    .text();
    assert!(
        text.contains("7 synth w4 1m\n") && text.contains("8 fix w3 1m WAIT"),
        "{text}"
    );
    let mut old = snap.clone();
    old.capabilities.peer_activity = false;
    for job in &mut old.jobs {
        job.progress = None;
    }
    for peer in &mut old.peers {
        peer.liveness = None;
    }
    let (chain, num) = numbered(&old);
    let before = flat(
        &layout::card(
            &View {
                snap: model::Frame {
                    view: old.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "perf",
            },
            "perf",
            46,
            None,
            Fit::Whole,
        )
        .text(),
    );
    assert!(
        before.contains("worker w1 ask") && !before.contains("WORK"),
        "a hub without activity names the worker only: {before}"
    );
    let mut left = snap.clone();
    left.peers.retain(|p| p.peer_id != "w1");
    left.peers.push(Peer {
        peer_id: "w9".into(),
        name: "w1".into(),
        status: "online".into(),
        ..Default::default()
    });
    set_progress(&mut left, "perf", crate::wire::Progress::Gone, false);
    left.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .actions = Some(vec![crate::wire::JobAction::Resend]);
    let (chain, num) = numbered(&left);
    let gone = flat(
        &layout::card(
            &View {
                snap: model::Frame {
                    view: left.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "perf",
            },
            "perf",
            46,
            None,
            Fit::Whole,
        )
        .text(),
    );
    assert!(
        gone.contains("worker w1 left the hub"),
        "the name now belongs to w9: {gone}"
    );
    assert!(
        gone.contains("resend amesh jobs update perf --state queued --assigned-peer PEER"),
        "the card says how to send it to a peer that is here: {gone}"
    );
    assert!(
        !card("perf").contains("resend"),
        "a worker still here needs none: {}",
        card("perf")
    );
    let mut answered = left.clone();
    let ask = answered
        .asks
        .iter_mut()
        .find(|a| a.correlation_id == "ask-perf")
        .unwrap();
    ask.open = false;
    ask.closed_by = Some("recipient".into());
    ask.state = Some(crate::wire::AskState::Closed {
        outcome: crate::wire::AskOutcome::AckedOk,
        failed_effective: false,
    });
    let row = answered
        .jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap();
    row.relation = Some(crate::wire::JobRelation::Settling);
    row.actions = Some(vec![]);
    let (chain, num) = numbered(&answered);
    let settled = flat(
        &layout::card(
            &View {
                snap: model::Frame {
                    view: answered.clone(),
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "perf",
            },
            "perf",
            46,
            None,
            Fit::Whole,
        )
        .text(),
    );
    assert!(
        settled.contains("acked ok, settling") && !settled.contains("resend"),
        "a worker that answered before it left is not sent the job again: {settled}"
    );
}

/* scope fans out to func, perf (its worker ended its turn), deliv, sec (failed) and upg
(at work); synth waits on all five and sec's failure blocks it; fix, ship and zip queue behind */
fn fanned() -> Snapshot {
    let mut synth = job(
        "synth",
        "queued",
        &["func", "perf", "deliv", "sec", "upg"],
        107,
    );
    synth.dispatch_state = Some(DispatchState::Blocked {
        dependencies: vec!["sec".into()],
        dependency: "sec".into(),
        reason: BlockReason::Failed,
    });
    let mut perf = with(
        job("perf", "running", &["scope"], 102),
        Progress::Idle {
            since: 90,
            why: IdleWhy::TurnEnded,
        },
        false,
    );
    perf.assigned_peer = Some("codex".into());
    Snapshot {
        captured_at: 100,
        jobs: vec![
            job("scope", "done", &[], 100),
            job("func", "done", &["scope"], 101),
            perf,
            job("deliv", "done", &["scope"], 103),
            job("sec", "failed", &["scope"], 104),
            with(
                job("upg", "running", &["scope"], 105),
                Progress::Work { since: 90 },
                true,
            ),
            synth,
            job("fix", "queued", &["synth"], 108),
            job("ship", "queued", &["fix"], 109),
            job("zip", "queued", &["ship"], 110),
        ],
        ..Default::default()
    }
}

/* audit is done; deploy's worker waits for a permission; verify queues behind it */
fn waits() -> Snapshot {
    Snapshot {
        captured_at: 100,
        jobs: vec![
            job("audit", "done", &[], 100),
            with(
                job("deploy", "running", &["audit"], 101),
                Progress::Wait {
                    since: 90,
                    reason: Some("Bash".into()),
                },
                false,
            ),
            job("verify", "queued", &["deploy"], 102),
        ],
        ..Default::default()
    }
}

fn glyph_at(g: &Grid, node: usize) -> (usize, usize) {
    g.rows
        .iter()
        .flat_map(|(r, row)| row.iter().map(move |(c, cell)| (*r, *c, cell)))
        .find(|(_, _, cell)| cell.glyph && cell.node == node)
        .map(|(r, c, _)| (r, c))
        .unwrap_or_else(|| panic!("no glyph for node {node}"))
}

#[test]
fn a_fan_out_lights_only_the_branches_to_frontier_jobs() {
    let snap = fanned();
    let (g, num) = flow(&snap, "zip", 46, false);
    let above = |id: &str| {
        let (r, c) = glyph_at(&g, num[id]);
        tone_at(&g, r - 1, c)
    };
    assert_eq!(
        above("func"),
        Tone::Dim,
        "a branch to a done job\n{}",
        g.text()
    );
    assert_eq!(above("perf"), Tone::Line, "to an idle holder");
    assert_eq!(above("sec"), Tone::Line, "to a failed job");
    assert_eq!(above("upg"), Tone::Line, "to a running job");
    let (r, c) = glyph_at(&g, num["scope"]);
    assert_eq!(
        tone_at(&g, r + 1, c),
        Tone::Line,
        "the join carries every branch"
    );
}

#[test]
fn the_spine_lights_into_a_blocked_job_and_dims_into_a_queued_one() {
    let snap = fanned();
    let (g, num) = flow(&snap, "zip", 46, false);
    let (r, c) = glyph_at(&g, num["synth"]);
    assert_eq!(
        tone_at(&g, r - 1, c),
        Tone::Line,
        "into synth, blocked\n{}",
        g.text()
    );
    let (r, c) = glyph_at(&g, num["fix"]);
    assert_eq!(tone_at(&g, r - 1, c), Tone::Dim, "into fix, only queued");
}

#[test]
fn labels_say_who_needs_a_person() {
    let snap = fanned();
    let (g, num) = flow(&snap, "fix", 46, false);
    let label = |id: &str| {
        let (r, c) = find(&g, &format!("{} {id}", num[id]));
        tone_at(&g, r, c)
    };
    assert_eq!(label("scope"), Tone::Dim, "done\n{}", g.text());
    assert_eq!(label("func"), Tone::Dim, "done");
    assert_eq!(label("perf"), Tone::Wait, "its worker ended its turn");
    assert_eq!(label("sec"), Tone::Fail, "failed");
    assert_eq!(label("upg"), Tone::Plain, "at work");
    assert_eq!(
        label("synth"),
        Tone::Near,
        "a neighbour of the selected job"
    );
    assert_eq!(label("zip"), Tone::Dim, "queued");
    let (r, c) = glyph_at(&g, num["perf"]);
    assert_eq!(tone_at(&g, r, c), Tone::Wait, "the idle holder's glyph");
}

#[test]
fn a_waiting_worker_gets_a_steady_badge() {
    let mut snap = waits();
    snap.jobs.push(job("notes", "queued", &["verify"], 103));
    let (g, num) = flow(&snap, "notes", 46, false);
    let text = g.text();
    assert!(!text.contains("WAIT!"), "{text}");
    let (r, c) = find(&g, " WAIT ");
    assert!(
        (c..c + 6).all(|x| tone_at(&g, r, x) == Tone::Badge),
        "the badge is one solid block\n{text}"
    );
    let (lr, lc) = find(&g, &format!("{} deploy", num["deploy"]));
    assert_eq!(lr, r, "on deploy's row");
    assert_eq!(
        tone_at(&g, lr, lc),
        Tone::Wait,
        "and its label, away from the selection, in the wait colour"
    );
}

#[test]
fn the_worker_name_reads_in_the_worker_role() {
    let snap = fanned();
    let (g, num) = flow(&snap, "zip", 46, false);
    let (r, c) = find(&g, &format!("{} scope cc", num["scope"]));
    let at = c + format!("{} scope ", num["scope"]).len();
    assert_eq!(tone_at(&g, r, at), Tone::Soft, "{}", g.text());
}

#[test]
fn horizontal_branches_follow_their_jobs() {
    let snap = fanned();
    let (g, num) = flow(&snap, "zip", 200, true);
    let branch = |id: &str| {
        let (r, c) = glyph_at(&g, num[id]);
        tone_at(&g, r, c - 2)
    };
    assert_eq!(branch("func"), Tone::Dim, "{}", g.text());
    assert_eq!(branch("deliv"), Tone::Dim, "a lower branch to a done job");
    assert_eq!(branch("perf"), Tone::Line);
    assert_eq!(branch("sec"), Tone::Line);
    assert_eq!(branch("upg"), Tone::Line);
}

#[test]
fn a_failed_neighbour_of_the_selection_reads_near() {
    let (g, num) = flow(&fanned(), "synth", 46, false);
    let (r, c) = find(&g, &format!("{} sec", num["sec"]));
    assert_eq!(
        tone_at(&g, r, c),
        Tone::Near,
        "selected synth's failed direct dependency loses Near:\n{}",
        g.text()
    );
}

#[test]
fn a_waiting_neighbour_of_the_selection_reads_near() {
    let (g, num) = flow(&waits(), "verify", 46, false);
    let (r, c) = find(&g, &format!("{} deploy", num["deploy"]));
    assert_eq!(
        tone_at(&g, r, c),
        Tone::Near,
        "selected verify's waiting direct dependency loses Near:\n{}",
        g.text()
    );
}

#[test]
fn without_colour_the_badge_reverses_and_pills_are_bold() {
    let a = app_with(waits());
    assert!(a
        .style(Tone::Badge)
        .add_modifier
        .contains(Modifier::REVERSED | Modifier::BOLD));
    for tone in [
        Tone::PillWait,
        Tone::PillFail,
        Tone::PillRun,
        Tone::PillText,
    ] {
        assert!(a.style(tone).add_modifier.contains(Modifier::BOLD));
    }
}

#[test]
fn every_strip_row_keeps_its_count_beside_a_long_chain() {
    let mut jobs = vec![job("one", "queued", &[], 1)];
    let mut prev = String::new();
    for i in 0..25 {
        let id = format!("c{i}");
        if i == 0 {
            jobs.push(job(&id, "queued", &[], 10));
        } else {
            jobs.push(job(&id, "queued", &[&prev], 10 + i as u64));
        }
        prev = id;
    }
    let snap = Snapshot {
        jobs,
        captured_at: 1000,
        ..Default::default()
    };
    let mut app = app_with(snap);
    let blocks = app.blocks();
    assert_eq!(blocks.len(), 2, "{}", blocks.len());
    let frame = app.snap.clone().unwrap();
    let rows = super::strip(&blocks, Some(0), &frame, 43, 4, false);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let texts: Vec<String> = rows
        .iter()
        .map(|row| row.iter().map(|(text, _)| text.as_str()).collect())
        .collect();
    for (row, text) in rows.iter().zip(&texts) {
        assert!(super::row_width(row) <= 43, "{text}");
        assert!(text.contains("0/1") || text.contains("0/25"), "{text}");
    }
    assert!(texts.iter().any(|text| text.contains("0/1")), "{texts:?}");
    assert!(texts.iter().any(|text| text.contains("0/25")), "{texts:?}");
    let wide = super::strip(&blocks, Some(0), &frame, 80, 4, false);
    let count_col = |row: &[(String, Tone)]| {
        let mut x = 0;
        for (text, _) in row {
            if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
                return x;
            }
            x += width(text);
        }
        panic!("no count in {row:?}");
    };
    assert_eq!(count_col(&wide[0]), count_col(&wide[1]), "{wide:?}");
}

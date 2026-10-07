use super::input::{Act, Input, Mode};
use super::layout::{self, width, Fit, Grid, Tone, View};
use super::model::{self, Activity, Ask, Job, Numbers, Peer, Snapshot};
use super::{App, Opts, PEER_HOLD};
use crate::wire::{
    BlockReason, DispatchState, IdleWhy, JobAction, JobProgress, JobRelation, Liveness, Progress,
};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Modifier;
use ratatui::Terminal;
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

fn job(id: &str, state: &str, deps: &[&str], created: u64) -> Job {
    Job {
        job_id: id.into(),
        title: id.into(),
        state: state.into(),
        assigned_peer: Some("cc".into()),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        dispatch: true,
        created_at: Some(created),
        prompt: format!("prompt of {id}"),
        ..Default::default()
    }
}

fn s1() -> Snapshot {
    let mut jobs = vec![job("scope", "done", &[], 100)];
    for (i, (id, state)) in [
        ("func", "done"),
        ("perf", "running"),
        ("deliv", "done"),
        ("sec", "failed"),
        ("upg", "running"),
    ]
    .iter()
    .enumerate()
    {
        jobs.push(job(id, state, &["scope"], 101 + i as u64));
    }
    jobs.push(job(
        "synth",
        "queued",
        &["func", "perf", "deliv", "sec", "upg"],
        110,
    ));
    jobs.push(job("fix", "queued", &["synth"], 111));
    jobs.push(job("deploy", "queued", &["fix"], 112));
    Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }
}

fn s4() -> Snapshot {
    let jobs = vec![
        job("audit", "done", &[], 100),
        job("fix-a", "done", &["audit"], 101),
        job("fix-b", "done", &["audit"], 102),
        job("review", "done", &["fix-a", "fix-b"], 103),
        job("deploy", "running", &["review"], 104),
        job("verify", "queued", &["deploy", "audit"], 105),
    ];
    Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }
}

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

/* s1 plus two jobs without dependencies, older than the chain */
fn mixed() -> Snapshot {
    let mut snap = s1();
    snap.jobs.push(job("lint", "running", &[], 50));
    snap.jobs.push(job("docs", "done", &[], 60));
    snap
}

/* what a card or screen says once borders and line breaks are gone */
fn flat(text: &str) -> String {
    text.replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn numbered(snap: &Snapshot) -> (model::Chain, HashMap<String, usize>) {
    let chain = model::chains(snap).remove(0);
    let num = Numbers::default()
        .assign(&chain)
        .into_iter()
        .map(|(n, id)| (id, n))
        .collect();
    (chain, num)
}

fn card(snap: &Snapshot, id: &str) -> String {
    let (chain, num) = numbered(snap);
    flat(
        &layout::card(
            &View {
                snap: as_frame(snap),
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

fn as_frame(view: &Snapshot) -> model::Frame {
    model::Frame {
        view: view.clone(),
        fresh: false,
    }
}

fn app_with(snap: Snapshot) -> App {
    let mut app = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    app.home = false;
    app.apply(Ok(snap));
    app
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

fn screen_text(app: &mut App, cols: usize, rows: usize) -> Vec<String> {
    app.screen(cols, rows, 0)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
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

/* s1 with its peers reporting: perf's worker w1 works on perf alone, upg's worker w2 has
two running jobs, lint's worker waits for a permission, and fix waits on an idle worker */
fn busy() -> Snapshot {
    let mut snap = s1();
    snap.capabilities.peer_activity = true;
    let peer = |id: &str, state: &str, reason: Option<&str>| Peer {
        peer_id: id.into(),
        name: id.into(),
        status: "online".into(),
        activity: Some(Activity {
            state: state.into(),
            since: 880,
            reason: reason.map(Into::into),
            ..Default::default()
        }),
        liveness: Some(match state {
            "work" => crate::wire::Liveness::Work { since: 880 },
            "wait" => crate::wire::Liveness::Wait {
                since: 880,
                reason: reason.map(Into::into),
            },
            "idle" => crate::wire::Liveness::Idle { since: 880 },
            _ => crate::wire::Liveness::Online,
        }),
        ..Default::default()
    };
    snap.peers = vec![
        peer("w1", "work", None),
        peer("w2", "work", None),
        peer(
            "w3",
            "wait",
            Some("Claude needs your permission to use Bash"),
        ),
        peer("w4", "idle", None),
    ];
    for (id, worker) in [
        ("perf", "w1"),
        ("upg", "w2"),
        ("fix", "w3"),
        ("deploy", "w2"),
        ("synth", "w4"),
    ] {
        let job = snap.jobs.iter_mut().find(|j| j.job_id == id).unwrap();
        job.state = "running".into();
        job.assigned_peer = Some(worker.into());
        job.worker = Some(worker.into());
        job.assignee_id = Some(worker.into());
        job.relation = Some(crate::wire::JobRelation::Open);
        job.progress = Some(crate::wire::JobProgress {
            busy: id == "perf",
            state: Some(match id {
                "fix" => crate::wire::Progress::Wait {
                    since: 880,
                    reason: Some("Claude needs your permission to use Bash".into()),
                },
                "synth" => crate::wire::Progress::Idle {
                    since: 900,
                    why: crate::wire::IdleWhy::NotPickedUp,
                },
                _ => crate::wire::Progress::Work { since: 880 },
            }),
        });
        job.ask_id = Some(format!("ask-{id}"));
        snap.asks.push(Ask {
            correlation_id: format!("ask-{id}"),
            to_peer_id: worker.into(),
            open: true,
            opened_at: Some(900),
            ..Default::default()
        });
    }
    if let Some(job) = snap.jobs.iter_mut().find(|j| j.job_id == "synth") {
        job.actions = Some(vec![crate::wire::JobAction::Nudge { to: "w4".into() }]);
    }
    snap
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

/* the flow row that holds `label` in a 46-column pane */
fn row_of(app: &mut App, label: &str, tick: usize) -> String {
    app.screen(46, 40, tick)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .find(|l| l.contains(label))
        .unwrap_or_else(|| panic!("no row with {label}"))
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

/* the design's sample data, as the approved mock-up drew it; clocks read UTC in tests */
fn at(h: u64, m: u64) -> u64 {
    h * 3600 + m * 60
}

#[allow(clippy::too_many_arguments)]
fn add(
    snap: &mut Snapshot,
    id: &str,
    state: &str,
    deps: &[&str],
    peer: &str,
    ask: Option<(&str, &str, u64)>,
    ended: Option<u64>,
    text: &str,
    prompt: &str,
) {
    let mut job = job(id, state, deps, snap.jobs.len() as u64);
    job.assigned_peer = Some(peer.into());
    job.finished_at = ended;
    job.prompt = prompt.into();
    job.result = (!text.is_empty()).then(|| text.into());
    let has_ask = ask.is_some();
    if let Some((short, from, sent)) = ask {
        job.ask_id = Some(format!("ask-{short}"));
        let open = state == "running";
        let failed = state == "failed";
        snap.asks.push(Ask {
            correlation_id: format!("ask-{short}"),
            from_peer: from.into(),
            to_peer: peer.into(),
            to_peer_id: peer.into(),
            open,
            failed,
            opened_at: Some(sent),
            closed_by: (!open).then(|| "recipient".into()),
            state: Some(if open {
                crate::wire::AskState::Open {
                    progress: crate::wire::Progress::Pending { since: Some(sent) },
                }
            } else {
                crate::wire::AskState::Closed {
                    outcome: if failed {
                        crate::wire::AskOutcome::AckedFailed
                    } else {
                        crate::wire::AskOutcome::AckedOk
                    },
                    failed_effective: failed,
                }
            }),
            ..Default::default()
        });
    }
    job.worker = Some(peer.into());
    job.assignee_id = Some(peer.into());
    job.relation = Some(if state == "running" {
        crate::wire::JobRelation::Open
    } else if has_ask {
        crate::wire::JobRelation::Closed
    } else {
        crate::wire::JobRelation::NoAsk
    });
    snap.jobs.push(job);
}

fn doing(id: &str, state: &str, since: u64, reason: Option<&str>) -> Peer {
    Peer {
        peer_id: id.into(),
        name: id.into(),
        status: "online".into(),
        activity: Some(Activity {
            state: state.into(),
            since,
            reason: reason.map(Into::into),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn set_live(snap: &mut Snapshot, rows: &[(&str, crate::wire::Liveness)]) {
    for peer in snap.peers.iter_mut().chain(snap.roster.iter_mut()) {
        if let Some((_, kind)) = rows.iter().find(|(id, _)| *id == peer.peer_id) {
            peer.liveness = Some(kind.clone());
        }
    }
}

fn set_progress(snap: &mut Snapshot, id: &str, state: crate::wire::Progress, busy: bool) {
    if let Some(job) = snap.jobs.iter_mut().find(|job| job.job_id == id) {
        job.progress = Some(crate::wire::JobProgress {
            state: Some(state),
            busy,
        });
    }
}

fn set_dispatch(snap: &mut Snapshot, id: &str, state: crate::wire::DispatchState) {
    if let Some(job) = snap.jobs.iter_mut().find(|job| job.job_id == id) {
        job.dispatch_state = Some(state);
    }
}

fn proto(now: u64) -> Snapshot {
    let mut snap = Snapshot {
        captured_at: now,
        ..Default::default()
    };
    snap.capabilities.peer_activity = true;
    snap
}

fn proto_s1() -> Snapshot {
    let mut s = proto(at(13, 42));
    let sent = |short, t| Some((short, "cc", t));
    add(
        &mut s,
        "scope",
        "done",
        &[],
        "cc",
        sent("1f0", at(13, 19)),
        Some(at(13, 20)),
        "5 review dimensions and a shared brief",
        "split the cleanup review into dimensions",
    );
    add(
        &mut s,
        "func",
        "done",
        &["scope"],
        "codex",
        sent("5b1", at(13, 21)),
        Some(at(13, 28)),
        "3 findings: 2 fixed (P2), 1 wontfix",
        "review correctness of the sweep and the delivery filter",
    );
    add(
        &mut s,
        "perf",
        "running",
        &["scope"],
        "codex",
        sent("9a1", at(13, 30)),
        None,
        "",
        "review perf: sweep lock time and inbox filter cost at 100k rows",
    );
    add(
        &mut s,
        "deliv",
        "done",
        &["scope"],
        "pi",
        sent("4c7", at(13, 21)),
        Some(at(13, 25)),
        "filter covers all 3 delivery paths; 1 P3",
        "review delivery and concurrency",
    );
    add(
        &mut s,
        "sec",
        "failed",
        &["scope"],
        "pi",
        sent("2ee1", at(13, 21)),
        Some(at(13, 39)),
        "no fixture for the ws recv path",
        "review security and data retention",
    );
    add(
        &mut s,
        "upg",
        "running",
        &["scope"],
        "pi-3",
        sent("7c2", at(13, 36)),
        None,
        "",
        "review the upgrade path for old state files and mixed versions",
    );
    add(
        &mut s,
        "synth",
        "queued",
        &["func", "perf", "deliv", "sec", "upg"],
        "cc",
        None,
        None,
        "",
        "merge the five reviews into one decision list",
    );
    add(
        &mut s,
        "fix",
        "queued",
        &["synth"],
        "cc",
        None,
        None,
        "",
        "apply the accepted fixes",
    );
    add(
        &mut s,
        "deploy",
        "queued",
        &["fix"],
        "cc",
        None,
        None,
        "",
        "install the build and restart the hub",
    );
    s.peers = vec![
        doing("cc", "work", at(13, 40), None),
        doing("codex", "idle", at(13, 37), None),
        doing("pi", "work", at(13, 40), None),
        doing("pi-3", "work", at(13, 36), None),
    ];
    set_live(
        &mut s,
        &[
            ("cc", crate::wire::Liveness::Work { since: at(13, 40) }),
            ("codex", crate::wire::Liveness::Idle { since: at(13, 37) }),
            ("pi", crate::wire::Liveness::Work { since: at(13, 40) }),
            ("pi-3", crate::wire::Liveness::Work { since: at(13, 36) }),
        ],
    );
    set_progress(
        &mut s,
        "perf",
        crate::wire::Progress::Idle {
            since: at(13, 37),
            why: crate::wire::IdleWhy::TurnEnded,
        },
        false,
    );
    s.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .actions = Some(vec![crate::wire::JobAction::Nudge { to: "codex".into() }]);
    set_progress(
        &mut s,
        "upg",
        crate::wire::Progress::Work { since: at(13, 36) },
        true,
    );
    set_dispatch(
        &mut s,
        "synth",
        crate::wire::DispatchState::Blocked {
            dependencies: vec!["perf".into(), "sec".into(), "upg".into()],
            dependency: "sec".into(),
            reason: crate::wire::BlockReason::Failed,
        },
    );
    set_dispatch(
        &mut s,
        "fix",
        crate::wire::DispatchState::Waiting {
            dependencies: vec!["synth".into()],
        },
    );
    set_dispatch(
        &mut s,
        "deploy",
        crate::wire::DispatchState::Waiting {
            dependencies: vec!["fix".into()],
        },
    );
    s
}

fn proto_s4() -> Snapshot {
    let mut s = proto(at(13, 42));
    let sent = |short, t| Some((short, "cc", t));
    add(
        &mut s,
        "audit",
        "done",
        &[],
        "codex",
        sent("a11", at(13, 30)),
        Some(at(13, 34)),
        "12 stale docs found, split in two halves",
        "list stale docs in the repo",
    );
    add(
        &mut s,
        "fix-a",
        "done",
        &["audit"],
        "pi",
        sent("b22", at(13, 34)),
        Some(at(13, 36)),
        "7 docs fixed",
        "fix the first half of the stale docs",
    );
    add(
        &mut s,
        "fix-b",
        "done",
        &["audit"],
        "pi-3",
        sent("c33", at(13, 34)),
        Some(at(13, 36)),
        "5 docs fixed",
        "fix the second half of the stale docs",
    );
    add(
        &mut s,
        "review",
        "done",
        &["fix-a", "fix-b"],
        "codex",
        sent("d44", at(13, 36)),
        Some(at(13, 39)),
        "approved with 1 nit",
        "review both halves together",
    );
    add(
        &mut s,
        "deploy",
        "running",
        &["review"],
        "cc",
        Some(("d3a9", "codex", at(13, 41))),
        None,
        "",
        "install the build and restart the hub",
    );
    add(
        &mut s,
        "verify",
        "queued",
        &["deploy", "audit"],
        "pi-3",
        None,
        None,
        "",
        "check the live hub against the audit list",
    );
    s.peers = vec![
        doing("codex", "idle", at(13, 39), None),
        doing(
            "cc",
            "wait",
            at(13, 41) + 20,
            Some("needs your Bash permission"),
        ),
        doing("pi", "work", at(13, 40), None),
        doing("pi-3", "idle", at(13, 36), None),
    ];
    set_live(
        &mut s,
        &[
            ("codex", crate::wire::Liveness::Idle { since: at(13, 39) }),
            (
                "cc",
                crate::wire::Liveness::Wait {
                    since: at(13, 41) + 20,
                    reason: Some("needs your Bash permission".into()),
                },
            ),
            ("pi", crate::wire::Liveness::Work { since: at(13, 40) }),
            ("pi-3", crate::wire::Liveness::Idle { since: at(13, 36) }),
        ],
    );
    set_progress(
        &mut s,
        "deploy",
        crate::wire::Progress::Wait {
            since: at(13, 41) + 20,
            reason: Some("needs your Bash permission".into()),
        },
        false,
    );
    set_dispatch(
        &mut s,
        "verify",
        crate::wire::DispatchState::Waiting {
            dependencies: vec!["deploy".into()],
        },
    );
    s
}

fn proto_s5() -> Snapshot {
    let mut s = proto(at(13, 26));
    add(
        &mut s,
        "scope",
        "done",
        &[],
        "cc",
        Some(("1f0", "cc", at(13, 10))),
        Some(at(13, 11)),
        "10 review dimensions",
        "split the release review",
    );
    /* the mock-up's workers, ask ids and results; upg and tests get workers of their
    own, since the mock-up spins both under one codex and a worker with two running jobs
    shows no spinner, and times are real timestamps where the mock-up's ages disagree */
    let reviews = [
        ("func", "done", "codex"),
        ("perf", "running", "pi"),
        ("deliv", "done", "pi-3"),
        ("sec", "failed", "pi-2"),
        ("upg", "running", "u1"),
        ("docs", "done", "pi"),
        ("api", "done", "pi-3"),
        ("cli", "queued", "pi-2"),
        ("tests", "running", "u2"),
        ("ux", "done", "pi"),
    ];
    for (i, (id, state, peer)) in reviews.iter().enumerate() {
        let short = format!("{i}a{i}");
        let ask = (*state != "queued").then_some((short.as_str(), "cc", at(13, 12)));
        let ended = matches!(*state, "done" | "failed").then_some(at(13, 20));
        let result = if *state == "failed" {
            format!("{id} review could not run its fixture")
        } else {
            format!("{id} review: no blocking findings")
        };
        add(
            &mut s,
            id,
            state,
            &["scope"],
            peer,
            ask,
            ended,
            &result,
            &format!("review the release for {id}"),
        );
    }
    add(
        &mut s,
        "synth",
        "queued",
        &reviews.map(|(id, _, _)| id),
        "cc",
        None,
        None,
        "",
        "merge the ten reviews",
    );
    add(
        &mut s,
        "fix",
        "queued",
        &["synth"],
        "cc",
        None,
        None,
        "",
        "apply the accepted fixes",
    );
    add(
        &mut s,
        "deploy",
        "queued",
        &["fix"],
        "cc",
        None,
        None,
        "",
        "ship the release",
    );
    let sec = s.jobs.iter_mut().find(|j| j.job_id == "sec").unwrap();
    sec.job_id = "job-58851116".into();
    for job in &mut s.jobs {
        for dep in &mut job.depends_on {
            if dep == "sec" {
                *dep = "job-58851116".into();
            }
        }
    }
    s.peers = vec![
        doing("cc", "work", at(13, 25), None),
        doing("codex", "idle", at(13, 20), None),
        doing("pi", "idle", at(13, 22), None),
        doing("pi-3", "idle", at(13, 20), None),
        doing("pi-2", "idle", at(13, 20), None),
        doing("u1", "work", at(13, 12), None),
        doing("u2", "work", at(13, 12), None),
    ];
    set_live(
        &mut s,
        &[
            ("cc", crate::wire::Liveness::Work { since: at(13, 25) }),
            ("codex", crate::wire::Liveness::Idle { since: at(13, 20) }),
            ("pi", crate::wire::Liveness::Idle { since: at(13, 22) }),
            ("pi-3", crate::wire::Liveness::Idle { since: at(13, 20) }),
            ("pi-2", crate::wire::Liveness::Idle { since: at(13, 20) }),
            ("u1", crate::wire::Liveness::Work { since: at(13, 12) }),
            ("u2", crate::wire::Liveness::Work { since: at(13, 12) }),
        ],
    );
    set_progress(
        &mut s,
        "perf",
        crate::wire::Progress::Idle {
            since: at(13, 22),
            why: crate::wire::IdleWhy::TurnEnded,
        },
        false,
    );
    set_progress(
        &mut s,
        "upg",
        crate::wire::Progress::Work { since: at(13, 12) },
        true,
    );
    set_progress(
        &mut s,
        "tests",
        crate::wire::Progress::Work { since: at(13, 12) },
        true,
    );
    if let Some(job) = s.jobs.iter_mut().find(|j| j.job_id == "job-58851116") {
        job.actions = Some(vec![crate::wire::JobAction::Retry {
            needs_assignee: false,
        }]);
    }
    s
}

/* frames of the jobs screen, rendered at the approved design's sizes: every card state at both
widths, S4's skip-level note and S5's box, under the chain strip, the attention line and the
footer the screen draws today */
const FRAMES: &str = r#"
===== S1 narrow scope
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
▸ deploy  ●─×─○─○─○  3/9  × sec   IDLE perf
────────────────────────────────────────── ● ─
attention  × sec
                    ● 1 scope cc 1m
    ┌───────┬───────┼───────┬───────┐
    ●       ◆       ●       ×       ⠋
  2 func  3 perf 4 deliv  5 sec   6 upg
    └───────┴───────┼───────┴───────┘
                    ○ 7 synth cc
                    │
                    ○ 8 fix cc
                    │
                    ○ 9 deploy cc

┌● 1 scope · done · took 1m · stage 1 of 5 ──┐
│ result  5 review dimensions and a shared   │
│         brief                              │
│ worker  cc · ask 1f0 cc▸cc · 13:19         │
│ prompt  split the cleanup review into      │
│         dimensions                         │
│                                            │
│                                            │
│ ⏎ full card                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S1 narrow perf
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
▸ deploy  ●─×─○─○─○  3/9  × sec   IDLE perf
────────────────────────────────────────── ● ─
attention  × sec
                    ● 1 scope cc 1m
    ┌───────┬───────┼───────┬───────┐
    ●       ◆       ●       ×       ⠋
  2 func  3 perf 4 deliv  5 sec   6 upg
    └───────┴───────┼───────┴───────┘
                    ○ 7 synth cc
                    │
                    ○ 8 fix cc
                    │
                    ○ 9 deploy cc

┌◆ 3 perf · running 12m ─────────────── IDLE ┐
│ why     codex ended its turn 5m ago        │
│ worker  codex · ask 9a1 cc▸codex · 13:30   │
│         unacked 12m · chain median 4m      │
│ prompt  review perf: sweep lock time and   │
│         inbox filter cost at 100k rows     │
│                                            │
│                                            │
│                                            │
│ y copy nudge · ⏎ full card                 │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S4 narrow audit
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
▸ verify  ●─●─●─◆─○  4/6  WAIT deploy
────────────────────────────────────────── ● ─
attention  WAIT cc 40s
        ● 1 audit codex 4m ─────┐
    ┌───┴───┐                   │
    ●       ●                   │
 2 fix-a 3 fix-b                │
    └───┬───┘                   │
        ● 4 review codex 3m     │
        │                       │
        ◆ 5 deploy cc 1m WAIT   │
        │                       │
        ○ 6 verify pi-3 ◀───────┘

┌● 1 audit · done · took 4m · stage 1 of 5 ──┐
│ result  12 stale docs found, split in two  │
│         halves                             │
│ worker  codex · ask a11 cc▸codex · 13:30   │
│ prompt  list stale docs in the repo        │
│                                            │
│                                            │
│ ⏎ full card                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S4 narrow deploy
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
▸ verify  ●─●─●─◆─○  4/6  WAIT deploy
────────────────────────────────────────── ● ─
attention  WAIT cc 40s
        ● 1 audit codex 4m ─────┐
    ┌───┴───┐                   │
    ●       ●                   │
 2 fix-a 3 fix-b                │
    └───┬───┘                   │
        ● 4 review codex 3m     │
        │                       │
        ◆ 5 deploy cc 1m WAIT   │
        │                       │
        ○ 6 verify pi-3 ◀───────┘

┌◆ 5 deploy · running 1m ────────────── WAIT ┐
│ why     needs your Bash permission         │
│ worker  cc · ask d3a9 codex▸cc · 13:41     │
│         unacked 1m · chain median 2m       │
│ prompt  install the build and restart the  │
│         hub                                │
│                                            │
│                                            │
│ ⏎ full card                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S5 narrow scope
7 online · ⠋ cc     ○ codex  ○ pi     ○ pi-2
           ○ pi-3   ⠋ u1     ⠋ u2
▸ deploy  ●─×─○─○─○  6/14  × sec   IDLE perf
────────────────────────────────────────── ● ─
attention  × sec
                    ● 1 scope cc 1m
┌───────────────────┴────────────────────────┐
│ 2-11 · 10 jobs · 5● 3◆ 1× 1○               │
│ ● 2 func              ◆ 3 perf             │
│ ● 4 deliv             × 5 sec              │
│ ⠋ 6 upg               ● 7 docs             │
│ ● 8 api               ○ 9 cli              │
│ ⠋ 10 tests            ● 11 ux              │
└───────────────────┬────────────────────────┘
                    ○ 12 synth cc
                    │
                    ○ 13 fix cc
                    │
                    ○ 14 deploy cc

┌● 1 scope · done · took 1m · stage 1 of 5 ──┐
│ result  10 review dimensions               │
│ worker  cc · ask 1f0 cc▸cc · 13:10         │
│ prompt  split the release review           │
│                                            │
│                                            │
│ ⏎ full card                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S5 narrow sec
7 online · ⠋ cc     ○ codex  ○ pi     ○ pi-2
           ○ pi-3   ⠋ u1     ⠋ u2
▸ deploy  ●─×─○─○─○  6/14  × sec   IDLE perf
────────────────────────────────────────── ● ─
attention  × sec
                    ● 1 scope cc 1m
┌───────────────────┴────────────────────────┐
│ 2-11 · 10 jobs · 5● 3◆ 1× 1○               │
│ ● 2 func              ◆ 3 perf             │
│ ● 4 deliv             × 5 sec              │
│ ⠋ 6 upg               ● 7 docs             │
│ ● 8 api               ○ 9 cli              │
│ ⠋ 10 tests            ● 11 ux              │
└───────────────────┬────────────────────────┘
                    ○ 12 synth cc
                    │
                    ○ 13 fix cc
                    │
                    ○ 14 deploy cc

┌× 5 sec · failed 6m ago ──────────── FAILED ┐
│ why     sec review could not run its       │
│         fixture                            │
│ worker  pi-2 · ask 3a3 cc▸pi-2 · 13:12     │
│ prompt  review the release for sec         │
│                                            │
│                                            │
│                                            │
│                                            │
│                                            │
│ y copy retry · ⏎ full card                 │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S1 wide upg
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
▸ deploy  ●─×─○─○─○  3/9  × sec   IDLE perf
──────────────────────────────────────────────────────────────────────────────────────────────── ● ─
attention  × sec
● 1 scope ─┬─ ● 2 func ──┬── ○ 7 synth ── ○ 8 fix ── ○ 9 deploy
           ├─ ◆ 3 perf ──┤
           ├─ ● 4 deliv ─┤
           ├─ × 5 sec ───┤
           └─ ⠋ 6 upg ───┘

┌◆ 6 upg · running 6m · stage 2 of 5 ──────────────────────────────────────────────────────────────┐
│ worker  pi-3 · ask 7c2 cc▸pi-3 · 13:36          │ prompt  review the upgrade path for old state  │
│         unacked 6m · chain median 4m            │         files and mixed versions               │
│                                                 │                                                │
│                                                 │                                                │
│ ⏎ full card                                     │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  tab chain  t timeline  enter card  / find  f hints  digits jump  esc back
===== S5 wide scope
7 online · ⠋ cc  ○ codex  ○ pi  ○ pi-2  ○ pi-3  ⠋ u1  ⠋ u2
▸ deploy  ●─×─○─○─○  6/14  × sec   IDLE perf
──────────────────────────────────────────────────────────────────────────────────────────────── ● ─
attention  × sec
● 1 scope ─┬─ ● 2 func ───┬── ○ 12 synth ── ○ 13 fix ── ○ 14 deploy
           ├─ ◆ 3 perf ───┤
           ├─ ● 4 deliv ──┤
           ├─ × 5 sec ────┤
           ├─ ⠋ 6 upg ────┤
           ├─ ● 7 docs ───┤
           ├─ ● 8 api ────┤
           ├─ ○ 9 cli ────┤
           ├─ ⠋ 10 tests ─┤
           └─ ● 11 ux ────┘

┌● 1 scope · done · took 1m · stage 1 of 5 ────────────────────────────────────────────────────────┐
│ result  10 review dimensions                    │ prompt  split the release review               │
│ worker  cc · ask 1f0 cc▸cc · 13:10              │                                                │
│                                                 │                                                │
│                                                 │                                                │
│ ⏎ full card                                     │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  tab chain  t timeline  enter card  / find  f hints  digits jump  esc back
===== S1 narrow synth
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
▸ deploy  ●─×─○─○─○  3/9  × sec   IDLE perf
────────────────────────────────────────── ● ─
attention  × sec
                    ● 1 scope cc 1m
    ┌───────┬───────┼───────┬───────┐
    ●       ◆       ●       ×       ⠋
  2 func  3 perf 4 deliv  5 sec   6 upg
    └───────┴───────┼───────┴───────┘
                    ○ 7 synth cc
                    │
                    ○ 8 fix cc
                    │
                    ○ 9 deploy cc

┌○ 7 synth · queued · stage 3 of 5 ─ BLOCKED ┐
│ why     5 sec failed: retry it first       │
│ worker  cc                                 │
│ prompt  merge the five reviews into one    │
│         decision list                      │
│                                            │
│                                            │
│ ⏎ full card                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S1 wide synth
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
▸ deploy  ●─×─○─○─○  3/9  × sec   IDLE perf
──────────────────────────────────────────────────────────────────────────────────────────────── ● ─
attention  × sec
● 1 scope ─┬─ ● 2 func ──┬── ○ 7 synth ── ○ 8 fix ── ○ 9 deploy
           ├─ ◆ 3 perf ──┤
           ├─ ● 4 deliv ─┤
           ├─ × 5 sec ───┤
           └─ ⠋ 6 upg ───┘

┌○ 7 synth · queued · stage 3 of 5 ─────────────────────────────────────────────────────── BLOCKED ┐
│ why     5 sec failed: retry it first            │ prompt  merge the five reviews into one        │
│ worker  cc                                      │         decision list                          │
│                                                 │                                                │
│                                                 │                                                │
│ ⏎ full card                                     │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  tab chain  t timeline  enter card  / find  f hints  digits jump  esc back
===== S4 narrow review
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
▸ verify  ●─●─●─◆─○  4/6  WAIT deploy
────────────────────────────────────────── ● ─
attention  WAIT cc 40s
        ● 1 audit codex 4m ─────┐
    ┌───┴───┐                   │
    ●       ●                   │
 2 fix-a 3 fix-b                │
    └───┬───┘                   │
        ● 4 review codex 3m     │
        │                       │
        ◆ 5 deploy cc 1m WAIT   │
        │                       │
        ○ 6 verify pi-3 ◀───────┘

┌● 4 review · done · took 3m · stage 3 of 5 ─┐
│ result  approved with 1 nit                │
│ worker  codex · ask d44 cc▸codex · 13:36   │
│ prompt  review both halves together        │
│                                            │
│                                            │
│                                            │
│ ⏎ full card                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  tab chain  t timeline
===== S4 wide deploy
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
▸ verify  ●─●─●─◆─○  4/6  WAIT deploy
──────────────────────────────────────────────────────────────────────────────────────────────── ● ─
attention  WAIT cc 40s  needs your Bash permission
● 1 audit ─┬─ ● 2 fix-a ─┬── ● 4 review ── ◆ 5 deploy ── ○ 6 verify
           └─ ● 3 fix-b ─┘                               ↑ also needs 1 audit

┌◆ 5 deploy · running 1m · stage 4 of 5 ───────────────────────────────────────────────────── WAIT ┐
│ why     needs your Bash permission              │ prompt  install the build and restart the hub  │
│ worker  cc · ask d3a9 codex▸cc · 13:41          │                                                │
│         unacked 1m · chain median 2m            │                                                │
│                                                 │                                                │
│                                                 │                                                │
│ ⏎ full card                                     │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  tab chain  t timeline  enter card  / find  f hints  digits jump  esc back
===== S5 wide sec
7 online · ⠋ cc  ○ codex  ○ pi  ○ pi-2  ○ pi-3  ⠋ u1  ⠋ u2
▸ deploy  ●─×─○─○─○  6/14  × sec   IDLE perf
──────────────────────────────────────────────────────────────────────────────────────────────── ● ─
attention  × sec
● 1 scope ─┬─ ● 2 func ───┬── ○ 12 synth ── ○ 13 fix ── ○ 14 deploy
           ├─ ◆ 3 perf ───┤
           ├─ ● 4 deliv ──┤
           ├─ × 5 sec ────┤
           ├─ ⠋ 6 upg ────┤
           ├─ ● 7 docs ───┤
           ├─ ● 8 api ────┤
           ├─ ○ 9 cli ────┤
           ├─ ⠋ 10 tests ─┤
           └─ ● 11 ux ────┘

┌× 5 sec · failed 6m ago · stage 2 of 5 ─────────────────────────────────────────────────── FAILED ┐
│ why     sec review could not run its fixture    │ prompt  review the release for sec             │
│ worker  pi-2 · ask 3a3 cc▸pi-2 · 13:12          │                                                │
│                                                 │                                                │
│                                                 │                                                │
│                                                 │                                                │
│ y copy retry · ⏎ full card                      │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  tab chain  t timeline  enter card  y copy  / find  f hints  digits jump
"#;

#[test]
fn screens_match_the_approved_design() {
    let frames: Vec<(&str, Vec<&str>)> = FRAMES
        .split("===== ")
        .filter(|f| !f.trim().is_empty())
        .map(|f| {
            let mut lines = f.lines();
            let name = lines.next().unwrap();
            (name, lines.collect())
        })
        .collect();
    assert_eq!(frames.len(), 13);
    /* every frame is compared, so a failure names all the frames it touches */
    let mut wrong = Vec::new();
    for (name, want) in frames {
        let parts: Vec<&str> = name.split(' ').collect();
        let mut snap = match parts[0] {
            "S1" => proto_s1(),
            "S4" => proto_s4(),
            _ => proto_s5(),
        };
        let cols = if parts[1] == "narrow" { 46 } else { 100 };
        let sel = if parts[2] == "sec" && parts[0] == "S5" {
            "job-58851116"
        } else {
            parts[2]
        };
        snap.roster = snap.peers.clone();
        snap.capabilities.roster = true;
        let mut app = app_with(snap);
        app.sel = Some(sel.into());
        let got: Vec<String> = screen_text(&mut app, cols, want.len());
        if got != want {
            wrong.push(format!(
                "{name}\n--- got\n{}\n--- want\n{}",
                got.join("\n"),
                want.join("\n")
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} frames differ:\n{}",
        wrong.len(),
        wrong.join("\n\n")
    );
}

#[test]
fn the_theme_defaults_to_the_design_and_a_file_overrides_it() {
    use super::theme::Theme;
    use ratatui::style::Color;
    let dir = std::env::temp_dir().join(format!("amesh-theme-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let design = Theme::default();
    assert_eq!(
        (design.done, design.line, design.select_bg),
        (
            Color::Rgb(0x7e, 0xc1, 0x6e),
            Color::Rgb(0x4f, 0x9e, 0xa8),
            Color::Rgb(0x3e, 0x44, 0x51)
        )
    );
    let theme = Theme::load(&file(
        "ok.json",
        r##"{"done": "green", "line": "#102030", "dim": "244", "text": "reset"}"##,
    ))
    .unwrap();
    assert_eq!(
        (theme.done, theme.line, theme.dim, theme.text),
        (
            Color::Green,
            Color::Rgb(0x10, 0x20, 0x30),
            Color::Indexed(244),
            Color::Reset
        )
    );
    assert_eq!(
        theme.fail, design.fail,
        "a role the file leaves out keeps the default"
    );
    let unknown = Theme::load(&file("unknown.json", r#"{"glow": "red"}"#)).unwrap_err();
    assert!(
        unknown.contains("unknown colour role glow") && unknown.contains("select_bg"),
        "{unknown}"
    );
    let bad = Theme::load(&file("bad.json", r##"{"done": "#12"}"##)).unwrap_err();
    assert!(bad.contains("done: #12 is not a colour"), "{bad}");
    assert!(Theme::load(&dir.join("missing.json")).is_err());
    let cube = design.indexed();
    assert_eq!(
        cube.done,
        Color::Indexed(107),
        "#7ec16e is 135,175,95 in the cube"
    );
    assert_eq!(cube.select, Color::Indexed(231), "white stays white");
    assert_eq!(
        cube.select_bg,
        Color::Indexed(238),
        "#3e4451 is nearer the grey ramp's 68 than the cube's 95"
    );
    let plain = Theme::load(&file("plain.json", r#"{"done": 12}"#)).unwrap_err();
    assert!(plain.contains("done: 12 is not a colour string"), "{plain}");
    assert_eq!(
        theme.clone().indexed().done,
        Color::Green,
        "names are left alone"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_screen_draws_in_the_theme() {
    use ratatui::style::Color;
    let mut app = App::new(Opts {
        circle: None,
        ascii: false,
        color: true,
        anim: true,
        theme: Default::default(),
    });
    app.home = false;
    app.apply(Ok(proto_s1()));
    app.sel = Some("scope".into());
    let lines = app.screen(46, 30, 0);
    let span = |text: &str| {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|s| s.content.contains(text))
            .unwrap_or_else(|| panic!("no span with {text}"))
            .style
    };
    let theme = super::theme::Theme::default();
    assert_eq!(
        (span("1 scope").fg, span("1 scope").bg),
        (Some(theme.select), Some(theme.select_bg)),
        "the selected job, as in the design"
    );
    assert_eq!(span("2 func").fg, Some(theme.near), "its neighbours");
    assert_eq!(span("7 synth").fg, Some(theme.text));
    assert_eq!(span("──────").fg, Some(theme.line));
    assert_eq!(span("5 review dimensions").fg, Some(theme.done));
    assert_eq!(span("cc · ask").fg, Some(theme.worker));
    assert_eq!(span("j/k move").fg, Some(theme.dim));
    let mut plain = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    plain.home = false;
    plain.apply(Ok(proto_s1()));
    assert!(
        plain
            .screen(46, 30, 0)
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|s| s.style.fg.is_none() && s.style.bg != Some(Color::Reset)),
        "NO_COLOR draws no colour"
    );
}

#[test]
fn text_wraps_between_cjk_characters_and_keeps_every_one() {
    let text = "结论：三条 prompt 同时发，生成过程中 snapshot 保持 work，abcdefghijklmnopqrstuvwxyz0123456789 结束";
    for max in [7, 10, 16, 34] {
        let lines = layout::wrap(text, max);
        assert!(lines.iter().all(|l| width(l) <= max), "{max}: {lines:?}");
        assert!(
            lines.iter().all(|l| !l.contains('…')),
            "nothing is cut: {lines:?}"
        );
        let kept: String = lines.concat().split_whitespace().collect();
        let all: String = text.split_whitespace().collect();
        assert_eq!(kept, all, "{max}: every character is kept");
    }
    assert_eq!(
        layout::wrap("结论：三条 prompt 同时发", 10),
        ["结论：三条", "prompt 同", "时发"],
        "CJK fills the line and breaks between characters"
    );
}

#[test]
fn the_card_fills_the_rows_the_pane_leaves() {
    let mut snap = proto_s1();
    let long =
        "the review found nothing that blocks the release but several rough edges ".repeat(8);
    for job in &mut snap.jobs {
        if job.job_id == "scope" {
            job.result = Some(long.clone());
            job.prompt = long.clone();
        }
    }
    let (chain, num) = numbered(&snap);
    let view = View {
        snap: model::Frame {
            view: snap.clone(),
            ..Default::default()
        },
        chain: &chain,
        num: &num,
        base: 0,
        sel: "scope",
    };
    let rows = |text: &str, key: &str| {
        let lines: Vec<&str> = text.lines().collect();
        let at = lines.iter().position(|l| l.contains(key)).unwrap();
        lines[at..]
            .iter()
            .take_while(|l| {
                (l.contains(key) || l.starts_with("│         "))
                    && !l.trim_matches('│').trim().is_empty()
            })
            .count()
    };
    let whole = layout::card(&view, "scope", 46, None, Fit::Whole).text();
    let (result, prompt) = (rows(&whole, "result"), rows(&whole, "prompt"));
    assert!(
        result > 10 && prompt > 10 && !whole.contains('…'),
        "{whole}"
    );
    let least = layout::card(&view, "scope", 46, None, Fit::Rows(0)).text();
    assert_eq!(
        (rows(&least, "result"), rows(&least, "prompt")),
        (2, 1),
        "the smallest card keeps two result lines and one prompt line:\n{least}"
    );
    for h in [12, 20, 30] {
        let text = layout::card(&view, "scope", 46, None, Fit::Rows(h)).text();
        assert_eq!(text.lines().count(), h, "exactly the rows given:\n{text}");
        let (r, p) = (rows(&text, "result"), rows(&text, "prompt"));
        assert!(r > 2 && p >= 1, "spare rows show the explanation:\n{text}");
        assert_eq!(
            text.matches('…').count(),
            usize::from(r < result) + usize::from(p < prompt),
            "a field cut short says so:\n{text}"
        );
    }
    let roomy = layout::card(&view, "scope", 46, None, Fit::Rows(60)).text();
    assert_eq!(roomy.lines().count(), 60, "the frame stretches:\n{roomy}");
    assert!(
        !roomy.contains('…') && roomy.lines().nth(58).unwrap().contains("⏎ full card"),
        "the fields use the rows above the keys:\n{roomy}"
    );
    let wide = layout::card(&view, "scope", 100, None, Fit::Rows(12)).text();
    assert_eq!(wide.lines().count(), 12, "{wide}");
    let right = wide
        .lines()
        .filter(|l| l.split('│').nth(2).is_some_and(|r| !r.trim().is_empty()))
        .count();
    assert_eq!(right, 9, "the prompt fills the right column:\n{wide}");
}

#[test]
fn the_screen_fills_the_pane_and_follows_its_size() {
    let mut snap = proto_s1();
    for job in &mut snap.jobs {
        if job.job_id == "perf" {
            job.prompt =
                "review perf: sweep lock time and inbox filter cost at 100k rows ".repeat(6);
        }
    }
    let mut app = app_with(snap);
    app.sel = Some("perf".into());
    for (cols, rows) in [(46, 45), (46, 30), (100, 44), (70, 60)] {
        let lines = screen_text(&mut app, cols, rows);
        assert_eq!(lines.len(), rows);
        assert!(
            lines[rows - 2].starts_with('└') && lines[rows - 2].ends_with('┘'),
            "{cols}x{rows}: the card reaches the footer:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().all(|l| width(l) <= cols),
            "{cols}x{rows}:\n{}",
            lines.join("\n")
        );
    }
    app.full = true;
    let tall = screen_text(&mut app, 46, 61)[1..].join("\n");
    assert!(
        !tall.contains('…') && flat(&tall).contains("100k rows review perf: sweep"),
        "a tall pane shows the whole prompt:\n{tall}"
    );
}

#[test]
fn the_ask_line_says_who_closed_it() {
    let mut snap = proto_s5();
    let close = |snap: &mut Snapshot, by: Option<&str>, failed: bool, job: &str| {
        let ask = snap
            .asks
            .iter_mut()
            .find(|a| a.correlation_id == "ask-3a3")
            .unwrap();
        ask.closed_by = by.map(Into::into);
        ask.failed = failed;
        ask.open = false;
        ask.state = Some(crate::wire::AskState::Closed {
            outcome: match (by, failed) {
                (Some("recipient"), true) => crate::wire::AskOutcome::AckedFailed,
                (Some("recipient"), false) => crate::wire::AskOutcome::AckedOk,
                (Some("hand"), true) => crate::wire::AskOutcome::HandFailed,
                (Some("hand"), false) => crate::wire::AskOutcome::HandOk,
                (Some("hub"), _) => crate::wire::AskOutcome::Hub,
                (_, true) => crate::wire::AskOutcome::ClosedFailed,
                _ => crate::wire::AskOutcome::ClosedOk,
            },
            failed_effective: failed || by == Some("hub"),
        });
        let row = snap
            .jobs
            .iter_mut()
            .find(|j| j.job_id == "job-58851116")
            .unwrap();
        row.state = job.into();
        row.relation = Some(if job == "running" {
            crate::wire::JobRelation::Settling
        } else {
            crate::wire::JobRelation::Closed
        });
    };
    let (chain, num) = numbered(&snap);
    let tail = |snap: &Snapshot| {
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
                    sel: "job-58851116",
                },
                "job-58851116",
                46,
                None,
                Fit::Whole,
            )
            .text(),
        );
        let at = text.find("13:12 · ").unwrap() + "13:12 · ".len();
        text[at..].split(" needs").next().unwrap().to_string()
    };
    for (by, failed, job, want) in [
        (Some("recipient"), true, "failed", "acked failed"),
        (Some("recipient"), false, "done", "acked ok"),
        (Some("hand"), false, "done", "closed by hand"),
        (Some("hand"), true, "failed", "closed by hand"),
        (Some("hub"), true, "failed", "closed by hub"),
        (None, true, "failed", "closed failed"),
        (None, false, "done", "closed ok"),
        (Some("recipient"), false, "running", "acked ok, settling"),
        (Some("hub"), true, "running", "closed by hub, settling"),
    ] {
        close(&mut snap, by, failed, job);
        assert_eq!(tail(&snap), want, "{by:?} failed={failed} job={job}");
    }
}

#[test]
fn widths_follow_the_pane() {
    let long = "review the upgrade path for old state files and mixed versions";
    let linear = Snapshot {
        captured_at: 1000,
        jobs: vec![
            job("a", "done", &[], 1),
            job("b", "done", &["a"], 2),
            job("c", "queued", &["b"], 3),
        ],
        ..Default::default()
    };
    let (chain, num) = numbered(&linear);
    let view = |snap: &Snapshot| View {
        snap: model::Frame {
            view: snap.clone(),
            ..Default::default()
        },
        chain: &chain,
        num: &num,
        base: 0,
        sel: "a",
    };
    let text = layout::vertical(&view(&linear), 46).text();
    assert!(
        text.lines().all(|l| l.find(['●', '○', '│']) == Some(20)),
        "a linear chain's spine sits at column 20 of 46, as in the design:\n{text}"
    );
    let mut wordy = linear.clone();
    wordy.jobs[1].title = long.into();
    let text = layout::vertical(&view(&wordy), 46).text();
    let row = text.lines().find(|l| l.contains("2 review")).unwrap();
    assert!(
        row.find('●') < Some(20) && text.lines().all(|l| width(l) <= 46),
        "a long row moves the spine left to show more of it:\n{text}"
    );
    let wide = layout::horizontal(&view(&wordy), 200).unwrap().text();
    assert!(
        wide.contains(long),
        "a wide flow shows whole names:\n{wide}"
    );
    let mut named = wordy.clone();
    named.jobs[2].title = format!("{long} and ship");
    let mut app = app_with(named);
    let top = screen_text(&mut app, 30, 20);
    assert!(
        top[1].starts_with("▸ review") && top[1].ends_with("  2/3") && width(&top[1]) <= 30,
        "a long chain name gives way to the counts:\n{}",
        top[1]
    );
    let mut app = app_with(wordy.clone());
    app.sel = Some("b".into());
    let head = |app: &mut App, cols| screen_text(app, cols, 30);
    let narrow = head(&mut app, 46);
    let broad = head(&mut app, 160);
    assert!(
        narrow[1].ends_with("  2/3") && broad[1].ends_with("  2/3"),
        "the counts always show:\n{}\n{}",
        narrow[1],
        broad[1]
    );
    let card_head = |lines: &[String]| lines.iter().find(|l| l.starts_with("┌●")).unwrap().clone();
    assert!(
        card_head(&broad).contains(long) && !card_head(&narrow).contains(long),
        "the card's title takes the width the pane gives:\n{}\n{}",
        card_head(&narrow),
        card_head(&broad)
    );
    app.sel = Some("a".into());
    app.full = true;
    let blocks = |lines: Vec<String>| lines.into_iter().find(|l| l.contains("blocks")).unwrap();
    let (short, full) = (blocks(head(&mut app, 46)), blocks(head(&mut app, 160)));
    assert!(
        width(&full) > width(&short) && full.contains(long),
        "a wider card names its one dependency whole:\n{short}\n{full}"
    );
}

#[test]
fn only_a_running_job_is_marked_waiting() {
    let mut snap = busy();
    let deploy = snap.jobs.iter_mut().find(|j| j.job_id == "deploy").unwrap();
    deploy.state = "done".into();
    deploy.assigned_peer = Some("w3".into());
    deploy.ask_id = None;
    let mut app = app_with(snap);
    assert!(
        row_of(&mut app, "8 fix", 0).contains("WAIT")
            && !row_of(&mut app, "9 deploy", 0).contains("WAIT"),
        "w3 waits in fix's turn; deploy, done, is not waiting"
    );
}

#[test]
fn a_jump_half_typed_into_a_chain_that_goes_is_dropped() {
    let chain = |p: &str, at: u64| -> Vec<Job> {
        (0..12)
            .map(|i| {
                let id = format!("{p}{i:02}");
                let prev = format!("{p}{:02}", i.max(1) - 1);
                let deps: Vec<&str> = if i == 0 { vec![] } else { vec![prev.as_str()] };
                job(&id, "queued", &deps, at + i)
            })
            .collect()
    };
    let both: Vec<Job> = chain("a", 0).into_iter().chain(chain("b", 100)).collect();
    let mut app = app_with(Snapshot {
        captured_at: 1000,
        jobs: both,
        ..Default::default()
    });
    app.screen(46, 40, 0);
    app.sel = Some("a05".into());
    app.key(KeyCode::Char('1'));
    assert!(
        matches!(app.input.mode, Mode::Jump(_)),
        "1 waits for 10..12"
    );
    app.apply(Ok(Snapshot {
        captured_at: 1001,
        jobs: chain("b", 100),
        ..Default::default()
    }));
    app.screen(46, 40, 0);
    assert_eq!(app.input.mode, Mode::Normal, "a's 1_ is not carried to b");
    app.key(KeyCode::Char('2'));
    assert_eq!(
        app.sel.as_deref(),
        Some("b01"),
        "2 is read fresh: b's second job, not its 12th"
    );
}

#[test]
fn the_full_card_scrolls_and_the_flow_keeps_its_keys() {
    let mut snap = proto_s1();
    for job in &mut snap.jobs {
        if matches!(job.job_id.as_str(), "func" | "deliv") {
            job.result = Some(
                (1..=40)
                    .map(|n| format!("finding {n} of the {}", job.job_id))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    }
    let mut app = app_with(snap);
    app.sel = Some("func".into());
    let top = |app: &mut App| {
        let lines = screen_text(app, 46, 21);
        let first = lines[5].clone();
        let at = lines
            .iter()
            .find(|l| l.starts_with("└─ lines"))
            .cloned()
            .unwrap_or_default();
        (lines[4].clone(), first, at)
    };
    app.key(KeyCode::Enter);
    let (head, first, at) = top(&mut app);
    assert!(
        head.starts_with("┌● 2 func") && at.starts_with("└─ lines 1-14 of "),
        "{head}\n{at}"
    );
    app.key(KeyCode::Char('j'));
    let (head, second, at) = top(&mut app);
    assert!(
        head.starts_with("┌● 2 func") && second != first && at.starts_with("└─ lines 2-15 "),
        "j scrolls one line under the pinned header:\n{first}\n{second}\n{at}"
    );
    app.key(KeyCode::Down);
    app.key(KeyCode::Up);
    assert!(top(&mut app).2.starts_with("└─ lines 2-15 "), "arrows too");
    app.key(KeyCode::PageDown);
    assert!(
        top(&mut app).2.starts_with("└─ lines 16-29 "),
        "a page is the rows shown"
    );
    app.key(KeyCode::Char('b'));
    assert!(top(&mut app).2.starts_with("└─ lines 2-15 "));
    app.key(KeyCode::Char('G'));
    let end = top(&mut app).2;
    let total = end
        .split(" of ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let total: usize = total.parse().unwrap();
    assert!(
        end.starts_with(&format!("└─ lines {}-{total} of {total}", total - 13)),
        "G shows the last full page: {end}"
    );
    app.key(KeyCode::Char('j'));
    assert_eq!(top(&mut app).2, end, "and stops there");
    app.key(KeyCode::Char('g'));
    assert!(top(&mut app).2.starts_with("└─ lines 1-14 "));
    app.key(KeyCode::Char(' '));
    app.key(KeyCode::Char('l'));
    assert_eq!(
        app.sel.as_deref(),
        Some("perf"),
        "l still moves within the stage"
    );
    let (head, _, _) = top(&mut app);
    assert!(head.starts_with("┌◆ 3 perf"), "{head}");
    app.key(KeyCode::Char('l'));
    app.key(KeyCode::Char('j'));
    app.key(KeyCode::Char('l'));
    app.key(KeyCode::Char('h'));
    assert_eq!(app.sel.as_deref(), Some("deliv"));
    assert!(
        top(&mut app).2.starts_with("└─ lines 1-14 "),
        "another job's card starts at its top"
    );
    app.key(KeyCode::Esc);
    app.key(KeyCode::Char('j'));
    assert_eq!(
        app.sel.as_deref(),
        Some("sec"),
        "back in the flow, j moves again"
    );
}

#[test]
fn the_counts_survive_a_narrow_pane() {
    let jobs: Vec<Job> = (0..40)
        .map(|i| {
            job(
                &format!("j{i:02}"),
                if i % 2 == 0 { "failed" } else { "running" },
                &[],
                i,
            )
        })
        .collect();
    let mut app = app_with(Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    });
    for cols in [30, 34, 46, 60] {
        let head = screen_text(&mut app, cols, 12)[1].clone();
        assert!(
            head.contains("  ×  0/40") && width(&head) <= cols,
            "{cols}: {head}"
        );
    }
    assert!(
        screen_text(&mut app, 46, 12)[1].ends_with("0/40  × j00   × j02"),
        "the pills give way before the count"
    );
    for cols in [46, 60] {
        assert!(
            screen_text(&mut app, cols, 12)[1].starts_with("▸ independent jobs  "),
            "the name stays whole while pills give way"
        );
    }
    assert_eq!(
        layout::wrap("中文", 1),
        ["…", "…"],
        "a character wider than a whole line gives way to …"
    );
    assert_eq!(layout::wrap("中文", 2), ["中", "文"]);
}

#[test]
fn the_full_card_reaches_the_end_of_a_long_text() {
    let mut snap = proto_s1();
    snap.detail = Some(crate::wire::JobDetail {
        job_id: "func".into(),
        prompt: format!("{} TAIL_SENTINEL", "word ".repeat(20_000)),
        result: Some(format!("{} RESULT_END", "finding ".repeat(2_000))),
        ..Default::default()
    });
    let mut app = app_with(snap);
    app.sel = Some("func".into());
    app.key(KeyCode::Enter);
    let first = screen_text(&mut app, 46, 20).join("\n");
    assert!(
        first.contains("┌● 2 func") && !first.contains("more characters"),
        "{first}"
    );
    app.key(KeyCode::End);
    let end = screen_text(&mut app, 46, 20).join("\n");
    assert!(
        end.contains("TAIL_SENTINEL"),
        "End reaches the prompt's last word:\n{end}"
    );
    let lines: usize = end
        .lines()
        .find(|l| l.starts_with("└─ lines"))
        .and_then(|l| l.split(" of ").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert!(
        lines > 2_000,
        "the whole text is there to scroll: {lines} lines"
    );
    layout::LONG_WRAPS.with(|n| n.set(0));
    for tick in 0..8 {
        app.screen(46, 20, tick);
    }
    assert_eq!(
        layout::LONG_WRAPS.with(|n| n.get()),
        0,
        "a second of frames wraps neither 100 KB text again"
    );
}

#[test]
fn a_card_that_takes_a_gone_jobs_place_starts_at_its_top() {
    let mut snap = proto_s1();
    for job in &mut snap.jobs {
        job.prompt = "word ".repeat(1_000);
    }
    let mut app = app_with(snap.clone());
    app.sel = Some("func".into());
    app.key(KeyCode::Enter);
    screen_text(&mut app, 46, 20);
    app.key(KeyCode::Char('j'));
    screen_text(&mut app, 46, 20);
    assert_eq!(app.scroll, 1);
    snap.jobs.retain(|j| j.job_id != "func");
    app.apply(Ok(snap));
    let lines = screen_text(&mut app, 46, 20);
    assert_ne!(app.sel.as_deref(), Some("func"));
    assert!(
        app.scroll == 0 && lines.iter().any(|l| l.starts_with("└─ lines 1-")),
        "{}",
        lines.join("\n")
    );
}

#[test]
fn the_card_under_the_flow_lays_out_only_its_rows() {
    let mut snap = proto_s1();
    snap.detail = Some(crate::wire::JobDetail {
        job_id: "func".into(),
        prompt: "word ".repeat(200_000),
        result: None,
        ..Default::default()
    });
    let mut app = app_with(snap);
    app.sel = Some("func".into());
    layout::WHOLE_CARDS.with(|n| n.set(0));
    for tick in 0..4 {
        app.screen(46, 20, tick);
    }
    assert_eq!(
        layout::WHOLE_CARDS.with(|n| n.get()),
        0,
        "no frame lays out the whole 1 MB card to show twenty rows of it"
    );
    app.key(KeyCode::Enter);
    app.screen(46, 20, 0);
    assert!(
        layout::WHOLE_CARDS.with(|n| n.get()) > 0,
        "the full card does, to scroll it"
    );
    assert_eq!(
        layout::wrap(&"alpha ".repeat(1000), 30)
            .concat()
            .matches("alpha")
            .count(),
        1000
    );
    assert_eq!(
        layout::wrap(&"bravo ".repeat(1000), 30)
            .concat()
            .matches("bravo")
            .count(),
        1000,
        "a text of the same length is its own entry"
    );
}

#[test]
fn ascii_reaches_the_headers_glyph_counts() {
    let jobs: Vec<Job> = (0..1000)
        .map(|i| {
            job(
                &format!("j{i:04}"),
                if i % 2 == 0 { "failed" } else { "running" },
                &[],
                i,
            )
        })
        .collect();
    let mut app = App::new(Opts {
        circle: None,
        ascii: true,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    app.home = false;
    app.apply(Ok(Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }));
    let head = screen_text(&mut app, 30, 12)[1].clone();
    assert_eq!(head, "> independent jobs  x  0/1000", "the strip, in ASCII");
}

#[test]
fn the_card_holds_its_wait_still() {
    let mut app = app_with(busy());
    app.sel = Some("fix".into());
    let at = |app: &mut App, tick| -> Vec<String> {
        app.screen(46, 40, tick)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    };
    let off = at(&mut app, 4);
    let row = off.iter().find(|l| l.contains("8 fix w3")).unwrap();
    let worker = off
        .iter()
        .find(|l| l.starts_with("┌") && l.contains(" WAIT "))
        .unwrap();
    assert!(
        row.contains("8 fix w3 1m WAIT") && !row.contains("WAIT!") && worker.contains(" WAIT "),
        "the flow row and the card's badge both hold WAIT steady:\n{row}\n{worker}"
    );
}

fn with(mut job: Job, progress: Progress, busy: bool) -> Job {
    job.progress = Some(JobProgress {
        state: Some(progress),
        busy,
    });
    job
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

fn flow(snap: &Snapshot, sel: &str, cols: usize, wide: bool) -> (Grid, HashMap<String, usize>) {
    let (chain, num) = numbered(snap);
    let v = View {
        snap: model::Frame {
            view: snap.clone(),
            ..Default::default()
        },
        chain: &chain,
        num: &num,
        base: 0,
        sel,
    };
    let g = match wide {
        true => layout::horizontal(&v, cols).expect("fits"),
        false => layout::vertical(&v, cols),
    };
    (g, num)
}

fn glyph_at(g: &Grid, node: usize) -> (usize, usize) {
    g.rows
        .iter()
        .flat_map(|(r, row)| row.iter().map(move |(c, cell)| (*r, *c, cell)))
        .find(|(_, _, cell)| cell.glyph && cell.node == node)
        .map(|(r, c, _)| (r, c))
        .unwrap_or_else(|| panic!("no glyph for node {node}"))
}

fn tone_at(g: &Grid, r: usize, c: usize) -> Tone {
    g.rows[&r][&c].tone
}

/* the row as text with a space for every empty cell, so a char index is its column */
fn cells_text(g: &Grid, r: usize) -> String {
    let row = &g.rows[&r];
    let end = row.keys().max().copied().unwrap_or(0);
    (0..=end)
        .map(|c| row.get(&c).map_or(' ', |cell| cell.ch))
        .collect()
}

fn find(g: &Grid, needle: &str) -> (usize, usize) {
    g.rows
        .keys()
        .find_map(|r| {
            cells_text(g, *r)
                .find(needle)
                .map(|i| (*r, cells_text(g, *r)[..i].chars().count()))
        })
        .unwrap_or_else(|| panic!("no {needle} in\n{}", g.text()))
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

fn peer(id: &str, status: &str, state: Option<&str>) -> Peer {
    Peer {
        peer_id: id.into(),
        name: id.into(),
        status: status.into(),
        activity: state.map(|s| Activity {
            state: s.into(),
            ..Default::default()
        }),
        liveness: state.map(|s| match s {
            "work" => crate::wire::Liveness::Work { since: 0 },
            "wait" => crate::wire::Liveness::Wait {
                since: 0,
                reason: None,
            },
            "idle" => crate::wire::Liveness::Idle { since: 0 },
            _ => crate::wire::Liveness::Unknown,
        }),
        ..Default::default()
    }
}

fn with_roster(mut snap: Snapshot, roster: Vec<Peer>) -> Snapshot {
    snap.capabilities.peer_activity = true;
    snap.capabilities.roster = true;
    snap.roster = roster;
    snap
}

fn joined(pieces: &[(String, layout::Tone)]) -> String {
    pieces.iter().map(|(text, _)| text.as_str()).collect()
}

#[test]
fn the_top_line_names_the_online_peers_and_what_each_does() {
    let snap = with_roster(
        Snapshot::default(),
        vec![
            peer("b-work", "online", Some("work")),
            peer("a-idle", "online", Some("idle")),
            peer("e-gone", "offline", Some("work")),
            peer("c-wait", "online", Some("wait")),
            peer("d-quiet", "online", None),
        ],
    );
    let pieces = layout::presence(&as_frame(&snap), 120, '⠹', true);
    assert_eq!(
        joined(&pieces),
        "4 online · ○ a-idle  ⠹ b-work  ! c-wait  ● d-quiet"
    );
    let tone = |mark: &str| {
        pieces
            .iter()
            .find(|(t, _)| t == mark)
            .map(|(_, tone)| *tone)
    };
    assert_eq!(tone("○"), Some(layout::Tone::Dim));
    assert_eq!(tone("⠹"), Some(layout::Tone::Run));
    assert_eq!(tone("!"), Some(layout::Tone::Wait));
    assert_eq!(tone("●"), Some(layout::Tone::Soft));
    assert_eq!(
        joined(&layout::presence(&as_frame(&snap), 120, '⠹', false)),
        "4 online · ○ a-idle  ⠹ b-work    c-wait  ● d-quiet",
        "the WAIT mark blinks"
    );
}

#[test]
fn an_overflowing_top_line_keeps_the_peers_that_need_attention() {
    let snap = with_roster(
        Snapshot::default(),
        vec![
            peer("a-idle", "online", Some("idle")),
            peer("b-idle", "online", Some("idle")),
            peer("c-idle", "online", None),
            peer("d-work", "online", Some("work")),
            peer("e-wait", "online", Some("wait")),
        ],
    );
    /* the label, two tokens and "  +3" take exactly 33 columns */
    assert_eq!(
        joined(&layout::presence(&as_frame(&snap), 33, '⠹', true)),
        "5 online · ⠹ d-work  ! e-wait  +3"
    );
    assert_eq!(
        joined(&layout::presence(&as_frame(&snap), 32, '⠹', true)),
        "5 online · ! e-wait  +4",
        "waiting outranks working"
    );
    assert_eq!(
        joined(&layout::presence(&as_frame(&snap), 20, '⠹', true)),
        "5 online",
        "no token fits: the count alone"
    );
}

#[test]
fn the_top_line_says_so_when_nobody_is_online_or_the_hub_cannot_tell() {
    let nobody = with_roster(Snapshot::default(), vec![peer("gone", "offline", None)]);
    assert_eq!(
        joined(&layout::presence(&as_frame(&nobody), 60, '⠹', true)),
        "no peers online"
    );
    let old = Snapshot {
        roster: vec![peer("x", "online", None)],
        ..Default::default()
    };
    assert_eq!(
        joined(&layout::presence(&as_frame(&old), 80, '⠹', true)),
        "peers: restart the hub on the current amesh to list them"
    );
    assert!(width(&joined(&layout::presence(&as_frame(&old), 20, '⠹', true))) <= 20);
}

#[test]
fn the_screen_puts_the_online_peers_above_the_chain() {
    let roster = vec![
        peer("cc", "online", Some("work")),
        peer("pi", "online", Some("idle")),
    ];
    let mut app = app_with(with_roster(s1(), roster.clone()));
    let lines = screen_text(&mut app, 46, 60);
    assert_eq!(lines[0], "2 online · ⠋ cc  ○ pi", "{}", lines.join("\n"));
    assert!(lines[1].starts_with("▸ deploy  "), "{}", lines.join("\n"));
    assert!(lines[2].starts_with("───"), "{}", lines.join("\n"));
    let mut empty = app_with(with_roster(Snapshot::default(), roster));
    let lines = screen_text(&mut empty, 46, 20);
    assert_eq!(
        lines[0], "2 online · ⠋ cc  ○ pi",
        "the line shows before any job exists"
    );
    assert_eq!(lines[1], format!("{:<45}●", "no jobs here yet"));
}

#[test]
fn the_top_line_keeps_its_marks_in_ascii_and_blinks() {
    let roster = vec![
        peer("a", "online", Some("idle")),
        peer("b", "online", Some("work")),
        peer("c", "online", Some("wait")),
        peer("d", "online", None),
    ];
    let mut app = app_with(with_roster(s1(), roster));
    app.opts.ascii = true;
    assert_eq!(
        screen_text(&mut app, 60, 40)[0],
        "4 online | o a  | b  ! c  * d"
    );
    let dark: String = app.screen(60, 40, 4)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(dark.trim_end(), "4 online | o a  | b    c  * d");
}

#[test]
fn a_stale_top_line_says_how_old_it_is() {
    let roster = vec![peer("worker", "online", Some("work"))];
    let mut app = app_with(with_roster(Snapshot::default(), roster));
    app.apply(Err("connection refused".into()));
    app.last_ok = Some(std::time::Instant::now() - std::time::Duration::from_secs(120));
    let lines = screen_text(&mut app, 60, 20);
    assert_eq!(lines[0], "1 online · ⠋ worker", "{}", lines.join("\n"));
    assert!(
        lines[1].starts_with("! connection refused · showing the snapshot from 12"),
        "the peers above are that old:\n{}",
        lines.join("\n")
    );
}

#[test]
fn an_overflowing_top_line_counts_a_name_too_long_to_show_and_goes_on() {
    let long = "w".repeat(40);
    let snap = with_roster(
        Snapshot::default(),
        vec![
            peer(&long, "online", Some("wait")),
            peer("a", "online", Some("work")),
        ],
    );
    assert_eq!(
        joined(&layout::presence(&as_frame(&snap), 40, '⠹', true)),
        "2 online · ⠹ a  +1",
        "the name that cannot fit is counted, the next one still shows"
    );
}

#[test]
fn the_top_line_falls_back_to_ids_and_keeps_to_its_width() {
    let mut unnamed = peer("id-only", "online", None);
    unnamed.name.clear();
    let twin = |id: &str| Peer {
        name: "same".into(),
        ..peer(id, "online", Some("idle"))
    };
    let snap = with_roster(Snapshot::default(), vec![twin("x2"), unnamed, twin("x1")]);
    assert_eq!(
        joined(&layout::presence(&as_frame(&snap), 80, '⠹', true)),
        "3 online · ● id-only  ○ same  ○ same"
    );
    let wide = with_roster(
        Snapshot::default(),
        vec![
            peer("审查", "online", None),
            peer("数据中心", "online", None),
        ],
    );
    for cols in [8, 10, 16, 19, 20, 30] {
        let line = joined(&layout::presence(&as_frame(&wide), cols, '⠹', true));
        assert!(width(&line) <= cols, "{cols}: {line}");
    }
    assert_eq!(
        joined(&layout::presence(&as_frame(&wide), 30, '⠹', true)),
        "2 online · ● 审查  ● 数据中心"
    );
}

#[test]
fn the_smallest_pane_keeps_the_top_line_and_the_header() {
    let mut app = app_with(with_roster(s1(), vec![peer("cc", "online", Some("work"))]));
    let lines = screen_text(&mut app, 46, 8);
    assert_eq!(lines.len(), 8, "{}", lines.join("\n"));
    assert_eq!(lines[0], "1 online · ⠋ cc");
    assert!(lines[1].starts_with("▸ deploy  "), "{}", lines.join("\n"));
}

fn grid_text(snap: &Snapshot, cols: usize, rows: usize) -> Vec<String> {
    let frame = as_frame(snap);
    layout::presence_lines(&frame, cols, rows, '⠹', true)
        .iter()
        .map(|line| joined(line).trim_end().to_string())
        .collect()
}

#[test]
fn a_top_line_that_does_not_fit_wraps_into_aligned_columns() {
    let snap = with_roster(
        Snapshot::default(),
        vec![
            peer("zeta-web-frontend-pi-2", "online", Some("idle")),
            peer("acme-web-frontend-claude-code", "online", Some("work")),
            peer("acme-web-frontend-codex", "online", Some("idle")),
        ],
    );
    assert_eq!(
        grid_text(&snap, 80, 4),
        [
            "3 online · ⠹ acme-web-frontend-claude-code  ○ acme-web-frontend-codex",
            "           ○ zeta-web-frontend-pi-2",
        ],
        "every name shows, each line indented under the first name"
    );
    assert_eq!(
        grid_text(&snap, 120, 4),
        ["3 online · ⠹ acme-web-frontend-claude-code  ○ acme-web-frontend-codex  ○ zeta-web-frontend-pi-2"],
        "one line while they all fit"
    );
    assert_eq!(grid_text(&snap, 60, 4).len(), 3, "one name per line");
    for cols in [42, 45, 60, 80, 94, 95] {
        for line in grid_text(&snap, cols, 4) {
            assert!(width(&line) <= cols, "{cols}: {line}");
        }
    }
    assert_eq!(
        layout::presence_lines(&as_frame(&snap), 80, 1, '⠹', true),
        vec![layout::presence(&as_frame(&snap), 80, '⠹', true)],
        "a single row keeps the top line and its count"
    );
}

#[test]
fn a_grid_taller_than_its_rows_keeps_attention_first_and_counts_the_rest() {
    let snap = with_roster(
        Snapshot::default(),
        vec![
            peer("a-idle", "online", Some("idle")),
            peer("b-idle", "online", Some("idle")),
            peer("c-idle", "online", None),
            peer("d-work", "online", Some("work")),
            peer("e-wait", "online", Some("wait")),
        ],
    );
    assert_eq!(
        grid_text(&snap, 33, 2),
        ["5 online · ○ a-idle  ⠹ d-work", "           ! e-wait  +2"]
    );
    assert_eq!(
        grid_text(&snap, 33, 3),
        [
            "5 online · ○ a-idle  ○ b-idle",
            "           ● c-idle  ⠹ d-work",
            "           ! e-wait",
        ],
        "three rows hold all five"
    );
}

#[test]
fn the_screen_moves_the_chain_below_a_wrapped_top_line() {
    let roster = vec![
        peer("acme-web-frontend-claude-code", "online", Some("work")),
        peer("acme-web-frontend-codex", "online", Some("idle")),
        peer("zeta-web-frontend-pi-2", "online", Some("idle")),
    ];
    let mut app = app_with(with_roster(s1(), roster.clone()));
    let lines = screen_text(&mut app, 80, 30);
    assert_eq!(
        lines[..2],
        [
            "3 online · ⠋ acme-web-frontend-claude-code  ○ acme-web-frontend-codex",
            "           ○ zeta-web-frontend-pi-2",
        ],
        "{}",
        lines.join("\n")
    );
    assert!(lines[2].starts_with("▸ deploy  "), "{}", lines.join("\n"));
    assert!(lines[3].starts_with("───"), "{}", lines.join("\n"));
    let mut empty = app_with(with_roster(Snapshot::default(), roster.clone()));
    let lines = screen_text(&mut empty, 80, 20);
    assert_eq!(
        lines[2],
        format!("{:<79}●", "no jobs here yet"),
        "{}",
        lines.join("\n")
    );
    let mut small = app_with(with_roster(s1(), roster));
    let lines = screen_text(&mut small, 80, 8);
    assert_eq!(lines.len(), 8, "{}", lines.join("\n"));
    assert!(
        lines[0].ends_with("+1"),
        "the smallest pane keeps one line and counts the rest:\n{}",
        lines.join("\n")
    );
    assert!(lines[1].starts_with("▸ deploy  "), "{}", lines.join("\n"));
}

#[test]
fn the_view_moves_down_at_once_and_back_up_once_the_roster_held_still() {
    let long = |id: &str| peer(&format!("acme-web-frontend-{id}"), "online", Some("idle"));
    let two = vec![long("claude-code"), long("codex")];
    let three = vec![
        long("claude-code"),
        long("codex"),
        peer("zeta-web-frontend-pi-2", "online", Some("idle")),
    ];
    let header = |app: &mut App| {
        let lines = screen_text(app, 80, 30);
        lines
            .iter()
            .position(|l| l.starts_with("▸ deploy  "))
            .unwrap_or_else(|| panic!("{}", lines.join("\n")))
    };
    let mut app = app_with(with_roster(s1(), two.clone()));
    assert_eq!(header(&mut app), 1, "two names fit on the top line");
    app.apply(Ok(with_roster(s1(), three)));
    assert_eq!(
        header(&mut app),
        2,
        "a third wraps it and the view moves down at once"
    );
    assert_eq!(
        screen_text(&mut app, 80, 8)[1].get(..12),
        Some("▸ deploy  "),
        "a small pane caps the rows kept"
    );
    for n in 1..PEER_HOLD {
        app.apply(Ok(with_roster(s1(), two.clone())));
        assert_eq!(
            header(&mut app),
            2,
            "{n} snapshots after it left, its row is kept"
        );
    }
    app.apply(Ok(with_roster(s1(), two.clone())));
    assert_eq!(header(&mut app), 1, "then the view moves back up");
}

fn with_events(mut snap: Snapshot, n: u64) -> Snapshot {
    snap.capabilities.event_count = true;
    snap.event_count = n;
    snap
}

#[test]
fn the_events_tag_counts_and_stays_out_of_hubs_that_do_not() {
    assert_eq!(layout::events_tag(&Snapshot::default()), None);
    for (n, want) in [(0, "0 events"), (1, "1 event"), (42, "42 events")] {
        assert_eq!(
            layout::events_tag(&with_events(Snapshot::default(), n)).as_deref(),
            Some(want)
        );
    }
}

#[test]
fn the_rule_shows_how_many_events_the_hub_keeps() {
    let mut app = app_with(with_events(s1(), 42));
    for cols in [46usize, 100] {
        let lines = screen_text(&mut app, cols, 40);
        assert!(
            lines[2].starts_with("───") && lines[2].ends_with(" 42 events · ● ─"),
            "{cols}: {}",
            lines[2]
        );
        assert_eq!(width(&lines[2]), cols);
    }
    let old = screen_text(&mut app_with(s1()), 46, 40);
    assert!(
        !old[2].contains("event"),
        "a hub that does not count shows nothing"
    );
}

#[test]
fn the_events_tag_ends_the_rule_of_a_screen_with_two_blocks() {
    let mut app = app_with(with_events(mixed(), 42));
    let wide = screen_text(&mut app, 46, 60);
    let rule = &wide[rule_row(&wide)];
    assert!(rule.ends_with("─ 42 events · ● ─"), "{rule}");
    app.opts.ascii = true;
    let ascii = screen_text(&mut app, 46, 60);
    let rule = &ascii[rule_row(&ascii)];
    assert!(rule.ends_with("- 42 events | * -"), "{rule}");
}

#[test]
fn the_view_without_jobs_shows_the_count_on_its_message_row() {
    let mut app = app_with(with_events(Snapshot::default(), 42));
    let lines = screen_text(&mut app, 46, 20);
    assert!(
        lines[1].starts_with("no jobs here yet") && lines[1].ends_with("42 events · ●"),
        "{}",
        lines[1]
    );
    assert_eq!(width(&lines[1]), 46);
    /* wide enough that the error line leaves room for the count, so only the rule for an
    error keeps it off */
    let fresh = screen_text(&mut app, 100, 20);
    assert!(fresh[1].ends_with("42 events · ●"), "{}", fresh[1]);
    app.apply(Err("connection refused".into()));
    let lines = screen_text(&mut app, 100, 20);
    assert!(
        lines[1].starts_with("! connection refused"),
        "the error is on screen: {}",
        lines[1]
    );
    assert!(
        !lines.join("\n").contains("42 events"),
        "a stale count stays off the screen"
    );
}

#[test]
fn a_stale_count_stays_off_the_rule_while_the_hub_is_unreachable() {
    let mut app = app_with(with_events(s1(), 42));
    let fresh = screen_text(&mut app, 100, 40);
    assert!(fresh[2].ends_with(" 42 events · ● ─"), "{}", fresh[2]);
    app.apply(Err("connection refused".into()));
    let lines = screen_text(&mut app, 100, 40);
    assert!(
        lines[3].starts_with("! connection refused"),
        "the error is on screen: {}",
        lines[3]
    );
    assert!(
        !lines[2].contains("events"),
        "the count is as old as the snapshot: {}",
        lines[2]
    );
}

const CC: &str = "openhitls-sm2-opt-claude-code";
const CODEX: &str = "openhitls-sm2-opt-codex";
const PI: &str = "openhitls-sm2-opt-pi";
const PI2: &str = "openhitls-sm2-opt-pi-2";

fn lone(cid: &str, from: &str, to: &str, opened_at: Option<u64>) -> Ask {
    Ask {
        correlation_id: cid.into(),
        from_peer: from.into(),
        to_peer: to.into(),
        to_peer_id: to.into(),
        open: true,
        opened_at,
        text: format!("question {cid}"),
        ..Default::default()
    }
}

fn closed(mut ask: Ask, at: u64, by: &str, failed: bool, reply: &str) -> Ask {
    ask.open = false;
    ask.closed_at = Some(at);
    ask.closed_by = (!by.is_empty()).then(|| by.into());
    ask.failed = failed;
    ask.reply = Some(reply.into());
    ask
}

/* asks and the peers they name, who are the view's own peers, as a hub with ask_list sends
them */
fn with_asks(mut snap: Snapshot, asks: Vec<Ask>, peers: Vec<Peer>) -> Snapshot {
    snap.capabilities.ask_list = true;
    snap.capabilities.peer_activity = true;
    snap.capabilities.roster = true;
    snap.asks.extend(asks);
    snap.roster.extend(peers.iter().cloned());
    snap.peers.extend(peers);
    snap
}

fn stamp(
    mut ask: Ask,
    state: crate::wire::AskState,
    actions: Vec<crate::wire::AskAction>,
    just: Option<bool>,
) -> Ask {
    ask.open = matches!(state, crate::wire::AskState::Open { .. });
    ask.state = Some(state);
    ask.actions = Some(actions);
    ask.closed_just_now = just;
    ask
}

/* the fan-out of the design: one question to three peers; codex has answered, pi's turn
ended without an answer */
fn fan_out() -> Snapshot {
    use crate::wire::{AskAction, AskOutcome, AskState, IdleWhy, Liveness, Progress};
    let mut snap = with_asks(
        Snapshot {
            captured_at: 1000,
            ..Default::default()
        },
        vec![
            stamp(
                lone("ask-1223c9e0", CC, PI2, Some(820)),
                AskState::Open {
                    progress: Progress::Work { since: 830 },
                },
                vec![],
                None,
            ),
            stamp(
                lone("ask-a3a07b51", CC, PI, Some(820)),
                AskState::Open {
                    progress: Progress::Idle {
                        since: 940,
                        why: IdleWhy::TurnEnded,
                    },
                },
                vec![AskAction::Nudge { to: PI.into() }],
                None,
            ),
            stamp(
                closed(
                    lone("ask-a904f2d6", CC, CODEX, Some(820)),
                    950,
                    "recipient",
                    false,
                    "2 findings, both fixed",
                ),
                AskState::Closed {
                    outcome: AskOutcome::AckedOk,
                    failed_effective: false,
                },
                vec![],
                Some(false),
            ),
        ],
        vec![
            doing(CC, "idle", 830, None),
            doing(CODEX, "idle", 950, None),
            doing(PI, "idle", 940, None),
            doing(PI2, "work", 830, None),
        ],
    );
    for peer in snap.peers.iter_mut().chain(snap.roster.iter_mut()) {
        peer.liveness = Some(match peer.peer_id.as_str() {
            PI2 => Liveness::Work { since: 830 },
            CODEX => Liveness::Idle { since: 950 },
            PI => Liveness::Idle { since: 940 },
            _ => Liveness::Idle { since: 830 },
        });
    }
    /* every sender here is a peer of the view whose name is its id, as the hub resolves it */
    for ask in snap.asks.iter_mut() {
        ask.from_peer_id = Some(ask.from_peer.clone());
    }
    snap
}

/* the design's busy asks: the fan-out, codex at a permission prompt on a newer ask, pi-2's
question that claude-code has not picked up, an ask to a peer that left, and a failed answer */
fn busy_asks() -> Snapshot {
    use crate::wire::{AskAction, AskOutcome, AskState, IdleWhy, Liveness, Progress};
    let mut snap = fan_out();
    let open = |cid, from, to, at, progress, actions| {
        let ask = lone(cid, from, to, Some(at));
        stamp(ask, AskState::Open { progress }, actions, None)
    };
    let reason = Some("Claude needs your permission to use Bash".to_string());
    let mut perm = open(
        "ask-5d0c11e2",
        CC,
        CODEX,
        960,
        Progress::Wait {
            since: 985,
            reason: reason.clone(),
        },
        vec![],
    );
    perm.text = "run the full sm2 bench on the arm64 box and post the table".into();
    let failed = closed(
        lone("ask-e81f3b07", CC, PI2, Some(600)),
        900,
        "recipient",
        true,
        "build broke: sm2_sign undefined after the rebase",
    );
    snap.asks.extend([
        perm,
        open(
            "ask-77aa0d93",
            PI2,
            CC,
            975,
            Progress::Idle {
                since: 975,
                why: IdleWhy::NotPickedUp,
            },
            vec![AskAction::Nudge { to: CC.into() }],
        ),
        open(
            "ask-0c9e4a18",
            CC,
            "openhitls-sm2-opt-gemini",
            700,
            Progress::Gone,
            vec![AskAction::CloseLeft],
        ),
        stamp(
            failed,
            AskState::Closed {
                outcome: AskOutcome::AckedFailed,
                failed_effective: true,
            },
            vec![],
            Some(false),
        ),
    ]);
    for ask in snap.asks.iter_mut() {
        ask.from_peer_id = Some(ask.from_peer.clone());
    }
    set_live(&mut snap, &[(CODEX, Liveness::Wait { since: 985, reason })]);
    snap
}

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

fn with_lone_asks(snap: Snapshot) -> Snapshot {
    with_asks(
        snap,
        vec![
            lone("ask-1223c9e0", CC, PI2, Some(820)),
            lone("ask-a3a07b51", CC, PI, Some(820)),
        ],
        vec![doing(PI2, "work", 830, None), doing(PI, "work", 830, None)],
    )
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

fn copy_app() -> App {
    let mut app = app_with(sample());
    app.sel = Some("j".into());
    app
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
        ..Default::default()
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

/* the screen at `tick`, one string a line */
fn screen_at(app: &mut App, cols: usize, tick: usize) -> Vec<String> {
    app.screen(cols, 40, tick)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn line_with(app: &mut App, cols: usize, tick: usize, needle: &str) -> String {
    let lines = screen_at(app, cols, tick);
    lines
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle}:\n{}", lines.join("\n")))
        .clone()
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

#[test]
fn a_job_the_hub_just_sent_gets_a_dot_along_its_rail_then_lights() {
    let mut snap = proto_s1();
    let now = snap.captured_at;
    let ask = snap
        .asks
        .iter_mut()
        .find(|a| a.correlation_id == "ask-7c2")
        .unwrap();
    /* the hub's word decides, not the stamp: an ask opened a minute ago the hub calls just
    sent still gets its dot */
    ask.opened_at = Some(now - 60);
    ask.opened_just_now = Some(true);
    let mut app = app_with(snap);
    let has = |lines: Vec<String>, want: &str| lines.iter().any(|l| l.trim_end() == want);
    /* the vertical flow: the spine above upg is the end of the fan-out bracket */
    assert!(
        has(
            screen_at(&mut app, 46, 0),
            "    ┌───────┬───────┼───────┬───────•"
        ),
        "the dot on the rail into it"
    );
    assert!(
        has(
            screen_at(&mut app, 46, 2),
            "    ●       ◆       ●       ×       ◉"
        ),
        "then its glyph lights"
    );
    /* the horizontal flow: the connector on its left, cell by cell */
    let row = |app: &mut App, tick| line_with(app, 100, tick, "6 upg");
    assert!(
        row(&mut app, 0).contains("•─ ⠋ 6 upg"),
        "{}",
        row(&mut app, 0)
    );
    assert!(
        row(&mut app, 2).contains("└• ⠹ 6 upg"),
        "{}",
        row(&mut app, 2)
    );
    assert!(
        row(&mut app, 4).contains("└─ ◉ 6 upg"),
        "{}",
        row(&mut app, 4)
    );
    app.opts.anim = false;
    assert!(
        has(
            screen_at(&mut app, 46, 2),
            "    ●       ◆       ●       ×       ⠋"
        ),
        "--no-anim leaves it still"
    );
    let mut earlier = app_with(proto_s1());
    assert!(
        has(
            screen_at(&mut earlier, 46, 0),
            "    ┌───────┬───────┼───────┬───────┐"
        ),
        "a job sent before the last second gets none"
    );
    assert_eq!(['▸', '◂', '•', '◉'].map(super::ascii), ['>', '<', '.', '@']);
}

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

#[test]
fn a_stale_snapshot_moves_nothing_it_reports_as_just_now() {
    let mut snap = proto_s1();
    let now = snap.captured_at;
    let find = |snap: &Snapshot, cid: &str| {
        let at = snap.asks.iter().position(|a| a.correlation_id == cid);
        at.unwrap()
    };
    let (acked, sent) = (find(&snap, "ask-5b1"), find(&snap, "ask-7c2"));
    snap.asks[acked].closed_at = Some(now);
    snap.asks[acked].closed_just_now = Some(true);
    snap.asks[sent].opened_at = Some(now);
    snap.asks[sent].opened_just_now = Some(true);
    let mut app = app_with(snap);
    app.last_ok = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(5));
    app.sel = Some("func".into());
    app.full = true;
    assert!(
        line_with(&mut app, 100, 2, "5b1 ").contains("5b1 cc>codex "),
        "the hub stopped answering: no walk back"
    );
    app.full = false;
    let lines = screen_at(&mut app, 46, 0);
    assert!(
        lines
            .iter()
            .any(|l| l.trim_end() == "    ┌───────┬───────┼───────┬───────┐"),
        "and no dot into the job it sent"
    );
}

#[test]
fn the_view_moves_only_what_the_hub_calls_just_now() {
    let mut snap = proto_s1();
    let now = snap.captured_at;
    for ask in snap.asks.iter_mut() {
        match ask.correlation_id.as_str() {
            "ask-5b1" => {
                ask.closed_at = Some(now);
                ask.closed_just_now = Some(false);
            }
            "ask-7c2" => {
                ask.opened_at = Some(now);
                ask.opened_just_now = Some(false);
            }
            _ => {}
        }
    }
    let mut app = app_with(snap);
    app.sel = Some("func".into());
    app.full = true;
    assert!(
        line_with(&mut app, 100, 2, "5b1 ").contains("5b1 cc>codex "),
        "an ack stamped this second that the hub does not call just now walks nothing back"
    );
    app.full = false;
    assert!(
        !screen_at(&mut app, 46, 2).iter().any(|l| l.contains('◉')),
        "nor does a job sent this second light"
    );
}

/* an online peer with a push channel or without, and the records waiting for it */
fn reached(id: &str, push: bool, queued: usize) -> Peer {
    Peer {
        push,
        queued,
        ..peer(id, "online", Some("idle"))
    }
}

fn delivering(snap: Snapshot, roster: Vec<Peer>, at: u64) -> Snapshot {
    let mut snap = with_roster(snap, roster);
    snap.capabilities.delivery = true;
    snap.captured_at = at;
    snap
}

/* the snapshot taken three times a second apart, so its full queues read as stuck */
fn app_stuck(snap: Snapshot) -> App {
    let mut app = app_with(Snapshot::default());
    for k in 0..3 {
        let mut s = snap.clone();
        s.captured_at += k;
        app.apply(Ok(s));
    }
    app
}

#[test]
fn delivery_fields_read_from_hubs_that_send_them_and_default_otherwise() {
    let new: Snapshot = serde_json::from_value(serde_json::json!({
        "hub_epoch": "e1",
        "capabilities": {"delivery": true},
        "roster": [{"peer_id": "pi", "push": true, "acks": true, "queued": 3}]
    }))
    .unwrap();
    assert_eq!(new.hub_epoch, "e1");
    assert!(new.capabilities.delivery);
    assert!(new.roster[0].push);
    assert_eq!(new.roster[0].queued, 3);
    let old: Snapshot =
        serde_json::from_value(serde_json::json!({"roster": [{"peer_id": "pi"}]})).unwrap();
    assert!(
        !old.capabilities.delivery,
        "an old hub says nothing about delivery"
    );
    assert!(!old.roster[0].push);
    assert_eq!(old.roster[0].queued, 0);
    assert!(old.hub_epoch.is_empty());
}

fn marked(
    id: &str,
    condition: crate::wire::DeliveryCondition,
    stuck: Option<(usize, u64)>,
) -> Peer {
    let mut peer = reached(id, true, 0);
    peer.liveness = Some(crate::wire::Liveness::Idle { since: 0 });
    peer.delivery = Some(crate::wire::Delivery {
        condition,
        stuck: stuck.map(|(count, since)| crate::wire::Stuck { count, since }),
    });
    peer
}

#[test]
fn the_top_line_marks_a_stuck_queue_and_dims_a_peer_without_push() {
    let snap = delivering(
        Snapshot::default(),
        vec![
            marked("a-pi", crate::wire::DeliveryCondition::Push, Some((2, 160))),
            marked("b-codex", crate::wire::DeliveryCondition::NoPush, None),
            marked("c-cc", crate::wire::DeliveryCondition::Push, None),
        ],
        200,
    );
    let mut frame = as_frame(&snap);
    let pieces = layout::presence(&frame, 120, '⠹', true);
    assert_eq!(joined(&pieces), "3 online · ○ a-pi ✉2  ○ b-codex  ○ c-cc");
    let tone = |text: &str| {
        pieces
            .iter()
            .find(|(t, _)| t == text)
            .map(|(_, tone)| *tone)
    };
    assert_eq!(tone(" ✉2"), Some(layout::Tone::Warn));
    assert_eq!(
        tone(" b-codex"),
        Some(layout::Tone::Dim),
        "no push channel dims the name"
    );
    assert_eq!(tone(" c-cc"), Some(layout::Tone::Soft));
    for peer in frame.roster.iter_mut() {
        peer.delivery = None;
    }
    let old = layout::presence(&frame, 120, '⠹', true);
    assert_eq!(
        joined(&old),
        "3 online · ○ a-pi  ○ b-codex  ○ c-cc",
        "an old hub gets no marks"
    );
    assert!(old
        .iter()
        .all(|(t, tone)| !t.contains("codex") || *tone == layout::Tone::Soft));
}

#[test]
fn a_wrapped_top_line_keeps_its_columns_with_a_queue_mark() {
    let roster = (0..6)
        .map(|i| {
            marked(
                &format!("{}-{i}", if i == 5 { "solo" } else { "peer" }),
                crate::wire::DeliveryCondition::Push,
                (i == 0).then_some((12, 150)),
            )
        })
        .collect();
    let snap = delivering(Snapshot::default(), roster, 200);
    let frame = as_frame(&snap);
    let lines: Vec<String> = layout::presence_lines(&frame, 50, 4, '⠹', true)
        .iter()
        .map(|line| joined(line))
        .collect();
    assert_eq!(
        lines,
        [
            "6 online · ○ peer-0 ✉12  ○ peer-1",
            "           ○ peer-2      ○ peer-3",
            "           ○ peer-4      ○ solo-5",
        ]
    );
}

#[test]
fn the_job_card_says_why_its_open_ask_may_go_unseen() {
    let mut snap = busy();
    snap.capabilities.delivery = true;
    for p in snap.peers.iter_mut() {
        p.push = true;
    }
    let w1 = snap.peers.iter_mut().find(|p| p.peer_id == "w1").unwrap();
    w1.delivery = Some(crate::wire::Delivery {
        condition: crate::wire::DeliveryCondition::NoPush,
        stuck: Some(crate::wire::Stuck {
            count: 2,
            since: snap.captured_at - 2,
        }),
    });
    let mut app = app_with(snap);
    app.full = true;
    app.sel = Some("perf".into());
    let lines = screen_at(&mut app, 100, 0);
    let at = lines
        .iter()
        .position(|l| l.contains("w1 WORK"))
        .unwrap_or_else(|| panic!("no worker line:\n{}", lines.join("\n")));
    assert!(
        lines[at + 1].contains("no push · 2 queued for 2s"),
        "the row under the worker says it:\n{}",
        lines.join("\n")
    );
    app.sel = Some("upg".into());
    let lines = screen_at(&mut app, 100, 0);
    assert!(
        !lines
            .iter()
            .any(|l| !l.starts_with("attention ")
                && (l.contains("no push") || l.contains("queued for"))),
        "a worker with a push channel and an empty queue gets no row:\n{}",
        lines.join("\n")
    );
}

#[test]
fn the_ask_card_says_why_an_open_ask_may_go_unseen() {
    let mut snap = fan_out();
    snap.capabilities.delivery = true;
    for p in snap.peers.iter_mut().chain(snap.roster.iter_mut()) {
        if p.peer_id == PI2 {
            p.delivery = Some(crate::wire::Delivery {
                condition: crate::wire::DeliveryCondition::NoPush,
                stuck: Some(crate::wire::Stuck {
                    count: 1,
                    since: snap.captured_at - 2,
                }),
            });
        }
    }
    let mut app = app_with(snap);
    app.key(KeyCode::Char('a'));
    app.key(KeyCode::Char('2'));
    let lines = screen_at(&mut app, 100, 0);
    let route = lines
        .iter()
        .position(|l| l.contains("│ from    claude-code▸pi-2 · 00:13"))
        .unwrap_or_else(|| panic!("no route line:\n{}", lines.join("\n")));
    assert!(
        lines[route + 1].contains("no push · 1 queued for 2s"),
        "the row under the route says it:\n{}",
        lines.join("\n")
    );
    app.key(KeyCode::Char('3'));
    let lines = screen_at(&mut app, 100, 0);
    assert!(
        !lines
            .iter()
            .any(|l| !l.starts_with("attention ") && l.contains("no push")),
        "a closed ask has nothing left to explain:\n{}",
        lines.join("\n")
    );
}

#[test]
fn a_healthy_mesh_draws_what_it_drew_before() {
    let jobs = {
        let snap = busy();
        let peers = snap.peers.clone();
        with_roster(snap, peers)
    };
    for (base, key) in [(jobs, None), (fan_out(), Some('a'))] {
        let mut healthy = base.clone();
        healthy.capabilities.delivery = true;
        for p in healthy.peers.iter_mut().chain(healthy.roster.iter_mut()) {
            p.push = true;
        }
        let (mut before, mut after) = (app_stuck(base), app_stuck(healthy));
        /* in colour, so a tone that changed shows even where the text did not */
        before.opts.color = true;
        after.opts.color = true;
        if let Some(k) = key {
            before.key(KeyCode::Char(k));
            after.key(KeyCode::Char(k));
        }
        for cols in [60, 100] {
            assert_eq!(
                before.screen(cols, 40, 0),
                after.screen(cols, 40, 0),
                "at {cols} columns"
            );
        }
    }
}

#[test]
fn the_queue_mark_reads_plus_in_ascii() {
    let snap = with_roster(
        busy(),
        vec![marked(
            "pi",
            crate::wire::DeliveryCondition::Push,
            Some((3, 0)),
        )],
    );
    let mut app = app_with(snap);
    app.opts.ascii = true;
    assert_eq!(screen_text(&mut app, 100, 40)[0], "1 online | o pi +3");
}

fn ev(seq: u64, kind: &str, from: &str, to: &str, body: &str) -> model::Event {
    let asked = kind == "ask" || kind == "ack";
    model::Event {
        seq: Some(seq),
        at: Some(1000 + seq),
        kind: kind.into(),
        from_peer: from.into(),
        to_peer: to.into(),
        correlation_id: if asked {
            format!("ask-{seq:08}")
        } else {
            String::new()
        },
        text: if kind == "ask" {
            body.into()
        } else {
            String::new()
        },
        message: if kind == "ask" {
            String::new()
        } else {
            body.into()
        },
        ..Default::default()
    }
}

/* the events screen of a view with two peers of one folder online, after `events` came in */
fn events_app(events: Vec<model::Event>) -> App {
    let snap = Snapshot {
        captured_at: 1100,
        hub_epoch: "e1".into(),
        ..Default::default()
    };
    let roster = vec![
        peer(CC, "online", Some("idle")),
        peer(PI, "online", Some("idle")),
    ];
    let mut app = app_with(with_roster(snap, roster));
    app.key(KeyCode::Char('e'));
    app.take_events(Ok(events));
    app
}

/* fan_out()'s asks as the hub records them: an event for each recipient, all from one sender
with one text in one second, numbered from `seq` */
fn fan_events(seq: u64) -> Vec<model::Event> {
    fan_out()
        .asks
        .iter()
        .zip(seq..)
        .map(|(ask, n)| model::Event {
            at: Some(1000 + seq),
            correlation_id: ask.correlation_id.clone(),
            ..ev(
                n,
                "ask",
                &ask.from_peer,
                &ask.to_peer,
                "review the SM2 sign path",
            )
        })
        .collect()
}

fn held_seqs(app: &App) -> Vec<Option<u64>> {
    app.feed
        .iter()
        .map(|h| match h {
            super::feed::Held::Event(_, e) => e.seq,
            super::feed::Held::Restart(_) => None,
        })
        .collect()
}

#[test]
fn e_opens_the_events_screen_and_e_or_a_leaves_it() {
    let mut app = app_with(with_lone_asks(s1()));
    screen_text(&mut app, 100, 40);
    app.key(KeyCode::Char('e'));
    assert!(app.events);
    let lines = screen_text(&mut app, 80, 40);
    assert!(
        lines.iter().any(|l| l.starts_with("events · ")),
        "{lines:?}"
    );
    assert_eq!(
        lines.last().unwrap(),
        "j/k move  enter card  c jobs  a asks"
    );
    app.key(KeyCode::Char('c'));
    assert!(!app.events && !app.asks && !app.home, "c goes to the jobs");
    app.key(KeyCode::Char('e'));
    app.key(KeyCode::Char('a'));
    assert!(!app.events && app.asks, "a goes to the asks");
    app.key(KeyCode::Char('e'));
    assert!(app.events, "e opens the events from the asks too");
    app.key(KeyCode::Char('a'));
    assert!(
        !app.events && app.asks,
        "a over the asks goes back to them, not to the jobs"
    );
    app.key(KeyCode::Char('e'));
    app.key(KeyCode::Char('c'));
    assert!(
        !app.events && !app.asks && !app.home,
        "c goes to the jobs, not back to the asks"
    );
}

#[test]
fn the_feed_keeps_what_is_new_and_at_most_500() {
    let mut app = events_app(vec![
        ev(1, "ask", "cc", "pi", "q1"),
        ev(2, "ack", "pi", "cc", "a1"),
    ]);
    app.take_events(Ok(vec![
        ev(2, "ack", "pi", "cc", "a1"),
        ev(3, "notify", "cc", "pi", "n"),
    ]));
    assert_eq!(
        held_seqs(&app),
        vec![Some(1), Some(2), Some(3)],
        "what is held already is not taken twice"
    );
    app.take_events(Ok(vec![ev(4, "chat_turn_delta", "", "", "partial")]));
    assert_eq!(held_seqs(&app).len(), 3, "streaming deltas stay out");
    app.take_events(Ok((10..610)
        .map(|s| ev(s, "notify", "cc", "pi", "n"))
        .collect()));
    let held = held_seqs(&app);
    assert_eq!(held.len(), 500);
    assert_eq!(held.first(), Some(&Some(110)), "the oldest went first");
}

#[test]
fn a_hub_that_numbers_no_events_is_read_whole() {
    let bare = |text: &str| model::Event {
        kind: "notify".into(),
        from_peer: "cc".into(),
        to_peer: "pi".into(),
        message: text.into(),
        ..Default::default()
    };
    let mut app = events_app(vec![bare("one"), bare("two")]);
    app.take_events(Ok(vec![bare("two"), bare("three")]));
    let texts: Vec<String> = app
        .feed
        .iter()
        .filter_map(|h| match h {
            super::feed::Held::Event(_, e) => Some(e.message.clone()),
            super::feed::Held::Restart(_) => None,
        })
        .collect();
    assert_eq!(texts, ["two", "three"]);
}

#[test]
fn the_rows_read_time_kind_route_and_text() {
    let fan = |seq, to: &str| model::Event {
        id: "bc-1".into(),
        ..ev(seq, "broadcast", CC, to, "deploying at ten")
    };
    let chat = model::Event {
        seq: Some(7),
        at: Some(1007),
        kind: "chat".into(),
        peer: CC.into(),
        role: "user".into(),
        text: "hello there".into(),
        ..Default::default()
    };
    let mut app = events_app(vec![
        ev(1, "ask", CC, PI, "list the modules\nsecond line"),
        ev(2, "ack", PI, CC, "seven modules"),
        ev(3, "notify", "amesh", CC, "job j1 is stalled"),
        fan(4, CODEX),
        fan(5, PI),
        fan(6, PI2),
        chat,
    ]);
    let rows = |app: &mut App, cols| -> Vec<String> {
        screen_text(app, cols, 40)
            .into_iter()
            .filter(|l| l.starts_with("00:16"))
            .collect()
    };
    assert_eq!(
        rows(&mut app, 100),
        [
            "00:16:41 ask    claude-code▸pi       list the modules",
            "00:16:42 ack ●  pi▸claude-code       seven modules",
            "00:16:43 hub    ▸claude-code         job j1 is stalled",
            "00:16:44 bcast  claude-code▸3 peers  deploying at ten",
            "00:16:47 chat   claude-code (user)   hello there",
        ]
    );
    assert_eq!(
        rows(&mut app, 46),
        [
            "00:16 ask    claude-code▸pi",
            "00:16 ack ●  pi▸claude-code",
            "00:16 hub    ▸claude-code",
            "00:16 bcast  claude-code▸3 peers",
            "00:16 chat   claude-code (user)",
        ],
        "a narrow pane gives the minutes and the short names in one column, and leaves out a text \
         it could only cut to a stub"
    );
}

#[test]
fn an_ack_says_how_it_ended_without_the_hubs_marker() {
    use layout::Tone;
    let said = |mut e: model::Event, failed: bool| {
        e.failed = Some(failed);
        e
    };
    let mut app = events_app(vec![
        ev(1, "ack", CODEX, CC, "2 findings, both fixed"),
        ev(2, "ack", PI2, CC, "[failed] build broke"),
        ev(3, "notify", CC, PI, "[failed] is only an ack's"),
        ev(4, "ack", PI, CC, "[failed]"),
        said(ev(5, "ack", PI, CC, "amesh: job j1 set to failed"), true),
        said(ev(6, "ack", PI, CC, "[failed]x is my own text"), false),
    ]);
    let rows: Vec<String> = screen_text(&mut app, 100, 40)
        .into_iter()
        .filter(|l| l.starts_with("00:16"))
        .collect();
    assert_eq!(
        rows,
        [
            "00:16:41 ack ●  codex▸claude-code  2 findings, both fixed",
            "00:16:42 ack ×  pi-2▸claude-code   build broke",
            "00:16:43 notify claude-code▸pi     [failed] is only an ack's",
            "00:16:44 ack ×  pi▸claude-code",
            "00:16:45 ack ×  pi▸claude-code     amesh: job j1 set to failed",
            "00:16:46 ack ●  pi▸claude-code     [failed]x is my own text",
        ]
    );
    let items = super::feed::items(&app.feed);
    let rows = super::feed::rows(&items, 46, "", false);
    assert_eq!(cell(&rows[0], "●"), Tone::Done);
    assert_eq!(cell(&rows[1], "×"), Tone::Fail);
    let card = |i: usize| super::feed::card(&items[i], 46, Fit::Whole, None);
    for (i, mark) in [
        (0, ('●', Tone::Done)),
        (1, ('×', Tone::Fail)),
        (2, ('•', Tone::Dim)),
    ] {
        let corner = card(i).rows[&0][&1];
        assert_eq!(
            (corner.ch, corner.tone),
            mark,
            "only an ack says how it ended"
        );
    }
    let failed = card(1).text();
    assert!(
        failed.contains("│ text    build broke ") && !failed.contains("[failed]"),
        "{failed}"
    );
}

#[test]
fn the_card_under_the_list_shortens_names_and_the_full_card_keeps_them_whole() {
    let chat = model::Event {
        peer: CC.into(),
        role: "user".into(),
        ..ev(2, "chat", "", "", "hello there")
    };
    let mut app = events_app(vec![ev(1, "notify", CC, PI, "ask a3a0 waits"), chat]);
    let names = |app: &mut App| -> Vec<String> {
        screen_text(app, 46, 24)
            .into_iter()
            .filter(|l| l.starts_with("│ from") || l.starts_with("│ to"))
            .collect()
    };
    assert_eq!(
        names(&mut app),
        ["│ from    claude-code (user)                 │"]
    );
    app.key(KeyCode::Char('k'));
    assert_eq!(
        names(&mut app),
        [
            "│ from    claude-code                        │",
            "│ to      pi                                 │",
        ]
    );
    app.key(KeyCode::Enter);
    assert_eq!(
        names(&mut app),
        [
            "│ from    openhitls-sm2-opt-claude-code      │",
            "│ to      openhitls-sm2-opt-pi               │",
        ],
        "the full card fits the pane and still names them whole"
    );
}

#[test]
fn the_selection_follows_the_newest_until_moved_and_holds_its_event() {
    let mut app = events_app(
        (1..=3)
            .map(|s| ev(s, "notify", "cc", "pi", &format!("n{s}")))
            .collect(),
    );
    let card = |app: &mut App| {
        screen_text(app, 100, 40)
            .into_iter()
            .find(|l| l.contains(" notify · "))
            .unwrap()
    };
    assert!(card(&mut app).contains("00:16:43"), "the newest first");
    app.key(KeyCode::Char('k'));
    assert!(card(&mut app).contains("00:16:42"));
    app.take_events(Ok(vec![ev(4, "notify", "cc", "pi", "n4")]));
    assert!(
        card(&mut app).contains("00:16:42"),
        "a new event leaves the selection where it is"
    );
    app.key(KeyCode::Char('j'));
    app.key(KeyCode::Char('j'));
    assert_eq!(app.ev_sel, None, "the newest row follows the newest again");
    app.take_events(Ok(vec![ev(5, "notify", "cc", "pi", "n5")]));
    assert!(card(&mut app).contains("00:16:45"));
    app.key(KeyCode::Char('k'));
    app.take_events(Ok((10..510)
        .map(|s| ev(s, "notify", "cc", "pi", "n"))
        .collect()));
    assert_eq!(
        app.ev_sel, None,
        "a selection that went with the oldest follows the newest"
    );
}

#[test]
fn a_hub_restart_leaves_a_divider_and_takes_the_new_ring() {
    let mut app = events_app(vec![ev(5, "ask", "cc", "pi", "before")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap.clone()));
    app.apply(Ok(snap.clone()));
    app.take_events(Ok(vec![ev(1, "ask", "cc", "pi", "after")]));
    app.apply(Ok(snap));
    let lines = screen_text(&mut app, 100, 40);
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle}:\n{}", lines.join("\n")))
    };
    let divider = "─── hub restarted 00:20:00 · ";
    assert!(at("before") < at(divider));
    assert!(
        at(divider) < at("after"),
        "the new ring comes in even with lower numbers"
    );
    assert_eq!(
        lines.iter().filter(|l| l.contains("hub restarted")).count(),
        1,
        "one divider per restart"
    );
    assert!(
        screen_text(&mut app, 46, 40)
            .iter()
            .any(|l| l.starts_with("─── hub restarted 00:20 ─")),
        "a narrow pane gives the divider's minutes"
    );
}

#[test]
fn the_events_screen_says_so_when_empty_or_a_read_fails() {
    let mut app = events_app(Vec::new());
    let lines = screen_text(&mut app, 100, 40);
    assert!(
        lines.iter().any(|l| l == "no events yet"),
        "an empty screen says so: {lines:?}"
    );
    app.take_events(Ok(vec![ev(1, "ask", "cc", "pi", "q")]));
    app.take_events(Err("hub unreachable".into()));
    let lines = screen_text(&mut app, 100, 40);
    assert!(
        lines.iter().any(|l| l == "! events: hub unreachable"),
        "a failed read says so: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("cc▸pi")),
        "the list stays: {lines:?}"
    );
    app.take_events(Ok(Vec::new()));
    assert!(
        !screen_text(&mut app, 100, 40)
            .iter()
            .any(|l| l.contains("! events")),
        "a good read clears it"
    );
}

#[test]
fn enter_shows_the_whole_event_and_esc_goes_back() {
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut events: Vec<model::Event> =
        (1..=30).map(|s| ev(s, "notify", "cc", "pi", "n")).collect();
    events.push(ev(31, "notify", "cc", "pi", &long));
    let mut app = events_app(events);
    assert!(
        !screen_text(&mut app, 100, 30)
            .iter()
            .any(|l| l.contains("line 40")),
        "the card under the list cuts the text"
    );
    app.key(KeyCode::Enter);
    assert!(app.full);
    app.key(KeyCode::Char('G'));
    assert!(
        screen_text(&mut app, 100, 30)
            .iter()
            .any(|l| l.contains("line 40")),
        "the full card scrolls to the end of the text"
    );
    app.key(KeyCode::Esc);
    assert!(!app.full);
}

#[test]
fn the_fetch_reads_events_only_while_their_screen_is_up() {
    let mut app = app_with(with_lone_asks(s1()));
    screen_text(&mut app, 100, 40);
    assert!(!app.wanted().events);
    app.key(KeyCode::Char('e'));
    assert_eq!(
        app.wanted(),
        super::Want {
            detail: None,
            events: true
        },
        "the events screen wants no card text"
    );
    app.key(KeyCode::Char('a'));
    assert!(!app.wanted().events, "the asks screen reads no events");
}

fn unnumbered(text: &str) -> model::Event {
    model::Event {
        kind: "notify".into(),
        from_peer: "cc".into(),
        to_peer: "pi".into(),
        message: text.into(),
        ..Default::default()
    }
}

#[test]
fn an_empty_ring_from_a_hub_that_numbers_no_events_clears_the_list() {
    let mut app = events_app(vec![unnumbered("old")]);
    app.take_events(Ok(Vec::new()));
    assert!(
        app.feed.is_empty(),
        "a hub that sends its whole ring each time sent an empty one"
    );
}

#[test]
fn the_events_screen_says_when_its_hub_numbers_no_events() {
    let mut app = events_app(vec![unnumbered("one"), unnumbered("two")]);
    let lines = screen_text(&mut app, 100, 40);
    assert!(
        lines.iter().any(|l| l
            == "this hub numbers no events, so only the newest opens; restart it on the current amesh"),
        "the screen says why j/k pick nothing: {lines:?}"
    );
    let mut app = events_app(vec![ev(1, "notify", "cc", "pi", "n")]);
    assert!(
        !screen_text(&mut app, 100, 40)
            .iter()
            .any(|l| l.contains("numbers no events")),
        "a hub that numbers them gets no such line"
    );
}

#[test]
fn a_hub_that_numbers_no_events_keeps_what_came_before_its_restart() {
    let mut app = events_app(vec![unnumbered("before")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap));
    app.take_events(Ok(vec![unnumbered("after")]));
    let lines = screen_text(&mut app, 100, 40);
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle}:\n{}", lines.join("\n")))
    };
    let divider = "hub restarted 00:20:00";
    assert!(
        at("before") < at(divider) && at(divider) < at("after"),
        "the ring read whole after a restart replaces only what came after it"
    );
}

#[test]
fn each_restart_gets_its_divider_even_with_no_events_between() {
    let mut app = events_app(vec![ev(5, "ask", "cc", "pi", "before")]);
    for (epoch, at) in [("e2", 1200), ("e3", 1260)] {
        let mut snap = app.snap.clone().unwrap().view;
        snap.hub_epoch = epoch.into();
        snap.captured_at = at;
        app.apply(Ok(snap));
    }
    let lines = screen_text(&mut app, 100, 40);
    for divider in ["hub restarted 00:20:00", "hub restarted 00:21:00"] {
        assert!(
            lines.iter().any(|l| l.contains(divider)),
            "each restart gets its own divider: {divider} missing in {lines:?}"
        );
    }
}

#[test]
fn a_hub_upgraded_to_number_its_events_is_read_by_seq_again() {
    let mut app = events_app(vec![unnumbered("old")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap));
    app.take_events(Ok(vec![ev(10, "notify", "cc", "pi", "fresh-ring")]));
    app.take_events(Ok(Vec::new()));
    let lines = screen_text(&mut app, 100, 40);
    assert!(
        lines.iter().any(|l| l.contains("fresh-ring")),
        "an empty read from a hub that numbers its events keeps what it has: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("numbers no events")),
        "the unnumbered events of an earlier ring say nothing about this one: {lines:?}"
    );
}

#[test]
fn asks_from_one_sender_with_one_text_in_one_second_are_one_row() {
    let mut app = events_app(fan_events(1));
    let rows: Vec<String> = screen_text(&mut app, 46, 24)
        .into_iter()
        .filter(|l| l.starts_with("00:16"))
        .collect();
    assert_eq!(rows, ["00:16 ask    claude-code▸3 peers"]);
    let ask = |seq, at, from: &str, text: &str| model::Event {
        at,
        ..ev(seq, "ask", from, PI, text)
    };
    let sizes = |events: Vec<model::Event>| -> Vec<usize> {
        let held: Vec<super::feed::Held> = events
            .into_iter()
            .map(|e| super::feed::Held::Event(0, Box::new(e)))
            .collect();
        super::feed::items(&held)
            .iter()
            .map(|item| match item {
                super::feed::Item::Event(_, group) => group.len(),
                super::feed::Item::Restart(_) => 0,
            })
            .collect()
    };
    let one = || ask(1, Some(5), CC, "q");
    assert_eq!(
        sizes(vec![one(), ask(2, Some(6), CC, "q")]),
        [1, 1],
        "another second"
    );
    assert_eq!(
        sizes(vec![one(), ask(2, Some(5), CODEX, "q")]),
        [1, 1],
        "another sender"
    );
    assert_eq!(
        sizes(vec![one(), ask(2, Some(5), CC, "r")]),
        [1, 1],
        "another text"
    );
    assert_eq!(
        sizes(vec![
            one(),
            ev(2, "notify", CC, PI, "n"),
            ask(3, Some(5), CC, "q")
        ]),
        [1, 1, 1],
        "an event between"
    );
    assert_eq!(
        sizes(vec![ask(1, None, CC, "q"), ask(2, None, CC, "q")]),
        [1, 1],
        "no second to share"
    );
}

#[test]
fn an_asks_card_lists_each_recipient_as_the_asks_screen_orders_them() {
    let gone = model::Event {
        seq: Some(1),
        to_peer: "openhitls-sm2-opt-gemini".into(),
        correlation_id: "ask-0000beef".into(),
        ..fan_events(2).remove(0)
    };
    let mut events = vec![gone];
    events.extend(fan_events(2));
    let mut app = events_app(events);
    let snap = fan_out();
    app.apply(Ok(snap.clone()));
    let to = |app: &mut App, rows| -> Vec<String> {
        let lines = screen_text(app, 46, rows);
        let from = lines.iter().position(|l| l.starts_with("│ from")).unwrap();
        let text = lines.iter().position(|l| l.starts_with("│ text")).unwrap();
        lines[from + 1..text].to_vec()
    };
    assert_eq!(
        to(&mut app, 24),
        [
            "│ to      pi     ◆ IDLE 1m     a3a0          │",
            "│         pi-2   ◆ working 3m  1223          │",
            "│         codex  ● acked ok    a904          │",
            "│         gemini                             │",
        ],
        "the asks screen's order, and a name alone for an ask the snapshot lost"
    );
    let items = super::feed::items(&app.feed);
    let card = super::feed::card(&items[0], 46, Fit::Rows(0), Some(&snap));
    let tone = |needle: &str, offset: usize| {
        let (r, c) = find(&card, needle);
        tone_at(&card, r, c + offset)
    };
    assert_eq!(tone("◆ IDLE", 0), Tone::Run);
    assert_eq!(tone("◆ IDLE", 2), Tone::PillWait);
    assert_eq!(
        tone("IDLE 1m", 7),
        Tone::Plain,
        "the pill ends with its word"
    );
    assert!(
        card.text().lines().next().unwrap().contains(" · 4 peers "),
        "{}",
        card.text()
    );
    assert_eq!(tone("a3a0", 0), Tone::Dim);
    assert_eq!(tone("gemini", 0), Tone::Soft);
    assert_eq!(
        tone("a open in asks", 0),
        Tone::Near,
        "the a key reads as a key"
    );
    app.key(KeyCode::Enter);
    assert_eq!(
        to(&mut app, 40),
        [
            "│ to      openhitls-sm2-opt-pi               │",
            "│         ◆ IDLE 1m     a3a0                 │",
            "│         openhitls-sm2-opt-pi-2             │",
            "│         ◆ working 3m  1223                 │",
            "│         openhitls-sm2-opt-codex            │",
            "│         ● acked ok    a904                 │",
            "│         openhitls-sm2-opt-gemini           │",
        ],
        "whole names too wide for the state beside them put it under them"
    );
    let far = model::Event {
        to_peer: "a-very-long-recipient-name-with-a-distinctive-ending".into(),
        correlation_id: "ask-0000feed".into(),
        ..fan_events(2).remove(0)
    };
    let held = vec![super::feed::Held::Event(0, Box::new(far))];
    let card = super::feed::card(&super::feed::items(&held)[0], 30, Fit::Whole, Some(&snap));
    assert!(
        card.text()
            .replace(['│', ' ', '\n'], "")
            .contains("a-very-long-recipient-name-with-a-distinctive-ending"),
        "a name wider than the card wraps whole:\n{}",
        card.text()
    );
    let mut hand = fan_out();
    hand.asks[2].state = Some(crate::wire::AskState::Closed {
        outcome: crate::wire::AskOutcome::HandOk,
        failed_effective: false,
    });
    let held: Vec<super::feed::Held> = fan_events(2)
        .into_iter()
        .map(|e| super::feed::Held::Event(0, Box::new(e)))
        .collect();
    let card = super::feed::card(&super::feed::items(&held)[0], 30, Fit::Whole, Some(&hand)).text();
    assert!(
        ["a3a0", "1223", "a904"].iter().all(|id| card.contains(id)),
        "a long state is cut before an id is lost:\n{card}"
    );
}

#[test]
fn a_opens_the_asks_screen_on_the_selected_ask_where_it_lists_it() {
    let mut events = fan_events(1);
    events.extend([
        model::Event {
            correlation_id: "ask-a904f2d6".into(),
            ..ev(4, "ack", CODEX, CC, "2 findings, both fixed")
        },
        model::Event {
            correlation_id: "ask-0000beef".into(),
            ..ev(5, "ask", CC, PI, "an ask the hub no longer holds")
        },
    ]);
    let mut app = events_app(events);
    app.apply(Ok(busy_asks()));
    let keys = |app: &mut App| {
        screen_text(app, 46, 24)
            .into_iter()
            .find(|l| l.contains("full card"))
            .unwrap()
    };
    app.key(KeyCode::Char('k'));
    app.key(KeyCode::Char('k'));
    assert!(
        keys(&mut app).starts_with("│ a open in asks · ⏎ full card "),
        "{}",
        keys(&mut app)
    );
    app.key(KeyCode::Char('a'));
    assert!(app.asks && !app.events);
    assert_eq!(
        app.ask_sel.as_deref(),
        Some("ask-a3a07b51"),
        "the fan-out's ask the asks screen lists first, not that screen's first row"
    );
    app.key(KeyCode::Char('e'));
    assert!(
        keys(&mut app).starts_with("│ ⏎ full card "),
        "an ask the asks screen does not list offers no a: {}",
        keys(&mut app)
    );
    app.key(KeyCode::Char('a'));
    assert_eq!(
        app.ask_sel.as_deref(),
        Some("ask-a3a07b51"),
        "the asks screen keeps its own selection"
    );
    app.key(KeyCode::Char('e'));
    app.key(KeyCode::Char('k'));
    assert!(
        keys(&mut app).starts_with("│ ⏎ full card "),
        "an ack is not an ask: {}",
        keys(&mut app)
    );
    app.key(KeyCode::Char('a'));
    assert_eq!(app.ask_sel.as_deref(), Some("ask-a3a07b51"));
}

#[test]
fn a_notice_from_the_hub_reads_hub_and_names_only_its_recipient() {
    let mut app = events_app(vec![
        ev(1, "notify", "amesh", CC, "job j1 is stalled"),
        ev(2, "notify", "amesh-pi", CC, "a peer named like the hub"),
        ev(3, "broadcast", "amesh", CC, "not a notice"),
    ]);
    let rows: Vec<String> = screen_text(&mut app, 100, 40)
        .into_iter()
        .filter(|l| l.starts_with("00:16"))
        .collect();
    assert_eq!(
        rows,
        [
            "00:16:41 hub    ▸claude-code          job j1 is stalled",
            "00:16:42 notify amesh-pi▸claude-code  a peer named like the hub",
            "00:16:43 bcast  amesh▸claude-code     not a notice",
        ]
    );
    let items = super::feed::items(&app.feed);
    let head = super::feed::card(&items[0], 46, Fit::Rows(0), None).text();
    assert!(head.starts_with("┌• hub · "), "{head}");
}

#[test]
fn event_rows_name_peers_soft_and_colour_only_asks_hub_notices_and_ack_glyphs() {
    use layout::Tone::{Dim, Done, Fail, Near, Plain, Soft, Wait};
    let app = events_app(vec![
        ev(1, "ask", CC, PI, "q"),
        ev(2, "ack", PI, CC, "a"),
        model::Event {
            failed: Some(true),
            ..ev(3, "ack", PI, CC, "broke")
        },
        ev(4, "notify", "amesh", CC, "job j1 is stalled"),
        ev(5, "notify", CC, PI, "n"),
        ev(6, "broadcast", CC, PI, "b"),
    ]);
    let items = super::feed::items(&app.feed);
    let rows = super::feed::rows(&items, 46, "openhitls-sm2-opt-", false);
    let tones = |i: usize| -> Vec<layout::Tone> { rows[i].iter().map(|(_, t)| *t).collect() };
    assert_eq!(tones(0), [Dim, Near, Soft, Plain], "{:?}", rows[0]);
    assert_eq!(tones(1), [Dim, Dim, Done, Soft, Plain], "{:?}", rows[1]);
    assert_eq!(
        (rows[1][1].0.as_str(), rows[1][2].0.as_str()),
        ("ack ", "●  "),
        "an ack keeps its six columns"
    );
    assert_eq!(tones(2), [Dim, Dim, Fail, Soft, Plain], "{:?}", rows[2]);
    assert_eq!(tones(3), [Dim, Wait, Soft, Plain], "{:?}", rows[3]);
    assert_eq!(tones(4), [Dim, Dim, Soft, Plain], "{:?}", rows[4]);
    assert_eq!(tones(5), [Dim, Dim, Soft, Plain], "{:?}", rows[5]);
}

#[test]
fn the_restart_row_is_a_rule_across_the_pane() {
    let mut app = events_app(vec![ev(5, "ask", "cc", "pi", "before")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap));
    app.take_events(Ok(vec![ev(1, "ask", "cc", "pi", "after")]));
    let row = |app: &mut App, cols| {
        screen_text(app, cols, 24)
            .into_iter()
            .find(|l| l.contains("hub restarted"))
            .unwrap()
    };
    let narrow = format!("─── hub restarted 00:20 {}", "─".repeat(22));
    assert_eq!(row(&mut app, 46), narrow);
    assert_eq!(
        row(&mut app, 100),
        format!(
            "─── hub restarted 00:20:00 · the rows above came from the previous hub process {}",
            "─".repeat(21)
        )
    );
    let items = super::feed::items(&app.feed);
    assert_eq!(
        super::feed::rows(&items, 46, "", false)[1],
        [(narrow, Tone::Line)],
        "the whole row is the rule's colour"
    );
    assert_eq!(
        super::feed::rows(&items, 20, "", false)[1],
        [("─── hub restarted 0…".to_string(), Tone::Line)],
        "a pane too narrow for even the minutes cuts the row"
    );
}

#[test]
fn the_selection_keeps_to_its_ring_when_the_new_ring_repeats_its_seqs() {
    let mut app = events_app(vec![
        ev(5, "notify", "cc", "pi", "old five"),
        ev(6, "notify", "cc", "pi", "old six"),
    ]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap));
    app.take_events(Ok(vec![
        ev(5, "notify", "cc", "pi", "new five"),
        ev(6, "notify", "cc", "pi", "new six"),
    ]));
    let text = |app: &mut App| {
        screen_text(app, 100, 40)
            .into_iter()
            .find(|l| l.starts_with("│ from"))
            .unwrap()
    };
    app.key(KeyCode::Char('k'));
    assert_eq!(app.ev_sel, Some((1, 5)));
    assert!(text(&mut app).contains("new five"), "{}", text(&mut app));
    app.key(KeyCode::Char('k'));
    assert!(text(&mut app).contains("old six"), "{}", text(&mut app));
    app.key(KeyCode::Char('j'));
    assert!(text(&mut app).contains("new five"), "{}", text(&mut app));
    /* the old ring goes, and its restart with it */
    app.take_events(Ok((10..508)
        .map(|s| ev(s, "notify", "cc", "pi", "n"))
        .collect()));
    assert!(!app
        .feed
        .iter()
        .any(|h| matches!(h, super::feed::Held::Restart(_))));
    assert!(
        text(&mut app).contains("new five"),
        "the selection stays on its event, now in the first ring: {}",
        text(&mut app)
    );
}

#[test]
fn the_selection_never_lands_on_another_hub_run() {
    let restart = |app: &mut App, epoch: &str| {
        let mut snap = app.snap.clone().unwrap().view;
        snap.hub_epoch = epoch.into();
        snap.captured_at = 1200;
        app.apply(Ok(snap));
    };
    let shown = |app: &mut App| {
        screen_text(app, 100, 40)
            .into_iter()
            .find(|l| l.starts_with("│ from"))
            .unwrap()
    };
    let mut app = events_app(vec![
        ev(5, "notify", "cc", "pi", "old five"),
        ev(6, "notify", "cc", "pi", "old six"),
    ]);
    app.key(KeyCode::Char('k'));
    assert_eq!(app.ev_sel, Some((0, 5)));
    restart(&mut app, "e2");
    app.take_events(Ok((5..505)
        .map(|s| ev(s, "notify", "cc", "pi", "new"))
        .collect()));
    assert_eq!(
        app.ev_sel, None,
        "its ring went with the oldest rows, so the newest is followed again"
    );
    let mut app = events_app(vec![ev(1, "notify", "cc", "pi", "e1 one")]);
    for epoch in ["e2", "e3"] {
        restart(&mut app, epoch);
        app.take_events(Ok(vec![
            ev(5, "notify", "cc", "pi", &format!("{epoch} five")),
            ev(6, "notify", "cc", "pi", &format!("{epoch} six")),
        ]));
    }
    app.key(KeyCode::Char('k'));
    assert!(
        shown(&mut app).contains("e3 five"),
        "two restarts in one second keep their rows apart: {}",
        shown(&mut app)
    );
}

#[test]
fn the_events_card_keeps_its_keys_however_short_the_pane() {
    let mut events: Vec<model::Event> = (1..=20).map(|s| ev(s, "notify", CC, PI, "n")).collect();
    let fan = fan_events(21);
    let lost = |seq, to: &str| model::Event {
        seq: Some(seq),
        to_peer: to.into(),
        correlation_id: format!("ask-{seq:08}"),
        ..fan[0].clone()
    };
    events.extend(fan.iter().cloned());
    events.extend([lost(24, "x"), lost(25, "y")]);
    let mut app = events_app(events);
    app.apply(Ok(fan_out()));
    let lines = screen_text(&mut app, 46, 24).join("\n");
    assert!(
        lines.contains("\n│ from    claude-code "),
        "the list gives way to the card down to all it needs:\n{lines}"
    );
    for (rows, why) in [
        (24, "the card the list leaves is shorter than all it holds"),
        (14, "a short pane"),
        (8, "a pane with no room for the card's frame"),
    ] {
        let lines = screen_text(&mut app, 46, rows);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("a open in asks · ⏎ full card")),
            "{why}:\n{}",
            lines.join("\n")
        );
    }
}

#[test]
fn the_events_card_ends_in_its_keys_under_a_text_it_marks_as_cut() {
    let long = (1..=30)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = events_app(vec![ev(1, "notify", CC, PI, &long)]);
    let card = |app: &mut App, cols| -> Vec<String> {
        let lines = screen_text(app, cols, 40);
        let top = lines.iter().position(|l| l.starts_with('┌')).unwrap();
        let end = lines.iter().position(|l| l.starts_with('└')).unwrap();
        lines[top..end].to_vec()
    };
    let compact = card(&mut app, 46);
    assert_eq!(
        compact[compact.len() - 2..],
        [
            format!("│         line 28…{}│", " ".repeat(27)),
            format!("│ ⏎ full card{}│", " ".repeat(32)),
        ]
    );
    app.key(KeyCode::Enter);
    let full = card(&mut app, 100);
    assert!(
        full.last().unwrap().starts_with("│ esc back "),
        "the full card's keys come under its text:\n{}",
        full.join("\n")
    );
}

#[test]
fn a_failed_close_crosses_the_arrow_and_holds_it_still() {
    use crate::wire::{AskOutcome, AskState};
    /* the hub's facts decide: closed a minute ago by the stamp, yet just now by the hub's
    word, and failed by failed_effective, not the raw flag */
    let closed = |by: &str, failed: bool, outcome: AskOutcome, effective: bool| {
        let mut snap = proto_s1();
        let now = snap.captured_at;
        let ask = snap.asks.iter_mut().find(|a| a.correlation_id == "ask-5b1");
        let ask = ask.unwrap();
        ask.closed_at = Some(now - 60);
        ask.closed_by = Some(by.into());
        ask.failed = failed;
        ask.state = Some(AskState::Closed {
            outcome,
            failed_effective: effective,
        });
        ask.closed_just_now = Some(true);
        snap
    };
    let cases = [
        ("hub", true, AskOutcome::Hub),
        ("hub", false, AskOutcome::Hub),
        ("recipient", true, AskOutcome::AckedFailed),
        ("hand", true, AskOutcome::HandFailed),
    ];
    for (by, failed, outcome) in cases {
        let frame = model::Frame {
            view: closed(by, failed, outcome, true),
            fresh: true,
        };
        let ask = frame.ask(Some("ask-5b1")).unwrap();
        assert_eq!(
            layout::arrow(&frame, ask, false),
            ("─×─".into(), layout::Tone::Fail),
            "a failed close by {by} (raw failed {failed}) is no answer walking back"
        );
        let mut app = app_with(closed(by, failed, outcome, true));
        app.full = true;
        app.sel = Some("func".into());
        for tick in [0, 2, 4] {
            let line = line_with(&mut app, 100, tick, "5b1 ");
            assert!(
                line.contains("5b1 cc─×─codex "),
                "{by}, tick {tick}: {line}"
            );
        }
    }
    let frame = model::Frame {
        view: closed("hand", false, AskOutcome::HandOk, false),
        fresh: true,
    };
    let ask = frame.ask(Some("ask-5b1")).unwrap();
    assert_eq!(
        layout::arrow(&frame, ask, false),
        ("─◂─".into(), layout::Tone::Back),
        "an operator's good ack goes back to the sender like the recipient's"
    );
}

#[test]
fn a_worker_idle_since_before_the_ask_gets_time_to_pick_it_up() {
    use layout::Tone;
    /* synth's worker w4 has been idle since 880 */
    let synth = |captured: u64, opened: u64, progress: crate::wire::Progress| {
        let mut snap = busy();
        snap.captured_at = captured;
        let ask = snap
            .asks
            .iter_mut()
            .find(|a| a.correlation_id == "ask-synth");
        ask.unwrap().opened_at = Some(opened);
        set_progress(&mut snap, "synth", progress, false);
        snap
    };
    let card = |snap: &Snapshot| {
        let (chain, num) = numbered(snap);
        let view = View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "synth",
        };
        flat(&layout::card(&view, "synth", 46, None, Fit::Whole).text())
    };
    let just_sent = synth(
        1000,
        995,
        crate::wire::Progress::Pending { since: Some(995) },
    );
    let text = card(&just_sent);
    assert!(
        text.contains("worker w4 · now IDLE") && !text.contains("IDLE!"),
        "{text}"
    );
    let text = card(&synth(
        1020,
        995,
        crate::wire::Progress::Idle {
            since: 995,
            why: crate::wire::IdleWhy::NotPickedUp,
        },
    ));
    assert!(
        text.contains("why w4: not picked up · 25s worker w4 IDLE"),
        "{text}"
    );
    let text = card(&synth(
        1000,
        870,
        crate::wire::Progress::Idle {
            since: 880,
            why: crate::wire::IdleWhy::TurnEnded,
        },
    ));
    assert!(
        text.contains("why w4 ended its turn 2m ago worker w4 IDLE"),
        "{text}"
    );
    let tie = synth(
        1010,
        1000,
        crate::wire::Progress::Idle {
            since: 1000,
            why: crate::wire::IdleWhy::AskStillOpen,
        },
    );
    let text = card(&tie);
    assert!(
        text.contains("why w4: ask still open · 10s worker w4 IDLE"),
        "idle in the second the ask opened: {text}"
    );
    use crate::wire::{AskAction, AskState, IdleWhy, Progress};
    let scene = |captured: u64, opened: u64, state: AskState, nudge: bool| {
        let actions = if nudge {
            vec![AskAction::Nudge { to: PI.into() }]
        } else {
            vec![]
        };
        with_asks(
            Snapshot {
                captured_at: captured,
                ..Default::default()
            },
            vec![stamp(
                lone("ask-x", CC, PI, Some(opened)),
                state,
                actions,
                None,
            )],
            vec![doing(PI, "idle", 880, None)],
        )
    };
    let state = |snap: &Snapshot| super::asks::state(snap, &snap.asks[0]);
    let ask_card = |snap: &Snapshot| {
        flat(&super::asks::card(&as_frame(snap), &snap.asks[0], 1, 46, None, Fit::Whole).text())
    };
    let early = scene(
        1000,
        995,
        AskState::Open {
            progress: Progress::Pending { since: Some(995) },
        },
        false,
    );
    assert_eq!(state(&early), ("waiting 5s".into(), Tone::Run));
    assert!(
        !ask_card(&early).contains("IDLE")
            && !ask_card(&early).contains("why")
            && !ask_card(&early).contains("nudge"),
        "{}",
        ask_card(&early)
    );
    let late = scene(
        1020,
        995,
        AskState::Open {
            progress: Progress::Idle {
                since: 995,
                why: IdleWhy::NotPickedUp,
            },
        },
        true,
    );
    assert_eq!(state(&late), ("IDLE 25s".into(), Tone::PillWait));
    assert!(
        ask_card(&late).contains("why openhitls-sm2-opt-pi: not picked up · 25s")
            && ask_card(&late).contains("nudge"),
        "{}",
        ask_card(&late)
    );
    let ended = scene(
        1000,
        980,
        AskState::Open {
            progress: Progress::Idle {
                since: 990,
                why: IdleWhy::TurnEnded,
            },
        },
        false,
    );
    assert_eq!(state(&ended), ("IDLE 10s".into(), Tone::PillWait));
    assert!(
        ask_card(&ended).contains("why openhitls-sm2-opt-pi ended its turn 10s ago"),
        "{}",
        ask_card(&ended)
    );
    let tie = scene(
        1010,
        1000,
        AskState::Open {
            progress: Progress::Idle {
                since: 1000,
                why: IdleWhy::AskStillOpen,
            },
        },
        false,
    );
    assert_eq!(state(&tie), ("IDLE 10s".into(), Tone::PillWait));
    assert!(
        ask_card(&tie).contains("why openhitls-sm2-opt-pi: ask still open · 10s"),
        "{}",
        ask_card(&tie)
    );
}

#[test]
fn a_stale_snapshot_holds_the_spinners_and_the_walking_arrow_still() {
    let frames = |app: &mut App| (0..6).map(|t| screen_at(app, 100, t)).collect::<Vec<_>>();
    /* frame 0 is the same either way */
    /* fresh for two seconds */
    let aged =
        |ms: u64| std::time::Instant::now().checked_sub(std::time::Duration::from_millis(ms));
    let check = |app: &mut App, what: &str| {
        app.last_ok = aged(1500);
        let live = frames(app);
        assert!(
            live.windows(2).any(|w| w[0] != w[1]),
            "{what}: a snapshot 1.5s old still moves"
        );
        app.last_ok = aged(2100);
        app.opts.anim = false;
        let still = screen_at(app, 100, 0);
        app.opts.anim = true;
        for (tick, frame) in frames(app).iter().enumerate().skip(1) {
            assert_eq!(
                frame, &still,
                "{what}, tick {tick}: a snapshot 2.1s old shows the --no-anim frame"
            );
        }
    };
    let mut walking = app_with(proto_s1());
    walking.sel = Some("upg".into());
    check(&mut walking, "jobs, arrow");
    let mut waiting = app_with(busy());
    waiting.sel = Some("fix".into());
    assert!(
        screen_at(&mut waiting, 100, 0)
            .iter()
            .any(|l| l.contains(" WAIT ")),
        "the jobs fixture shows WAIT"
    );
    check(&mut waiting, "jobs, WAIT!");
    let mut asks = app_with(with_asks(
        Snapshot {
            captured_at: 1000,
            ..Default::default()
        },
        vec![
            stamp(
                lone("ask-x", CC, PI, Some(900)),
                crate::wire::AskState::Open {
                    progress: crate::wire::Progress::Work { since: 900 },
                },
                vec![],
                None,
            ),
            stamp(
                lone("ask-y", CC, PI2, Some(900)),
                crate::wire::AskState::Open {
                    progress: crate::wire::Progress::Wait {
                        since: 950,
                        reason: Some("Bash".into()),
                    },
                },
                vec![],
                None,
            ),
        ],
        vec![
            doing(CC, "work", 900, None),
            doing(PI, "work", 900, None),
            doing(PI2, "wait", 950, Some("Bash")),
        ],
    ));
    asks.key(KeyCode::Char('a'));
    assert!(
        screen_at(&mut asks, 100, 0)
            .iter()
            .any(|l| l.contains("WAIT 50s")),
        "the asks fixture shows WAIT"
    );
    assert!(
        screen_at(&mut asks, 100, 4)
            .iter()
            .any(|l| l.contains("WAIT 50s")),
        "and holds it on the odd half second"
    );
    check(&mut asks, "asks");
}

fn ask_with(state: crate::wire::AskState, opened: Option<u64>) -> Ask {
    Ask {
        correlation_id: "ask-row".into(),
        from_peer: "amesh-claude-code".into(),
        to_peer_id: "amesh-pi".into(),
        open: matches!(state, crate::wire::AskState::Open { .. }),
        opened_at: opened,
        state: Some(state),
        ..Default::default()
    }
}

fn b3b_snap(ask: Ask, peers: Vec<Peer>) -> Snapshot {
    Snapshot {
        captured_at: 1000,
        asks: vec![ask],
        peers: peers.clone(),
        roster: peers,
        ..Default::default()
    }
}

fn b3b_row(ask: Ask, fresh: bool) -> Vec<(String, layout::Tone)> {
    let snap = b3b_snap(ask, vec![]);
    let mut frame = as_frame(&snap);
    frame.fresh = fresh;
    let order = super::asks::order(&snap);
    super::asks::rows(&frame, &order, 80).remove(0)
}

fn b3b_card(ask: Ask, peers: Vec<Peer>) -> String {
    let snap = b3b_snap(ask, peers);
    super::asks::card(&as_frame(&snap), &snap.asks[0], 1, 90, None, Fit::Whole).text()
}

fn cell(row: &[(String, layout::Tone)], text: &str) -> layout::Tone {
    row.iter()
        .find(|(t, _)| t.contains(text))
        .unwrap_or_else(|| panic!("no {text} in {row:?}"))
        .1
}

#[test]
fn b3b_glyph_open_is_run() {
    let row = b3b_row(
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
fn b3b_glyph_failed_is_fail() {
    let row = b3b_row(
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
fn b3b_glyph_closed_ok_is_done() {
    let row = b3b_row(
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
fn b3b_arrow_work_is_flow() {
    let row = b3b_row(
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
fn b3b_arrow_pending_is_dim() {
    let row = b3b_row(
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
fn b3b_arrow_stale_close_is_plain() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        },
        Some(820),
    );
    ask.closed_just_now = Some(true);
    let row = b3b_row(ask, false);
    assert_eq!(cell(&row, ">"), layout::Tone::Plain);
}

#[test]
fn b3b_arrow_just_failed_is_fail() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::Hub,
            failed_effective: true,
        },
        Some(820),
    );
    ask.closed_just_now = Some(true);
    let row = b3b_row(ask, true);
    assert_eq!(cell(&row, "─×─"), layout::Tone::Fail);
}

#[test]
fn b3b_arrow_just_ok_is_back() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        },
        Some(820),
    );
    ask.closed_just_now = Some(true);
    let row = b3b_row(ask, true);
    assert_eq!(cell(&row, "─◂─"), layout::Tone::Back);
}

#[test]
fn b3b_card_wait() {
    let text = b3b_card(
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

fn b3b_idle(why: crate::wire::IdleWhy) -> String {
    b3b_card(
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
fn b3b_card_idle_not_picked_up() {
    let text = b3b_idle(crate::wire::IdleWhy::NotPickedUp);
    assert!(
        text.contains("│ why     amesh-pi: not picked up · 1m "),
        "{text}"
    );
}

#[test]
fn b3b_card_idle_still_open() {
    let text = b3b_idle(crate::wire::IdleWhy::AskStillOpen);
    assert!(
        text.contains("│ why     amesh-pi: ask still open · 1m "),
        "{text}"
    );
}

#[test]
fn b3b_card_idle_turn_ended() {
    let text = b3b_idle(crate::wire::IdleWhy::TurnEnded);
    assert!(
        text.contains("│ why     amesh-pi ended its turn 1m ago "),
        "{text}"
    );
}

#[test]
fn b3b_card_gone() {
    let mut from = doing("amesh-claude-code", "work", 1, None);
    from.liveness = Some(crate::wire::Liveness::Online);
    let text = b3b_card(
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
fn b3b_card_offline() {
    let text = b3b_card(
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

fn b3b_pending_to(kind: crate::wire::Liveness) -> String {
    let mut peer = doing("amesh-pi", "idle", 1, None);
    peer.liveness = Some(kind);
    b3b_card(
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
fn b3b_card_now_work() {
    let text = b3b_pending_to(crate::wire::Liveness::Work { since: 1 });
    assert!(text.contains("· now WORK"), "{text}");
}

#[test]
fn b3b_card_now_idle() {
    let text = b3b_pending_to(crate::wire::Liveness::Idle { since: 1 });
    assert!(text.contains("· now IDLE"), "{text}");
}

#[test]
fn b3b_card_now_wait() {
    let text = b3b_pending_to(crate::wire::Liveness::Wait {
        since: 1,
        reason: None,
    });
    assert!(text.contains("· now WAIT"), "{text}");
}

#[test]
fn b3b_card_now_offline() {
    let text = b3b_pending_to(crate::wire::Liveness::Offline);
    assert!(text.contains("· offline"), "{text}");
}

#[test]
fn b3b_card_online_is_empty() {
    let text = b3b_pending_to(crate::wire::Liveness::Online);
    assert!(!text.contains("· now"), "{text}");
}

#[test]
fn b3b_times_open() {
    let text = b3b_card(
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
fn b3b_times_unknown() {
    let text = b3b_card(
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
fn b3b_times_closed() {
    let mut ask = ask_with(
        crate::wire::AskState::Closed {
            outcome: crate::wire::AskOutcome::AckedOk,
            failed_effective: false,
        },
        Some(820),
    );
    ask.closed_at = Some(950);
    let text = b3b_card(ask, vec![]);
    assert!(
        text.contains("sent 00:13 · closed 00:15 · took 2m"),
        "{text}"
    );
}

#[test]
fn b3b_command_nudge() {
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
    let text = b3b_card(ask, vec![]);
    assert!(
        text.contains("amesh peer notify amesh-pi 'ask row waits for your ack'"),
        "{text}"
    );
}

#[test]
fn b3b_command_close_left() {
    let mut ask = ask_with(
        crate::wire::AskState::Open {
            progress: crate::wire::Progress::Gone,
        },
        Some(820),
    );
    ask.actions = Some(vec![crate::wire::AskAction::CloseLeft]);
    let text = b3b_card(ask, vec![]);
    assert!(
        text.contains("amesh peer ack ask-row --failed true --message 'recipient left'"),
        "{text}"
    );
}

#[test]
fn b3b_header_counts_hub_as_failed() {
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

fn b3c_card(job: Job, asks: Vec<Ask>) -> String {
    let snap = Snapshot {
        captured_at: 1000,
        jobs: vec![job],
        asks,
        ..Default::default()
    };
    let (chain, num) = numbered(&snap);
    flat(
        &layout::card(
            &View {
                snap: model::Frame {
                    view: snap,
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "job",
            },
            "job",
            80,
            None,
            Fit::Whole,
        )
        .text(),
    )
}

fn b3c_job(state: &str) -> Job {
    Job {
        job_id: "job".into(),
        title: "title".into(),
        state: state.into(),
        assigned_peer: Some("codex".into()),
        worker: Some("codex".into()),
        assignee_id: Some("codex".into()),
        ..Default::default()
    }
}

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

fn b3c_scene(job: Job, asks: Vec<Ask>, peers: Vec<Peer>) -> (Snapshot, layout::Grid) {
    let snap = Snapshot {
        captured_at: 1000,
        jobs: vec![job],
        asks,
        peers,
        ..Default::default()
    };
    let (chain, num) = numbered(&snap);
    let grid = layout::card(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "job",
        },
        "job",
        80,
        None,
        Fit::Whole,
    );
    (snap, grid)
}

fn b3c_text(job: Job, asks: Vec<Ask>, peers: Vec<Peer>) -> String {
    flat(&b3c_scene(job, asks, peers).1.text())
}

fn b3c_tone(grid: &layout::Grid, needle: char) -> layout::Tone {
    grid.rows
        .values()
        .flat_map(|row| row.values())
        .find(|cell| cell.ch == needle)
        .unwrap_or_else(|| panic!("no {needle}"))
        .tone
}

fn b3c_word_tone(grid: &layout::Grid, word: &str) -> layout::Tone {
    for row in grid.rows.values() {
        let cells: Vec<_> = row.values().collect();
        let text: String = cells.iter().map(|cell| cell.ch).collect();
        if let Some(at) = text.find(word) {
            let chars = text[..at].chars().count();
            return cells[chars].tone;
        }
    }
    panic!("no {word}");
}

fn b3c_ask(from: &str, to_id: &str, to_name: &str) -> Ask {
    Ask {
        correlation_id: "ask-1".into(),
        from_peer: from.into(),
        to_peer: to_name.into(),
        to_peer_id: to_id.into(),
        open: true,
        opened_at: Some(820),
        ..Default::default()
    }
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

fn sample() -> Snapshot {
    Snapshot {
        captured_at: 1000,
        jobs: vec![Job {
            job_id: "j".into(),
            title: "review".into(),
            state: "running".into(),
            assigned_peer: Some("codex".into()),
            worker: Some("codex".into()),
            ask_id: Some("ask-1".into()),
            relation: Some(JobRelation::Open),
            progress: Some(JobProgress {
                state: Some(Progress::Idle {
                    since: 940,
                    why: IdleWhy::TurnEnded,
                }),
                busy: false,
            }),
            prompt: "check the patch".into(),
            actions: Some(vec![
                JobAction::Nudge { to: "codex".into() },
                JobAction::Resend,
            ]),
            ..Default::default()
        }],
        asks: vec![Ask {
            correlation_id: "ask-1".into(),
            from_peer: "boss".into(),
            to_peer: "codex".into(),
            to_peer_id: "codex".into(),
            opened_at: Some(820),
            open: true,
            ..Default::default()
        }],
        ..Default::default()
    }
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

fn badge(g: &Grid, word: &str, tone: Tone, cols: usize) {
    let start = cols - 1 - word.len();
    for (i, ch) in word.chars().enumerate() {
        let cell = &g.rows[&0][&(start + i)];
        assert_eq!((cell.ch, cell.tone), (ch, tone), "badge {word}, cell {i}");
    }
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

fn b3d_row(condition: crate::wire::DeliveryCondition, stuck: Option<(usize, u64)>) -> String {
    let mut peer = peer("pi", "online", None);
    peer.delivery = Some(crate::wire::Delivery {
        condition,
        stuck: stuck.map(|(count, since)| crate::wire::Stuck { count, since }),
    });
    let frame = as_frame(&Snapshot {
        captured_at: 1000,
        ..Default::default()
    });
    joined(&layout::delivery(&frame, &peer))
}

#[test]
fn b3d_delivery_no_push() {
    assert_eq!(
        b3d_row(crate::wire::DeliveryCondition::NoPush, None),
        "no push"
    );
}

#[test]
fn b3e_v1_header_counts_closed() {
    let ask = Ask {
        correlation_id: "ask-1".into(),
        open: false,
        failed: true,
        closed_by: Some("hub".into()),
        ..Default::default()
    };
    let order = [&ask];
    assert_eq!(
        super::asks::header(&order, false),
        "asks · 0 open · 1 closed"
    );
}

#[test]
fn b3e_v1_worker_is_the_raw_assignee() {
    let mut job = b3c_job("running");
    job.worker = None;
    job.relation = None;
    job.progress = None;
    job.assigned_peer = Some("codex".into());
    let text = b3c_card(job, vec![]);
    assert!(
        text.contains("worker codex") && !text.contains("IDLE!") && !text.contains("· now"),
        "{text}"
    );
}

#[test]
fn b3e_v1_closed_ask_is_neutral() {
    let ask = Ask {
        correlation_id: "ask-1".into(),
        open: false,
        failed: true,
        closed_by: Some("recipient".into()),
        ..Default::default()
    };
    let (word, tone) = super::asks::state(&Snapshot::default(), &ask);
    assert_eq!((word, tone), ("closed".into(), layout::Tone::Plain));
}

#[test]
fn b3e_v1_closed_glyph_is_plain() {
    let ask = Ask {
        correlation_id: "ask-1".into(),
        open: false,
        failed: true,
        ..Default::default()
    };
    let rows = super::asks::rows(&as_frame(&Snapshot::default()), &[&ask], 80);
    assert!(rows[0]
        .iter()
        .any(|(text, tone)| text == "●" && *tone == layout::Tone::Plain));
}

#[test]
fn an_unsupported_hub_shows_one_state_and_no_panel() {
    let value =
        serde_json::from_str(include_str!("../../tests/fixtures/wire/job_relations.json")).unwrap();
    let mut app = app_with(Snapshot::default());
    app.receive(super::decoded(value));
    assert!(screen_text(&mut app, 140, 30).join("\n").contains("j-busy"));
    app.receive(super::decoded(serde_json::json!({"schema_version": 3})));
    let text = screen_text(&mut app, 140, 30).join("\n");
    assert!(
        text.contains("unsupported hub") && !text.contains("j-busy") && !text.contains("online"),
        "no panel from the last frame: {text}"
    );
    app.asks = true;
    let text = screen_text(&mut app, 140, 30).join("\n");
    assert!(text.contains("unsupported hub"), "{text}");
    app.asks = false;
    app.events = true;
    let text = screen_text(&mut app, 140, 30).join("\n");
    assert!(
        text.contains("unsupported hub") && !text.contains("showing the snapshot"),
        "nothing from a snapshot is shown: {text}"
    );
}

#[test]
fn an_error_from_the_fetch_keeps_the_last_frame() {
    let mut app = app_with(s1());
    let before = app.snap.as_ref().unwrap().jobs[0].job_id.clone();
    app.apply(Err("schema_version is not a number: \"no\"".into()));
    assert_eq!(app.snap.as_ref().unwrap().jobs[0].job_id, before);
    assert!(app.error.as_deref().unwrap().contains("schema_version"));
}

#[test]
fn b3d_delivery_queued() {
    assert_eq!(
        b3d_row(crate::wire::DeliveryCondition::Push, Some((2, 940))),
        "2 queued for 1m"
    );
}

#[test]
fn b3d_delivery_both() {
    assert_eq!(
        b3d_row(crate::wire::DeliveryCondition::NoPush, Some((2, 940))),
        "no push · 2 queued for 1m"
    );
}

#[test]
fn b3d_delivery_offline_is_empty() {
    assert!(b3d_row(crate::wire::DeliveryCondition::Offline, Some((2, 940))).is_empty());
}

#[test]
fn b3d_delivery_unknown_is_empty() {
    assert!(b3d_row(crate::wire::DeliveryCondition::Unknown, Some((2, 940))).is_empty());
}

#[test]
fn b3d_mark_work() {
    let mut peer = peer("pi", "online", None);
    peer.liveness = Some(crate::wire::Liveness::Work { since: 1 });
    let pieces = layout::presence(
        &as_frame(&with_roster(Snapshot::default(), vec![peer])),
        80,
        '⠹',
        true,
    );
    assert!(pieces
        .iter()
        .any(|(t, tone)| t == "⠹" && *tone == layout::Tone::Run));
}

#[test]
fn b3d_mark_wait() {
    let mut peer = peer("pi", "online", None);
    peer.liveness = Some(crate::wire::Liveness::Wait {
        since: 1,
        reason: None,
    });
    let pieces = layout::presence(
        &as_frame(&with_roster(Snapshot::default(), vec![peer])),
        80,
        '⠹',
        true,
    );
    assert!(pieces
        .iter()
        .any(|(t, tone)| t == "!" && *tone == layout::Tone::Wait));
}

#[test]
fn b3d_mark_online() {
    let mut peer = peer("pi", "online", None);
    peer.liveness = Some(crate::wire::Liveness::Online);
    let pieces = layout::presence(
        &as_frame(&with_roster(Snapshot::default(), vec![peer])),
        80,
        '⠹',
        true,
    );
    assert!(pieces
        .iter()
        .any(|(t, tone)| t == "●" && *tone == layout::Tone::Soft));
}

#[test]
fn b3d_stuck_under_nopush() {
    let text = joined(&layout::presence(
        &as_frame(&with_roster(
            Snapshot::default(),
            vec![marked(
                "pi",
                crate::wire::DeliveryCondition::NoPush,
                Some((4, 900)),
            )],
        )),
        80,
        '⠹',
        true,
    ));
    assert!(text.contains("✉4"), "{text}");
}

#[test]
fn b3d_offline_hides_stuck() {
    let text = joined(&layout::presence(
        &as_frame(&with_roster(
            Snapshot::default(),
            vec![marked(
                "pi",
                crate::wire::DeliveryCondition::Offline,
                Some((4, 900)),
            )],
        )),
        80,
        '⠹',
        true,
    ));
    assert!(!text.contains('✉'), "{text}");
}

#[test]
fn job_card_delivery_follows_the_fixed_worker() {
    use crate::wire::*;
    /* the name passed to another peer: the queue that matters is the recipient's */
    let mut job = b3c_job("running");
    job.ask_id = Some("ask-1".into());
    job.worker = Some("old".into());
    job.assignee_id = Some("new".into());
    job.relation = Some(JobRelation::Open);
    job.progress = Some(JobProgress {
        state: Some(Progress::Pending { since: Some(990) }),
        busy: false,
    });
    let mut old = peer("old", "online", None);
    old.liveness = Some(Liveness::Online);
    old.delivery = Some(Delivery {
        condition: DeliveryCondition::NoPush,
        stuck: Some(Stuck {
            count: 7,
            since: 940,
        }),
    });
    let mut new = peer("new", "online", None);
    new.name = "codex".into();
    new.liveness = Some(Liveness::Online);
    new.delivery = Some(Delivery {
        condition: DeliveryCondition::Push,
        stuck: None,
    });
    let text = b3c_text(job, vec![b3c_ask("boss", "old", "codex")], vec![old, new]);
    assert!(
        text.contains("7 queued for 1m") && text.contains("no push"),
        "{text}"
    );
}

#[test]
fn a_v1_card_claims_nothing_about_present_peers() {
    let ask = b3c_ask("boss", "old", "old");
    let snap = Snapshot {
        schema_version: 1,
        captured_at: 1000,
        asks: vec![ask],
        peers: vec![peer("boss", "online", None), peer("old", "online", None)],
        ..Default::default()
    };
    let crate::wire::Decoded::V1Neutral(decoded) =
        crate::wire::decode_snapshot(serde_json::to_value(snap).unwrap()).unwrap()
    else {
        panic!("v1");
    };
    let text = super::asks::card(
        &as_frame(&decoded),
        &decoded.asks[0],
        1,
        90,
        None,
        Fit::Whole,
    )
    .text();
    assert!(!text.contains("left the hub"), "{text}");
}

#[test]
fn a_sender_the_hub_left_unresolved_is_not_looked_up_by_name() {
    use crate::wire::*;
    /* the sender's name equals another peer's id; the hub resolved it to nobody */
    let mut ask = b3c_ask("x", "w", "w");
    ask.state = Some(AskState::Open {
        progress: Progress::Pending { since: Some(820) },
    });
    ask.from_peer_id = None;
    let mut x = peer("x", "online", Some("work"));
    x.name = "alpha".into();
    let snap = Snapshot {
        schema_version: 2,
        captured_at: 1000,
        asks: vec![ask],
        peers: vec![x, peer("w", "online", None)],
        ..Default::default()
    };
    let text = super::asks::card(&as_frame(&snap), &snap.asks[0], 1, 90, None, Fit::Whole).text();
    assert!(!text.contains("now WORK"), "{text}");
    assert!(text.contains("left the hub"), "{text}");
}

#[test]
fn an_unknown_block_reason_renders_neutral() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/wire/job_dispatch.json")).unwrap();
    let job = value["jobs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|j| j["job_id"] == "q-blocked")
        .unwrap();
    job["dispatch_state"]["reason"] = serde_json::json!("quota");
    let crate::wire::Decoded::V2(snap) = crate::wire::decode_snapshot(value).unwrap() else {
        panic!("v2");
    };
    let chain = model::chains(&snap)
        .into_iter()
        .find(|c| c.topo().iter().any(|id| id == "q-blocked"))
        .unwrap();
    let num = model::Numbers::default()
        .assign(&chain)
        .into_iter()
        .map(|(n, id)| (id, n))
        .collect();
    let view = View {
        snap: as_frame(&snap),
        chain: &chain,
        num: &num,
        base: 0,
        sel: "q-blocked",
    };
    let text = flat(&layout::card(&view, "q-blocked", 120, None, Fit::Whole).text());
    assert!(!text.contains("was deleted"), "{text}");
    assert!(!text.contains("retry it first"), "{text}");
    assert!(text.contains("blocked"), "{text}");
    let mut job = b3c_job("queued");
    job.dispatch_state = Some(crate::wire::DispatchState::Blocked {
        dependencies: vec![],
        dependency: "opaque-dep".into(),
        reason: crate::wire::BlockReason::Unknown,
    });
    let (_, grid) = b3c_scene(job, vec![], vec![]);
    assert_eq!(b3c_word_tone(&grid, "opaque-dep"), layout::Tone::Dim);
}

#[test]
fn a_finished_jobs_offline_worker_keeps_its_word() {
    let mut job = b3c_job("done");
    job.relation = Some(crate::wire::JobRelation::NoAsk);
    let mut p = peer("codex", "offline", None);
    p.liveness = Some(crate::wire::Liveness::Offline);
    let text = b3c_text(job, vec![], vec![p]);
    assert!(text.contains("codex · offline"), "{text}");
}

#[test]
fn a_malformed_v2_snapshot_keeps_the_last_frame_end_to_end() {
    let mut app = app_with(s1());
    let before = app.snap.as_ref().unwrap().jobs[0].job_id.clone();
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/wire/job_relations.json")).unwrap();
    value["asks"][0].as_object_mut().unwrap().remove("state");
    app.receive(super::decoded(value));
    assert_eq!(app.snap.as_ref().unwrap().jobs[0].job_id, before);
    assert!(
        app.error.as_deref().unwrap().contains("lacks state"),
        "{:?}",
        app.error
    );
}

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

/* the first row drawn in rule characters */
fn rule_row(lines: &[String]) -> usize {
    lines
        .iter()
        .position(|l| l.starts_with("────") || l.starts_with("----"))
        .unwrap_or_else(|| panic!("no rule:\n{}", lines.join("\n")))
}

/* the first row that starts with `prefix` */
fn first_row(lines: &[String], prefix: &str) -> usize {
    lines
        .iter()
        .position(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no row starting {prefix:?}:\n{}", lines.join("\n")))
}

/* an age of about 120 s reads 121s, or 122s on a slow run */
fn aged(row: &str, head: &str, tail: &str) -> bool {
    ["121s", "122s"]
        .iter()
        .any(|age| row.ends_with(&format!("{head}{age}{tail}")))
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

#[test]
fn event_rows_fit_the_pane_whatever_the_kind() {
    for kind in ["ask", "ack", "notify", "broadcast", "chat", "未来事件"] {
        for at in [None, Some(1001)] {
            let event = model::Event {
                kind: kind.into(),
                at,
                from_peer: "long-long-sender".into(),
                to_peer: "long-long-recipient".into(),
                text: "preview of a real event".into(),
                ..Default::default()
            };
            let items = vec![
                super::feed::Item::Event(0, vec![&event]),
                super::feed::Item::Restart(1000),
            ];
            for cols in 30..=120 {
                for calm in [false, true] {
                    for row in super::feed::rows(&items, cols, "", calm) {
                        let w = super::row_width(&row);
                        assert!(w <= cols, "{kind} at {cols}: {w} {row:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_box_edge_cuts_its_head_to_the_room() {
    let row = layout::edge(
        30,
        vec![("a".repeat(40), Tone::Title), (" count".into(), Tone::Dim)],
        vec![("─┐".into(), Tone::Line)],
    );
    assert_eq!(super::row_width(&row), 30, "{row:?}");
}

#[test]
fn a_boxed_card_keeps_every_head_framed_keeps() {
    let badges = [
        None,
        Some((" WAIT ", Tone::Badge)),
        Some((" OFFLINE ", Tone::PillFail)),
    ];
    let top = |g: &Grid| g.text().lines().next().unwrap_or("").to_string();
    for cols in 29..=95 {
        for badge in badges {
            for n in 0..=32 {
                let head = format!(" {} ", "界x".repeat(n));
                let framed = layout::framed(('x', Tone::Near), &head, badge, &[], &[], cols);
                let boxed = layout::boxed(('x', Tone::Near), &head, badge, &[], &[], cols);
                assert!(
                    boxed.rows.values().all(|r| r.keys().all(|c| *c <= cols)),
                    "{cols}: {}",
                    boxed.text()
                );
                assert!(
                    top(&framed).contains('…') || !top(&boxed).contains('…'),
                    "{cols}: {} / {}",
                    top(&framed),
                    top(&boxed)
                );
            }
        }
    }
}

fn held_from(events: Vec<model::Event>) -> Vec<super::feed::Held> {
    events
        .into_iter()
        .map(|e| super::feed::Held::Event(0, Box::new(e)))
        .collect()
}

fn pending_to(liveness: Option<Liveness>, opened: u64, captured: u64) -> Snapshot {
    let ask = ask_with(
        crate::wire::AskState::Open {
            progress: Progress::Pending {
                since: Some(opened),
            },
        },
        Some(opened),
    );
    let peers = match liveness {
        Some(kind) => {
            let mut peer = doing("amesh-pi", "idle", 1, None);
            peer.liveness = Some(kind);
            vec![peer]
        }
        None => Vec::new(),
    };
    Snapshot {
        captured_at: captured,
        asks: vec![ask],
        peers: peers.clone(),
        roster: peers,
        ..Default::default()
    }
}

#[test]
fn event_routes_share_one_column_and_short_previews_are_left_out() {
    let events = vec![
        ev(1, "ask", "cc", "pi", "abcdefghijklmnop"),
        ev(2, "ask", "amesh-claude-code", "x", "abcdefghijklmnop"),
    ];
    let held = held_from(events);
    let items = super::feed::items(&held);
    let rows = super::feed::rows(&items, 80, "", false);
    let route_w = |row: &[(String, Tone)]| {
        row.iter()
            .find(|(_, tone)| *tone == Tone::Soft)
            .map(|(text, _)| width(text))
            .expect("route")
    };
    assert_eq!(route_w(&rows[0]), route_w(&rows[1]), "{rows:?}");
    assert!(
        rows.iter().all(|row| row
            .iter()
            .any(|(text, _)| text.contains("abcdefghijklmnop"))),
        "{rows:?}"
    );
    let used: usize = rows[0]
        .iter()
        .take_while(|(text, _)| !text.contains("abcdefghijklmnop"))
        .map(|(text, _)| width(text))
        .sum();
    let cols = used + 2 + super::asks::PREVIEW_MIN - 1;
    let drop = super::feed::rows(&items, cols, "", false);
    assert!(
        drop.iter()
            .all(|row| !row.iter().any(|(_, tone)| *tone == Tone::Plain)),
        "{drop:?}"
    );
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

#[test]
fn home_event_rows_dim_the_kind_words_and_keep_the_ack_glyphs() {
    let ask = ev(2, "ask", "cc", "pi", "please look");
    let ok = ev(3, "ack", "pi", "cc", "done");
    let mut fail = ev(4, "ack", "pi", "cc", "no");
    fail.failed = Some(true);
    let hub = ev(1, "notify", "amesh", "circle", "restarted");
    let held = held_from(vec![hub, ask, ok, fail]);
    let items = super::feed::items(&held);
    let rows = super::feed::rows(&items, 80, "", true);
    assert_eq!(cell(&rows[0], "hub"), Tone::Dim, "{rows:?}");
    assert_eq!(cell(&rows[1], "ask"), Tone::Dim, "{rows:?}");
    assert_eq!(cell(&rows[2], "●"), Tone::Done, "{rows:?}");
    assert_eq!(cell(&rows[3], "×"), Tone::Fail, "{rows:?}");
}

#[test]
fn a_home_strip_keeps_the_selected_name_and_the_done_count_plain() {
    let snap = Snapshot {
        jobs: vec![
            job("build", "done", &[], 1),
            job("test", "queued", &["build"], 2),
        ],
        captured_at: 1000,
        ..Default::default()
    };
    let mut app = app_with(snap);
    let blocks = app.blocks();
    let frame = app.snap.clone().unwrap();
    let rows = super::strip(&blocks, Some(0), &frame, 80, 4, true);
    let row = &rows[0];
    assert_eq!(
        row[0].0, "▸ ",
        "HOME's one selection still marks the chain: {row:?}"
    );
    assert_eq!(cell(row, "test"), Tone::Plain, "{row:?}");
    assert_eq!(cell(row, "1"), Tone::Plain, "{row:?}");
}

#[test]
fn an_ask_to_a_peer_without_activity_reads_no_activity() {
    let snap = pending_to(Some(Liveness::Online), 820, 1000);
    assert_eq!(
        super::asks::state(&snap, &snap.asks[0]),
        ("no activity 3m".into(), Tone::Run)
    );
}

#[test]
fn an_ask_to_a_working_peer_still_reads_waiting() {
    let snap = pending_to(Some(Liveness::Work { since: 1 }), 820, 1000);
    assert_eq!(
        super::asks::state(&snap, &snap.asks[0]),
        ("waiting 3m".into(), Tone::Run)
    );
}

#[test]
fn an_ask_in_the_idle_grace_still_reads_waiting() {
    let snap = pending_to(Some(Liveness::Idle { since: 995 }), 995, 1000);
    assert_eq!(
        super::asks::state(&snap, &snap.asks[0]),
        ("waiting 5s".into(), Tone::Run)
    );
}

#[test]
fn an_ask_without_its_peer_in_view_still_reads_waiting() {
    let snap = pending_to(None, 820, 1000);
    assert_eq!(
        super::asks::state(&snap, &snap.asks[0]),
        ("waiting 3m".into(), Tone::Run)
    );
}

/* the HOME design's busy circle: busy_asks(), the report chain at work beside a failed lint, and
the events around the fan-out, as the design drew them */
fn home_busy() -> (Snapshot, Vec<model::Event>) {
    let mut snap = busy_asks();
    let mut bench = job("bench-baseline", "done", &[], 900);
    bench.title = "bench baseline".into();
    bench.assigned_peer = Some(CODEX.into());
    bench.finished_at = Some(960);
    let mut tune = job("tune-ladder", "running", &["bench-baseline"], 901);
    tune.title = "tune ladder".into();
    tune.assigned_peer = Some(PI2.into());
    tune.worker = Some(PI2.into());
    let mut report = job("report", "queued", &["tune-ladder"], 902);
    report.dispatch_state = Some(DispatchState::Waiting {
        dependencies: vec!["tune-ladder".into()],
    });
    let mut lint = job("lint", "failed", &[], 903);
    lint.assigned_peer = Some(PI.into());
    lint.result = Some("clippy: 3 warnings".into());
    lint.finished_at = Some(990);
    snap.jobs = vec![bench, tune, report, lint];
    let mut events = vec![model::Event {
        seq: Some(1),
        at: Some(1001),
        kind: "chat".into(),
        peer: CC.into(),
        role: "user".into(),
        text: "fan the timing review out to pi, pi-2 and codex".into(),
        ..Default::default()
    }];
    for (i, to) in [CODEX, PI, PI2].iter().enumerate() {
        let mut b = ev(
            2 + i as u64,
            "broadcast",
            CC,
            to,
            "heads-up: SM2 sign path review starting",
        );
        b.id = "b1".into();
        b.at = Some(1002);
        events.push(b);
    }
    events.extend(fan_events(5));
    events.push(ev(8, "ack", CODEX, CC, "2 findings, both fixed"));
    events.push(model::Event {
        failed: Some(true),
        ..ev(
            9,
            "ack",
            PI2,
            CC,
            "[failed] build broke: sm2_sign undefined after the rebase",
        )
    });
    events.push(ev(
        10,
        "notify",
        "amesh",
        CC,
        "job j-7f3a sm2-perf is running: pi has not acked",
    ));
    events.push(ev(11, "notify", CC, PI, "ask a3a0 waits for your ack"));
    (snap, events)
}

/* the design's calm circle: the same work with nobody waiting on a person; codex past its
prompt, the other answers in, lint passed on retry */
fn home_calm() -> (Snapshot, Vec<model::Event>) {
    use crate::wire::{AskState, Liveness, Progress};
    let (mut snap, busy) = home_busy();
    let work = |cid, to, at, since, text: &str| {
        let mut ask = lone(cid, CC, to, Some(at));
        ask.text = text.into();
        ask.from_peer_id = Some(CC.into());
        let progress = Progress::Work { since };
        stamp(ask, AskState::Open { progress }, vec![], None)
    };
    snap.asks = vec![
        work("ask-1223c9e0", PI2, 820, 830, "review the SM2 sign path"),
        work(
            "ask-5d0c11e2",
            CODEX,
            960,
            990,
            "run the full sm2 bench on the arm64 box and post the table",
        ),
    ];
    set_live(&mut snap, &[(CODEX, Liveness::Work { since: 990 })]);
    snap.jobs[3].state = "done".into();
    let mut events = busy[..7].to_vec();
    for (seq, at, from, to, text) in [
        (8, 1006, PI, CC, "no timing leak in sm2_sign.c"),
        (9, 1007, CC, PI2, "the standard curves only"),
        (10, 1008, CODEX, CC, "2 findings, both fixed"),
        (11, 1009, PI, CC, "lint: clippy clean"),
    ] {
        events.push(model::Event {
            at: Some(at),
            ..ev(seq, "ack", from, to, text)
        });
    }
    (snap, events)
}

fn home_app((snap, events): (Snapshot, Vec<model::Event>)) -> App {
    let mut app = app_with(snap);
    app.home = true;
    app.take_events(Ok(events));
    app
}

/* the busy circle with its snapshot 11.5s old and the last events read failing */
fn home_stale() -> App {
    let mut app = home_app(home_busy());
    app.take_events(Err(format!(
        "GET /events?since=11: {}: connection refused",
        crate::cli::DAEMON_UNREACHABLE
    )));
    app.last_ok = std::time::Instant::now().checked_sub(std::time::Duration::from_millis(11_500));
    app
}

/* HOME at the approved design's sizes: the busy circle, the same 12s stale with its events read
failing, and the calm one */
const HOME_FRAMES: &str = r#"
===== busy 46
4 online · ○ claude-code  ! codex
           ○ pi           ⠋ pi-2
┌─ asks · 4 need you · +1 ─────────────── ● ─┐
│▸ ◆ 5d0c claude-code─▸─codex   WAIT 15s     │
│  ◆ a3a0 claude-code─▸─pi      IDLE 1m      │
│  ◆ 77aa pi-2─▸─claude-code    IDLE 25s     │
│  ◆ 0c9e claude-code─▸─gemini  LEFT         │
└────────────────────────────────────────────┘
┌─ chains · 2 ───────────────────────────────┐
│  independent jobs  ×      0/1  × lint      │
│  report            ●─◆─○  1/3  1 run       │
└────────────────────────────────────────────┘
┌─ events · 11 kept · newest 00:16 ──────────┐
│  00:16 hub    ▸claude-code                 │
│  00:16 notify claude-code▸pi               │
└────────────────────────────────────────────┘
┌─ ◆ 1 ask 5d0c · open 40s ─────────── WAIT ─┐
│  why     Claude needs your permission to   │
│          use Bash                          │
│  from    claude-code▸codex · 00:16         │
│  text    run the full sm2 bench on the…    │
│  ⏎ full card                               │
└────────────────────────────────────────────┘
j/k move  c jobs  a asks  e events  enter card
===== busy 100
4 online · ○ claude-code  ! codex  ○ pi  ⠋ pi-2
┌─ asks · 4 need you ─────────────────────────── ● ─┐ ┌─ ◆ 1 ask 5d0c · open 40s ─────────── WAIT ─┐
│▸ ◆ 5d0c claude-code─▸─codex   WAIT 15s            │ │  why     Claude needs your permission to   │
│  ◆ a3a0 claude-code─▸─pi      IDLE 1m             │ │          use Bash                          │
│  ◆ 77aa pi-2─▸─claude-code    IDLE 25s            │ │  from    claude-code▸codex · 00:16         │
│  ◆ 0c9e claude-code─▸─gemini  LEFT                │ │  text    run the full sm2 bench on the     │
│  ◆ 1223 claude-code─▸─pi-2    working 3m          │ │          arm64 box and post the table      │
└───────────────────────────────────────────────────┘ │                                            │
┌─ chains · 2 ──────────────────────────────────────┐ │                                            │
│  independent jobs  ×      0/1  × lint             │ │                                            │
│  report            ●─◆─○  1/3  1 run              │ │                                            │
└───────────────────────────────────────────────────┘ │                                            │
┌─ events · 11 kept · newest 00:16 ─────────────────┐ │                                            │
│  00:16 chat   claude-code (user)                  │ │                                            │
│  00:16 bcast  claude-code▸3 peers                 │ │                                            │
│  00:16 ask    claude-code▸3 peers                 │ │                                            │
│  00:16 ack ●  codex▸claude-code                   │ │                                            │
│  00:16 ack ×  pi-2▸claude-code                    │ │                                            │
│  00:16 hub    ▸claude-code                        │ │                                            │
│  00:16 notify claude-code▸pi                      │ │                                            │
│                                                   │ │                                            │
│                                                   │ │  ⏎ full card                               │
└───────────────────────────────────────────────────┘ └────────────────────────────────────────────┘
j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events
===== stale 46
4 online · ○ claude-code  ! codex
           ○ pi           ⠋ pi-2
! events: hub unreachable
┌─ asks · 4 need you · +1 ─────────── ○ 12s ─┐
│▸ ◆ 5d0c claude-code─▸─codex   WAIT 15s     │
│  ◆ a3a0 claude-code─▸─pi      IDLE 1m      │
│  ◆ 77aa pi-2─▸─claude-code    IDLE 25s     │
│  ◆ 0c9e claude-code─▸─gemini  LEFT         │
└────────────────────────────────────────────┘
┌─ chains · 2 ───────────────────────────────┐
│  independent jobs  ×      0/1  × lint      │
│  report            ●─◆─○  1/3  1 run       │
└────────────────────────────────────────────┘
┌─ events · 11 kept · newest 00:16 ──────────┐
│  00:16 notify claude-code▸pi               │
└────────────────────────────────────────────┘
┌─ ◆ 1 ask 5d0c · open 40s ─────────── WAIT ─┐
│  why     Claude needs your permission to   │
│          use Bash                          │
│  from    claude-code▸codex · 00:16         │
│  text    run the full sm2 bench on the…    │
│  ⏎ full card                               │
└────────────────────────────────────────────┘
j/k move  c jobs  a asks  e events  enter card
===== stale 100
4 online · ○ claude-code  ! codex  ○ pi  ⠋ pi-2
! events: hub unreachable
┌─ asks · 4 need you ─────────────────────── ○ 12s ─┐ ┌─ ◆ 1 ask 5d0c · open 40s ─────────── WAIT ─┐
│▸ ◆ 5d0c claude-code─▸─codex   WAIT 15s            │ │  why     Claude needs your permission to   │
│  ◆ a3a0 claude-code─▸─pi      IDLE 1m             │ │          use Bash                          │
│  ◆ 77aa pi-2─▸─claude-code    IDLE 25s            │ │  from    claude-code▸codex · 00:16         │
│  ◆ 0c9e claude-code─▸─gemini  LEFT                │ │  text    run the full sm2 bench on the     │
│  ◆ 1223 claude-code─▸─pi-2    working 3m          │ │          arm64 box and post the table      │
└───────────────────────────────────────────────────┘ │                                            │
┌─ chains · 2 ──────────────────────────────────────┐ │                                            │
│  independent jobs  ×      0/1  × lint             │ │                                            │
│  report            ●─◆─○  1/3  1 run              │ │                                            │
└───────────────────────────────────────────────────┘ │                                            │
┌─ events · 11 kept · newest 00:16 ─────────────────┐ │                                            │
│  00:16 chat   claude-code (user)                  │ │                                            │
│  00:16 bcast  claude-code▸3 peers                 │ │                                            │
│  00:16 ask    claude-code▸3 peers                 │ │                                            │
│  00:16 ack ●  codex▸claude-code                   │ │                                            │
│  00:16 ack ×  pi-2▸claude-code                    │ │                                            │
│  00:16 hub    ▸claude-code                        │ │                                            │
│  00:16 notify claude-code▸pi                      │ │                                            │
│                                                   │ │  ⏎ full card                               │
└───────────────────────────────────────────────────┘ └────────────────────────────────────────────┘
j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events
===== calm 46
4 online · ○ claude-code  ⠋ codex
           ○ pi           ⠋ pi-2
┌─ asks · 2 open ──────────────────────── ● ─┐
│▸ ◆ 1223 claude-code─▸─pi-2   working 3m    │
│  ◆ 5d0c claude-code─▸─codex  working 40s   │
└────────────────────────────────────────────┘
┌─ chains · 2 ───────────────────────────────┐
│  report            ●─◆─○  1/3  1 run       │
│  independent jobs  ●      1/1              │
└────────────────────────────────────────────┘
┌─ events · 11 kept · newest 00:16 ──────────┐
│  00:16 ask    claude-code▸3 peers          │
│  00:16 ack ●  pi▸claude-code               │
│  00:16 ack ●  claude-code▸pi-2             │
│  00:16 ack ●  codex▸claude-code            │
│  00:16 ack ●  pi▸claude-code               │
└────────────────────────────────────────────┘
┌─ ◆ 1 ask 1223 · open 3m ───────────────────┐
│  from    claude-code▸pi-2 · 00:13          │
│  text    review the SM2 sign path          │
│                                            │
│  ⏎ full card                               │
└────────────────────────────────────────────┘
j/k move  c jobs  a asks  e events  enter card
===== calm 100
4 online · ○ claude-code  ⠋ codex  ○ pi  ⠋ pi-2
┌─ asks · 2 open ─────────────────────────────── ● ─┐ ┌─ ◆ 1 ask 1223 · open 3m ───────────────────┐
│▸ ◆ 1223 claude-code─▸─pi-2   working 3m           │ │  from    claude-code▸pi-2 · 00:13          │
│  ◆ 5d0c claude-code─▸─codex  working 40s          │ │  text    review the SM2 sign path          │
└───────────────────────────────────────────────────┘ │                                            │
┌─ chains · 2 ──────────────────────────────────────┐ │                                            │
│  report            ●─◆─○  1/3  1 run              │ │                                            │
│  independent jobs  ●      1/1                     │ │                                            │
└───────────────────────────────────────────────────┘ │                                            │
┌─ events · 11 kept · newest 00:16 ─────────────────┐ │                                            │
│  00:16 chat   claude-code (user)                  │ │                                            │
│  00:16 bcast  claude-code▸3 peers                 │ │                                            │
│  00:16 ask    claude-code▸3 peers                 │ │                                            │
│  00:16 ack ●  pi▸claude-code                      │ │                                            │
│  00:16 ack ●  claude-code▸pi-2                    │ │                                            │
│  00:16 ack ●  codex▸claude-code                   │ │                                            │
│  00:16 ack ●  pi▸claude-code                      │ │                                            │
│                                                   │ │                                            │
│                                                   │ │                                            │
│                                                   │ │                                            │
│                                                   │ │                                            │
│                                                   │ │  ⏎ full card                               │
└───────────────────────────────────────────────────┘ └────────────────────────────────────────────┘
j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events
"#;

#[test]
fn home_matches_the_approved_design() {
    let frames: Vec<(&str, Vec<&str>)> = HOME_FRAMES
        .split("===== ")
        .filter(|f| !f.trim().is_empty())
        .map(|f| {
            let mut lines = f.lines();
            (lines.next().unwrap(), lines.collect())
        })
        .collect();
    assert_eq!(frames.len(), 6);
    let mut wrong = Vec::new();
    for (name, want) in frames {
        assert_eq!(want.len(), 24, "{name}");
        let (scene, cols) = name.split_once(' ').unwrap();
        let mut app = match scene {
            "busy" => home_app(home_busy()),
            "stale" => home_stale(),
            _ => home_app(home_calm()),
        };
        let got = screen_text(&mut app, cols.parse().unwrap(), want.len());
        if got != want {
            wrong.push(format!(
                "{name}\n--- got\n{}\n--- want\n{}",
                got.join("\n"),
                want.join("\n")
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} frames differ:\n{}",
        wrong.len(),
        wrong.join("\n\n")
    );
}

#[test]
fn the_tui_starts_on_home() {
    let mut app = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    assert!(app.home, "HOME is the first screen");
    assert_eq!(screen_text(&mut app, 46, 24)[1], "waiting for the hub");
    app.apply(Ok(home_busy().0));
    assert_eq!(
        screen_text(&mut app, 46, 24).last().unwrap(),
        "j/k move  c jobs  a asks  e events  enter card"
    );
}

/* the busy circle at 46x24: four asks, two chains and two events can be selected */
#[test]
fn home_moves_through_its_rows_and_tabs_through_its_chains() {
    use super::Pick;
    let ask = |cid: &str| Pick::Ask(cid.into());
    let chain = |id: &str| Pick::Chain(id.into());
    let mut app = home_app(home_busy());
    let first = screen_text(&mut app, 46, 24);
    let rows = vec![
        ask("ask-5d0c11e2"),
        ask("ask-a3a07b51"),
        ask("ask-77aa0d93"),
        ask("ask-0c9e4a18"),
        chain("lint"),
        chain("bench-baseline"),
        Pick::Event((0, 10)),
        Pick::Event((0, 11)),
    ];
    assert_eq!(app.home_rows, rows, "the boxes' rows, top to bottom");
    assert_eq!(app.home_sel.as_ref(), Some(&rows[0]), "the first ask");
    app.key(KeyCode::Char('k'));
    assert_eq!(app.home_sel.as_ref(), Some(&rows[0]), "k stops at the top");
    for want in &rows[1..] {
        app.key(KeyCode::Char('j'));
        assert_eq!(app.home_sel.as_ref(), Some(want));
    }
    app.key(KeyCode::Down);
    assert_eq!(app.home_sel.as_ref(), rows.last(), "j stops at the bottom");
    app.key(KeyCode::Up);
    app.key(KeyCode::Up);
    assert_eq!(app.home_sel, Some(chain("bench-baseline")));
    let lines = screen_text(&mut app, 46, 24);
    assert_eq!(
        lines[..16]
            .iter()
            .map(|l| l.replace('▸', " "))
            .collect::<Vec<_>>(),
        first[..16]
            .iter()
            .map(|l| l.replace('▸', " "))
            .collect::<Vec<_>>(),
        "the boxes hold still while the selection moves"
    );
    assert!(lines[10].starts_with("│▸ report "), "{}", lines[10]);
    assert!(lines[3].starts_with("│  ◆ 5d0c"), "{}", lines[3]);
    assert!(
        lines[16].starts_with("┌─ ◆ 2 tune ladder"),
        "the card follows to the chain's most urgent job: {}",
        lines[16]
    );
    app.key(KeyCode::Down);
    app.key(KeyCode::BackTab);
    assert_eq!(
        app.home_sel,
        Some(chain("bench-baseline")),
        "backtab from an event takes the last chain"
    );
    app.key(KeyCode::Tab);
    assert_eq!(
        app.home_sel,
        Some(chain("lint")),
        "tab wraps among the chains"
    );
    app.key(KeyCode::Tab);
    assert_eq!(app.home_sel, Some(chain("bench-baseline")));
    app.key(KeyCode::Up);
    app.key(KeyCode::Up);
    app.key(KeyCode::Tab);
    assert_eq!(
        app.home_sel,
        Some(chain("lint")),
        "tab from an ask takes the first chain"
    );
    app.key(KeyCode::BackTab);
    assert_eq!(app.home_sel, Some(chain("bench-baseline")));
    for key in ['f', '/', '1', 'l', 't', 'n', 'G']
        .map(KeyCode::Char)
        .into_iter()
        .chain([KeyCode::Esc])
    {
        app.key(key);
        assert_eq!(app.input.mode, Mode::Normal, "{key:?} starts no mode");
        assert!(app.home && !app.full, "{key:?} does nothing on HOME");
    }
    assert_eq!(app.home_sel, Some(chain("bench-baseline")));
}

#[test]
fn enter_on_home_opens_the_selections_screen_with_its_full_card() {
    let mut app = home_app(home_busy());
    screen_text(&mut app, 46, 24);
    app.key(KeyCode::Enter);
    assert!(!app.home && app.asks && app.full);
    assert_eq!(app.ask_sel.as_deref(), Some("ask-5d0c11e2"));
    assert!(screen_text(&mut app, 46, 24)
        .iter()
        .any(|l| l.starts_with("┌◆ 1 ask 5d0c")));
    let mut app = home_app(home_busy());
    screen_text(&mut app, 46, 24);
    app.key(KeyCode::BackTab);
    screen_text(&mut app, 46, 24);
    app.key(KeyCode::Enter);
    assert!(!app.home && !app.asks && !app.events && app.full);
    assert_eq!(
        app.sel.as_deref(),
        Some("tune-ladder"),
        "the report chain's card is its running job's"
    );
    let mut app = home_app(home_busy());
    screen_text(&mut app, 46, 24);
    for _ in 0..7 {
        app.key(KeyCode::Char('j'));
    }
    app.key(KeyCode::Enter);
    assert!(!app.home && app.events && app.full);
    assert_eq!(app.ev_sel, Some((0, 11)));
}

/* at 100x24 HOME lists all seven events, the fan-out among them */
#[test]
fn c_a_and_e_on_home_carry_the_selection_to_their_screens() {
    use super::Pick;
    let fresh = || {
        let mut app = home_app(home_busy());
        screen_text(&mut app, 100, 24);
        app
    };
    let mut app = fresh();
    app.key(KeyCode::BackTab);
    screen_text(&mut app, 100, 24);
    app.key(KeyCode::Char('c'));
    assert!(!app.home && !app.asks && !app.events && !app.full);
    assert_eq!(
        app.sel.as_deref(),
        Some("tune-ladder"),
        "a chain: its card's job"
    );
    let mut app = fresh();
    app.sel = Some("lint".into());
    app.key(KeyCode::Char('j'));
    screen_text(&mut app, 100, 24);
    app.key(KeyCode::Char('c'));
    assert_eq!(app.sel.as_deref(), Some("lint"), "an ask moves no job");
    app.key(KeyCode::Char('c'));
    app.key(KeyCode::Char('a'));
    assert!(app.asks && !app.home && !app.full);
    assert_eq!(
        app.ask_sel.as_deref(),
        Some("ask-a3a07b51"),
        "an ask: itself"
    );
    let mut app = fresh();
    for _ in 0..9 {
        app.key(KeyCode::Char('j'));
    }
    assert_eq!(app.home_sel, Some(Pick::Event((0, 5))), "the fan-out");
    app.key(KeyCode::Char('a'));
    assert_eq!(
        app.ask_sel.as_deref(),
        Some("ask-a3a07b51"),
        "an ask's event: the ask the asks screen lists first"
    );
    let mut app = fresh();
    for _ in 0..9 {
        app.key(KeyCode::Char('j'));
    }
    app.key(KeyCode::Char('e'));
    assert!(app.events && !app.home && !app.full);
    assert_eq!(app.ev_sel, Some((0, 5)), "an event: itself");
    let mut app = fresh();
    app.key(KeyCode::Char('e'));
    assert_eq!(app.ev_sel, None, "an ask leaves the events on the newest");
}

#[test]
fn each_screens_own_letter_and_esc_go_home_after_a_full_card_closes() {
    use KeyCode::{Char, Enter, Esc};
    let on = |app: &App| match (app.home, app.events, app.asks) {
        (true, ..) => 'h',
        (_, true, _) => 'e',
        (_, _, true) => 'a',
        _ => 'c',
    };
    let mut app = home_app(home_busy());
    screen_text(&mut app, 46, 24);
    let steps = [
        (Char('c'), 'c', false),
        (Char('c'), 'h', false),
        (Char('c'), 'c', false),
        (Esc, 'h', false),
        (Char('c'), 'c', false),
        (Enter, 'c', true),
        (Esc, 'c', false),
        (Esc, 'h', false),
        (Char('c'), 'c', false),
        (Enter, 'c', true),
        (Char('c'), 'h', false),
        (Char('a'), 'a', false),
        (Char('a'), 'h', false),
        (Char('a'), 'a', false),
        (Esc, 'h', false),
        (Char('a'), 'a', false),
        (Enter, 'a', true),
        (Esc, 'a', false),
        (Esc, 'h', false),
        (Char('a'), 'a', false),
        (Enter, 'a', true),
        (Char('a'), 'h', false),
        (Char('e'), 'e', false),
        (Char('e'), 'h', false),
        (Char('e'), 'e', false),
        (Esc, 'h', false),
        (Char('e'), 'e', false),
        (Enter, 'e', true),
        (Esc, 'e', false),
        (Esc, 'h', false),
        (Char('e'), 'e', false),
        (Enter, 'e', true),
        (Char('e'), 'h', false),
        /* the other letters keep moving between the screens */
        (Char('c'), 'c', false),
        (Char('a'), 'a', false),
        (Char('c'), 'c', false),
        (Char('e'), 'e', false),
        (Char('c'), 'c', false),
        (Char('e'), 'e', false),
        (Char('a'), 'a', false),
        (Char('e'), 'e', false),
        (Char('e'), 'h', false),
    ];
    for (i, (key, screen, full)) in steps.into_iter().enumerate() {
        app.key(key);
        screen_text(&mut app, 46, 24);
        assert_eq!((on(&app), app.full), (screen, full), "step {i}: {key:?}");
        if screen == 'h' {
            assert!(
                !app.asks && !app.events,
                "step {i}: no screen stays up under HOME"
            );
        }
    }
    let mut empty = home_app((busy_asks(), Vec::new()));
    screen_text(&mut empty, 46, 24);
    for (key, screen) in [
        (Char('c'), 'c'),
        (Char('c'), 'h'),
        (Char('c'), 'c'),
        (Esc, 'h'),
    ] {
        empty.key(key);
        assert!(screen_text(&mut empty, 46, 24)
            .iter()
            .any(|l| screen == 'h' || l.starts_with("no jobs here yet")));
        assert_eq!(on(&empty), screen, "no jobs: {key:?}");
    }
}

#[test]
fn home_wants_the_full_text_of_its_card_and_the_events() {
    use super::Want;
    let mut app = home_app(home_busy());
    screen_text(&mut app, 46, 24);
    assert_eq!(
        app.wanted(),
        Want {
            detail: Some("ask=ask-5d0c11e2".into()),
            events: true
        }
    );
    app.key(KeyCode::BackTab);
    screen_text(&mut app, 46, 24);
    assert_eq!(
        app.wanted(),
        Want {
            detail: Some("detail=tune-ladder".into()),
            events: true
        },
        "the job the chain's card shows"
    );
    app.key(KeyCode::Char('j'));
    screen_text(&mut app, 46, 24);
    assert_eq!(
        app.wanted(),
        Want {
            detail: None,
            events: true
        }
    );
}

#[test]
fn y_on_home_copies_the_command_of_the_card_it_shows() {
    let (mut snap, events) = home_busy();
    snap.jobs[3].actions = Some(vec![JobAction::Retry {
        needs_assignee: false,
    }]);
    let mut app = home_app((snap, events));
    let copy = |app: &mut App, key: char| {
        screen_text(app, 100, 24);
        let Act::Copy(index) = app.key(KeyCode::Char(key)) else {
            panic!("{key} copies");
        };
        let mut bytes = Vec::new();
        app.copy_command(index, false, &mut bytes);
        let footer = screen_text(app, 100, 24).pop().unwrap();
        (!bytes.is_empty()).then_some(footer)
    };
    assert_eq!(copy(&mut app, 'y'), None, "a prompt calls for no command");
    app.key(KeyCode::Char('j'));
    assert_eq!(
        copy(&mut app, 'y').as_deref(),
        Some("sent to clipboard (OSC 52): amesh peer notify openhitls-sm2-opt-pi 'ask a3a0 waits for your ack'")
    );
    assert_eq!(copy(&mut app, 'Y'), None, "the ask has one command");
    app.key(KeyCode::Tab);
    assert_eq!(
        copy(&mut app, 'y').as_deref(),
        Some("sent to clipboard (OSC 52): amesh jobs update lint --state queued"),
        "the failed chain's card is lint's"
    );
    app.key(KeyCode::Char('j'));
    app.key(KeyCode::Char('j'));
    assert_eq!(copy(&mut app, 'y'), None, "an event's card has no command");
}

#[test]
fn the_screens_name_c_for_the_jobs_and_their_own_letter_for_home() {
    let mut app = app_with(busy_asks());
    app.key(KeyCode::Char('a'));
    let footer = |app: &mut App, cols| screen_text(app, cols, 30).pop().unwrap();
    assert_eq!(
        footer(&mut app, 100),
        "j/k ↑↓ move  digits jump  / find  enter card  esc back  c jobs"
    );
    assert_eq!(
        footer(&mut app, 46),
        "j/k move  1-9 jump  enter card  c jobs"
    );
    app.key(KeyCode::Char('e'));
    assert_eq!(
        footer(&mut app, 100),
        "j/k ↑↓ move  enter card  esc back  c jobs  a asks"
    );
    assert_eq!(footer(&mut app, 46), "j/k move  enter card  c jobs  a asks");
    assert!(screen_text(&mut app, 100, 30)
        .contains(&"events appear as peers ask, ack, notify and broadcast; e: back".to_string()));
    let mut none = app_with(with_asks(Snapshot::default(), Vec::new(), Vec::new()));
    none.key(KeyCode::Char('a'));
    assert!(screen_text(&mut none, 100, 30)
        .contains(&"asks appear when a peer calls amesh_ask; a: back".to_string()));
    let mut old = app_with(Snapshot::default());
    old.key(KeyCode::Char('a'));
    assert!(screen_text(&mut old, 100, 30).contains(&"a: back".to_string()));
}

/* the busy circle's asks all at a prompt: the asks box takes the rows the card and the chains
leave and counts the rest */
#[test]
fn home_never_draws_past_its_footer_and_counts_the_asks_it_leaves_out() {
    let (mut snap, events) = home_busy();
    let wait = snap.asks[3].clone();
    assert_eq!(wait.correlation_id, "ask-5d0c11e2");
    snap.asks = (0..24)
        .map(|i| Ask {
            correlation_id: format!("ask-{i:02}{}", &wait.correlation_id[6..]),
            ..wait.clone()
        })
        .collect();
    let mut app = home_app((snap, events));
    for (cols, shown, footer) in [
        (
            100,
            16,
            "j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events",
        ),
        (46, 8, "j/k move  c jobs  a asks  e events  enter card"),
    ] {
        let lines = screen_text(&mut app, cols, 24);
        let all = lines.join("\n");
        assert_eq!(lines.len(), 24);
        assert_eq!(lines[23], footer, "{cols}:\n{all}");
        assert!(
            lines[22].starts_with('└'),
            "{cols}: the boxes close above it:\n{all}"
        );
        let count = |ch: char| all.matches(ch).count();
        assert_eq!(count('┌'), count('└'), "{cols}: every box closes:\n{all}");
        let head = lines.iter().find(|l| l.starts_with("┌─ asks")).unwrap();
        assert!(
            head.contains(&format!("24 need you · +{} need you", 24 - shown)),
            "{cols}: {head}"
        );
        assert_eq!(all.matches("WAIT 15s").count(), shown, "{cols}:\n{all}");
        assert!(
            all.contains("┌─ chains · 2 "),
            "{cols}: the chains keep their room"
        );
    }
}

#[test]
fn asks_at_work_take_three_rows_at_most_and_no_more_than_the_card_leaves() {
    let (mut snap, _) = home_calm();
    snap.jobs.clear();
    let work = snap.asks[0].clone();
    snap.asks = (0..6)
        .map(|i| Ask {
            correlation_id: format!("ask-{i}1223c9"),
            ..work.clone()
        })
        .collect();
    let mut app = home_app((snap, Vec::new()));
    let heads = |app: &mut App, cols| {
        let lines = screen_text(app, cols, 24);
        let at = lines.iter().position(|l| l.starts_with("┌─ asks")).unwrap();
        let n = lines.iter().filter(|l| l.contains("working 3m")).count();
        (lines[at].clone(), n)
    };
    let (head, n) = heads(&mut app, 46);
    assert!(
        head.starts_with("┌─ asks · 6 open · +3 ─") && n == 3,
        "{head}: {n}"
    );
    let (head, n) = heads(&mut app, 100);
    assert!(
        head.starts_with("┌─ asks · 6 open ─") && n == 6,
        "a wide pane: {head}: {n}"
    );
    let lines = screen_text(&mut app, 46, 12);
    let all = lines.join("\n");
    assert!(lines[2].starts_with("┌─ asks · 6 open · +4 ─"), "{all}");
    assert!(lines[5].starts_with('└'), "two rows:\n{all}");
    assert!(lines[6].starts_with("┌─ ◆ 1 ask 0122 · open 3m"), "{all}");
    assert!(lines[9].starts_with("│  ⏎ full card"), "{all}");
    assert!(lines[10].starts_with('└'), "{all}");
    assert_eq!(lines[11], "j/k move  c jobs  a asks  e events  enter card");
}

/* six recipients and no room: the card the default selection needs takes the pane */
#[test]
fn an_events_only_fan_out_keeps_its_cards_keys_at_30x8() {
    use super::Pick;
    let fan: Vec<model::Event> = (0..6u64)
        .map(|i| model::Event {
            at: Some(1050),
            correlation_id: format!("ask-fan{i}"),
            ..ev(
                1 + i,
                "ask",
                CC,
                &format!("p{i}"),
                "review the SM2 sign path",
            )
        })
        .collect();
    let snap = Snapshot {
        captured_at: 1100,
        ..Default::default()
    };
    let mut app = home_app((snap, fan));
    let lines = screen_text(&mut app, 30, 8);
    let all = lines.join("\n");
    assert_eq!(lines.len(), 8);
    assert!(lines[1].starts_with("┌─ • ask · "), "{all}");
    assert!(
        lines[5].starts_with("│  ⏎ full card"),
        "the keys stay:\n{all}"
    );
    assert!(lines[6].starts_with('└'), "{all}");
    assert_eq!(
        lines[7], "j/k move  c jobs  a asks …   ●",
        "no box carries the freshness, so the footer does"
    );
    app.last_ok = std::time::Instant::now().checked_sub(std::time::Duration::from_millis(11_500));
    assert!(screen_text(&mut app, 30, 8)[7].ends_with("  ○ 12s"));
    assert!(app.home_rows.is_empty(), "no box has room for a row");
    assert_eq!(app.home_sel, Some(Pick::Event((0, 1))));
    app.key(KeyCode::Char('j'));
    assert_eq!(app.home_sel, Some(Pick::Event((0, 1))));
    app.key(KeyCode::Enter);
    assert!(app.events && app.full);
    assert_eq!(app.ev_sel, Some((0, 1)));
}

#[test]
fn restarts_and_unnumbered_events_show_on_home_but_take_no_selection() {
    use super::Pick;
    let plain = |epoch: &str, at| Snapshot {
        captured_at: at,
        hub_epoch: epoch.into(),
        ..Default::default()
    };
    let rows_of = |app: &mut App| {
        let lines = screen_text(app, 46, 24);
        let top = lines
            .iter()
            .position(|l| l.starts_with("┌─ events"))
            .unwrap_or_else(|| panic!("no events box:\n{}", lines.join("\n")));
        let end = top
            + lines[top..]
                .iter()
                .position(|l| l.starts_with('└'))
                .unwrap();
        (lines[top + 1..end].to_vec(), lines[end + 1..].to_vec())
    };
    let mut app = home_app((plain("", 1000), vec![unnumbered("one"), unnumbered("two")]));
    let (rows, below) = rows_of(&mut app);
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|r| r.starts_with("│  --:-- notify")),
        "{rows:?}"
    );
    assert!(app.home_rows.is_empty() && app.home_sel.is_none());
    assert!(
        !below.iter().any(|l| l.starts_with('┌')),
        "no card: {below:?}"
    );
    app.key(KeyCode::Char('j'));
    app.key(KeyCode::Enter);
    assert!(app.home && !app.full && app.home_sel.is_none());
    let mut app = home_app((plain("e1", 1000), vec![ev(1, "notify", CC, PI, "before")]));
    app.apply(Ok(plain("e2", 1010)));
    let (rows, _) = rows_of(&mut app);
    assert!(rows[0].starts_with("│▸ 00:16 notify"), "{rows:?}");
    assert!(rows[1].starts_with("│  ─── hub restarted"), "{rows:?}");
    assert_eq!(app.home_rows, [Pick::Event((0, 1))]);
    app.key(KeyCode::Char('j'));
    assert_eq!(
        app.home_sel,
        Some(Pick::Event((0, 1))),
        "j passes no restart"
    );
    app.feed = vec![super::feed::Held::Restart(1010)];
    let (rows, below) = rows_of(&mut app);
    assert!(rows[0].starts_with("│  ─── hub restarted"), "{rows:?}");
    assert!(app.home_rows.is_empty() && app.home_sel.is_none());
    assert!(
        !below.iter().any(|l| l.starts_with('┌')),
        "no card: {below:?}"
    );
}

#[test]
fn a_selection_home_no_longer_shows_falls_back_to_the_default_then_the_first_row() {
    use super::Pick;
    let mut app = home_app(home_busy());
    screen_text(&mut app, 100, 24);
    for _ in 0..9 {
        app.key(KeyCode::Char('j'));
    }
    assert_eq!(app.home_sel, Some(Pick::Event((0, 5))));
    screen_text(&mut app, 46, 24);
    assert_eq!(
        app.home_sel,
        Some(Pick::Ask("ask-5d0c11e2".into())),
        "the fan-out left the events box: the default"
    );
    for _ in 0..6 {
        app.key(KeyCode::Char('j'));
    }
    assert_eq!(app.home_sel, Some(Pick::Event((0, 10))));
    let lines = screen_text(&mut app, 46, 16);
    let all = lines.join("\n");
    assert_eq!(
        app.home_sel,
        Some(Pick::Chain("lint".into())),
        "neither shows: the first row\n{all}"
    );
    assert!(
        lines[2].starts_with("┌─ chains"),
        "the asks give way first:\n{all}"
    );
    assert!(lines[3].starts_with("│▸ independent jobs"), "{all}");
    assert!(
        lines[6].starts_with("┌─ × 1 lint"),
        "the card follows:\n{all}"
    );
}

#[test]
fn a_mode_takes_its_keys_before_any_screen_moves() {
    for open in ['a', 'c'] {
        let mut app = home_app(home_busy());
        screen_text(&mut app, 46, 24);
        app.key(KeyCode::Char(open));
        let screen = (app.home, app.asks, app.events);
        app.key(KeyCode::Char('/'));
        for c in "cae".chars() {
            app.key(KeyCode::Char(c));
        }
        assert_eq!(app.input.mode, Mode::Search("cae".into()), "{open}");
        assert_eq!((app.home, app.asks, app.events), screen, "{open}: a query");
        app.key(KeyCode::Esc);
        assert_eq!(app.input.mode, Mode::Normal, "{open}");
        assert_eq!(
            (app.home, app.asks, app.events),
            screen,
            "{open}: esc cancels it"
        );
    }
    let jobs: Vec<Job> = (0..12)
        .map(|i| job(&format!("j{i}"), "queued", &[], i))
        .collect();
    let snap = Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    };
    let mut app = home_app((snap, Vec::new()));
    screen_text(&mut app, 46, 24);
    app.key(KeyCode::Char('c'));
    screen_text(&mut app, 46, 24);
    app.key(KeyCode::Char('1'));
    for c in "cae".chars() {
        app.key(KeyCode::Char(c));
        assert_eq!(
            app.input.mode,
            Mode::Jump("1".into()),
            "{c} waits for a digit"
        );
    }
    app.key(KeyCode::Esc);
    assert_eq!(app.input.mode, Mode::Normal);
    assert!(
        !app.home && !app.asks && !app.events,
        "esc cancels the jump"
    );
    for c in "cae".chars() {
        app.key(KeyCode::Char('f'));
        assert_eq!(
            app.key(KeyCode::Char(c)),
            Act::Hint(super::HINT_KEYS.find(c).unwrap())
        );
        assert!(
            !app.home && !app.asks && !app.events,
            "{c} is a hint letter"
        );
    }
}

#[test]
fn home_says_what_it_lacks_and_what_went_wrong() {
    let mut app = home_app((Snapshot::default(), Vec::new()));
    let line = screen_text(&mut app, 46, 24)[1].clone();
    assert!(
        line.starts_with("no open asks, jobs or events yet") && line.ends_with(" ●"),
        "nothing to show, and how fresh that is: {line}"
    );
    app.take_events(Err("GET /events: timed out".into()));
    assert_eq!(
        screen_text(&mut app, 46, 24)[1],
        "! events: GET /events: timed out"
    );
    app.apply(Err(crate::cli::DAEMON_UNREACHABLE.to_string()));
    assert_eq!(
        screen_text(&mut app, 46, 24)[1],
        "! hub unreachable",
        "the snapshot read before the events"
    );
    let mut none = home_app((Snapshot::default(), Vec::new()));
    none.snap = None;
    let keys = "j/k move  c jobs  a asks  e events  enter card";
    assert_eq!(
        screen_text(&mut none, 46, 24)[23],
        keys,
        "waiting, HOME still names its keys"
    );
    none.apply(Err(crate::cli::DAEMON_UNREACHABLE.to_string()));
    let lines = screen_text(&mut none, 46, 24);
    assert_eq!(
        (lines[1].as_str(), lines[23].as_str()),
        ("hub unreachable", keys)
    );
    none.apply(Err("a long error ".repeat(60)));
    let lines = screen_text(&mut none, 100, 8);
    assert_eq!(lines.len(), 8);
    assert_eq!(
        lines[7], "j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events",
        "a long error stops above the footer: {lines:?}"
    );
}

#[test]
fn home_walks_the_arrow_of_an_ask_at_work() {
    let mut app = home_app(home_calm());
    let row = |app: &mut App, tick| {
        let line = &app.screen(46, 24, tick)[3];
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    };
    assert!(row(&mut app, 0).contains("claude-code─▸─pi-2"));
    assert!(
        row(&mut app, 2).contains("claude-code──▸pi-2"),
        "the arrow steps toward the recipient"
    );
}

#[test]
fn the_events_kept_are_events_and_restarts_are_not_counted() {
    let mut held = held_from(vec![
        ev(1, "ask", CC, PI, "one"),
        ev(2, "ack", PI, CC, "two"),
    ]);
    held.insert(1, super::feed::Held::Restart(1001));
    assert_eq!(super::feed::kept(&held), (2, Some(1002)));
}

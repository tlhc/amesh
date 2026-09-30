use super::input::{Act, Input, Mode};
use super::layout::{self, width, Fit, View};
use super::model::{self, Activity, Ask, Job, Numbers, Peer, Snapshot};
use super::{App, Opts, PEER_HOLD};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
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
            snap: &snap,
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
            snap: &snap,
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
            snap: &snap,
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
            snap: &snap,
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

fn app_with(snap: Snapshot) -> App {
    let mut app = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
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
    assert_eq!(lines[1], "independent jobs · 1/2 done · 1 run", "{text}");
    assert!(
        lines[2].starts_with("───") && lines[2].trim_end().ends_with("─ +2 run · 1/2 ─"),
        "the rule says which of two blocks this is, and what runs in the other:\n{text}"
    );
    assert!(
        text.contains("1-2 · 2 jobs · 1● 1◆ 0× 0○")
            && text
                .lines()
                .any(|l| l.contains("1 lint the tree") && l.contains("2 docs")),
        "independent jobs share the box:\n{text}"
    );
    assert!(
        !text.contains("chain deploy"),
        "one block at a time:\n{text}"
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
        text.lines().nth(1) == Some("chain deploy · 3/9 done · 1 fail · 2 run")
            && text.lines().nth(2).unwrap().ends_with("─ +1 run · 2/2 ─")
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
        "j/k move  h/l stage  digits jump  f hint",
        "the design's footer on the last row:\n{}",
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
        tiny.iter().any(|l| l.contains("more lines · enter")),
        "a card that cannot fit at all is cut and says so:\n{}",
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
    assert!(
        text.contains("! hub unreachable · showing the snapshot from"),
        "{text}"
    );
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
        assert_eq!(rows[1], "independent jobs · 1/2 done · 1 run", "{text}");
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
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "sec")
        .unwrap()
        .result = Some("no fixture".into());
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .ask_id = Some("ask-p".into());
    snap.asks.push(Ask {
        correlation_id: "ask-p".into(),
        open: false,
        opened_at: Some(900),
        ..Default::default()
    });
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
            snap: &snap,
            chain: &chain,
            num: &num,
            base: 0,
            sel: id,
        };
        layout::card(&v, id, 46, None, Fit::Whole).text()
    };
    let sec = card("sec");
    assert!(
        sec.lines().nth(1).unwrap().contains("reason") && sec.contains("no fixture"),
        "{sec}"
    );
    assert!(
        flat(&sec).contains("retry amesh jobs update sec --state queued"),
        "{sec}"
    );
    let wide = layout::card(
        &View {
            snap: &snap,
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
    let card = |snap: &Snapshot, id: &str| {
        let (chain, num) = numbered(snap);
        flat(
            &layout::card(
                &View {
                    snap,
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
        card(&snap, "verify").contains("waits 5 deploy ◆"),
        "{}",
        card(&snap, "verify")
    );
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "deploy")
        .unwrap()
        .state = "done".into();
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
    assert!(card(&snap, "verify").contains("no peer named ghost: not sent"));
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "verify")
        .unwrap()
        .dispatch = false;
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
    assert!(
        card(&snap, "verify").contains("blocked 5 deploy cancelled: retry it first"),
        "{}",
        card(&snap, "verify")
    );
    snap.jobs.retain(|j| j.job_id != "deploy");
    let text = card(&snap, "verify");
    assert!(
        text.contains("waits deploy deleted")
            && text.contains("blocked deploy was deleted: this job cannot start"),
        "{text}"
    );
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
                snap: &snap,
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
    let text = flat(
        &layout::card(
            &View {
                snap: &snap,
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
    app.apply(Ok(s4()));
    for line in app.screen(46, 60, 0) {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.is_ascii(), "{text}");
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
                snap: &snap,
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
                snap: &ended,
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
        head.contains("failed 1m ago") && !head.contains('…') && !head.contains("stage"),
        "the stage gives way first:\n{head}"
    );
    assert!(
        card(100).lines().next().unwrap().contains("stage 2 of 5"),
        "{}",
        card(100)
    );
    let wide = layout::horizontal(
        &View {
            snap: &snap,
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
                snap: &snap,
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
            snap: &snap,
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
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .ask_id = Some("ask-gone".into());
    snap.jobs
        .iter_mut()
        .find(|j| j.job_id == "func")
        .unwrap()
        .ask_id = Some("ask-old".into());
    let (chain, num) = numbered(&snap);
    let card = |id: &str| {
        flat(
            &layout::card(
                &View {
                    snap: &snap,
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
        job.ask_id = Some(format!("ask-{id}"));
        snap.asks.push(Ask {
            correlation_id: format!("ask-{id}"),
            to_peer_id: worker.into(),
            open: true,
            opened_at: Some(900),
            ..Default::default()
        });
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
        row_of(&mut app, "8 fix", 0).contains("8 fix w3 1m WAIT!"),
        "WAIT! follows the worker on the even half second"
    );
    assert!(
        !row_of(&mut app, "8 fix", 4).contains("WAIT!"),
        "and blinks off on the odd one"
    );
    let mut still = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: false,
        theme: Default::default(),
    });
    still.apply(Ok(busy()));
    assert_eq!(glyph_of(&mut still, "3 perf", 1), '⠋', "--no-anim");
    assert!(
        row_of(&mut still, "8 fix", 4).contains("WAIT!"),
        "--no-anim keeps the WAIT! mark"
    );
    let mut elsewhere = busy();
    elsewhere
        .peers
        .iter_mut()
        .find(|p| p.peer_id == "w1")
        .unwrap()
        .running = Some(2);
    let mut app = app_with(elsewhere);
    assert_eq!(
        glyph_of(&mut app, "3 perf", 1),
        '◆',
        "w1 runs another job in a circle this pane filters out"
    );
    let mut old = busy();
    old.capabilities.peer_activity = false;
    let mut app = app_with(old);
    assert_eq!(
        glyph_of(&mut app, "3 perf", 1),
        '◆',
        "no spinner from a hub without activity"
    );
    assert!(
        !row_of(&mut app, "8 fix", 0).contains("WAIT!"),
        "nor a WAIT! mark"
    );
    let mut ascii = App::new(Opts {
        circle: None,
        ascii: true,
        color: false,
        anim: true,
        theme: Default::default(),
    });
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
                    snap: &snap,
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
        synth.contains(
            "worker w4 IDLE! 2m · turn ended ask synt w4 · 00:15 · unacked ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ waiting 1m"
        ) && synth.contains("nudge amesh peer notify w4 'synth?'"),
        "{synth}"
    );
    assert!(
        !card("upg").contains("⠋"),
        "w2 works on two jobs, so upg's card says what w2 does: {}",
        card("upg")
    );
    let text = layout::vertical(
        &View {
            snap: &snap,
            chain: &chain,
            num: &num,
            base: 0,
            sel: "perf",
        },
        46,
    )
    .text();
    assert!(
        text.contains("7 synth w4 1m\n") && text.contains("8 fix w3 1m WAIT!"),
        "{text}"
    );
    let mut old = snap.clone();
    old.capabilities.peer_activity = false;
    let (chain, num) = numbered(&old);
    let before = flat(
        &layout::card(
            &View {
                snap: &old,
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
    let (chain, num) = numbered(&left);
    let gone = flat(
        &layout::card(
            &View {
                snap: &left,
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
    let (chain, num) = numbered(&answered);
    let settled = flat(
        &layout::card(
            &View {
                snap: &answered,
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
    if let Some((short, from, sent)) = ask {
        job.ask_id = Some(format!("ask-{short}"));
        snap.asks.push(Ask {
            correlation_id: format!("ask-{short}"),
            from_peer: from.into(),
            to_peer: peer.into(),
            to_peer_id: peer.into(),
            open: state == "running",
            failed: state == "failed",
            opened_at: Some(sent),
            closed_by: (state != "running").then(|| "recipient".into()),
            ..Default::default()
        });
    }
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
        }),
        ..Default::default()
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
    s
}

/* frames of the approved mock-up, exported from its generator: every card state at both
widths, S4's skip-level note and S5's box. Five changes are agreed: three the data forces, an
open ask reading "unacked" (the hub keeps no read receipts), the nudge text single-quoted
for the shell, and the 14-job chain named after its last job, as the design's rule says,
instead of the mock-up's "ship"; and two the user chose, a card that holds still (its glyph
and lists show ◆, its worker reads WORK) while the flow carries the spinner, and a top line
that names every online peer, in columns aligned under the first name, where the mock-up
counted the ones that did not fit. The mock-up's
other frames differ from these fixtures only where its sample data contradicts itself: ages
that disagree with its own times, one worker both working and idle, two spinners under one
worker, a queued job with an ask, and retry commands naming the mock-up's random job ids */
const FRAMES: &str = r#"
===== S1 narrow scope
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
chain deploy · 3/9 done · 1 fail · 2 run
──────────────────────────────────────────────

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
│ worker  cc · now WORK                      │
│ ask     1f0 cc>cc · 13:19 · acked ok       │
│ blocks  2 func ● · 3 perf ◆ · 4 deliv ●    │
│ times   sent 13:19 · ended 13:20 · took 1m │
│ prompt  split the cleanup review into      │
│         dimensions                         │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S1 narrow perf
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
chain deploy · 3/9 done · 1 fail · 2 run
──────────────────────────────────────────────

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

┌◆ 3 perf · running 12m · stage 2 of 5 ──────┐
│ worker  codex IDLE! 5m · turn ended        │
│ ask     9a1 cc─▸─codex · 13:30 · unacked   │
│         ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ waiting 12m           │
│ needs   1 scope ●                          │
│ blocks  7 synth ○                          │
│ times   sent 13:30 · running 12m           │
│ prompt  review perf: sweep lock time and   │
│         inbox filter cost at 100k rows     │
│ nudge   amesh peer notify codex 'perf?'    │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S4 narrow audit
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
chain verify · 4/6 done · 1 run
──────────────────────────────────────────────

        ● 1 audit codex 4m ─────┐
    ┌───┴───┐                   │
    ●       ●                   │
 2 fix-a 3 fix-b                │
    └───┬───┘                   │
        ● 4 review codex 3m     │
        │                       │
        ◆ 5 deploy cc 1m WAIT!  │
        │                       │
        ○ 6 verify pi-3 ◀───────┘

┌● 1 audit · done · took 4m · stage 1 of 5 ──┐
│ result  12 stale docs found, split in two  │
│         halves                             │
│ worker  codex · now IDLE                   │
│ ask     a11 cc>codex · 13:30 · acked ok    │
│ blocks  2 fix-a ● · 3 fix-b ● · 6 verify ○ │
│ times   sent 13:30 · ended 13:34 · took 4m │
│ prompt  list stale docs in the repo        │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S4 narrow deploy
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
chain verify · 4/6 done · 1 run
──────────────────────────────────────────────

        ● 1 audit codex 4m ─────┐
    ┌───┴───┐                   │
    ●       ●                   │
 2 fix-a 3 fix-b                │
    └───┬───┘                   │
        ● 4 review codex 3m     │
        │                       │
        ◆ 5 deploy cc 1m WAIT!  │
        │                       │
        ○ 6 verify pi-3 ◀───────┘

┌◆ 5 deploy · running 1m · stage 4 of 5 ─────┐
│ worker  cc WAIT! 40s                       │
│         needs your Bash permission         │
│ ask     d3a9 codex─▸─cc · 13:41 · unacked  │
│ needs   4 review ●                         │
│ blocks  6 verify ○                         │
│ times   sent 13:41 · running 1m            │
│ prompt  install the build and restart the  │
│         hub                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S5 narrow scope
7 online · ⠋ cc     ○ codex  ○ pi     ○ pi-2
           ○ pi-3   ⠋ u1     ⠋ u2
chain deploy · 6/14 done · 1 fail · 3 run
──────────────────────────────────────────────

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
│ worker  cc · now WORK                      │
│ ask     1f0 cc>cc · 13:10 · acked ok       │
│ blocks  2 func ● · 3 perf ◆ · 4 deliv ●    │
│ times   sent 13:10 · ended 13:11 · took 1m │
│ prompt  split the release review           │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S5 narrow sec
7 online · ⠋ cc     ○ codex  ○ pi     ○ pi-2
           ○ pi-3   ⠋ u1     ⠋ u2
chain deploy · 6/14 done · 1 fail · 3 run
──────────────────────────────────────────────

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

┌× 5 sec · failed 6m ago · stage 2 of 5 ─────┐
│ reason  sec review could not run its       │
│         fixture                            │
│ worker  pi-2 · now IDLE                    │
│ ask     3a3 cc>pi-2 · 13:12 · acked failed │
│ needs   1 scope ●                          │
│ blocks  12 synth ○                         │
│ times   sent 13:12 · failed 13:20          │
│ prompt  review the release for sec         │
│ retry   amesh jobs update job-58851116     │
│         --state queued                     │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S1 wide upg
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
chain deploy · 3/9 done · 1 fail · 2 run
────────────────────────────────────────────────────────────────────────────────────────────────────

● 1 scope ─┬─ ● 2 func ──┬── ○ 7 synth ── ○ 8 fix ── ○ 9 deploy
           ├─ ◆ 3 perf ──┤
           ├─ ● 4 deliv ─┤
           ├─ × 5 sec ───┤
           └─ ⠋ 6 upg ───┘

┌◆ 6 upg · running 6m · stage 2 of 5 ──────────────────────────────────────────────────────────────┐
│ worker  pi-3 WORK · turn 6m                     │ prompt  review the upgrade path for old state  │
│ ask     7c2 cc─▸─pi-3 · 13:36 · unacked         │         files and mixed versions               │
│ needs   1 scope ●                               │                                                │
│ blocks  7 synth ○                               │                                                │
│ times   sent 13:36 · running 6m                 │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain
===== S5 wide scope
7 online · ⠋ cc  ○ codex  ○ pi  ○ pi-2  ○ pi-3  ⠋ u1  ⠋ u2
chain deploy · 6/14 done · 1 fail · 3 run
────────────────────────────────────────────────────────────────────────────────────────────────────

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
│ worker  cc · now WORK                           │                                                │
│ ask     1f0 cc>cc · 13:10 · acked ok            │                                                │
│ blocks  2 func ● · 3 perf ◆ · 4 deliv ●         │                                                │
│ times   sent 13:10 · ended 13:11 · took 1m      │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain
===== S1 narrow synth
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
chain deploy · 3/9 done · 1 fail · 2 run
──────────────────────────────────────────────

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

┌○ 7 synth · queued · stage 3 of 5 ──────────┐
│ worker  cc (when ready)                    │
│ waits   3 perf ◆  5 sec ×  6 upg ◆         │
│ blocked 5 sec failed: retry it first       │
│ blocks  8 fix ○                            │
│ times   not sent yet                       │
│ prompt  merge the five reviews into one    │
│         decision list                      │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S1 wide synth
4 online · ⠋ cc  ○ codex  ⠋ pi  ⠋ pi-3
chain deploy · 3/9 done · 1 fail · 2 run
────────────────────────────────────────────────────────────────────────────────────────────────────

● 1 scope ─┬─ ● 2 func ──┬── ○ 7 synth ── ○ 8 fix ── ○ 9 deploy
           ├─ ◆ 3 perf ──┤
           ├─ ● 4 deliv ─┤
           ├─ × 5 sec ───┤
           └─ ⠋ 6 upg ───┘

┌○ 7 synth · queued · stage 3 of 5 ────────────────────────────────────────────────────────────────┐
│ worker  cc (when ready)                         │ prompt  merge the five reviews into one        │
│ waits   3 perf ◆  5 sec ×  6 upg ◆              │         decision list                          │
│ blocked 5 sec failed: retry it first            │                                                │
│ blocks  8 fix ○                                 │                                                │
│ times   not sent yet                            │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain
===== S4 narrow review
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
chain verify · 4/6 done · 1 run
──────────────────────────────────────────────

        ● 1 audit codex 4m ─────┐
    ┌───┴───┐                   │
    ●       ●                   │
 2 fix-a 3 fix-b                │
    └───┬───┘                   │
        ● 4 review codex 3m     │
        │                       │
        ◆ 5 deploy cc 1m WAIT!  │
        │                       │
        ○ 6 verify pi-3 ◀───────┘

┌● 4 review · done · took 3m · stage 3 of 5 ─┐
│ result  approved with 1 nit                │
│ worker  codex · now IDLE                   │
│ ask     d44 cc>codex · 13:36 · acked ok    │
│ needs   2 fix-a ●  3 fix-b ●               │
│ blocks  5 deploy ◆                         │
│ times   sent 13:36 · ended 13:39 · took 3m │
│ prompt  review both halves together        │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S4 wide deploy
4 online · ! cc  ○ codex  ⠋ pi  ○ pi-3
chain verify · 4/6 done · 1 run
────────────────────────────────────────────────────────────────────────────────────────────────────

● 1 audit ─┬─ ● 2 fix-a ─┬── ● 4 review ── ◆ 5 deploy ── ○ 6 verify
           └─ ● 3 fix-b ─┘                               ↑ also needs 1 audit

┌◆ 5 deploy · running 1m · stage 4 of 5 ───────────────────────────────────────────────────────────┐
│ worker  cc WAIT! 40s                            │ prompt  install the build and restart the hub  │
│         needs your Bash permission              │                                                │
│ ask     d3a9 codex─▸─cc · 13:41 · unacked       │                                                │
│ needs   4 review ●                              │                                                │
│ blocks  6 verify ○                              │                                                │
│ times   sent 13:41 · running 1m                 │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain
===== S5 wide sec
7 online · ⠋ cc  ○ codex  ○ pi  ○ pi-2  ○ pi-3  ⠋ u1  ⠋ u2
chain deploy · 6/14 done · 1 fail · 3 run
────────────────────────────────────────────────────────────────────────────────────────────────────

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

┌× 5 sec · failed 6m ago · stage 2 of 5 ───────────────────────────────────────────────────────────┐
│ reason  sec review could not run its fixture    │ prompt  review the release for sec             │
│ worker  pi-2 · now IDLE                         │ retry   amesh jobs update job-58851116 --state │
│ ask     3a3 cc>pi-2 · 13:12 · acked failed      │         queued                                 │
│ needs   1 scope ●                               │                                                │
│ blocks  12 synth ○                              │                                                │
│ times   sent 13:12 · failed 13:20               │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain
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
    assert_eq!(span("cc · now WORK").fg, Some(theme.worker));
    assert_eq!(span("acked ok").fg, Some(theme.done));
    assert_eq!(span("j/k move").fg, Some(theme.dim));
    let mut plain = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
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
        snap: &snap,
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
            .take_while(|l| l.contains(key) || l.starts_with("│         "))
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
        (1, 1),
        "the smallest card keeps a line of each:\n{least}"
    );
    for h in [12, 20, 30] {
        let text = layout::card(&view, "scope", 46, None, Fit::Rows(h)).text();
        assert_eq!(text.lines().count(), h, "exactly the rows given:\n{text}");
        let (r, p) = (rows(&text, "result"), rows(&text, "prompt"));
        assert!(
            r >= p.min(result) && r + p == h - 6,
            "the result takes the rows first, the prompt the rest:\n{text}"
        );
        assert_eq!(
            text.matches('…').count(),
            usize::from(r < result) + usize::from(p < prompt),
            "a field cut short says so:\n{text}"
        );
    }
    let roomy = layout::card(&view, "scope", 46, None, Fit::Rows(60)).text();
    assert_eq!(roomy.lines().count(), 60, "the frame stretches:\n{roomy}");
    assert!(
        !roomy.contains('…') && roomy.lines().nth(58).unwrap().starts_with("│  "),
        "all of it shows, then blank rows down to the frame:\n{roomy}"
    );
    let wide = layout::card(&view, "scope", 100, None, Fit::Rows(12)).text();
    assert_eq!(wide.lines().count(), 12, "{wide}");
    let right = wide
        .lines()
        .filter(|l| l.split('│').nth(2).is_some_and(|r| !r.trim().is_empty()))
        .count();
    assert_eq!(right, 10, "the prompt fills the right column:\n{wide}");
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
        snap.jobs
            .iter_mut()
            .find(|j| j.job_id == "job-58851116")
            .unwrap()
            .state = job.into();
    };
    let (chain, num) = numbered(&snap);
    let tail = |snap: &Snapshot| {
        let text = flat(
            &layout::card(
                &View {
                    snap,
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
    let view = |snap| View {
        snap,
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
        top[1].starts_with("chain review")
            && top[1].ends_with(" · 2/3 done")
            && width(&top[1]) <= 30,
        "a long chain name gives way to the counts:\n{}",
        top[1]
    );
    let mut app = app_with(wordy.clone());
    app.sel = Some("b".into());
    let head = |app: &mut App, cols| screen_text(app, cols, 30);
    let narrow = head(&mut app, 46);
    let broad = head(&mut app, 160);
    assert!(
        narrow[1].ends_with(" · 2/3 done") && broad[1].ends_with(" · 2/3 done"),
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
        row_of(&mut app, "8 fix", 0).contains("WAIT!")
            && !row_of(&mut app, "9 deploy", 0).contains("WAIT!"),
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
            head.ends_with("0/40 done · 20 fail · 20 run") && width(&head) <= cols,
            "{cols}: {head}"
        );
    }
    assert!(screen_text(&mut app, 60, 12)[1].starts_with("independent jobs · "));
    assert!(screen_text(&mut app, 46, 12)[1].starts_with("independent jo… · "));
    assert_eq!(
        layout::wrap("中文", 1),
        ["…", "…"],
        "a character wider than a whole line gives way to …"
    );
    assert_eq!(layout::wrap("中文", 2), ["中", "文"]);
}

#[test]
fn counts_wider_than_the_pane_keep_their_numbers() {
    let head = |counts, cols| super::header("independent jobs", counts, cols);
    assert_eq!(head([0, 10000, 10000, 10000], 30), "0/10000● 10000× 10000◆");
    assert_eq!(head([0, 10000, 10000, 10000], 36), "0/10000● 10000× 10000◆");
    assert_eq!(
        head([0, 10000, 10000, 10000], 37),
        "0/10000 done · 10000 fail · 10000 run"
    );
    assert_eq!(
        head([3, 9, 1, 2], 46),
        "independent jobs · 3/9 done · 1 fail · 2 run"
    );
    assert_eq!(head([3, 9, 0, 0], 30), "independent jobs · 3/9 done");
    for cols in 30..60 {
        let text = head([12345, 99999, 54321, 33333], cols);
        assert!(
            width(&text) <= cols
                && ["12345", "99999", "54321", "33333"]
                    .iter()
                    .all(|n| text.contains(n)),
            "{cols}: {text}"
        );
    }
}

#[test]
fn the_full_card_reaches_the_end_of_a_long_text() {
    let mut snap = proto_s1();
    snap.detail = Some(model::Detail {
        job_id: "func".into(),
        prompt: format!("{} TAIL_SENTINEL", "word ".repeat(20_000)),
        result: Some(format!("{} RESULT_END", "finding ".repeat(2_000))),
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
    snap.detail = Some(model::Detail {
        job_id: "func".into(),
        prompt: "word ".repeat(200_000),
        result: None,
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
    app.apply(Ok(Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }));
    let head = screen_text(&mut app, 30, 12)[1].clone();
    assert_eq!(head, "0/1000* 500x 500>", "the glyph counts, in ASCII");
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
    let worker = off.iter().find(|l| l.contains("worker  w3")).unwrap();
    assert!(
        !row.contains("WAIT!") && worker.contains("w3 WAIT! 2m"),
        "the flow row blinks, the card does not:\n{row}\n{worker}"
    );
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
    let pieces = layout::presence(&snap, 120, '⠹', true);
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
        joined(&layout::presence(&snap, 120, '⠹', false)),
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
        joined(&layout::presence(&snap, 33, '⠹', true)),
        "5 online · ⠹ d-work  ! e-wait  +3"
    );
    assert_eq!(
        joined(&layout::presence(&snap, 32, '⠹', true)),
        "5 online · ! e-wait  +4",
        "waiting outranks working"
    );
    assert_eq!(
        joined(&layout::presence(&snap, 20, '⠹', true)),
        "5 online",
        "no token fits: the count alone"
    );
}

#[test]
fn the_top_line_says_so_when_nobody_is_online_or_the_hub_cannot_tell() {
    let nobody = with_roster(Snapshot::default(), vec![peer("gone", "offline", None)]);
    assert_eq!(
        joined(&layout::presence(&nobody, 60, '⠹', true)),
        "no peers online"
    );
    let old = Snapshot {
        roster: vec![peer("x", "online", None)],
        ..Default::default()
    };
    assert_eq!(
        joined(&layout::presence(&old, 80, '⠹', true)),
        "peers: restart the hub on the current amesh to list them"
    );
    assert!(width(&joined(&layout::presence(&old, 20, '⠹', true))) <= 20);
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
    assert!(
        lines[1].starts_with("chain deploy · "),
        "{}",
        lines.join("\n")
    );
    assert!(lines[2].starts_with("───"), "{}", lines.join("\n"));
    let mut empty = app_with(with_roster(Snapshot::default(), roster));
    let lines = screen_text(&mut empty, 46, 20);
    assert_eq!(
        lines[0], "2 online · ⠋ cc  ○ pi",
        "the line shows before any job exists"
    );
    assert_eq!(lines[1], "no jobs here yet");
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
        joined(&layout::presence(&snap, 40, '⠹', true)),
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
        joined(&layout::presence(&snap, 80, '⠹', true)),
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
        let line = joined(&layout::presence(&wide, cols, '⠹', true));
        assert!(width(&line) <= cols, "{cols}: {line}");
    }
    assert_eq!(
        joined(&layout::presence(&wide, 30, '⠹', true)),
        "2 online · ● 审查  ● 数据中心"
    );
}

#[test]
fn the_smallest_pane_keeps_the_top_line_and_the_header() {
    let mut app = app_with(with_roster(s1(), vec![peer("cc", "online", Some("work"))]));
    let lines = screen_text(&mut app, 46, 8);
    assert_eq!(lines.len(), 8, "{}", lines.join("\n"));
    assert_eq!(lines[0], "1 online · ⠋ cc");
    assert!(
        lines[1].starts_with("chain deploy · "),
        "{}",
        lines.join("\n")
    );
}

fn grid_text(snap: &Snapshot, cols: usize, rows: usize) -> Vec<String> {
    layout::presence_lines(snap, cols, rows, '⠹', true)
        .iter()
        .map(|line| joined(line).trim_end().to_string())
        .collect()
}

#[test]
fn a_top_line_that_does_not_fit_wraps_into_aligned_columns() {
    let snap = with_roster(
        Snapshot::default(),
        vec![
            peer("acme-web-frontend-pi-2", "online", Some("idle")),
            peer("acme-web-frontend-claude-code", "online", Some("work")),
            peer("acme-web-frontend-codex", "online", Some("idle")),
        ],
    );
    assert_eq!(
        grid_text(&snap, 80, 4),
        [
            "3 online · ⠹ acme-web-frontend-claude-code  ○ acme-web-frontend-codex",
            "           ○ acme-web-frontend-pi-2",
        ],
        "every name shows, each line indented under the first name"
    );
    assert_eq!(
        grid_text(&snap, 120, 4),
        ["3 online · ⠹ acme-web-frontend-claude-code  ○ acme-web-frontend-codex  ○ acme-web-frontend-pi-2"],
        "one line while they all fit"
    );
    assert_eq!(grid_text(&snap, 60, 4).len(), 3, "one name per line");
    for cols in [42, 45, 60, 80, 94, 95] {
        for line in grid_text(&snap, cols, 4) {
            assert!(width(&line) <= cols, "{cols}: {line}");
        }
    }
    assert_eq!(
        layout::presence_lines(&snap, 80, 1, '⠹', true),
        vec![layout::presence(&snap, 80, '⠹', true)],
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
        peer("acme-web-frontend-pi-2", "online", Some("idle")),
    ];
    let mut app = app_with(with_roster(s1(), roster.clone()));
    let lines = screen_text(&mut app, 80, 30);
    assert_eq!(
        lines[..2],
        [
            "3 online · ⠋ acme-web-frontend-claude-code  ○ acme-web-frontend-codex",
            "           ○ acme-web-frontend-pi-2",
        ],
        "{}",
        lines.join("\n")
    );
    assert!(
        lines[2].starts_with("chain deploy · "),
        "{}",
        lines.join("\n")
    );
    assert!(lines[3].starts_with("───"), "{}", lines.join("\n"));
    let mut empty = app_with(with_roster(Snapshot::default(), roster.clone()));
    let lines = screen_text(&mut empty, 80, 20);
    assert_eq!(lines[2], "no jobs here yet", "{}", lines.join("\n"));
    let mut small = app_with(with_roster(s1(), roster));
    let lines = screen_text(&mut small, 80, 8);
    assert_eq!(lines.len(), 8, "{}", lines.join("\n"));
    assert!(
        lines[0].ends_with("+1"),
        "the smallest pane keeps one line and counts the rest:\n{}",
        lines.join("\n")
    );
    assert!(
        lines[1].starts_with("chain deploy · "),
        "{}",
        lines.join("\n")
    );
}

#[test]
fn the_view_moves_down_at_once_and_back_up_once_the_roster_held_still() {
    let long = |id: &str| peer(&format!("acme-web-frontend-{id}"), "online", Some("idle"));
    let two = vec![long("claude-code"), long("codex")];
    let three = vec![long("claude-code"), long("codex"), long("pi-2")];
    let header = |app: &mut App| {
        let lines = screen_text(app, 80, 30);
        lines
            .iter()
            .position(|l| l.starts_with("chain deploy · "))
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
        screen_text(&mut app, 80, 8)[1].get(..13),
        Some("chain deploy "),
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
            lines[2].starts_with("───") && lines[2].ends_with(" 42 events ─"),
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
fn the_events_tag_joins_the_block_tag_and_gives_way_first_when_narrow() {
    let mut app = app_with(with_events(mixed(), 42));
    let wide = screen_text(&mut app, 46, 60);
    assert!(
        wide[2].ends_with(" 42 events · +2 run · 1/2 ─"),
        "{}",
        wide[2]
    );
    let narrow = screen_text(&mut app, 30, 60);
    assert!(
        narrow[2].ends_with("─ +2 run · 1/2 ─") && !narrow[2].contains("events"),
        "{}",
        narrow[2]
    );
    app.opts.ascii = true;
    let ascii = screen_text(&mut app, 46, 60);
    assert!(
        ascii[2].ends_with(" 42 events | +2 run | 1/2 -"),
        "{}",
        ascii[2]
    );
}

#[test]
fn the_view_without_jobs_shows_the_count_on_its_message_row() {
    let mut app = app_with(with_events(Snapshot::default(), 42));
    let lines = screen_text(&mut app, 46, 20);
    assert!(
        lines[1].starts_with("no jobs here yet") && lines[1].ends_with("42 events"),
        "{}",
        lines[1]
    );
    assert_eq!(width(&lines[1]), 46);
    /* wide enough that the error line leaves room for the count, so only the rule for an
    error keeps it off */
    let fresh = screen_text(&mut app, 100, 20);
    assert!(fresh[1].ends_with("42 events"), "{}", fresh[1]);
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
    assert!(fresh[2].ends_with(" 42 events ─"), "{}", fresh[2]);
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

/* the fan-out of the design: one question to three peers; codex has answered, pi's turn
ended without an answer */
fn fan_out() -> Snapshot {
    with_asks(
        Snapshot {
            captured_at: 1000,
            ..Default::default()
        },
        vec![
            lone("ask-1223c9e0", CC, PI2, Some(820)),
            lone("ask-a3a07b51", CC, PI, Some(820)),
            closed(
                lone("ask-a904f2d6", CC, CODEX, Some(820)),
                950,
                "recipient",
                false,
                "2 findings, both fixed",
            ),
        ],
        vec![
            doing(CC, "idle", 830, None),
            doing(CODEX, "idle", 950, None),
            doing(PI, "idle", 940, None),
            doing(PI2, "work", 830, None),
        ],
    )
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
fn the_asks_are_listed_open_by_age_then_closed_latest_first() {
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
        ],
        vec![],
    );
    let ids: Vec<&str> = super::asks::order(&snap)
        .iter()
        .map(|a| a.correlation_id.as_str())
        .collect();
    assert_eq!(ids, ["ask-a", "ask-b", "ask-legacy", "ask-c2", "ask-c1"]);
}

#[test]
fn each_ask_reads_in_the_words_of_the_job_card() {
    use layout::Tone;
    let base = Snapshot {
        captured_at: 1000,
        ..Default::default()
    };
    let open = |p: Option<Peer>| {
        let snap = with_asks(
            base.clone(),
            vec![lone("ask-x", CC, PI, Some(820))],
            p.into_iter().collect(),
        );
        super::asks::state(&snap, &snap.asks[0])
    };
    assert_eq!(
        open(Some(doing(PI, "work", 900, None))),
        ("waiting 3m".into(), Tone::Run)
    );
    assert_eq!(
        open(Some(doing(PI, "idle", 940, None))),
        ("IDLE! 1m".into(), Tone::Fail)
    );
    assert_eq!(
        open(Some(doing(PI, "wait", 960, None))),
        ("WAIT! 40s".into(), Tone::Wait)
    );
    assert_eq!(
        open(Some(peer(PI, "offline", None))),
        ("offline".into(), Tone::Fail)
    );
    assert_eq!(open(None), ("left the hub".into(), Tone::Fail));
    let mut quiet = with_asks(
        base.clone(),
        vec![lone("ask-x", CC, PI, Some(820))],
        vec![doing(PI, "idle", 940, None)],
    );
    quiet.capabilities.peer_activity = false;
    assert_eq!(
        super::asks::state(&quiet, &quiet.asks[0]),
        ("waiting 3m".into(), Tone::Run),
        "IDLE! needs the hub's activity reports"
    );
    let shut = |by: &str, failed: bool| {
        let snap = with_asks(
            base.clone(),
            vec![closed(
                lone("ask-x", CC, PI, Some(820)),
                950,
                by,
                failed,
                "r",
            )],
            vec![],
        );
        super::asks::state(&snap, &snap.asks[0])
    };
    assert_eq!(shut("recipient", false), ("acked ok".into(), Tone::Done));
    assert_eq!(shut("recipient", true), ("acked failed".into(), Tone::Fail));
    assert_eq!(shut("hand", false), ("closed by hand".into(), Tone::Done));
    assert_eq!(shut("hub", true), ("closed by hub".into(), Tone::Fail));
    assert_eq!(shut("", false), ("closed ok".into(), Tone::Done));
    assert_eq!(shut("", true), ("closed failed".into(), Tone::Fail));
}

#[test]
fn ask_rows_leave_out_the_shared_prefix_and_keep_the_state_whole() {
    let snap = fan_out();
    let order = super::asks::order(&snap);
    let text = |cols: usize| -> Vec<String> {
        super::asks::rows(&snap, &order, cols)
            .iter()
            .map(|row| row_text(row))
            .collect()
    };
    let wide = text(100);
    assert!(
        wide[0].starts_with("1 ◆ 1223 …claude-code─▸─…pi-2  "),
        "{}",
        wide[0]
    );
    assert!(
        wide[0].contains("waiting 3m") && wide[0].contains("question ask-1223c9e0"),
        "{}",
        wide[0]
    );
    assert!(wide[1].contains("IDLE! 1m"), "{}", wide[1]);
    assert!(
        wide[2].starts_with("3 ● a904 …claude-code>…codex")
            && wide[2].contains("acked ok")
            && wide[2].contains("→ 2 findings, both fixed"),
        "{}",
        wide[2]
    );
    assert!(wide.iter().all(|row| width(row) <= 100));
    let narrow = text(46);
    assert!(
        narrow[0].trim_end().ends_with("waiting 3m"),
        "no room for a preview: {}",
        narrow[0]
    );
    let mut far = fan_out();
    far.asks.push(lone(
        "ask-0594aa3c",
        "crypto-software-agile-pi-2",
        "crypto-software-agile-pi-3",
        None,
    ));
    let order = super::asks::order(&far);
    let rows: Vec<String> = super::asks::rows(&far, &order, 46)
        .iter()
        .map(|row| row_text(row))
        .collect();
    let gone = rows.iter().find(|r| r.contains("0594")).unwrap();
    assert!(
        gone.trim_end().ends_with("left the hub") && gone.contains('…') && width(gone) <= 46,
        "the route gives way: {gone}"
    );
    let mut anon = fan_out();
    anon.asks
        .push(lone("ask-anon0001", "anonymous", PI, Some(900)));
    let order = super::asks::order(&anon);
    let rows = super::asks::rows(&anon, &order, 100);
    assert!(
        rows.iter()
            .any(|row| row_text(row).contains(" anonymous─▸─…pi ")),
        "a name without a row keeps the prefix: {:?}",
        rows.iter().map(|r| row_text(r)).collect::<Vec<_>>()
    );
    let mut across = fan_out();
    across.asks.push(lone("ask-1167aa00", "far", PI, Some(900)));
    across.peers.push(doing("far", "idle", 900, None));
    let order = super::asks::order(&across);
    let rows: Vec<String> = super::asks::rows(&across, &order, 46)
        .iter()
        .map(|row| row_text(row))
        .collect();
    assert!(
        rows.iter().any(|r| r.contains("far─▸─…pi "))
            && rows.iter().any(|r| r.contains("…claude-code─▸─…pi-2")),
        "a sender from another circle keeps its name and leaves the prefix to the circle's own: {rows:?}"
    );
    let mut all = fan_out();
    all.roster.clear();
    let order = super::asks::order(&all);
    let rows: Vec<String> = super::asks::rows(&all, &order, 46)
        .iter()
        .map(|row| row_text(row))
        .collect();
    assert!(
        [PI2, PI, CODEX]
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
    let rows: Vec<String> = super::asks::rows(&kin, &order, 200)
        .iter()
        .map(|row| row_text(row))
        .collect();
    assert!(
        rows.iter().any(|r| r.contains(" anonymous─▸─…pi-2 ")),
        "anonymous keeps its name and leaves the prefix alone: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains(&format!(" {foreign}─▸─…pi "))),
        "a name from another circle keeps its whole name: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains(" …claude-code─▸─openhitls-sm2-opt-pi-3 ")),
        "a peer that left keeps its whole name: {rows:?}"
    );
}

#[test]
fn the_ask_card_puts_the_reply_first_and_gives_a_command_where_one_is_due() {
    let snap = fan_out();
    let find = |cid: &str| snap.asks.iter().find(|a| a.correlation_id == cid).unwrap();
    let text = |cid: &str, cols: usize| {
        super::asks::card(&snap, find(cid), 1, cols, None, Fit::Rows(24)).text()
    };
    let mut long = fan_out();
    long.asks[2].reply = Some("word ".repeat(80));
    let codex = super::asks::card(&long, &long.asks[2], 3, 46, None, Fit::Rows(30)).text();
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
    let whole = super::asks::card(&long, &long.asks[2], 3, 46, None, Fit::Whole).text();
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
    assert!(pi.contains("IDLE! 1m · turn ended"), "{pi}");
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
    gone.peers.retain(|p| p.peer_id != PI);
    let left = super::asks::card(&gone, &gone.asks[1], 2, 90, None, Fit::Rows(12)).text();
    assert!(
        left.contains("openhitls-sm2-opt-pi · left the hub")
            && left.contains(
                "│ close   amesh peer ack ask-a3a07b51 --failed true --message 'recipient left'"
            ),
        "{left}"
    );
    let mut old = fan_out();
    old.asks[0].opened_at = None;
    let legacy = super::asks::card(&old, &old.asks[0], 1, 46, None, Fit::Rows(20)).text();
    assert!(
        legacy.contains("open, age unknown")
            && legacy.contains("sent before the hub kept the time"),
        "{legacy}"
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
    assert!(lines[at + 3].starts_with("1 ◆ 1223"), "{}", lines[at + 3]);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("┌◆ 1 ask 1223 · waiting 3m")),
        "the first ask's card"
    );
    assert!(lines.last().unwrap().ends_with("a jobs"));
    app.key(KeyCode::Char('a'));
    assert!(
        screen_text(&mut app, 100, 30)
            .iter()
            .any(|l| l.starts_with("no jobs here yet")),
        "a again goes back"
    );
}

#[test]
fn the_asks_screen_moves_jumps_finds_and_opens_the_full_card() {
    let mut app = app_with(fan_out());
    app.key(KeyCode::Char('a'));
    screen_text(&mut app, 100, 30);
    app.key(KeyCode::Char('j'));
    assert_eq!(app.ask_sel.as_deref(), Some("ask-a3a07b51"));
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
    assert_eq!(app.ask_sel.as_deref(), Some("ask-a3a07b51"));
    let mut next = fan_out();
    next.asks[0] = closed(next.asks[0].clone(), 990, "recipient", true, "no toolchain");
    app.apply(Ok(next));
    let lines = screen_text(&mut app, 100, 30);
    assert_eq!(
        app.ask_sel.as_deref(),
        Some("ask-a3a07b51"),
        "the same ask, now first"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("┌◆ 1 ask a3a0")),
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
        .position(|l| text(l).starts_with("1 ◆ 1223"))
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
    let idle = lines[row + 1]
        .spans
        .iter()
        .find(|s| s.content.contains("IDLE!"))
        .unwrap();
    assert_eq!(
        idle.style.fg,
        Some(app.opts.theme.fail),
        "an unselected row keeps its colours"
    );
    let sel = &lines[row];
    assert!(sel
        .spans
        .iter()
        .all(|s| s.style.bg == Some(app.opts.theme.select_bg)));
    let waiting = sel
        .spans
        .iter()
        .find(|s| s.content.contains("waiting"))
        .unwrap();
    assert_eq!(
        waiting.style.fg,
        Some(app.opts.theme.run),
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
            .any(|l| l.starts_with("1 > 1223 ~claude-code->-~pi-2")),
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
    assert!(wide.ends_with(" 2 asks open · 1234567 events ─"), "{wide}");
    let both = " 2 asks open ·".width() + " 1234567 events ".width() + 1 + 4;
    let narrow = rule(&mut app, both - 1);
    assert!(
        narrow.ends_with(" 2 asks open ─"),
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
    assert!(row.ends_with("2 asks open · 3 events"), "{row}");
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
    assert!(row.ends_with("2 asks open"), "the events go first: {row}");
    let mut quiet = with_events(fan_out(), 3);
    quiet.asks.clear();
    let mut app = app_with(quiet);
    let lines = screen_text(&mut app, 100, 30);
    let row = lines
        .iter()
        .find(|l| l.starts_with("no jobs here yet"))
        .unwrap();
    assert!(
        row.ends_with("  3 events") && !lines.iter().any(|l| l.contains("press a")),
        "no asks, no count and no pointer: {lines:?}"
    );
}

#[test]
fn the_footer_names_a_while_the_view_holds_asks() {
    let mut app = app_with(s1());
    assert_eq!(
        screen_text(&mut app, 100, 40).last().unwrap(),
        "j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain"
    );
    let mut app = app_with(with_lone_asks(s1()));
    assert_eq!(
        screen_text(&mut app, 100, 40).last().unwrap(),
        "j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  tab chain  a asks"
    );
    assert_eq!(
        screen_text(&mut app, 46, 40).last().unwrap(),
        "j/k move  h/l stage  1-9 jump  f hint  a asks"
    );
}

#[test]
fn the_fetch_asks_for_the_text_the_card_shows() {
    let mut app = app_with(with_lone_asks(s1()));
    screen_text(&mut app, 100, 40);
    assert_eq!(app.wanted().as_deref(), Some("detail=perf"));
    app.key(KeyCode::Char('a'));
    assert_eq!(
        app.wanted().as_deref(),
        Some("ask=ask-1223c9e0"),
        "the asks screen starts on its first ask"
    );
    app.key(KeyCode::Char('j'));
    assert_eq!(app.wanted().as_deref(), Some("ask=ask-a3a07b51"));
}

#[test]
fn the_full_ask_card_shows_the_whole_text_once_it_arrives() {
    let mut snap = fan_out();
    snap.asks[0].text = "x ".repeat(200);
    let mut app = app_with(snap.clone());
    app.key(KeyCode::Char('a'));
    app.key(KeyCode::Enter);
    assert!(
        !screen_text(&mut app, 100, 30)
            .iter()
            .any(|l| l.contains("END")),
        "the preview until the detail comes"
    );
    snap.ask_detail = Some(model::AskDetail {
        correlation_id: "ask-1223c9e0".into(),
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
            lines.iter().any(|l| l.starts_with("1 ◆ 1223")),
            "the blank row gives way to the list at {cols}x8: {lines:?}"
        );
    }
    app.key(KeyCode::Char('j'));
    let lines = screen_text(&mut app, 46, 8);
    assert!(
        lines.iter().any(|l| l.starts_with("2 ◆ a3a0")),
        "the one row there is the selected ask: {lines:?}"
    );
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
    let closed = |ago: u64| {
        let mut snap = proto_s1();
        let now = snap.captured_at;
        let ask = snap.asks.iter_mut().find(|a| a.correlation_id == "ask-5b1");
        ask.unwrap().closed_at = Some(now - ago);
        let mut app = app_with(snap);
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
    assert!(line_with(&mut app, 100, 0, " 1223 ").contains("…claude-code─▸─…pi-2 "));
    assert!(
        line_with(&mut app, 100, 2, " 1223 ").contains("…claude-code──▸…pi-2 "),
        "the recipient works on it: the arrow steps"
    );
    assert!(
        line_with(&mut app, 100, 2, " a3a0 ").contains("…claude-code─▸─…pi "),
        "an idle recipient holds it still"
    );
}

#[test]
fn a_job_the_hub_just_sent_gets_a_dot_along_its_rail_then_lights() {
    let mut snap = proto_s1();
    let now = snap.captured_at;
    let ask = snap.asks.iter_mut().find(|a| a.correlation_id == "ask-7c2");
    ask.unwrap().opened_at = Some(now);
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
    snap.asks[sent].opened_at = Some(now);
    let mut app = app_with(snap);
    app.last_ok = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(5));
    app.sel = Some("func".into());
    assert!(
        line_with(&mut app, 100, 2, "5b1 ").contains("5b1 cc>codex "),
        "the hub stopped answering: no walk back"
    );
    let lines = screen_at(&mut app, 46, 0);
    assert!(
        lines
            .iter()
            .any(|l| l.trim_end() == "    ┌───────┬───────┼───────┬───────┐"),
        "and no dot into the job it sent"
    );
}

#[test]
fn nothing_is_just_now_after_the_capture_or_from_a_hub_without_one() {
    for captured_at in [0, at(13, 42)] {
        let mut snap = proto_s1();
        snap.captured_at = captured_at;
        let later = if captured_at == 0 {
            at(13, 43)
        } else {
            captured_at + 1
        };
        for ask in snap.asks.iter_mut() {
            match ask.correlation_id.as_str() {
                "ask-5b1" => ask.closed_at = Some(later),
                "ask-7c2" => ask.opened_at = Some(later),
                _ => {}
            }
        }
        let mut app = app_with(snap);
        app.sel = Some("func".into());
        assert!(
            line_with(&mut app, 100, 2, "5b1 ").contains("5b1 cc>codex "),
            "captured at {captured_at}: an ack stamped after it is not just now"
        );
        assert!(
            !screen_at(&mut app, 46, 2).iter().any(|l| l.contains('◉')),
            "captured at {captured_at}: nor a job sent after it"
        );
    }
}

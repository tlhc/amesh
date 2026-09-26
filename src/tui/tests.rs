use super::input::{Act, Input, Mode};
use super::layout::{self, width, Fit, View};
use super::model::{self, Activity, Ask, Job, Numbers, Peer, Snapshot};
use super::{App, Opts};
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
    assert_eq!(lines[0], "independent jobs · 1/2 done · 1 run", "{text}");
    assert!(
        lines[1].starts_with("───") && lines[1].trim_end().ends_with("─ +2 run · 1/2 ─"),
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
        text.starts_with("chain deploy · 3/9 done · 1 fail · 2 run\n")
            && text.lines().nth(1).unwrap().ends_with("─ +1 run · 2/2 ─")
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
        assert_eq!(rows[0], "independent jobs · 1/2 done · 1 run", "{text}");
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
widths, S4's skip-level note and S5's box. Four changes are agreed: three the data forces, an
open ask reading "unacked" (the hub keeps no read receipts), the nudge text single-quoted
for the shell, and the 14-job chain named after its last job, as the design's rule says,
instead of the mock-up's "ship"; and one the user chose, a card that holds still (its glyph
and lists show ◆, its worker reads WORK) while the flow carries the spinner. The mock-up's
other frames differ from these fixtures only where its sample data contradicts itself: ages
that disagree with its own times, one worker both working and idle, two spinners under one
worker, a queued job with an ask, and retry commands naming the mock-up's random job ids */
const FRAMES: &str = r#"
===== S1 narrow scope
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
│ ask     9a1 cc>codex · 13:30 · unacked     │
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
│ ask     d3a9 codex>cc · 13:41 · unacked    │
│ needs   4 review ●                         │
│ blocks  6 verify ○                         │
│ times   sent 13:41 · running 1m            │
│ prompt  install the build and restart the  │
│         hub                                │
└────────────────────────────────────────────┘
j/k move  h/l stage  digits jump  f hint
===== S5 narrow scope
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
chain deploy · 3/9 done · 1 fail · 2 run
────────────────────────────────────────────────────────────────────────────────────────────────────

● 1 scope ─┬─ ● 2 func ──┬── ○ 7 synth ── ○ 8 fix ── ○ 9 deploy
           ├─ ◆ 3 perf ──┤
           ├─ ● 4 deliv ─┤
           ├─ × 5 sec ───┤
           └─ ⠋ 6 upg ───┘

┌◆ 6 upg · running 6m · stage 2 of 5 ──────────────────────────────────────────────────────────────┐
│ worker  pi-3 WORK · turn 6m                     │ prompt  review the upgrade path for old state  │
│ ask     7c2 cc>pi-3 · 13:36 · unacked           │         files and mixed versions               │
│ needs   1 scope ●                               │                                                │
│ blocks  7 synth ○                               │                                                │
│ times   sent 13:36 · running 6m                 │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain
===== S5 wide scope
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
chain verify · 4/6 done · 1 run
────────────────────────────────────────────────────────────────────────────────────────────────────

● 1 audit ─┬─ ● 2 fix-a ─┬── ● 4 review ── ◆ 5 deploy ── ○ 6 verify
           └─ ● 3 fix-b ─┘                               ↑ also needs 1 audit

┌◆ 5 deploy · running 1m · stage 4 of 5 ───────────────────────────────────────────────────────────┐
│ worker  cc WAIT! 40s                            │ prompt  install the build and restart the hub  │
│         needs your Bash permission              │                                                │
│ ask     d3a9 codex>cc · 13:41 · unacked         │                                                │
│ needs   4 review ●                              │                                                │
│ blocks  6 verify ○                              │                                                │
│ times   sent 13:41 · running 1m                 │                                                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k ↑↓ move  h/l ←→ stage  digits jump  f hints  / find  enter card  esc back  tab chain
===== S5 wide sec
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
        let snap = match parts[0] {
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
    let tall = screen_text(&mut app, 46, 60).join("\n");
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
        top[0].starts_with("chain review")
            && top[0].ends_with(" · 2/3 done")
            && width(&top[0]) <= 30,
        "a long chain name gives way to the counts:\n{}",
        top[0]
    );
    let mut app = app_with(wordy.clone());
    app.sel = Some("b".into());
    let head = |app: &mut App, cols| screen_text(app, cols, 30);
    let narrow = head(&mut app, 46);
    let broad = head(&mut app, 160);
    assert!(
        narrow[0].ends_with(" · 2/3 done") && broad[0].ends_with(" · 2/3 done"),
        "the counts always show:\n{}\n{}",
        narrow[0],
        broad[0]
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
        let lines = screen_text(app, 46, 20);
        let first = lines[4].clone();
        let at = lines
            .iter()
            .find(|l| l.starts_with("└─ lines"))
            .cloned()
            .unwrap_or_default();
        (lines[3].clone(), first, at)
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
        let head = screen_text(&mut app, cols, 12)[0].clone();
        assert!(
            head.ends_with("0/40 done · 20 fail · 20 run") && width(&head) <= cols,
            "{cols}: {head}"
        );
    }
    assert!(screen_text(&mut app, 60, 12)[0].starts_with("independent jobs · "));
    assert!(screen_text(&mut app, 46, 12)[0].starts_with("independent jo… · "));
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
    let head = screen_text(&mut app, 30, 12)[0].clone();
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

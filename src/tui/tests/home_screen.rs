use super::*;

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
j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events  i knowledge
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
j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events  i knowledge
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
j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events  i knowledge
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
            events: true,
            ..Default::default()
        }
    );
    app.key(KeyCode::BackTab);
    screen_text(&mut app, 46, 24);
    assert_eq!(
        app.wanted(),
        Want {
            detail: Some("detail=tune-ladder".into()),
            events: true,
            ..Default::default()
        },
        "the job the chain's card shows"
    );
    app.key(KeyCode::Char('j'));
    screen_text(&mut app, 46, 24);
    assert_eq!(
        app.wanted(),
        Want {
            detail: None,
            events: true,
            ..Default::default()
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
            "j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events  i knowledge",
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
fn unnumbered_events_show_on_home_but_take_no_selection_and_a_restart_clears_them() {
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
    let (rows, _) = rows_of(&mut app);
    assert!(rows[0].starts_with("│▸ 00:16 notify"), "{rows:?}");
    app.apply(Ok(plain("e2", 1010)));
    let screen = screen_text(&mut app, 46, 24).join("\n");
    assert!(
        !screen.contains("notify"),
        "the old ring's row goes at once:\n{screen}"
    );
    assert!(app.home_rows.is_empty() && app.home_sel.is_none());
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
        lines[7], "j/k ↑↓ move  tab chain  enter card  c jobs  a asks  e events  i knowledge",
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

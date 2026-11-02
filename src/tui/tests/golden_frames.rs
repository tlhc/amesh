use super::*;

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

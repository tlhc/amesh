use super::*;

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

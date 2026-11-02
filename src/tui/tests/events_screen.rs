use super::*;

#[test]
fn event_sync_removes_deleted_rows_and_keeps_the_selection() {
    let mut app = events_app((1..=3).map(|s| ev(s, "notify", "cc", "pi", "n")).collect());
    app.key(KeyCode::Char('k'));
    app.take_events(Ok(vec![
        ev(2, "notify", "cc", "pi", "n"),
        ev(4, "notify", "cc", "pi", "new"),
    ]));
    assert_eq!(held_seqs(&app), [Some(2), Some(4)]);
    assert_eq!(app.ev_sel, Some((0, 2)));
}

#[test]
fn event_sync_follows_newest_when_the_selected_row_is_deleted() {
    let mut app = events_app((1..=3).map(|s| ev(s, "notify", "cc", "pi", "n")).collect());
    app.key(KeyCode::Char('k'));
    app.take_events(Ok(vec![ev(3, "notify", "cc", "pi", "n")]));
    assert_eq!(app.ev_sel, None);
    assert_eq!(held_seqs(&app), [Some(3)]);
}

#[test]
fn event_sync_clears_an_empty_ring_after_a_failed_read() {
    let mut app = events_app(vec![ev(1, "notify", "cc", "pi", "old")]);
    app.take_events(Err("hub unreachable".into()));
    assert_eq!(held_seqs(&app), [Some(1)]);
    app.take_events(Ok(Vec::new()));
    assert!(app.feed.is_empty());
    assert!(app.ev_error.is_none());
    assert!(screen_text(&mut app, 100, 40)
        .iter()
        .any(|line| line == "no events yet"));
}

#[test]
fn event_sync_drops_previous_hub_history() {
    let mut app = events_app(vec![ev(1, "notify", "cc", "pi", "old")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    app.apply(Ok(snap));
    app.take_events(Ok(vec![ev(1, "notify", "cc", "pi", "new")]));
    assert_eq!(app.feed.len(), 1);
    assert!(matches!(&app.feed[0], super::feed::Held(1, e) if e.message == "new"));
}

#[test]
fn event_sync_drops_the_old_ring_as_soon_as_the_hub_restarts() {
    let mut app = events_app(vec![
        ev(5, "notify", "cc", "pi", "old five"),
        ev(6, "notify", "cc", "pi", "old six"),
    ]);
    app.key(KeyCode::Char('k'));
    assert_eq!(app.ev_sel, Some((0, 5)));
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    app.apply(Ok(snap));
    assert!(app.feed.is_empty());
    assert_eq!(app.ev_sel, None);
    let screen = screen_text(&mut app, 100, 40).join("\n");
    assert!(
        !screen.contains("old") && !screen.contains("hub restarted"),
        "{screen}"
    );
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

fn held_seqs(app: &App) -> Vec<Option<u64>> {
    app.feed.iter().map(|h| h.1.seq).collect()
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
fn the_feed_replaces_the_ring_and_keeps_at_most_500() {
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
        vec![Some(2), Some(3)],
        "the response replaces the previous ring"
    );
    app.take_events(Ok(vec![ev(4, "chat_turn_delta", "", "", "partial")]));
    assert!(held_seqs(&app).is_empty(), "streaming deltas stay out");
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
    let texts: Vec<String> = app.feed.iter().map(|h| h.1.message.clone()).collect();
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
    app.take_events(Ok((1..=4)
        .map(|s| ev(s, "notify", "cc", "pi", &format!("n{s}")))
        .collect()));
    assert!(
        card(&mut app).contains("00:16:42"),
        "a new event leaves the selection where it is"
    );
    app.key(KeyCode::Char('j'));
    app.key(KeyCode::Char('j'));
    assert_eq!(app.ev_sel, None, "the newest row follows the newest again");
    app.take_events(Ok((1..=5)
        .map(|s| ev(s, "notify", "cc", "pi", &format!("n{s}")))
        .collect()));
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
fn a_hub_restart_replaces_the_old_ring_with_lower_seqs() {
    let mut app = events_app(vec![ev(5, "ask", "cc", "pi", "before")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap.clone()));
    app.apply(Ok(snap.clone()));
    app.take_events(Ok(vec![ev(1, "ask", "cc", "pi", "after")]));
    app.apply(Ok(snap));
    assert_eq!(held_seqs(&app), [Some(1)]);
    let lines = screen_text(&mut app, 100, 40).join("\n");
    assert!(lines.contains("after"));
    assert!(!lines.contains("before"));
    assert!(!lines.contains("hub restarted"));
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
            events: true,
            ..Default::default()
        },
        "the events screen wants no card text"
    );
    app.key(KeyCode::Char('a'));
    assert!(!app.wanted().events, "the asks screen reads no events");
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
fn a_hub_that_numbers_no_events_replaces_previous_hub_history() {
    let mut app = events_app(vec![unnumbered("before")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap));
    app.take_events(Ok(vec![unnumbered("after")]));
    assert_eq!(app.feed.len(), 1);
    let lines = screen_text(&mut app, 100, 40).join("\n");
    assert!(lines.contains("after"));
    assert!(!lines.contains("before"));
}

#[test]
fn a_hub_upgraded_to_number_its_events_still_replaces_the_ring() {
    let mut app = events_app(vec![unnumbered("old")]);
    let mut snap = app.snap.clone().unwrap().view;
    snap.hub_epoch = "e2".into();
    snap.captured_at = 1200;
    app.apply(Ok(snap));
    app.take_events(Ok(vec![ev(10, "notify", "cc", "pi", "fresh-ring")]));
    app.take_events(Ok(Vec::new()));
    let lines = screen_text(&mut app, 100, 40);
    assert!(
        app.feed.is_empty(),
        "an empty ring removes the previous response: {lines:?}"
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
            .map(|e| super::feed::Held(0, Box::new(e)))
            .collect();
        super::feed::items(&held)
            .iter()
            .map(|item| item.1.len())
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
    let held = vec![super::feed::Held(0, Box::new(far))];
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
        .map(|e| super::feed::Held(0, Box::new(e)))
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
    assert!(text(&mut app).contains("new five"), "{}", text(&mut app));
    app.key(KeyCode::Char('j'));
    assert!(text(&mut app).contains("new six"), "{}", text(&mut app));
    /* A replacement ring also removes the selected event. */
    app.take_events(Ok((10..508)
        .map(|s| ev(s, "notify", "cc", "pi", "n"))
        .collect()));
    assert_eq!(app.ev_sel, None);
    assert_eq!(held_seqs(&app).first(), Some(&Some(10)));
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
            let items = vec![super::feed::Item(0, vec![&event])];
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

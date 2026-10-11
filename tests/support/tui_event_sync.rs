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

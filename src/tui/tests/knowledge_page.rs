use super::*;

/* the knowledge page's rows, as the hub sends them */
fn note(
    id: &str,
    title: &str,
    status: &str,
    expires_at: i64,
    revoked_at: Option<i64>,
) -> knowledge::Item {
    knowledge::Item {
        id: id.into(),
        circle: "project-1".into(),
        title: title.into(),
        status: status.into(),
        created_at: 900,
        expires_at,
        revoked_at,
    }
}

fn page(items: Vec<knowledge::Item>, total: u64) -> knowledge::Page {
    knowledge::Page {
        captured_at: 1000,
        total,
        items,
    }
}

fn record(id: &str) -> knowledge::Detail {
    knowledge::Detail {
        id: id.into(),
        circle: "project-1".into(),
        title: format!("title {id}"),
        body: "first line\n    indented line".into(),
        applies_to: "src/hub".into(),
        evidence: "cargo test".into(),
        publisher_peer: "peer-a".into(),
        publisher_session: "session-a".into(),
        created_at: 900,
        expires_at: 7900,
        revoked_at: None,
        revoke_reason: None,
        revoked_by_peer: None,
        revoked_by_session: None,
        status: "active".into(),
    }
}

#[test]
fn knowledge_key_opens_the_page_and_wants_its_rows() {
    let mut app = app_with(mixed());
    app.sel = Some("lint".into());
    assert_eq!(app.wanted().detail.as_deref(), Some("detail=lint"));
    assert!(!app.wanted().knowledge);
    app.key(KeyCode::Char('i'));
    assert!(app.knowledge && !app.home);
    assert!(app.wanted().knowledge);
    /* the page shows its own rows; it must not keep asking for the hidden job's body */
    assert!(
        app.wanted().detail.is_none(),
        "knowledge asked for the hidden job's body"
    );
    /* and there is no row to open before the first page arrives */
    app.key(KeyCode::Enter);
    assert!(!app.full, "enter before the first page opened a blank card");
    app.take_knowledge(Ok(page(
        vec![note("kn-a", "alpha", "active", 7900, None)],
        1,
    )));
    assert_eq!(app.wanted().kdetail.as_deref(), Some("kn-a"));
    app.key(KeyCode::Char('i'));
    assert!(!app.knowledge && app.home, "i again goes home");
}

#[test]
fn knowledge_page_follows_the_selected_id_across_a_reorder() {
    let mut app = app_with(mixed());
    app.key(KeyCode::Char('i'));
    app.take_knowledge(Ok(page(
        vec![
            note("kn-a", "alpha", "active", 7900, None),
            note("kn-b", "beta", "active", 7900, None),
        ],
        2,
    )));
    assert_eq!(app.kn_sel.as_deref(), Some("kn-a"));
    app.key(KeyCode::Char('j'));
    assert_eq!(app.kn_sel.as_deref(), Some("kn-b"));
    /* the same page again must not pull the selection back to the first row */
    app.take_knowledge(Ok(page(
        vec![
            note("kn-a", "alpha", "active", 7900, None),
            note("kn-b", "beta", "active", 7900, None),
        ],
        2,
    )));
    assert_eq!(
        app.kn_sel.as_deref(),
        Some("kn-b"),
        "a refresh keeps the id"
    );
    app.take_knowledge(Ok(page(
        vec![
            note("kn-b", "beta", "active", 7900, None),
            note("kn-a", "alpha", "active", 7900, None),
        ],
        2,
    )));
    assert_eq!(
        app.kn_sel.as_deref(),
        Some("kn-b"),
        "the id stays selected when rows move"
    );
    /* digits typed against an order that has since changed would land on another row */
    let twelve = |swap: bool| {
        let mut rows: Vec<_> = (1..=12)
            .map(|n| {
                note(
                    &format!("kn-{n}"),
                    &format!("title{n}"),
                    "active",
                    7900,
                    None,
                )
            })
            .collect();
        if swap {
            rows.swap(0, 11);
        }
        page(rows, 12)
    };
    app.take_knowledge(Ok(twelve(false)));
    app.key(KeyCode::Char('1'));
    assert!(matches!(app.input.mode, Mode::Jump(_)));
    app.take_knowledge(Ok(twelve(true)));
    assert!(
        matches!(app.input.mode, Mode::Normal),
        "a changed order cancels a pending jump"
    );
    app.key(KeyCode::Enter);
    assert_eq!(app.kn_sel.as_deref(), Some("kn-b"));
}

#[test]
fn knowledge_page_falls_back_when_the_selected_id_leaves_the_window() {
    let mut app = app_with(mixed());
    app.key(KeyCode::Char('i'));
    app.take_knowledge(Ok(page(
        vec![
            note("kn-a", "alpha", "active", 7900, None),
            note("kn-b", "beta", "active", 7900, None),
        ],
        2,
    )));
    app.key(KeyCode::Char('j'));
    app.take_knowledge(Ok(page(
        vec![note("kn-a", "alpha", "active", 7900, None)],
        1,
    )));
    assert_eq!(
        app.kn_sel.as_deref(),
        Some("kn-b"),
        "a refresh alone does not move the selection"
    );
    screen_text(&mut app, 120, 40);
    assert_eq!(
        app.kn_sel.as_deref(),
        Some("kn-a"),
        "the screen falls back to a row that is here"
    );
}

#[test]
fn knowledge_detail_ignores_an_answer_for_another_id() {
    let mut app = app_with(mixed());
    app.key(KeyCode::Char('i'));
    app.take_knowledge(Ok(page(
        vec![
            note("kn-a", "alpha", "active", 7900, None),
            note("kn-b", "beta", "active", 7900, None),
        ],
        2,
    )));
    app.take_knowledge_detail("kn-b", Ok(Some(record("kn-b"))));
    assert!(
        app.kn_detail.is_none(),
        "an answer for a row the screen left is dropped"
    );
    app.take_knowledge_detail("kn-a", Ok(Some(record("kn-a"))));
    assert_eq!(app.kn_detail.as_ref().unwrap().id, "kn-a");
    /* a knowledge answer that arrives after the screen closed belongs to a card that is gone */
    app.key(KeyCode::Char('i'));
    app.key(KeyCode::Char('c'));
    app.key(KeyCode::Enter);
    assert!(app.full && !app.knowledge);
    app.take_knowledge_detail("kn-a", Ok(None));
    assert!(
        app.full,
        "a knowledge 404 closed a card the jobs screen opened"
    );
    /* nor may a late page retire a jump armed on the page now in front */
    app.input.mode = Mode::Jump("1".into());
    app.take_knowledge(Ok(page(
        vec![
            note("kn-b", "beta", "active", 7900, None),
            note("kn-a", "alpha", "active", 7900, None),
        ],
        2,
    )));
    assert!(
        matches!(app.input.mode, Mode::Jump(_)),
        "a knowledge page cancelled another screen's jump"
    );
}

#[test]
fn knowledge_404_clears_the_card_and_closes_the_full_card() {
    let mut app = app_with(mixed());
    app.key(KeyCode::Char('i'));
    app.take_knowledge(Ok(page(
        vec![note("kn-a", "alpha", "active", 7900, None)],
        1,
    )));
    app.take_knowledge_detail("kn-a", Ok(Some(record("kn-a"))));
    app.key(KeyCode::Enter);
    assert!(app.full && app.kn_sel.as_deref() == Some("kn-a"));
    /* a page that no longer holds the row must not move a full card: only the detail 404 does */
    app.key(KeyCode::Char('j'));
    assert_eq!(app.scroll, 1);
    app.take_knowledge(Ok(page(
        vec![note("kn-b", "beta", "active", 7900, None)],
        1,
    )));
    let text = screen_text(&mut app, 120, 40).join("\n");
    assert!(
        text.contains("first line"),
        "a list refresh moved the full card: {text}"
    );
    assert_eq!(app.scroll, 1, "a refresh reset the full card's scroll");
    /* nor may an empty page replace it before the 404 arrives */
    app.take_knowledge(Ok(page(vec![], 0)));
    let text = screen_text(&mut app, 120, 40).join("\n");
    assert!(
        text.contains("first line"),
        "an empty list replaced the card: {text}"
    );
    app.take_knowledge_detail("kn-a", Ok(None));
    assert!(app.kn_detail.is_none());
    assert!(!app.full, "404 closes the card");
    /* the page is empty now, so Enter has no row to open */
    app.key(KeyCode::Enter);
    assert!(!app.full, "enter on an empty page opened a blank card");
}

#[test]
fn knowledge_detail_failure_keeps_the_last_row_and_marks_it_stale() {
    let mut app = app_with(mixed());
    app.key(KeyCode::Char('i'));
    app.take_knowledge(Ok(page(
        vec![note("kn-a", "alpha", "active", 7900, None)],
        1,
    )));
    app.take_knowledge_detail("kn-a", Ok(Some(record("kn-a"))));
    app.take_knowledge_detail("kn-a", Err("hub unreachable".into()));
    assert!(app.kn_detail.is_some(), "a failed read keeps what it had");
    assert!(app.kn_detail_stale);
    app.take_knowledge_detail("kn-a", Ok(None));
    assert!(!app.kn_detail_stale, "404 left the stale warning up");
}

#[test]
fn knowledge_notice_names_the_page_and_the_cut() {
    assert_eq!(
        knowledge::notice(50, 105, 120).unwrap(),
        "50 of 105 shown · active first · the rest not listed"
    );
    assert_eq!(
        knowledge::notice(50, 105, 80).unwrap(),
        "50 of 105 · active first · rest cut"
    );
    assert!(
        knowledge::notice(5, 5, 120).is_none(),
        "nothing cut, nothing said"
    );
}

#[test]
fn knowledge_find_walks_loaded_titles_and_ids() {
    let mut app = app_with(mixed());
    app.key(KeyCode::Char('i'));
    app.take_knowledge(Ok(page(
        vec![
            note("kn-aaa", "alpha", "active", 7900, None),
            note("kn-bbb", "beta", "active", 7900, None),
        ],
        2,
    )));
    app.key(KeyCode::Char('/'));
    for c in "bbb".chars() {
        app.key(KeyCode::Char(c));
    }
    app.key(KeyCode::Enter);
    assert_eq!(
        app.kn_sel.as_deref(),
        Some("kn-bbb"),
        "a query can name an id"
    );
    app.key(KeyCode::Char('/'));
    for c in "alpha".chars() {
        app.key(KeyCode::Char(c));
    }
    app.key(KeyCode::Enter);
    assert_eq!(app.kn_sel.as_deref(), Some("kn-aaa"));
    /* the page carries no letter hints, so f must not leave a mode that eats the next key */
    app.key(KeyCode::Char('f'));
    app.key(KeyCode::Char('j'));
    assert_eq!(
        app.kn_sel.as_deref(),
        Some("kn-bbb"),
        "f left a hint mode that ate j"
    );
}

#[test]
fn knowledge_screen_renders_the_list_the_card_and_the_cut_notice() {
    let mut app = app_with(mixed());
    app.key(KeyCode::Char('i'));
    app.take_knowledge(Ok(page(
        vec![note("kn-a", "alpha note", "active", 7900, None)],
        105,
    )));
    app.take_knowledge_detail("kn-a", Ok(Some(record("kn-a"))));
    let text = screen_text(&mut app, 120, 40).join("\n");
    assert!(text.contains("knowledge · all circles"), "{text}");
    assert!(
        text.contains("1 of 105 shown · active first · the rest not listed"),
        "{text}"
    );
    assert!(text.contains("alpha note"), "{text}");
    assert!(text.contains("left "), "{text}");
    assert!(text.contains("author") && text.contains("peer-a"), "{text}");
    assert!(
        text.contains("checked") && text.contains("cargo test"),
        "{text}"
    );
    assert!(text.contains("publisher-reported"), "{text}");
    assert!(
        text.contains(&format!("{}indented line", " ".repeat(layout::KEY + 4))),
        "the card dropped the body's hanging indent: {text}"
    );
    /* the same rules at 30 columns: colw 26, room 18, half 9 */
    let mut detail = record("kn-a");
    detail.body = "intro\n\tone tab\n            deep\n    alpha beta gamma delta".into();
    let text = knowledge::card(&detail, 1000, 30).text();
    assert!(
        text.contains(&format!("│{}one tab", " ".repeat(13))),
        "a tab is not four columns: {text}"
    );
    assert!(
        text.contains(&format!("│{}deep", " ".repeat(18))),
        "the indent is not capped at half the room: {text}"
    );
    assert!(
        text.contains(&format!("│{}gamma delta", " ".repeat(13))),
        "the wrap lost its hanging indent: {text}"
    );
    /* a stored empty session reads as missing, not as a trailing slash */
    let mut detail = record("kn-a");
    detail.publisher_session = String::new();
    detail.revoked_at = Some(950);
    detail.revoked_by_peer = Some("peer-b".into());
    detail.revoked_by_session = Some(String::new());
    app.take_knowledge_detail("kn-a", Ok(Some(detail)));
    let text = screen_text(&mut app, 120, 40).join("\n");
    assert!(text.contains("peer-a / -"), "{text}");
    assert!(text.contains("peer-b / -"), "{text}");
    /* the circle stays a short tag even on a wide pane */
    let mut item = note("kn-a", "alpha", "active", 7900, None);
    item.circle = format!("project-{}", "c".repeat(32));
    let tag = &knowledge::row(&item, 1, 1000, true, 120)[1].0;
    assert!(
        layout::width(tag) <= 21,
        "circle tag took {} columns: {tag}",
        layout::width(tag)
    );
    let rows: Vec<_> = (1..=10)
        .map(|n| {
            note(
                &format!("kn-{n}"),
                &format!("title{n}"),
                "active",
                7900,
                None,
            )
        })
        .collect();
    app.take_knowledge(Ok(page(rows, 10)));
    let text = screen_text(&mut app, 120, 40);
    let col = |t: &str| text.iter().find_map(|l| l.find(t)).unwrap();
    assert_eq!(
        col("title9 "),
        col("title10"),
        "the number is not padded to the widest one"
    );
    let mut item = note("kn-a", "a very long title", "expired", 900, None);
    item.circle = "project-6c6ddbd092a8".into();
    let row = knowledge::row(&item, 1, 1000, true, 30);
    let shown = layout::fit(&row.iter().map(|(s, _)| s.as_str()).collect::<String>(), 30);
    assert!(
        shown.contains("expired"),
        "a narrow pane kept no room for the state: {shown}"
    );
}

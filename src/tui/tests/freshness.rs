use super::*;

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

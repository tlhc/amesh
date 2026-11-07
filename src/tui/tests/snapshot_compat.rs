use super::*;

fn row(condition: crate::wire::DeliveryCondition, stuck: Option<(usize, u64)>) -> String {
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
fn delivery_no_push() {
    assert_eq!(row(crate::wire::DeliveryCondition::NoPush, None), "no push");
}

#[test]
fn v1_header_counts_closed() {
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
fn v1_worker_is_the_raw_assignee() {
    let mut job = single_job("running");
    job.worker = None;
    job.relation = None;
    job.progress = None;
    job.assigned_peer = Some("codex".into());
    let text = single_card(job, vec![]);
    assert!(
        text.contains("worker codex") && !text.contains("IDLE!") && !text.contains("· now"),
        "{text}"
    );
}

#[test]
fn v1_closed_ask_is_neutral() {
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
fn v1_closed_glyph_is_plain() {
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
    let value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wire/job_relations.json"
    )))
    .unwrap();
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
fn delivery_queued() {
    assert_eq!(
        row(crate::wire::DeliveryCondition::Push, Some((2, 940))),
        "2 queued for 1m"
    );
}

#[test]
fn delivery_both() {
    assert_eq!(
        row(crate::wire::DeliveryCondition::NoPush, Some((2, 940))),
        "no push · 2 queued for 1m"
    );
}

#[test]
fn delivery_offline_is_empty() {
    assert!(row(crate::wire::DeliveryCondition::Offline, Some((2, 940))).is_empty());
}

#[test]
fn delivery_unknown_is_empty() {
    assert!(row(crate::wire::DeliveryCondition::Unknown, Some((2, 940))).is_empty());
}

#[test]
fn mark_work() {
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
fn mark_wait() {
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
fn mark_online() {
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
fn stuck_under_nopush() {
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
fn offline_hides_stuck() {
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
    let mut job = single_job("running");
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
    let text = single_text(
        job,
        vec![single_ask("boss", "old", "codex")],
        vec![old, new],
    );
    assert!(
        text.contains("7 queued for 1m") && text.contains("no push"),
        "{text}"
    );
}

#[test]
fn a_v1_card_claims_nothing_about_present_peers() {
    let ask = single_ask("boss", "old", "old");
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
    let mut ask = single_ask("x", "w", "w");
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
    let mut value: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wire/job_dispatch.json"
    )))
    .unwrap();
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
    let mut job = single_job("queued");
    job.dispatch_state = Some(crate::wire::DispatchState::Blocked {
        dependencies: vec![],
        dependency: "opaque-dep".into(),
        reason: crate::wire::BlockReason::Unknown,
    });
    let (_, grid) = single_scene(job, vec![], vec![]);
    assert_eq!(word_tone(&grid, "opaque-dep"), layout::Tone::Dim);
}

#[test]
fn a_finished_jobs_offline_worker_keeps_its_word() {
    let mut job = single_job("done");
    job.relation = Some(crate::wire::JobRelation::NoAsk);
    let mut p = peer("codex", "offline", None);
    p.liveness = Some(crate::wire::Liveness::Offline);
    let text = single_text(job, vec![], vec![p]);
    assert!(text.contains("codex · offline"), "{text}");
}

#[test]
fn a_malformed_v2_snapshot_keeps_the_last_frame_end_to_end() {
    let mut app = app_with(s1());
    let before = app.snap.as_ref().unwrap().jobs[0].job_id.clone();
    let mut value: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wire/job_relations.json"
    )))
    .unwrap();
    value["asks"][0].as_object_mut().unwrap().remove("state");
    app.receive(super::decoded(value));
    assert_eq!(app.snap.as_ref().unwrap().jobs[0].job_id, before);
    assert!(
        app.error.as_deref().unwrap().contains("lacks state"),
        "{:?}",
        app.error
    );
}

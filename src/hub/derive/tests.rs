use super::super::{Activity, Config, Inbox, QueuedRecord};
use super::*;
use serde_json::json;
fn hub() -> Hub {
    Hub {
        db: rusqlite::Connection::open_in_memory().unwrap(),
        peers: HashMap::new(),
        asks: HashMap::new(),
        jobs: HashMap::new(),
        schedules: HashMap::new(),
        sockets: HashMap::new(),
        recv_live: Default::default(),
        recv_known: Default::default(),
        owed: HashMap::new(),
        inbox: Inbox::default(),
        conn_gen: 0,
        events: vec![],
        event_seq: 0,
        batches: HashMap::new(),
        mcp_servers: HashMap::new(),
        sweep_at: 0,
        config: Config::default(),
        epoch: String::new(),
        commits: tokio::sync::watch::channel(0).0,
        activity: HashMap::new(),
    }
}
fn peer(h: &mut Hub, id: &str, name: &str, status: &str, state: &str, since: u64) {
    h.peers.insert(
        id.into(),
        Peer {
            peer_id: id.into(),
            name: name.into(),
            status: status.into(),
            path: String::new(),
            backend: String::new(),
            circle: "one".into(),
            description: String::new(),
            session_id: String::new(),
            last_seen: 0,
        },
    );
    if !state.is_empty() {
        h.activity.insert(
            id.into(),
            Activity {
                state: state.into(),
                since,
                observed_at: since,
                source: String::new(),
                reason: Some("permission".into()),
            },
        );
    }
}
fn ask() -> Ask {
    Ask {
        correlation_id: "a".into(),
        from_peer: "sender".into(),
        to_peer: "alias".into(),
        to_peer_id: "p".into(),
        text: String::new(),
        open: true,
        reply: None,
        failed: false,
        closed_at: None,
        opened_at: Some(90),
        closed_by: None,
    }
}
fn job() -> Job {
    Job {
        job_id: "j".into(),
        title: String::new(),
        prompt: String::new(),
        path: String::new(),
        backend: String::new(),
        assigned_peer: Some("alias".into()),
        state: "running".into(),
        result_summary: None,
        circle: "one".into(),
        depends_on: vec![],
        ask_id: Some("a".into()),
        from_peer: String::new(),
        dispatch: true,
        nudge_at: None,
        finished_at: None,
        created_at: None,
        handoff: None,
    }
}
#[test]
fn progress_gone() {
    let mut h = hub();
    if "absent" != "absent" {
        peer(&mut h, "p", "alias", "absent", "work", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Gone,
        "progress_gone"
    );
}
#[test]
fn progress_offline_wait() {
    let mut h = hub();
    if "offline" != "absent" {
        peer(&mut h, "p", "alias", "offline", "wait", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Offline,
        "progress_offline_wait"
    );
}
#[test]
fn progress_offline_work() {
    let mut h = hub();
    if "offline" != "absent" {
        peer(&mut h, "p", "alias", "offline", "work", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Offline,
        "progress_offline_work"
    );
}
#[test]
fn progress_offline_idle() {
    let mut h = hub();
    if "offline" != "absent" {
        peer(&mut h, "p", "alias", "offline", "idle", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Offline,
        "progress_offline_idle"
    );
}
#[test]
fn progress_no_activity() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Pending { since: Some(90) },
        "progress_no_activity"
    );
}
#[test]
fn progress_unknown_activity() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "other", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Pending { since: Some(90) },
        "progress_unknown_activity"
    );
}
#[test]
fn progress_pickup_nine() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "idle", 80);
    }
    let mut a = ask();
    a.opened_at = Some(91);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Pending { since: Some(91) },
        "progress_pickup_nine"
    );
}
#[test]
fn progress_pickup_ten() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "idle", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Idle {
            since: 90,
            why: IdleWhy::NotPickedUp
        },
        "progress_pickup_ten"
    );
}
#[test]
fn progress_pickup_equal() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "idle", 90);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Idle {
            since: 90,
            why: IdleWhy::AskStillOpen
        },
        "progress_pickup_equal"
    );
}
#[test]
fn progress_pickup_equal_nine() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "idle", 91);
    }
    let mut a = ask();
    a.opened_at = Some(91);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Pending { since: Some(91) },
        "progress_pickup_equal_nine"
    );
}
#[test]
fn progress_turn_ended() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "idle", 95);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Idle {
            since: 95,
            why: IdleWhy::TurnEnded
        },
        "progress_turn_ended"
    );
}
#[test]
fn progress_opened_absent() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "idle", 80);
    }
    let mut a = ask();
    a.opened_at = None;
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Idle {
            since: 80,
            why: IdleWhy::TurnEnded
        },
        "progress_opened_absent"
    );
}
#[test]
fn progress_opened_future() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "idle", 80);
    }
    let mut a = ask();
    a.opened_at = Some(101);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Pending { since: Some(101) },
        "progress_opened_future"
    );
}
#[test]
fn progress_wait() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "wait", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Wait {
            since: 80,
            reason: Some("permission".into())
        },
        "progress_wait"
    );
}
#[test]
fn progress_work() {
    let mut h = hub();
    if "online" != "absent" {
        peer(&mut h, "p", "alias", "online", "work", 80);
    }
    let mut a = ask();
    a.opened_at = Some(90);
    assert_eq!(
        progress(&h, Some("p"), Some(&a), 100),
        Progress::Work { since: 80 },
        "progress_work"
    );
}
#[test]
fn progress_hand_idle() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "idle", 80);
    assert_eq!(
        progress(&h, Some("p"), None, 100),
        Progress::Pending { since: None },
        "progress_hand_idle"
    );
}
#[test]
fn progress_no_id() {
    let h = hub();
    assert_eq!(
        progress(&h, None, None, 100),
        Progress::Gone,
        "progress_no_id"
    );
}
#[test]
fn outcome_recipient_false() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = false;
    a.closed_by = Some("recipient".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::AckedOk,
            failed_effective: false
        },
        "outcome_recipient_false"
    );
}
#[test]
fn outcome_recipient_true() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = true;
    a.closed_by = Some("recipient".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::AckedFailed,
            failed_effective: true
        },
        "outcome_recipient_true"
    );
}
#[test]
fn outcome_hand_false() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = false;
    a.closed_by = Some("hand".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::HandOk,
            failed_effective: false
        },
        "outcome_hand_false"
    );
}
#[test]
fn outcome_hand_true() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = true;
    a.closed_by = Some("hand".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::HandFailed,
            failed_effective: true
        },
        "outcome_hand_true"
    );
}
#[test]
fn outcome_hub_false() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = false;
    a.closed_by = Some("hub".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::Hub,
            failed_effective: true
        },
        "outcome_hub_false"
    );
}
#[test]
fn outcome_hub_true() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = true;
    a.closed_by = Some("hub".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::Hub,
            failed_effective: true
        },
        "outcome_hub_true"
    );
}
#[test]
fn outcome_legacy_false() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = false;
    a.closed_by = None;
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::ClosedOk,
            failed_effective: false
        },
        "outcome_legacy_false"
    );
}
#[test]
fn outcome_legacy_true() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = true;
    a.closed_by = None;
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::ClosedFailed,
            failed_effective: true
        },
        "outcome_legacy_true"
    );
}
#[test]
fn outcome_future_false() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = false;
    a.closed_by = Some("future".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::ClosedOk,
            failed_effective: false
        },
        "outcome_future_false"
    );
}
#[test]
fn outcome_future_true() {
    let h = hub();
    let mut a = ask();
    a.open = false;
    a.failed = true;
    a.closed_by = Some("future".into());
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Closed {
            outcome: AskOutcome::ClosedFailed,
            failed_effective: true
        },
        "outcome_future_true"
    );
}
#[test]
fn ask_open() {
    let h = hub();
    let a = ask();
    assert_eq!(
        ask_state(&h, &a, 100),
        AskState::Open {
            progress: Progress::Gone
        },
        "ask_open"
    );
}
#[test]
fn relation_running_none_false() {
    let h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = false;
    j.ask_id = None;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::NoAsk,
        "relation_running_none_false"
    );
}
#[test]
fn relation_running_none_true() {
    let h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    j.ask_id = None;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::NoAsk,
        "relation_running_none_true"
    );
}
#[test]
fn relation_running_open_false() {
    let mut h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = false;
    let mut a = ask();
    a.open = true;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Open,
        "relation_running_open_false"
    );
}
#[test]
fn relation_running_open_true() {
    let mut h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let mut a = ask();
    a.open = true;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Open,
        "relation_running_open_true"
    );
}
#[test]
fn relation_running_closed_false() {
    let mut h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = false;
    let mut a = ask();
    a.open = false;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Closed,
        "relation_running_closed_false"
    );
}
#[test]
fn relation_running_closed_true() {
    let mut h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let mut a = ask();
    a.open = false;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Settling,
        "relation_running_closed_true"
    );
}
#[test]
fn relation_running_missing_false() {
    let h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = false;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Missing { will_fail: false },
        "relation_running_missing_false"
    );
}
#[test]
fn relation_running_missing_true() {
    let h = hub();
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Missing { will_fail: true },
        "relation_running_missing_true"
    );
}
#[test]
fn relation_done_none_false() {
    let h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = false;
    j.ask_id = None;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::NoAsk,
        "relation_done_none_false"
    );
}
#[test]
fn relation_done_none_true() {
    let h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = true;
    j.ask_id = None;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::NoAsk,
        "relation_done_none_true"
    );
}
#[test]
fn relation_done_open_false() {
    let mut h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = false;
    let mut a = ask();
    a.open = true;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Open,
        "relation_done_open_false"
    );
}
#[test]
fn relation_done_open_true() {
    let mut h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = true;
    let mut a = ask();
    a.open = true;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Open,
        "relation_done_open_true"
    );
}
#[test]
fn relation_done_closed_false() {
    let mut h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = false;
    let mut a = ask();
    a.open = false;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Closed,
        "relation_done_closed_false"
    );
}
#[test]
fn relation_done_closed_true() {
    let mut h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = true;
    let mut a = ask();
    a.open = false;
    h.asks.insert("a".into(), a);
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::Closed,
        "relation_done_closed_true"
    );
}
#[test]
fn relation_done_missing_false() {
    let h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = false;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::CleanedUp,
        "relation_done_missing_false"
    );
}
#[test]
fn relation_done_missing_true() {
    let h = hub();
    let mut j = job();
    j.state = "done".into();
    j.dispatch = true;
    assert_eq!(
        job_relation(&h, &j),
        JobRelation::CleanedUp,
        "relation_done_missing_true"
    );
}
#[test]
fn worker_fixed_gone() {
    let mut h = hub();
    let j = job();
    h.asks.insert("a".into(), ask());
    assert_eq!(worker_id(&h, &j), Some("p"), "worker_fixed_gone");
}
#[test]
fn worker_fixed_reused() {
    let mut h = hub();
    let j = job();
    h.asks.insert("a".into(), ask());
    peer(&mut h, "replacement", "alias", "online", "work", 80);
    assert_eq!(worker_id(&h, &j), Some("p"), "worker_fixed_reused");
}
#[test]
fn worker_empty() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let j = job();
    let mut a = ask();
    a.to_peer_id.clear();
    h.asks.insert("a".into(), a);
    assert_eq!(worker_id(&h, &j), None, "worker_empty");
}
#[test]
fn worker_missing() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let j = job();
    assert_eq!(worker_id(&h, &j), None, "worker_missing");
}
#[test]
fn worker_hand_alias() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let mut j = job();
    j.ask_id = None;
    assert_eq!(worker_id(&h, &j), Some("p"), "worker_hand_alias");
}
#[test]
fn worker_nonrunning() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let mut j = job();
    j.state = "done".into();
    assert_eq!(worker_id(&h, &j), Some("p"), "worker_nonrunning");
}
#[test]
fn counted_closed() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let j = job();
    let mut a = ask();
    a.open = false;
    h.asks.insert("a".into(), a);
    assert_eq!(counted_worker_id(&h, &j), Some("p"), "counted_closed");
}
#[test]
fn counted_empty() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let j = job();
    let mut a = ask();
    a.to_peer_id.clear();
    h.asks.insert("a".into(), a);
    assert_eq!(counted_worker_id(&h, &j), Some(""), "counted_empty");
}
#[test]
fn counted_missing() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let j = job();
    assert_eq!(counted_worker_id(&h, &j), None, "counted_missing");
}
#[test]
fn counted_hand() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let mut j = job();
    j.ask_id = None;
    assert_eq!(counted_worker_id(&h, &j), Some("p"), "counted_hand");
}
#[test]
fn counted_done() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let mut j = job();
    j.state = "done".into();
    h.asks.insert("a".into(), ask());
    assert_eq!(counted_worker_id(&h, &j), None, "counted_done");
}
#[test]
fn global_count_zero() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    assert_eq!(
        ReadModel::new(&h, 100).running_count("p"),
        0,
        "global_count_zero"
    );
}
#[test]
fn global_count_one() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let j = job();
    h.jobs.insert("j".into(), j);
    assert_eq!(
        ReadModel::new(&h, 100).running_count("p"),
        1,
        "global_count_one"
    );
}
#[test]
fn a_wait_builds_no_running_count() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    h.jobs.insert("j".into(), job());
    let model = ReadModel::new(&h, 100);
    model.wait(&h.asks["a"]);
    model.wait_push_hint(&h.asks["a"], Some("alias"));
    assert!(model.running.get().is_none(), "a wait scans no jobs");
    assert_eq!(model.running_count("p"), 1, "built on first use");
}
#[test]
fn global_count_two_cross_circle() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let mut j = job();
    h.jobs.insert("j".into(), j.clone());
    j.circle = "two".into();
    h.jobs.insert("j2".into(), j);
    assert_eq!(
        ReadModel::new(&h, 100).running_count("p"),
        2,
        "global_count_two_cross_circle"
    );
}
#[test]
fn global_count_missing_plus_healthy() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let mut j = job();
    h.jobs.insert("j".into(), j.clone());
    j.ask_id = Some("missing".into());
    h.jobs.insert("j2".into(), j);
    assert_eq!(
        ReadModel::new(&h, 100).running_count("p"),
        1,
        "global_count_missing_plus_healthy"
    );
}
#[test]
fn global_count_closed() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let j = job();
    h.asks.get_mut("a").unwrap().open = false;
    h.jobs.insert("j".into(), j);
    assert_eq!(
        ReadModel::new(&h, 100).running_count("p"),
        1,
        "global_count_closed"
    );
}
#[test]
fn job_progress_active() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let j = job();
    h.jobs.insert("j".into(), j.clone());
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        Some(JobProgress {
            state: Some(Progress::Work { since: 80 }),
            busy: true
        }),
        "job_progress_active"
    );
}
#[test]
fn job_progress_two() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let j = job();
    h.jobs.insert("j".into(), j.clone());
    h.jobs.insert("second".into(), j.clone());
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        Some(JobProgress {
            state: Some(Progress::Work { since: 80 }),
            busy: false
        }),
        "job_progress_two"
    );
}
#[test]
fn job_progress_closed() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let j = job();
    h.jobs.insert("j".into(), j.clone());
    h.asks.get_mut("a").unwrap().open = false;
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        Some(JobProgress {
            state: None,
            busy: false
        }),
        "job_progress_closed"
    );
}
#[test]
fn job_progress_missing() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let j = job();
    h.jobs.insert("j".into(), j.clone());
    h.asks.clear();
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        Some(JobProgress {
            state: None,
            busy: false
        }),
        "job_progress_missing"
    );
}
#[test]
fn job_progress_offline() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let j = job();
    h.jobs.insert("j".into(), j.clone());
    h.peers.get_mut("p").unwrap().status = "offline".into();
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        Some(JobProgress {
            state: Some(Progress::Offline),
            busy: false
        }),
        "job_progress_offline"
    );
}
#[test]
fn job_progress_nonrunning() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let mut j = job();
    h.jobs.insert("j".into(), j.clone());
    j.state = "done".into();
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        None,
        "job_progress_nonrunning"
    );
}
#[test]
fn job_progress_hand() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    h.asks.insert("a".into(), ask());
    let mut j = job();
    j.ask_id = None;
    h.jobs.insert("j".into(), j.clone());
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        Some(JobProgress {
            state: Some(Progress::Work { since: 80 }),
            busy: true
        }),
        "job_progress_hand"
    );
}
#[test]
fn liveness_online() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 80);
    assert_eq!(
        liveness(&h, &h.peers["p"]),
        Liveness::Online,
        "liveness_online"
    );
}
#[test]
fn liveness_wait() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "wait", 80);
    assert_eq!(
        liveness(&h, &h.peers["p"]),
        Liveness::Wait {
            since: 80,
            reason: Some("permission".into())
        },
        "liveness_wait"
    );
}
#[test]
fn liveness_idle() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "idle", 80);
    assert_eq!(
        liveness(&h, &h.peers["p"]),
        Liveness::Idle { since: 80 },
        "liveness_idle"
    );
}
#[test]
fn liveness_work() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    assert_eq!(
        liveness(&h, &h.peers["p"]),
        Liveness::Work { since: 80 },
        "liveness_work"
    );
}
#[test]
fn liveness_future() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "future", 80);
    assert_eq!(
        liveness(&h, &h.peers["p"]),
        Liveness::Online,
        "liveness_future"
    );
}
#[test]
fn liveness_offline() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "offline", "wait", 80);
    assert_eq!(
        liveness(&h, &h.peers["p"]),
        Liveness::Offline,
        "liveness_offline"
    );
}
#[test]
fn closed_now_absent() {
    let mut a = ask();
    a.closed_at = None;
    assert_eq!(closed_just_now(&a, 100), false, "closed_now_absent");
}
#[test]
fn closed_now_future() {
    let mut a = ask();
    a.closed_at = Some(101);
    assert_eq!(closed_just_now(&a, 100), false, "closed_now_future");
}
#[test]
fn closed_now_equal() {
    let mut a = ask();
    a.closed_at = Some(100);
    assert_eq!(closed_just_now(&a, 100), true, "closed_now_equal");
}
#[test]
fn closed_now_one() {
    let mut a = ask();
    a.closed_at = Some(99);
    assert_eq!(closed_just_now(&a, 100), true, "closed_now_one");
}
#[test]
fn closed_now_two() {
    let mut a = ask();
    a.closed_at = Some(98);
    assert_eq!(closed_just_now(&a, 100), false, "closed_now_two");
}
#[test]
fn opened_now_absent() {
    let mut a = ask();
    a.opened_at = None;
    assert_eq!(opened_just_now(&a, 100), false, "opened_now_absent");
}
#[test]
fn opened_now_future() {
    let mut a = ask();
    a.opened_at = Some(101);
    assert_eq!(opened_just_now(&a, 100), false, "opened_now_future");
}
#[test]
fn opened_now_equal() {
    let mut a = ask();
    a.opened_at = Some(100);
    assert_eq!(opened_just_now(&a, 100), true, "opened_now_equal");
}
#[test]
fn opened_now_one() {
    let mut a = ask();
    a.opened_at = Some(99);
    assert_eq!(opened_just_now(&a, 100), true, "opened_now_one");
}
#[test]
fn opened_now_two() {
    let mut a = ask();
    a.opened_at = Some(98);
    assert_eq!(opened_just_now(&a, 100), false, "opened_now_two");
}
#[test]
fn delivery_offline() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "offline", "", 0);
    h.sockets
        .insert("p".into(), (1, tokio::sync::mpsc::unbounded_channel().0));
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).condition,
        DeliveryCondition::Offline,
        "delivery_offline"
    );
}
#[test]
fn delivery_push() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    h.sockets
        .insert("p".into(), (1, tokio::sync::mpsc::unbounded_channel().0));
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).condition,
        DeliveryCondition::Push,
        "delivery_push"
    );
}
#[test]
fn delivery_no_push() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).condition,
        DeliveryCondition::NoPush,
        "delivery_no_push"
    );
}
#[test]
fn delivery_stuck_young() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 99)], true);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).stuck,
        None,
        "delivery_stuck_young"
    );
}
#[test]
fn delivery_stuck_boundary() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 98)], true);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).stuck,
        Some(Stuck {
            count: 1,
            since: 98
        }),
        "delivery_stuck_boundary"
    );
}
#[test]
fn delivery_stuck_oldest_last() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 99)], true);
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 90)], true);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).stuck,
        Some(Stuck {
            count: 2,
            since: 90
        }),
        "delivery_stuck_oldest_last"
    );
}
#[test]
fn delivery_stuck_offline() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "offline", "", 0);
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 90)], true);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).stuck,
        Some(Stuck {
            count: 1,
            since: 90
        }),
        "delivery_stuck_offline"
    );
}
#[test]
fn delivery_stuck_future() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 101)], true);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).stuck,
        None,
        "delivery_stuck_future"
    );
}
#[test]
fn delivery_stuck_empty() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).stuck,
        None,
        "delivery_stuck_empty"
    );
}
#[test]
fn delivery_stuck_turnover() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 90)], true);
    h.inbox.take("p");
    h.inbox
        .enqueue("p", [QueuedRecord::new(json!({}), 99)], true);
    assert_eq!(
        delivery(&h, &h.peers["p"], 100).stuck,
        None,
        "delivery_stuck_turnover"
    );
}
#[test]
fn ask_action_gone() {
    let a = ask();
    assert_eq!(
        ask_actions(
            &AskState::Open {
                progress: Progress::Gone
            },
            &a
        ),
        vec![AskAction::CloseLeft],
        "ask_action_gone"
    );
}
#[test]
fn ask_action_idle() {
    let a = ask();
    assert_eq!(
        ask_actions(
            &AskState::Open {
                progress: Progress::Idle {
                    since: 80,
                    why: IdleWhy::TurnEnded
                }
            },
            &a
        ),
        vec![AskAction::Nudge { to: "p".into() }],
        "ask_action_idle"
    );
}
#[test]
fn ask_action_offline() {
    let a = ask();
    assert_eq!(
        ask_actions(
            &AskState::Open {
                progress: Progress::Offline
            },
            &a
        ),
        vec![],
        "ask_action_offline"
    );
}
#[test]
fn ask_action_closed() {
    let a = ask();
    assert_eq!(
        ask_actions(
            &AskState::Closed {
                outcome: AskOutcome::HandOk,
                failed_effective: false
            },
            &a
        ),
        vec![],
        "ask_action_closed"
    );
}
#[test]
fn ask_action_unknown() {
    let a = ask();
    assert_eq!(
        ask_actions(&AskState::Unknown, &a),
        vec![],
        "ask_action_unknown"
    );
}
#[test]
fn job_action_retry() {
    let mut j = job();
    j.state = "failed".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Gone),
        busy: false,
    };
    assert_eq!(
        job_actions(
            &j,
            &JobRelation::Missing { will_fail: true },
            Some(&p),
            Some("p")
        ),
        vec![JobAction::Retry {
            needs_assignee: false
        }],
        "job_action_retry"
    );
}
#[test]
fn job_action_retry_assignee() {
    let mut j = job();
    j.state = "failed".into();
    j.dispatch = false;
    let p = JobProgress {
        state: Some(Progress::Gone),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::NoAsk, Some(&p), Some("p")),
        vec![JobAction::Retry {
            needs_assignee: true
        }],
        "job_action_retry_assignee"
    );
}
#[test]
fn job_action_send() {
    let mut j = job();
    j.state = "queued".into();
    j.dispatch = false;
    let p = JobProgress {
        state: Some(Progress::Gone),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::NoAsk, Some(&p), Some("p")),
        vec![JobAction::Send],
        "job_action_send"
    );
}
#[test]
fn job_action_queued_auto() {
    let mut j = job();
    j.state = "queued".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Gone),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::NoAsk, Some(&p), Some("p")),
        vec![],
        "job_action_queued_auto"
    );
}
#[test]
fn job_action_nudge() {
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Idle {
            since: 80,
            why: IdleWhy::TurnEnded,
        }),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::Open, Some(&p), Some("p")),
        vec![JobAction::Nudge { to: "p".into() }],
        "job_action_nudge"
    );
}
#[test]
fn job_action_resend() {
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Gone),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::Open, Some(&p), Some("p")),
        vec![JobAction::Resend],
        "job_action_resend"
    );
}
#[test]
fn job_action_missing() {
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Gone),
        busy: false,
    };
    assert_eq!(
        job_actions(
            &j,
            &JobRelation::Missing { will_fail: true },
            Some(&p),
            Some("p")
        ),
        vec![],
        "job_action_missing"
    );
}
#[test]
fn job_action_closed() {
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Idle {
            since: 80,
            why: IdleWhy::TurnEnded,
        }),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::Closed, Some(&p), Some("p")),
        vec![],
        "job_action_closed"
    );
}
#[test]
fn job_action_offline() {
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Offline),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::Open, Some(&p), Some("p")),
        vec![],
        "job_action_offline"
    );
}
#[test]
fn job_action_hand_gone() {
    let mut j = job();
    j.state = "running".into();
    j.dispatch = true;
    let p = JobProgress {
        state: Some(Progress::Gone),
        busy: false,
    };
    assert_eq!(
        job_actions(&j, &JobRelation::NoAsk, Some(&p), Some("p")),
        vec![],
        "job_action_hand_gone"
    );
}
#[test]
fn resolve_collision() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    peer(&mut h, "other", "p", "online", "", 0);
    peer(&mut h, "anon", "anonymous", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).resolve_id("p"),
        Some("p"),
        "resolve_collision"
    );
}
#[test]
fn resolve_alias() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    peer(&mut h, "other", "p", "online", "", 0);
    peer(&mut h, "anon", "anonymous", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).resolve_id("alias"),
        Some("p"),
        "resolve_alias"
    );
}
#[test]
fn resolve_anonymous() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    peer(&mut h, "other", "p", "online", "", 0);
    peer(&mut h, "anon", "anonymous", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).sender_id("anonymous"),
        None,
        "resolve_anonymous"
    );
}
#[test]
fn resolve_absent() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    peer(&mut h, "other", "p", "online", "", 0);
    peer(&mut h, "anon", "anonymous", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).resolve_id("absent"),
        None,
        "resolve_absent"
    );
}
#[test]
fn resolve_empty() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    peer(&mut h, "other", "p", "online", "", 0);
    peer(&mut h, "anon", "anonymous", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).resolve_id(""),
        None,
        "resolve_empty"
    );
}
#[test]
fn assignee_anonymous_resolves() {
    let mut h = hub();
    peer(&mut h, "anon", "anonymous", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).resolve_id("anonymous"),
        Some("anon"),
        "assignee_anonymous_resolves"
    );
}
#[test]
fn job_progress_closed_held() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "work", 80);
    let mut a = ask();
    a.open = false;
    h.asks.insert("a".into(), a);
    let mut j = job();
    j.dispatch = false;
    h.jobs.insert("j".into(), j.clone());
    assert_eq!(
        ReadModel::new(&h, 100).job_progress(&j),
        Some(JobProgress {
            state: None,
            busy: false
        }),
        "job_progress_closed_held"
    );
}
#[test]
fn sender_empty_is_unresolved() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).sender_id(""),
        None,
        "sender_empty_is_unresolved"
    );
}
#[test]
fn sender_alias_resolves() {
    let mut h = hub();
    peer(&mut h, "p", "alias", "online", "", 0);
    assert_eq!(
        ReadModel::new(&h, 100).sender_id("alias"),
        Some("p"),
        "sender_alias_resolves"
    );
}
#[test]
fn global_count_empty_recipient() {
    let mut h = hub();
    let mut a = ask();
    a.to_peer_id.clear();
    h.asks.insert("a".into(), a);
    h.jobs.insert("j".into(), job());
    assert_eq!(
        ReadModel::new(&h, 100).running_count(""),
        1,
        "global_count_empty_recipient"
    );
}
#[test]
fn progress_fixed_id_is_not_a_name() {
    let mut h = hub();
    peer(&mut h, "replacement", "p", "online", "work", 80);
    assert_eq!(
        progress(&h, Some("p"), Some(&ask()), 100),
        Progress::Gone,
        "progress_fixed_id_is_not_a_name"
    );
}

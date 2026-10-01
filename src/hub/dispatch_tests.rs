use super::derive::assess_dispatch;
use super::*;
use crate::wire::{BlockReason, DispatchState};

fn fixture() -> (Hub, tests::TempState, Job) {
    let path = tests::TempState::new("dispatch");
    let mut hub = Hub::open(&path).unwrap();
    hub.sweep_at = u64::MAX;
    for (id, name) in [("worker", "alias"), ("creator", "creator")] {
        hub.peers.insert(id.into(), serde_json::from_value(json!({
            "peer_id": id, "name": name, "status": "online", "path": "",
            "backend": "pi", "circle": "one", "description": "", "session_id": "", "last_seen": 0
        })).unwrap());
    }
    let job = serde_json::from_value(json!({
        "job_id": "j", "title": "dispatch", "prompt": "", "path": "", "backend": "",
        "assigned_peer": "alias", "state": "queued", "circle": "one", "dispatch": true,
        "from_peer": "creator"
    }))
    .unwrap();
    (hub, path, job)
}

fn dependency(hub: &mut Hub, job: &mut Job, id: &str, state: &str) {
    job.depends_on.push(id.into());
    if state != "deleted" {
        let mut row = job.clone();
        row.job_id = id.into();
        row.state = state.into();
        row.dispatch = false;
        row.depends_on.clear();
        hub.jobs.insert(id.into(), row);
    }
}

fn check_scheduler(hub: &mut Hub, job: Job, expected: DispatchState, nudge: bool) {
    hub.jobs.insert("j".into(), job);
    advance_jobs(hub);
    let row = &hub.jobs["j"];
    match expected {
        DispatchState::Ready { peer_id } => {
            assert_eq!(row.state, "running", "scheduler state");
            assert_eq!(
                hub.asks[row.ask_id.as_ref().unwrap()].to_peer_id,
                peer_id,
                "scheduler strict recipient"
            );
        }
        _ => {
            assert_eq!(row.state, "queued", "scheduler state");
            assert!(row.ask_id.is_none(), "scheduler ask absent");
        }
    }
    assert_eq!(row.nudge_at.is_some(), nudge, "scheduler nudge");
}

#[test]
fn dispatch_ready() {
    let (hub, _path, job) = fixture();

    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Ready {
            peer_id: "worker".into()
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_ready() {
    let (mut hub, _path, job) = fixture();

    check_scheduler(
        &mut hub,
        job,
        DispatchState::Ready {
            peer_id: "worker".into(),
        },
        true,
    );
}
#[test]
fn dispatch_offline() {
    let (mut hub, _path, job) = fixture();
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Ready {
            peer_id: "worker".into()
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_offline() {
    let (mut hub, _path, job) = fixture();
    hub.peers.get_mut("worker").unwrap().status = "offline".into();
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Ready {
            peer_id: "worker".into(),
        },
        true,
    );
}
#[test]
fn dispatch_id_collision() {
    let (mut hub, _path, mut job) = fixture();
    hub.peers.get_mut("creator").unwrap().name = "worker".into();
    job.assigned_peer = Some("worker".into());
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Ready {
            peer_id: "worker".into()
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_id_collision() {
    let (mut hub, _path, mut job) = fixture();
    hub.peers.get_mut("creator").unwrap().name = "worker".into();
    job.assigned_peer = Some("worker".into());
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Ready {
            peer_id: "worker".into(),
        },
        true,
    );
}
#[test]
fn dispatch_empty_circle() {
    let (mut hub, _path, mut job) = fixture();
    job.circle.clear();
    hub.peers.get_mut("worker").unwrap().circle = "two".into();
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Ready {
            peer_id: "worker".into()
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_empty_circle() {
    let (mut hub, _path, mut job) = fixture();
    job.circle.clear();
    hub.peers.get_mut("worker").unwrap().circle = "two".into();
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Ready {
            peer_id: "worker".into(),
        },
        true,
    );
}
#[test]
fn dispatch_other_circle() {
    let (mut hub, _path, job) = fixture();
    hub.peers.get_mut("worker").unwrap().circle = "two".into();
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::OtherCircle {
            peer_id: "worker".into()
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_other_circle() {
    let (mut hub, _path, job) = fixture();
    hub.peers.get_mut("worker").unwrap().circle = "two".into();
    check_scheduler(
        &mut hub,
        job,
        DispatchState::OtherCircle {
            peer_id: "worker".into(),
        },
        true,
    );
}
#[test]
fn dispatch_held() {
    let (hub, _path, mut job) = fixture();
    job.dispatch = false;
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(assessment.state, DispatchState::Held, "state");
    assert_eq!(assessment.nudge_eligible, false, "nudge eligibility");
}
#[test]
fn scheduler_held() {
    let (mut hub, _path, mut job) = fixture();
    job.dispatch = false;
    check_scheduler(&mut hub, job, DispatchState::Held, false);
}
#[test]
fn dispatch_held_unresolved() {
    let (hub, _path, mut job) = fixture();
    job.dispatch = false;
    job.assigned_peer = Some("missing".into());
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(assessment.state, DispatchState::Held, "state");
    assert_eq!(assessment.nudge_eligible, false, "nudge eligibility");
}
#[test]
fn scheduler_held_unresolved() {
    let (mut hub, _path, mut job) = fixture();
    job.dispatch = false;
    job.assigned_peer = Some("missing".into());
    check_scheduler(&mut hub, job, DispatchState::Held, false);
}
#[test]
fn dispatch_unassigned() {
    let (hub, _path, mut job) = fixture();
    job.assigned_peer = None;
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(assessment.state, DispatchState::Unassigned, "state");
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_unassigned() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = None;
    check_scheduler(&mut hub, job, DispatchState::Unassigned, true);
}
#[test]
fn dispatch_empty_assignee() {
    let (hub, _path, mut job) = fixture();
    job.assigned_peer = Some(String::new());
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(assessment.state, DispatchState::Unassigned, "state");
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_empty_assignee() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = Some(String::new());
    check_scheduler(&mut hub, job, DispatchState::Unassigned, true);
}
#[test]
fn dispatch_held_unassigned() {
    let (hub, _path, mut job) = fixture();
    job.assigned_peer = None;
    job.dispatch = false;
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(assessment.state, DispatchState::Unassigned, "state");
    assert_eq!(assessment.nudge_eligible, false, "nudge eligibility");
}
#[test]
fn scheduler_held_unassigned() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = None;
    job.dispatch = false;
    check_scheduler(&mut hub, job, DispatchState::Unassigned, false);
}
#[test]
fn dispatch_held_empty() {
    let (hub, _path, mut job) = fixture();
    job.assigned_peer = Some(String::new());
    job.dispatch = false;
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(assessment.state, DispatchState::Unassigned, "state");
    assert_eq!(assessment.nudge_eligible, false, "nudge eligibility");
}
#[test]
fn scheduler_held_empty() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = Some(String::new());
    job.dispatch = false;
    check_scheduler(&mut hub, job, DispatchState::Unassigned, false);
}
#[test]
fn dispatch_no_peer() {
    let (hub, _path, mut job) = fixture();
    job.assigned_peer = Some("missing".into());
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::NoPeer {
            name: "missing".into()
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_no_peer() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = Some("missing".into());
    check_scheduler(
        &mut hub,
        job,
        DispatchState::NoPeer {
            name: "missing".into(),
        },
        true,
    );
}
#[test]
fn dispatch_done() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "done");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Ready {
            peer_id: "worker".into()
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_done() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "done");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Ready {
            peer_id: "worker".into(),
        },
        true,
    );
}
#[test]
fn dispatch_waiting() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "queued");
    dependency(&mut hub, &mut job, "r", "running");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Waiting {
            dependencies: vec!["d".into(), "r".into()]
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, false, "nudge eligibility");
}
#[test]
fn scheduler_waiting() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "queued");
    dependency(&mut hub, &mut job, "r", "running");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Waiting {
            dependencies: vec!["d".into(), "r".into()],
        },
        false,
    );
}
#[test]
fn dispatch_waiting_held() {
    let (mut hub, _path, mut job) = fixture();
    job.dispatch = false;
    dependency(&mut hub, &mut job, "d", "running");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Waiting {
            dependencies: vec!["d".into()]
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, false, "nudge eligibility");
}
#[test]
fn scheduler_waiting_held() {
    let (mut hub, _path, mut job) = fixture();
    job.dispatch = false;
    dependency(&mut hub, &mut job, "d", "running");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Waiting {
            dependencies: vec!["d".into()],
        },
        false,
    );
}
#[test]
fn dispatch_failed() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "failed");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Blocked {
            dependencies: vec!["d".into()],
            dependency: "d".into(),
            reason: BlockReason::Failed
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_failed() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "failed");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Blocked {
            dependencies: vec!["d".into()],
            dependency: "d".into(),
            reason: BlockReason::Failed,
        },
        true,
    );
}
#[test]
fn dispatch_cancelled() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "cancelled");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Blocked {
            dependencies: vec!["d".into()],
            dependency: "d".into(),
            reason: BlockReason::Cancelled
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_cancelled() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "cancelled");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Blocked {
            dependencies: vec!["d".into()],
            dependency: "d".into(),
            reason: BlockReason::Cancelled,
        },
        true,
    );
}
#[test]
fn dispatch_deleted() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "deleted");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Blocked {
            dependencies: vec!["d".into()],
            dependency: "d".into(),
            reason: BlockReason::Deleted
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_deleted() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "deleted");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Blocked {
            dependencies: vec!["d".into()],
            dependency: "d".into(),
            reason: BlockReason::Deleted,
        },
        true,
    );
}
#[test]
fn dispatch_blocked_order() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_blocked_order() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled,
        },
        true,
    );
}
#[test]
fn dispatch_blocked_held() {
    let (mut hub, _path, mut job) = fixture();
    job.dispatch = false;
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, false, "nudge eligibility");
}
#[test]
fn scheduler_blocked_held() {
    let (mut hub, _path, mut job) = fixture();
    job.dispatch = false;
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled,
        },
        false,
    );
}
#[test]
fn dispatch_blocked_unassigned() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = None;
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_blocked_unassigned() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = None;
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled,
        },
        true,
    );
}
#[test]
fn dispatch_blocked_other_circle() {
    let (mut hub, _path, mut job) = fixture();
    hub.peers.get_mut("worker").unwrap().circle = "two".into();
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    let assessment = assess_dispatch(&hub, &job);
    assert_eq!(
        assessment.state,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled
        },
        "state"
    );
    assert_eq!(assessment.nudge_eligible, true, "nudge eligibility");
}
#[test]
fn scheduler_blocked_other_circle() {
    let (mut hub, _path, mut job) = fixture();
    hub.peers.get_mut("worker").unwrap().circle = "two".into();
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "second", "cancelled");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "done", "done");
    dependency(&mut hub, &mut job, "gone", "deleted");
    check_scheduler(
        &mut hub,
        job,
        DispatchState::Blocked {
            dependencies: vec!["z".into(), "second".into(), "first".into(), "gone".into()],
            dependency: "second".into(),
            reason: BlockReason::Cancelled,
        },
        true,
    );
}

#[test]
fn scheduler_waiting_clears_stale_nudge() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "d", "running");
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert_eq!(hub.jobs["j"].nudge_at, None);
}

#[test]
fn scheduler_settles_before_assessment() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "z", "running");
    let upstream = hub.jobs.get_mut("z").unwrap();
    upstream.dispatch = true;
    upstream.ask_id = Some("a".into());
    hub.asks.insert(
        "a".into(),
        serde_json::from_value(json!({
            "correlation_id": "a", "from_peer": "creator", "to_peer": "alias",
            "to_peer_id": "worker", "text": "", "open": false, "failed": false
        }))
        .unwrap(),
    );
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert_eq!(hub.jobs["j"].state, "running");
}

#[test]
fn scheduler_terminal_is_not_dispatched() {
    let (mut hub, _path, mut job) = fixture();
    job.state = "done".into();
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert!(hub.jobs["j"].ask_id.is_none());
}

#[test]
fn scheduler_held_preserves_existing_nudge() {
    let (mut hub, _path, mut job) = fixture();
    job.dispatch = false;
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert_eq!(hub.jobs["j"].nudge_at, Some(1));
}

#[test]
fn scheduler_running_reminder() {
    let (mut hub, _path, mut job) = fixture();
    job.state = "running".into();
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert!(hub
        .inbox
        .take("creator")
        .iter()
        .any(|record| record.event()["message"]
            .as_str()
            .is_some_and(|message| message.contains("alias has not acked"))));
}

#[test]
fn scheduler_reminder_failed_first() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "first", "failed");
    dependency(&mut hub, &mut job, "last", "failed");
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert!(hub
        .inbox
        .take("creator")
        .iter()
        .any(|record| record.event()["message"]
            .as_str()
            .is_some_and(|message| message.contains("blocked by first (failed)"))));
}

#[test]
fn scheduler_reminder_cancelled_first() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "first", "cancelled");
    dependency(&mut hub, &mut job, "last", "failed");
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert!(hub
        .inbox
        .take("creator")
        .iter()
        .any(|record| record.event()["message"]
            .as_str()
            .is_some_and(|message| message.contains("blocked by first (cancelled)"))));
}

#[test]
fn scheduler_reminder_deleted_first() {
    let (mut hub, _path, mut job) = fixture();
    dependency(&mut hub, &mut job, "z", "running");
    dependency(&mut hub, &mut job, "first", "deleted");
    dependency(&mut hub, &mut job, "last", "failed");
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert!(hub
        .inbox
        .take("creator")
        .iter()
        .any(|record| record.event()["message"]
            .as_str()
            .is_some_and(|message| message.contains("blocked by first (deleted)"))));
}

#[test]
fn scheduler_reminder_other_circle() {
    let (mut hub, _path, mut job) = fixture();
    hub.peers.get_mut("worker").unwrap().circle = "other".into();
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert!(hub
        .inbox
        .take("creator")
        .iter()
        .any(|record| record.event()["message"]
            .as_str()
            .is_some_and(|message| message == "job j dispatch is queued: worker is in another circle; amesh_job_update can retry, reassign or cancel")));
}

#[test]
fn scheduler_reminder_unassigned() {
    let (mut hub, _path, mut job) = fixture();
    job.assigned_peer = None;
    job.nudge_at = Some(1);
    hub.jobs.insert("j".into(), job);
    advance_jobs(&mut hub);
    assert!(hub
        .inbox
        .take("creator")
        .iter()
        .any(|record| record.event()["message"]
            .as_str()
            .is_some_and(|message| message == "job j dispatch is queued: no assignee; amesh_job_update can retry, reassign or cancel")));
}

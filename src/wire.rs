#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

/* the read model the hub serves and the TUI renders: one set of types for both ends. The
raw fields keep their v1 names and types so an older reader still decodes a v2 snapshot;
the derived fields are optional only so a v1 snapshot decodes, and a v2 one must carry them */
pub(crate) const SNAPSHOT_VERSION: u64 = 2;

pub(crate) type Stamp = u64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Progress {
    Gone,
    Offline,
    Pending {
        since: Option<Stamp>,
    },
    Wait {
        since: Stamp,
        reason: Option<String>,
    },
    Idle {
        since: Stamp,
        why: IdleWhy,
    },
    Work {
        since: Stamp,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdleWhy {
    NotPickedUp,
    AskStillOpen,
    TurnEnded,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AskOutcome {
    AckedOk,
    AckedFailed,
    HandOk,
    HandFailed,
    Hub,
    ClosedOk,
    ClosedFailed,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AskState {
    Open {
        progress: Progress,
    },
    Closed {
        outcome: AskOutcome,
        failed_effective: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum JobRelation {
    #[serde(rename = "none")]
    NoAsk,
    Open,
    Settling,
    Closed,
    Missing {
        will_fail: bool,
    },
    CleanedUp,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct JobProgress {
    pub state: Option<Progress>,
    pub busy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockReason {
    Failed,
    Cancelled,
    Deleted,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DispatchState {
    Waiting {
        dependencies: Vec<String>,
    },
    Blocked {
        dependencies: Vec<String>,
        dependency: String,
        reason: BlockReason,
    },
    Held,
    Unassigned,
    NoPeer {
        name: String,
    },
    OtherCircle {
        peer_id: String,
    },
    Ready {
        peer_id: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AskAction {
    CloseLeft,
    Nudge {
        to: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum JobAction {
    Retry {
        needs_assignee: bool,
    },
    Send,
    Nudge {
        to: String,
    },
    Resend,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Liveness {
    Offline,
    Online,
    Wait {
        since: Stamp,
        reason: Option<String>,
    },
    Work {
        since: Stamp,
    },
    Idle {
        since: Stamp,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Stuck {
    pub count: usize,
    pub since: Stamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryCondition {
    Offline,
    Push,
    NoPush,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Delivery {
    pub condition: DeliveryCondition,
    pub stuck: Option<Stuck>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Capabilities {
    pub job_created_at: bool,
    pub ask_opened_at: bool,
    pub ask_closed_by: bool,
    pub peer_activity: bool,
    pub roster: bool,
    pub event_count: bool,
    pub ask_list: bool,
    pub delivery: bool,
}

/* what a runtime last reported, kept on the wire for older readers; a v2 reader renders
liveness instead */
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Activity {
    pub state: String,
    pub since: Stamp,
    pub observed_at: Stamp,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AskView {
    pub correlation_id: String,
    pub from_peer: String,
    pub to_peer: String,
    pub to_peer_id: String,
    pub open: bool,
    pub failed: bool,
    pub opened_at: Option<Stamp>,
    pub closed_at: Option<Stamp>,
    pub closed_by: Option<String>,
    pub text: String,
    pub text_len: usize,
    pub reply: Option<String>,
    pub reply_len: usize,
    pub from_peer_id: Option<String>,
    pub state: Option<AskState>,
    pub actions: Option<Vec<AskAction>>,
    pub closed_just_now: Option<bool>,
    pub opened_just_now: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct JobView {
    pub job_id: String,
    pub title: String,
    pub title_len: usize,
    pub state: String,
    pub assigned_peer: Option<String>,
    pub from_peer: String,
    pub circle: String,
    pub depends_on: Vec<String>,
    pub ask_id: Option<String>,
    pub dispatch: bool,
    pub created_at: Option<Stamp>,
    pub finished_at: Option<Stamp>,
    pub prompt: String,
    pub prompt_len: usize,
    pub result: Option<String>,
    pub result_len: usize,
    pub from_peer_id: Option<String>,
    pub worker: Option<String>,
    pub assignee_id: Option<String>,
    pub relation: Option<JobRelation>,
    pub progress: Option<JobProgress>,
    pub dispatch_state: Option<DispatchState>,
    pub actions: Option<Vec<JobAction>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PeerView {
    pub peer_id: String,
    pub name: String,
    pub backend: String,
    pub circle: String,
    pub status: String,
    pub last_seen: Stamp,
    pub activity: Option<Activity>,
    pub running: Option<usize>,
    pub push: bool,
    pub acks: bool,
    pub queued: usize,
    pub liveness: Option<Liveness>,
    pub delivery: Option<Delivery>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Missing {
    pub asks: Vec<String>,
    pub peers: Vec<String>,
    pub jobs: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct JobDetail {
    pub job_id: String,
    pub title: String,
    pub prompt: String,
    pub result: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AskDetail {
    pub correlation_id: String,
    pub text: String,
    pub reply: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SnapshotView {
    pub schema_version: u64,
    pub captured_at: Stamp,
    pub hub_epoch: String,
    pub event_count: u64,
    pub capabilities: Capabilities,
    pub jobs: Vec<JobView>,
    pub asks: Vec<AskView>,
    pub peers: Vec<PeerView>,
    pub roster: Vec<PeerView>,
    pub missing: Missing,
    pub detail: Option<JobDetail>,
    pub ask_detail: Option<AskDetail>,
}

/* what an open ask's wait adds, flattened beside the ask's own fields; a closed ask's
answer stays as it was */
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct WaitDerived {
    pub schema_version: u64,
    pub captured_at: Stamp,
    pub state: AskState,
    pub from_peer_id: Option<String>,
    pub actions: Vec<AskAction>,
    pub for_secs: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum Decoded {
    /* a hub before the read model: raw facts only, every derived field cleared */
    V1Neutral(SnapshotView),
    V2(SnapshotView),
    Unsupported(u64),
}

pub(crate) fn decode_snapshot(value: Value) -> Result<Decoded, String> {
    let version = match value.get("schema_version") {
        None => 1,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| format!("schema_version is not a number: {v}"))?,
    };
    if version > SNAPSHOT_VERSION {
        return Ok(Decoded::Unsupported(version));
    }
    let mut view: SnapshotView = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if version < SNAPSHOT_VERSION {
        view.strip_derived();
        return Ok(Decoded::V1Neutral(view));
    }
    view.check_v2()?;
    Ok(Decoded::V2(view))
}

impl SnapshotView {
    fn strip_derived(&mut self) {
        for ask in &mut self.asks {
            ask.from_peer_id = None;
            ask.state = None;
            ask.actions = None;
            ask.closed_just_now = None;
            ask.opened_just_now = None;
        }
        for job in &mut self.jobs {
            job.from_peer_id = None;
            job.worker = None;
            job.assignee_id = None;
            job.relation = None;
            job.progress = None;
            job.dispatch_state = None;
            job.actions = None;
        }
        for peer in self.peers.iter_mut().chain(&mut self.roster) {
            peer.liveness = None;
            peer.delivery = None;
        }
        self.missing.jobs.clear();
    }

    /* a field a v2 hub always sends; its absence is a broken hub, never a v1 one */
    fn check_v2(&self) -> Result<(), String> {
        let lacks = |kind: &str, id: &str, field: &str| format!("v2 {kind} {id} lacks {field}");
        for ask in &self.asks {
            let id = &ask.correlation_id;
            if ask.state.is_none() {
                return Err(lacks("ask", id, "state"));
            }
            if ask.actions.is_none() {
                return Err(lacks("ask", id, "actions"));
            }
            if ask.closed_just_now.is_none() {
                return Err(lacks("ask", id, "closed_just_now"));
            }
            if ask.opened_just_now.is_none() {
                return Err(lacks("ask", id, "opened_just_now"));
            }
        }
        for job in &self.jobs {
            let id = &job.job_id;
            if job.relation.is_none() {
                return Err(lacks("job", id, "relation"));
            }
            if job.actions.is_none() {
                return Err(lacks("job", id, "actions"));
            }
            if job.state == "running" && job.progress.is_none() {
                return Err(lacks("job", id, "progress"));
            }
            if job.state == "queued" && job.dispatch_state.is_none() {
                return Err(lacks("job", id, "dispatch_state"));
            }
        }
        for (kind, rows) in [("peer", &self.peers), ("roster", &self.roster)] {
            for peer in rows {
                if peer.liveness.is_none() {
                    return Err(lacks(kind, &peer.peer_id, "liveness"));
                }
                if peer.delivery.is_none() {
                    return Err(lacks(kind, &peer.peer_id, "delivery"));
                }
                if peer.running.is_none() {
                    return Err(lacks(kind, &peer.peer_id, "running"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pinned<T>(value: &T, expected: serde_json::Value)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(value).unwrap(), expected);
        assert_eq!(&serde_json::from_value::<T>(expected).unwrap(), value);
    }

    #[test]
    fn progress_variants_have_pinned_json() {
        pinned(&Progress::Gone, json!({"kind": "gone"}));
        pinned(&Progress::Offline, json!({"kind": "offline"}));
        pinned(
            &Progress::Pending { since: Some(5) },
            json!({"kind": "pending", "since": 5}),
        );
        pinned(
            &Progress::Pending { since: None },
            json!({"kind": "pending", "since": null}),
        );
        pinned(
            &Progress::Wait {
                since: 7,
                reason: Some("Bash".into()),
            },
            json!({"kind": "wait", "since": 7, "reason": "Bash"}),
        );
        pinned(
            &Progress::Idle {
                since: 9,
                why: IdleWhy::NotPickedUp,
            },
            json!({"kind": "idle", "since": 9, "why": "not_picked_up"}),
        );
        pinned(
            &Progress::Work { since: 11 },
            json!({"kind": "work", "since": 11}),
        );
    }

    #[test]
    fn ask_states_and_outcomes_have_pinned_json() {
        pinned(
            &AskState::Open {
                progress: Progress::Work { since: 1 },
            },
            json!({"kind": "open", "progress": {"kind": "work", "since": 1}}),
        );
        pinned(
            &AskState::Closed {
                outcome: AskOutcome::HandFailed,
                failed_effective: true,
            },
            json!({"kind": "closed", "outcome": "hand_failed", "failed_effective": true}),
        );
        for (outcome, name) in [
            (AskOutcome::AckedOk, "acked_ok"),
            (AskOutcome::AckedFailed, "acked_failed"),
            (AskOutcome::HandOk, "hand_ok"),
            (AskOutcome::HandFailed, "hand_failed"),
            (AskOutcome::Hub, "hub"),
            (AskOutcome::ClosedOk, "closed_ok"),
            (AskOutcome::ClosedFailed, "closed_failed"),
        ] {
            pinned(&outcome, json!(name));
        }
    }

    #[test]
    fn job_facts_have_pinned_json() {
        pinned(&JobRelation::NoAsk, json!({"kind": "none"}));
        pinned(&JobRelation::Settling, json!({"kind": "settling"}));
        pinned(
            &JobRelation::Missing { will_fail: false },
            json!({"kind": "missing", "will_fail": false}),
        );
        pinned(&JobRelation::CleanedUp, json!({"kind": "cleaned_up"}));
        pinned(
            &JobProgress {
                state: None,
                busy: false,
            },
            json!({"state": null, "busy": false}),
        );
        pinned(
            &DispatchState::Blocked {
                dependencies: vec!["j1".into(), "j2".into()],
                dependency: "j2".into(),
                reason: BlockReason::Cancelled,
            },
            json!({"kind": "blocked", "dependencies": ["j1", "j2"], "dependency": "j2", "reason": "cancelled"}),
        );
        pinned(
            &DispatchState::NoPeer {
                name: "ghost".into(),
            },
            json!({"kind": "no_peer", "name": "ghost"}),
        );
        pinned(
            &DispatchState::Ready {
                peer_id: "p".into(),
            },
            json!({"kind": "ready", "peer_id": "p"}),
        );
        pinned(
            &JobAction::Retry {
                needs_assignee: true,
            },
            json!({"kind": "retry", "needs_assignee": true}),
        );
        pinned(
            &AskAction::Nudge { to: "p".into() },
            json!({"kind": "nudge", "to": "p"}),
        );
    }

    #[test]
    fn peer_facts_have_pinned_json() {
        pinned(&Liveness::Online, json!({"kind": "online"}));
        pinned(
            &Liveness::Idle { since: 3 },
            json!({"kind": "idle", "since": 3}),
        );
        pinned(
            &Delivery {
                condition: DeliveryCondition::NoPush,
                stuck: Some(Stuck {
                    count: 2,
                    since: 40,
                }),
            },
            json!({"condition": "no_push", "stuck": {"count": 2, "since": 40}}),
        );
    }

    #[test]
    fn missing_lists_jobs_and_defaults_empty() {
        pinned(
            &Missing {
                asks: vec![],
                peers: vec![],
                jobs: vec!["j".into()],
            },
            json!({"asks": [], "peers": [], "jobs": ["j"]}),
        );
        let old: Missing = serde_json::from_value(json!({"asks": [], "peers": []})).unwrap();
        assert!(old.jobs.is_empty());
    }

    #[test]
    fn unknown_strings_decode_to_unknown() {
        assert_eq!(
            serde_json::from_value::<IdleWhy>(json!("later")).unwrap(),
            IdleWhy::Unknown
        );
        assert_eq!(
            serde_json::from_value::<AskOutcome>(json!("later")).unwrap(),
            AskOutcome::Unknown
        );
        assert_eq!(
            serde_json::from_value::<BlockReason>(json!("later")).unwrap(),
            BlockReason::Unknown
        );
        assert_eq!(
            serde_json::from_value::<DeliveryCondition>(json!("later")).unwrap(),
            DeliveryCondition::Unknown
        );
    }

    #[test]
    fn unknown_tags_decode_to_unknown() {
        let later = json!({"kind": "later", "extra": 1});
        assert_eq!(
            serde_json::from_value::<Progress>(later.clone()).unwrap(),
            Progress::Unknown
        );
        assert_eq!(
            serde_json::from_value::<AskState>(later.clone()).unwrap(),
            AskState::Unknown
        );
        assert_eq!(
            serde_json::from_value::<JobRelation>(later.clone()).unwrap(),
            JobRelation::Unknown
        );
        assert_eq!(
            serde_json::from_value::<DispatchState>(later.clone()).unwrap(),
            DispatchState::Unknown
        );
        assert_eq!(
            serde_json::from_value::<Liveness>(later.clone()).unwrap(),
            Liveness::Unknown
        );
        assert_eq!(
            serde_json::from_value::<AskAction>(later.clone()).unwrap(),
            AskAction::Unknown
        );
        assert_eq!(
            serde_json::from_value::<JobAction>(later).unwrap(),
            JobAction::Unknown
        );
    }

    #[test]
    fn a_known_variant_with_a_bad_payload_fails() {
        let bad = json!({"kind": "idle", "since": "soon", "why": "turn_ended"});
        assert!(serde_json::from_value::<Progress>(bad).is_err());
    }

    /* one complete v2 snapshot: a running job on an open ask, a queued job, its peer */
    fn v2() -> serde_json::Value {
        json!({
            "schema_version": 2,
            "captured_at": 100,
            "hub_epoch": "e",
            "capabilities": {},
            "jobs": [
                {"job_id": "j1", "title": "t", "state": "running", "assigned_peer": "w",
                 "from_peer": "boss", "circle": "c", "depends_on": [], "ask_id": "a1",
                 "dispatch": true, "created_at": 1, "finished_at": null, "prompt": "p",
                 "result": null, "from_peer_id": "boss", "worker": "w", "assignee_id": "w",
                 "relation": {"kind": "open"},
                 "progress": {"state": {"kind": "work", "since": 90}, "busy": true},
                 "dispatch_state": null, "actions": []},
                {"job_id": "j2", "title": "t2", "state": "queued", "assigned_peer": "w",
                 "from_peer": "boss", "circle": "c", "depends_on": ["j1"], "ask_id": null,
                 "dispatch": true, "created_at": 2, "finished_at": null, "prompt": "p",
                 "result": null, "from_peer_id": "boss", "worker": null, "assignee_id": "w",
                 "relation": {"kind": "none"}, "progress": null,
                 "dispatch_state": {"kind": "waiting", "dependencies": ["j1"]},
                 "actions": []}
            ],
            "asks": [
                {"correlation_id": "a1", "from_peer": "boss", "to_peer": "w",
                 "to_peer_id": "w", "open": true, "failed": false, "opened_at": 80,
                 "closed_at": null, "closed_by": null, "text": "q", "reply": null,
                 "from_peer_id": "boss",
                 "state": {"kind": "open", "progress": {"kind": "work", "since": 90}},
                 "actions": [], "closed_just_now": false, "opened_just_now": false}
            ],
            "peers": [
                {"peer_id": "w", "name": "w", "backend": "codex", "circle": "c",
                 "status": "online", "last_seen": 99, "activity": null, "running": 1,
                 "push": true, "acks": true, "queued": 0,
                 "liveness": {"kind": "work", "since": 90},
                 "delivery": {"condition": "push", "stuck": null}}
            ],
            "roster": [],
            "missing": {"asks": [], "peers": [], "jobs": []}
        })
    }

    fn v2_without(pointer: &str) -> Result<Decoded, String> {
        let mut value = v2();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        value
            .pointer_mut(parent)
            .and_then(|row| row.as_object_mut())
            .unwrap()
            .remove(key);
        decode_snapshot(value)
    }

    fn rejected(pointer: &str, field: &str, row: &str) {
        let error = v2_without(pointer).unwrap_err();
        assert!(
            error.contains(field) && error.contains(row),
            "{pointer}: {error}"
        );
    }

    #[test]
    fn the_complete_fixture_decodes_as_v2() {
        assert!(matches!(decode_snapshot(v2()), Ok(Decoded::V2(_))));
    }

    #[test]
    fn v2_requires_the_ask_state() {
        rejected("/asks/0/state", "state", "a1");
    }

    #[test]
    fn v2_requires_the_ask_actions() {
        rejected("/asks/0/actions", "actions", "a1");
    }

    #[test]
    fn v2_requires_closed_just_now() {
        rejected("/asks/0/closed_just_now", "closed_just_now", "a1");
    }

    #[test]
    fn v2_requires_opened_just_now() {
        rejected("/asks/0/opened_just_now", "opened_just_now", "a1");
    }

    #[test]
    fn v2_requires_the_job_relation() {
        rejected("/jobs/0/relation", "relation", "j1");
    }

    #[test]
    fn v2_requires_the_job_actions() {
        rejected("/jobs/1/actions", "actions", "j2");
    }

    #[test]
    fn v2_requires_progress_on_a_running_job() {
        rejected("/jobs/0/progress", "progress", "j1");
    }

    #[test]
    fn v2_requires_dispatch_state_on_a_queued_job() {
        rejected("/jobs/1/dispatch_state", "dispatch_state", "j2");
    }

    #[test]
    fn v2_requires_peer_liveness() {
        rejected("/peers/0/liveness", "liveness", "w");
    }

    #[test]
    fn v2_requires_peer_delivery() {
        rejected("/peers/0/delivery", "delivery", "w");
    }

    #[test]
    fn v2_requires_peer_running() {
        rejected("/peers/0/running", "running", "w");
    }

    #[test]
    fn v2_requires_roster_liveness() {
        let mut value = v2();
        let mut row = value["peers"][0].clone();
        row.as_object_mut().unwrap().remove("liveness");
        value["roster"] = json!([row]);
        let error = decode_snapshot(value).unwrap_err();
        assert!(
            error.contains("liveness") && error.contains("roster"),
            "{error}"
        );
    }

    #[test]
    fn v1_drops_every_derived_field() {
        let mut value = v2();
        value["schema_version"] = json!(1);
        value["missing"]["jobs"] = json!(["gone"]);
        let Ok(Decoded::V1Neutral(view)) = decode_snapshot(value) else {
            panic!("v1 must decode neutral");
        };
        let ask = &view.asks[0];
        assert!(ask.state.is_none() && ask.actions.is_none());
        assert!(ask.from_peer_id.is_none() && ask.closed_just_now.is_none());
        assert!(ask.opened_just_now.is_none());
        let job = &view.jobs[0];
        assert!(job.relation.is_none() && job.progress.is_none() && job.actions.is_none());
        assert!(job.worker.is_none() && job.assignee_id.is_none() && job.from_peer_id.is_none());
        assert!(view.jobs[1].dispatch_state.is_none());
        let peer = &view.peers[0];
        assert!(peer.liveness.is_none() && peer.delivery.is_none());
        assert!(view.missing.jobs.is_empty(), "{:?}", view.missing);
    }

    #[test]
    fn a_missing_version_is_v1() {
        let mut value = v2();
        value.as_object_mut().unwrap().remove("schema_version");
        assert!(matches!(decode_snapshot(value), Ok(Decoded::V1Neutral(_))));
    }

    #[test]
    fn a_later_version_is_unsupported() {
        let mut value = v2();
        value["schema_version"] = json!(3);
        assert!(matches!(
            decode_snapshot(value),
            Ok(Decoded::Unsupported(3))
        ));
    }

    #[test]
    fn a_string_version_is_an_error() {
        let mut value = v2();
        value["schema_version"] = json!("2");
        let error = decode_snapshot(value).unwrap_err();
        assert!(error.contains("schema_version is not a number"), "{error}");
    }

    #[test]
    fn every_fixture_decodes_as_v2_or_its_named_version() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let decoded = decode_snapshot(value);
            match name.as_str() {
                "v1.json" => assert!(matches!(decoded, Ok(Decoded::V1Neutral(_))), "{name}"),
                "v3.json" => assert!(matches!(decoded, Ok(Decoded::Unsupported(3))), "{name}"),
                _ => assert!(matches!(decoded, Ok(Decoded::V2(_))), "{name}: {decoded:?}"),
            }
            seen += 1;
        }
        assert!(seen >= 8, "fixtures found: {seen}");
    }
}

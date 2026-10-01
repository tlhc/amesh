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
mod tests;

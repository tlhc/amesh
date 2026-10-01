use super::{event_in_circle, preview, resolve, Ask, Hub, Job, Peer};
use crate::wire::*;
use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap, HashSet};

pub(super) struct ReadModel<'a> {
    pub(super) hub: &'a Hub,
    pub(super) now: u64,
    /* built on first use; a wait never reads it */
    running: OnceCell<HashMap<&'a str, usize>>,
}

impl<'a> ReadModel<'a> {
    pub(super) fn new(hub: &'a Hub, now: u64) -> Self {
        Self {
            hub,
            now,
            running: OnceCell::new(),
        }
    }

    pub(super) fn snapshot(&self, q: &super::SnapshotQuery) -> SnapshotView {
        let hub = self.hub;
        let circle = q.circle.as_deref().filter(|c| !c.is_empty());
        let mut jobs: Vec<&Job> = hub
            .jobs
            .values()
            .filter(|job| circle.is_none_or(|c| job.circle == c))
            .collect();
        jobs.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        /* a dependency the hub no longer has: the scheduler's deleted blocker */
        let missing_jobs: BTreeSet<&String> = jobs
            .iter()
            .flat_map(|job| &job.depends_on)
            .filter(|id| !hub.jobs.contains_key(*id))
            .collect();
        let ask_ids: BTreeSet<&String> =
            jobs.iter().filter_map(|job| job.ask_id.as_ref()).collect();
        let (mut asks, mut missing_asks) = (Vec::new(), Vec::new());
        for cid in ask_ids {
            match hub.asks.get(cid) {
                Some(ask) => asks.push(ask),
                None => missing_asks.push(cid.clone()),
            }
        }
        /* and the asks no job points at, for the TUI's asks screen: in the view's circle when the
        sender's name resolves to a row there or the recipient's id is one there; all of them
        without a circle. An omitted sender is stored as anonymous and names no circle, whoever
        took that name */
        let taken: HashSet<&str> = hub
            .jobs
            .values()
            .filter_map(|job| job.ask_id.as_deref())
            .collect();
        let mut loose: Vec<&Ask> = hub
            .asks
            .values()
            .filter(|ask| !taken.contains(ask.correlation_id.as_str()))
            .filter(|ask| {
                circle.is_none_or(|c| {
                    (ask.from_peer != "anonymous"
                        && resolve(hub, &ask.from_peer).is_some_and(|peer| peer.circle == c))
                        || hub
                            .peers
                            .get(&ask.to_peer_id)
                            .is_some_and(|peer| peer.circle == c)
                })
            })
            .collect();
        loose.sort_by(|a, b| a.correlation_id.cmp(&b.correlation_id));
        asks.extend(loose);
        /* a recipient is a fixed peer_id: once that peer is gone, whoever took its name since is
        someone else. Assignees and senders are names, resolved the way the hub will use them */
        let recipients: BTreeSet<&str> = asks.iter().map(|ask| ask.to_peer_id.as_str()).collect();
        let mut names: BTreeSet<&str> = BTreeSet::new();
        for job in &jobs {
            names.extend(job.assigned_peer.as_deref());
            names.insert(&job.from_peer);
        }
        for ask in &asks {
            names.insert(&ask.from_peer);
        }
        let lookups = recipients
            .into_iter()
            .map(|id| (id, hub.peers.get(id), false))
            .chain(
                names
                    .into_iter()
                    .map(|name| (name, resolve(hub, name), true)),
            );
        let (mut peers, mut missing_peers, mut seen) =
            (Vec::new(), BTreeSet::new(), HashSet::new());
        for (reference, found, name) in lookups.filter(|(reference, _, _)| !reference.is_empty()) {
            match found {
                Some(peer) if seen.insert(peer.peer_id.clone()) => peers.push(self.peer(peer)),
                Some(_) => {}
                /* "anonymous" as a name stands for no sender, unless a peer really took it; a
                recipient id that is gone is missing whatever it reads */
                None if name && reference == "anonymous" => {}
                None => {
                    missing_peers.insert(reference.to_string());
                }
            }
        }
        /* every row in the view's circle, for the TUI's line of who is online; peers stays what
        the jobs and the listed asks refer to */
        let mut listed: Vec<&Peer> = hub
            .peers
            .values()
            .filter(|peer| circle.is_none_or(|c| peer.circle == c))
            .collect();
        listed.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        let roster = listed.into_iter().map(|peer| self.peer(peer)).collect();
        /* what GET /events?circle= would list, counted */
        let event_count = hub
            .events
            .iter()
            .filter(|event| circle.is_none_or(|c| event_in_circle(event, c)))
            .count();
        let detail = q
            .detail
            .as_deref()
            .and_then(|id| jobs.iter().find(|job| job.job_id == id))
            .map(|job| JobDetail {
                job_id: job.job_id.clone(),
                title: job.title.clone(),
                prompt: job.prompt.clone(),
                result: job.result_summary.clone(),
            });
        let ask_detail = q
            .ask
            .as_deref()
            .and_then(|id| asks.iter().find(|ask| ask.correlation_id == id))
            .map(|ask| AskDetail {
                correlation_id: ask.correlation_id.clone(),
                text: ask.text.clone(),
                reply: ask.reply.clone(),
            });
        SnapshotView {
            schema_version: SNAPSHOT_VERSION,
            captured_at: self.now,
            hub_epoch: hub.epoch.clone(),
            event_count: event_count as u64,
            capabilities: Capabilities {
                job_created_at: true,
                ask_opened_at: true,
                ask_closed_by: true,
                peer_activity: true,
                roster: true,
                event_count: true,
                ask_list: true,
                delivery: true,
            },
            jobs: jobs.into_iter().map(|job| self.job(job)).collect(),
            asks: asks.into_iter().map(|ask| self.ask(ask)).collect(),
            peers,
            roster,
            missing: Missing {
                asks: missing_asks,
                peers: missing_peers.into_iter().collect(),
                jobs: missing_jobs.into_iter().cloned().collect(),
            },
            detail,
            ask_detail,
        }
    }

    pub(super) fn ask(&self, ask: &Ask) -> AskView {
        let state = ask_state(self.hub, ask, self.now);
        let actions = ask_actions(&state, ask);
        AskView {
            correlation_id: ask.correlation_id.clone(),
            from_peer: ask.from_peer.clone(),
            to_peer: ask.to_peer.clone(),
            to_peer_id: ask.to_peer_id.clone(),
            open: ask.open,
            failed: ask.failed,
            opened_at: ask.opened_at,
            closed_at: ask.closed_at,
            closed_by: ask.closed_by.clone(),
            text: preview(&ask.text),
            text_len: ask.text.chars().count(),
            reply: ask.reply.as_deref().map(preview),
            reply_len: ask.reply.as_deref().map_or(0, |r| r.chars().count()),
            from_peer_id: self.sender_id(&ask.from_peer).map(str::to_owned),
            state: Some(state),
            actions: Some(actions),
            closed_just_now: Some(closed_just_now(ask, self.now)),
            opened_just_now: Some(opened_just_now(ask, self.now)),
        }
    }

    pub(super) fn job(&self, job: &Job) -> JobView {
        let relation = job_relation(self.hub, job);
        let progress = self.job_progress(job);
        let worker = worker_id(self.hub, job);
        let actions = job_actions(job, &relation, progress.as_ref(), worker);
        JobView {
            job_id: job.job_id.clone(),
            title: preview(&job.title),
            title_len: job.title.chars().count(),
            state: job.state.clone(),
            assigned_peer: job.assigned_peer.clone(),
            from_peer: job.from_peer.clone(),
            circle: job.circle.clone(),
            depends_on: job.depends_on.clone(),
            ask_id: job.ask_id.clone(),
            dispatch: job.dispatch,
            created_at: job.created_at,
            finished_at: job.finished_at,
            prompt: preview(&job.prompt),
            prompt_len: job.prompt.chars().count(),
            result: job.result_summary.as_deref().map(preview),
            result_len: job
                .result_summary
                .as_deref()
                .map_or(0, |r| r.chars().count()),
            from_peer_id: self.sender_id(&job.from_peer).map(str::to_owned),
            worker: worker.map(str::to_owned),
            assignee_id: job
                .assigned_peer
                .as_deref()
                .and_then(|name| self.resolve_id(name))
                .map(str::to_owned),
            relation: Some(relation),
            progress,
            dispatch_state: (job.state == "queued").then(|| assess_dispatch(self.hub, job).state),
            actions: Some(actions),
        }
    }

    pub(super) fn peer(&self, peer: &Peer) -> PeerView {
        PeerView {
            peer_id: peer.peer_id.clone(),
            name: peer.name.clone(),
            backend: peer.backend.clone(),
            circle: peer.circle.clone(),
            status: peer.status.clone(),
            last_seen: peer.last_seen,
            activity: self
                .hub
                .activity
                .get(&peer.peer_id)
                .map(|activity| Activity {
                    state: activity.state.clone(),
                    since: activity.since,
                    observed_at: activity.observed_at,
                    source: activity.source.clone(),
                    reason: activity.reason.clone(),
                }),
            running: Some(self.running_count(&peer.peer_id)),
            push: self.hub.sockets.contains_key(&peer.peer_id),
            acks: self.hub.recv_live.contains(&peer.peer_id),
            queued: self.hub.inbox.count(&peer.peer_id),
            liveness: Some(liveness(self.hub, peer)),
            delivery: Some(delivery(self.hub, peer, self.now)),
        }
    }

    pub(super) fn wait(&self, ask: &Ask) -> WaitDerived {
        let state = ask_state(self.hub, ask, self.now);
        let for_secs = match &state {
            AskState::Open { progress } => progress_age(progress, self.now),
            _ => None,
        };
        let actions = ask_actions(&state, ask);
        WaitDerived {
            schema_version: SNAPSHOT_VERSION,
            captured_at: self.now,
            state,
            from_peer_id: self.sender_id(&ask.from_peer).map(str::to_owned),
            actions,
            for_secs,
        }
    }

    pub(super) fn wait_push_hint(&self, ask: &Ask, caller: Option<&str>) -> bool {
        caller
            .and_then(|name| resolve(self.hub, name))
            .zip(resolve(self.hub, &ask.from_peer))
            .is_some_and(|(caller, asker)| {
                caller.peer_id == asker.peer_id && self.hub.sockets.contains_key(&caller.peer_id)
            })
    }

    pub(super) fn running_count(&self, id: &str) -> usize {
        let hub = self.hub;
        let running = self.running.get_or_init(|| {
            let mut running = HashMap::new();
            for id in hub
                .jobs
                .values()
                .filter_map(|job| counted_worker_id(hub, job))
            {
                *running.entry(id).or_default() += 1;
            }
            running
        });
        running.get(id).copied().unwrap_or(0)
    }

    pub(super) fn resolve_id(&self, name: &str) -> Option<&'a str> {
        resolve(self.hub, name).map(|peer| peer.peer_id.as_str())
    }

    pub(super) fn sender_id(&self, name: &str) -> Option<&'a str> {
        if name.is_empty() || name == "anonymous" {
            return None;
        }
        self.resolve_id(name)
    }

    pub(super) fn job_progress(&self, job: &Job) -> Option<JobProgress> {
        if job.state != "running" {
            return None;
        }
        let relation = job_relation(self.hub, job);
        let state = matches!(relation, JobRelation::NoAsk | JobRelation::Open).then(|| {
            progress(
                self.hub,
                worker_id(self.hub, job),
                job.ask_id.as_ref().and_then(|id| self.hub.asks.get(id)),
                self.now,
            )
        });
        let busy = matches!(state, Some(Progress::Work { .. }))
            && worker_id(self.hub, job).is_some_and(|id| self.running_count(id) == 1);
        Some(JobProgress { state, busy })
    }
}

pub(super) fn progress_age(progress: &Progress, now: u64) -> Option<u64> {
    let since = match progress {
        Progress::Pending { since } => *since,
        Progress::Wait { since, .. } | Progress::Idle { since, .. } | Progress::Work { since } => {
            Some(*since)
        }
        Progress::Gone | Progress::Offline | Progress::Unknown => None,
    };
    since.map(|since| now.saturating_sub(since))
}

pub(super) fn progress(hub: &Hub, id: Option<&str>, ask: Option<&Ask>, now: u64) -> Progress {
    let Some(peer) = id
        .filter(|id| !id.is_empty())
        .and_then(|id| hub.peers.get(id))
    else {
        return Progress::Gone;
    };
    if peer.status != "online" {
        return Progress::Offline;
    }
    let pending = Progress::Pending {
        since: ask.and_then(|ask| ask.opened_at),
    };
    let Some(activity) = hub.activity.get(&peer.peer_id) else {
        return pending;
    };
    match activity.state.as_str() {
        "wait" => Progress::Wait {
            since: activity.since,
            reason: activity.reason.clone(),
        },
        "work" => Progress::Work {
            since: activity.since,
        },
        "idle" => {
            let Some(ask) = ask else {
                return pending;
            };
            match ask.opened_at {
                Some(at) if activity.since <= at => {
                    if now.saturating_sub(at) < 10 {
                        return pending;
                    }
                    Progress::Idle {
                        since: at,
                        why: if activity.since < at {
                            IdleWhy::NotPickedUp
                        } else {
                            IdleWhy::AskStillOpen
                        },
                    }
                }
                _ => Progress::Idle {
                    since: activity.since,
                    why: IdleWhy::TurnEnded,
                },
            }
        }
        _ => pending,
    }
}

pub(super) fn ask_state(hub: &Hub, ask: &Ask, now: u64) -> AskState {
    if ask.open {
        return AskState::Open {
            progress: progress(hub, Some(&ask.to_peer_id), Some(ask), now),
        };
    }
    let outcome = match (ask.closed_by.as_deref(), ask.failed) {
        (Some("recipient"), false) => AskOutcome::AckedOk,
        (Some("recipient"), true) => AskOutcome::AckedFailed,
        (Some("hand"), false) => AskOutcome::HandOk,
        (Some("hand"), true) => AskOutcome::HandFailed,
        (Some("hub"), _) => AskOutcome::Hub,
        (_, false) => AskOutcome::ClosedOk,
        (_, true) => AskOutcome::ClosedFailed,
    };
    AskState::Closed {
        outcome,
        failed_effective: ask.failed || matches!(outcome, AskOutcome::Hub),
    }
}

pub(super) fn job_relation(hub: &Hub, job: &Job) -> JobRelation {
    let Some(id) = &job.ask_id else {
        return JobRelation::NoAsk;
    };
    match hub.asks.get(id) {
        Some(ask) if ask.open => JobRelation::Open,
        Some(_) if job.state == "running" && job.dispatch => JobRelation::Settling,
        Some(_) => JobRelation::Closed,
        None if job.state == "running" => JobRelation::Missing {
            will_fail: job.dispatch,
        },
        None => JobRelation::CleanedUp,
    }
}

pub(super) fn worker_id<'a>(hub: &'a Hub, job: &'a Job) -> Option<&'a str> {
    if job.state == "running" {
        if let Some(id) = &job.ask_id {
            return hub
                .asks
                .get(id)
                .map(|ask| ask.to_peer_id.as_str())
                .filter(|id| !id.is_empty());
        }
    }
    job.assigned_peer
        .as_deref()
        .and_then(|name| resolve(hub, name))
        .map(|peer| peer.peer_id.as_str())
}

pub(super) fn counted_worker_id<'a>(hub: &'a Hub, job: &'a Job) -> Option<&'a str> {
    if job.state != "running" {
        return None;
    }
    match &job.ask_id {
        Some(id) => hub.asks.get(id).map(|ask| ask.to_peer_id.as_str()),
        None => job
            .assigned_peer
            .as_deref()
            .and_then(|name| resolve(hub, name))
            .map(|peer| peer.peer_id.as_str()),
    }
}

pub(super) fn liveness(hub: &Hub, peer: &Peer) -> Liveness {
    if peer.status != "online" {
        return Liveness::Offline;
    }
    match hub.activity.get(&peer.peer_id) {
        Some(activity) => match activity.state.as_str() {
            "wait" => Liveness::Wait {
                since: activity.since,
                reason: activity.reason.clone(),
            },
            "work" => Liveness::Work {
                since: activity.since,
            },
            "idle" => Liveness::Idle {
                since: activity.since,
            },
            _ => Liveness::Online,
        },
        None => Liveness::Online,
    }
}

pub(super) fn delivery(hub: &Hub, peer: &Peer, now: u64) -> Delivery {
    Delivery {
        condition: if peer.status != "online" {
            DeliveryCondition::Offline
        } else if hub.sockets.contains_key(&peer.peer_id) {
            DeliveryCondition::Push
        } else {
            DeliveryCondition::NoPush
        },
        stuck: hub
            .inbox
            .stuck(&peer.peer_id, now)
            .map(|(count, since)| Stuck { count, since }),
    }
}

pub(super) fn closed_just_now(ask: &Ask, now: u64) -> bool {
    ask.closed_at
        .and_then(|at| now.checked_sub(at))
        .is_some_and(|age| age <= 1)
}

pub(super) fn opened_just_now(ask: &Ask, now: u64) -> bool {
    ask.opened_at
        .and_then(|at| now.checked_sub(at))
        .is_some_and(|age| age <= 1)
}

pub(super) fn ask_actions(state: &AskState, ask: &Ask) -> Vec<AskAction> {
    match state {
        AskState::Open {
            progress: Progress::Gone,
        } => vec![AskAction::CloseLeft],
        AskState::Open {
            progress: Progress::Idle { .. },
        } => vec![AskAction::Nudge {
            to: ask.to_peer_id.clone(),
        }],
        _ => vec![],
    }
}

pub(super) fn job_actions(
    job: &Job,
    relation: &JobRelation,
    progress: Option<&JobProgress>,
    worker: Option<&str>,
) -> Vec<JobAction> {
    if job.state == "failed" {
        return vec![JobAction::Retry {
            needs_assignee: !job.dispatch,
        }];
    }
    if job.state == "queued" && !job.dispatch {
        return vec![JobAction::Send];
    }
    if job.state != "running" || !matches!(relation, JobRelation::NoAsk | JobRelation::Open) {
        return vec![];
    }
    match progress.and_then(|progress| progress.state.as_ref()) {
        Some(Progress::Idle { .. }) => worker
            .map(|to| vec![JobAction::Nudge { to: to.to_owned() }])
            .unwrap_or_default(),
        Some(Progress::Gone) if matches!(relation, JobRelation::Open) => vec![JobAction::Resend],
        _ => vec![],
    }
}
#[derive(Debug)]
pub(super) struct DispatchAssessment {
    pub(super) state: DispatchState,
    pub(super) nudge_eligible: bool,
}

pub(super) fn assess_dispatch(hub: &Hub, job: &Job) -> DispatchAssessment {
    let mut dependencies = Vec::new();
    let mut blocker = None;
    for id in &job.depends_on {
        let state = hub.jobs.get(id).map(|dependency| dependency.state.as_str());
        if state == Some("done") {
            continue;
        }
        dependencies.push(id.clone());
        let reason = match state {
            None => Some(BlockReason::Deleted),
            Some("failed") => Some(BlockReason::Failed),
            Some("cancelled") => Some(BlockReason::Cancelled),
            _ => None,
        };
        if blocker.is_none() {
            blocker = reason.map(|reason| (id.clone(), reason));
        }
    }
    let nudge_eligible = job.dispatch && (dependencies.is_empty() || blocker.is_some());
    let state = if let Some((dependency, reason)) = blocker {
        DispatchState::Blocked {
            dependencies,
            dependency,
            reason,
        }
    } else if !dependencies.is_empty() {
        DispatchState::Waiting { dependencies }
    } else if let Some(name) = job.assigned_peer.as_deref().filter(|name| !name.is_empty()) {
        if !job.dispatch {
            DispatchState::Held
        } else if let Some(peer) = resolve(hub, name) {
            if !job.circle.is_empty() && peer.circle != job.circle {
                DispatchState::OtherCircle {
                    peer_id: peer.peer_id.clone(),
                }
            } else {
                DispatchState::Ready {
                    peer_id: peer.peer_id.clone(),
                }
            }
        } else {
            DispatchState::NoPeer { name: name.into() }
        }
    } else {
        DispatchState::Unassigned
    };
    DispatchAssessment {
        state,
        nudge_eligible,
    }
}

#[cfg(test)]
mod tests;

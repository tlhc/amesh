use super::input::{Act, Input, Mode};
use super::layout::{self, width, Fit, Grid, Tone, View};
use super::model::{self, Activity, Ask, Job, Numbers, Peer, Snapshot};
use super::*;
use crate::wire::{
    BlockReason, DispatchState, IdleWhy, JobAction, JobProgress, JobRelation, Liveness, Progress,
};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Modifier;
use ratatui::Terminal;
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

mod ask_states;
mod asks_screen;
mod attention_line;
mod clipboard;
mod compact_card;
mod events_screen;
mod freshness;
mod golden_frames;
mod home_screen;
mod job_states;
mod jobs_flow;
mod knowledge_page;
mod snapshot_compat;
mod status_line;
mod timeline;

fn job(id: &str, state: &str, deps: &[&str], created: u64) -> Job {
    Job {
        job_id: id.into(),
        title: id.into(),
        state: state.into(),
        assigned_peer: Some("cc".into()),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        dispatch: true,
        created_at: Some(created),
        prompt: format!("prompt of {id}"),
        ..Default::default()
    }
}

fn s1() -> Snapshot {
    let mut jobs = vec![job("scope", "done", &[], 100)];
    for (i, (id, state)) in [
        ("func", "done"),
        ("perf", "running"),
        ("deliv", "done"),
        ("sec", "failed"),
        ("upg", "running"),
    ]
    .iter()
    .enumerate()
    {
        jobs.push(job(id, state, &["scope"], 101 + i as u64));
    }
    jobs.push(job(
        "synth",
        "queued",
        &["func", "perf", "deliv", "sec", "upg"],
        110,
    ));
    jobs.push(job("fix", "queued", &["synth"], 111));
    jobs.push(job("deploy", "queued", &["fix"], 112));
    Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }
}

fn s4() -> Snapshot {
    let jobs = vec![
        job("audit", "done", &[], 100),
        job("fix-a", "done", &["audit"], 101),
        job("fix-b", "done", &["audit"], 102),
        job("review", "done", &["fix-a", "fix-b"], 103),
        job("deploy", "running", &["review"], 104),
        job("verify", "queued", &["deploy", "audit"], 105),
    ];
    Snapshot {
        captured_at: 1000,
        jobs,
        ..Default::default()
    }
}

/* s1 plus two jobs without dependencies, older than the chain */
fn mixed() -> Snapshot {
    let mut snap = s1();
    snap.jobs.push(job("lint", "running", &[], 50));
    snap.jobs.push(job("docs", "done", &[], 60));
    snap
}

/* what a card or screen says once borders and line breaks are gone */
fn flat(text: &str) -> String {
    text.replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn numbered(snap: &Snapshot) -> (model::Chain, HashMap<String, usize>) {
    let chain = model::chains(snap).remove(0);
    let num = Numbers::default()
        .assign(&chain)
        .into_iter()
        .map(|(n, id)| (id, n))
        .collect();
    (chain, num)
}

fn card(snap: &Snapshot, id: &str) -> String {
    let (chain, num) = numbered(snap);
    flat(
        &layout::card(
            &View {
                snap: as_frame(snap),
                chain: &chain,
                num: &num,
                base: 0,
                sel: id,
            },
            id,
            46,
            None,
            Fit::Whole,
        )
        .text(),
    )
}

fn as_frame(view: &Snapshot) -> model::Frame {
    model::Frame {
        view: view.clone(),
        fresh: false,
    }
}

fn app_with(snap: Snapshot) -> App {
    let mut app = App::new(Opts {
        circle: None,
        ascii: false,
        color: false,
        anim: true,
        theme: Default::default(),
    });
    app.home = false;
    app.apply(Ok(snap));
    app
}

fn screen_text(app: &mut App, cols: usize, rows: usize) -> Vec<String> {
    app.screen(cols, rows, 0)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/* s1 with its peers reporting: perf's worker w1 works on perf alone, upg's worker w2 has
two running jobs, lint's worker waits for a permission, and fix waits on an idle worker */
fn busy() -> Snapshot {
    let mut snap = s1();
    snap.capabilities.peer_activity = true;
    let peer = |id: &str, state: &str, reason: Option<&str>| Peer {
        peer_id: id.into(),
        name: id.into(),
        status: "online".into(),
        activity: Some(Activity {
            state: state.into(),
            since: 880,
            reason: reason.map(Into::into),
            ..Default::default()
        }),
        liveness: Some(match state {
            "work" => crate::wire::Liveness::Work { since: 880 },
            "wait" => crate::wire::Liveness::Wait {
                since: 880,
                reason: reason.map(Into::into),
            },
            "idle" => crate::wire::Liveness::Idle { since: 880 },
            _ => crate::wire::Liveness::Online,
        }),
        ..Default::default()
    };
    snap.peers = vec![
        peer("w1", "work", None),
        peer("w2", "work", None),
        peer(
            "w3",
            "wait",
            Some("Claude needs your permission to use Bash"),
        ),
        peer("w4", "idle", None),
    ];
    for (id, worker) in [
        ("perf", "w1"),
        ("upg", "w2"),
        ("fix", "w3"),
        ("deploy", "w2"),
        ("synth", "w4"),
    ] {
        let job = snap.jobs.iter_mut().find(|j| j.job_id == id).unwrap();
        job.state = "running".into();
        job.assigned_peer = Some(worker.into());
        job.worker = Some(worker.into());
        job.assignee_id = Some(worker.into());
        job.relation = Some(crate::wire::JobRelation::Open);
        job.progress = Some(crate::wire::JobProgress {
            busy: id == "perf",
            state: Some(match id {
                "fix" => crate::wire::Progress::Wait {
                    since: 880,
                    reason: Some("Claude needs your permission to use Bash".into()),
                },
                "synth" => crate::wire::Progress::Idle {
                    since: 900,
                    why: crate::wire::IdleWhy::NotPickedUp,
                },
                _ => crate::wire::Progress::Work { since: 880 },
            }),
        });
        job.ask_id = Some(format!("ask-{id}"));
        snap.asks.push(Ask {
            correlation_id: format!("ask-{id}"),
            to_peer_id: worker.into(),
            open: true,
            opened_at: Some(900),
            ..Default::default()
        });
    }
    if let Some(job) = snap.jobs.iter_mut().find(|j| j.job_id == "synth") {
        job.actions = Some(vec![crate::wire::JobAction::Nudge { to: "w4".into() }]);
    }
    snap
}

/* the flow row that holds `label` in a 46-column pane */
fn row_of(app: &mut App, label: &str, tick: usize) -> String {
    app.screen(46, 40, tick)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .find(|l| l.contains(label))
        .unwrap_or_else(|| panic!("no row with {label}"))
}

/* the design's sample data, as the approved mock-up drew it; clocks read UTC in tests */
fn at(h: u64, m: u64) -> u64 {
    h * 3600 + m * 60
}

#[allow(clippy::too_many_arguments)]
fn add(
    snap: &mut Snapshot,
    id: &str,
    state: &str,
    deps: &[&str],
    peer: &str,
    ask: Option<(&str, &str, u64)>,
    ended: Option<u64>,
    text: &str,
    prompt: &str,
) {
    let mut job = job(id, state, deps, snap.jobs.len() as u64);
    job.assigned_peer = Some(peer.into());
    job.finished_at = ended;
    job.prompt = prompt.into();
    job.result = (!text.is_empty()).then(|| text.into());
    let has_ask = ask.is_some();
    if let Some((short, from, sent)) = ask {
        job.ask_id = Some(format!("ask-{short}"));
        let open = state == "running";
        let failed = state == "failed";
        snap.asks.push(Ask {
            correlation_id: format!("ask-{short}"),
            from_peer: from.into(),
            to_peer: peer.into(),
            to_peer_id: peer.into(),
            open,
            failed,
            opened_at: Some(sent),
            closed_by: (!open).then(|| "recipient".into()),
            state: Some(if open {
                crate::wire::AskState::Open {
                    progress: crate::wire::Progress::Pending { since: Some(sent) },
                }
            } else {
                crate::wire::AskState::Closed {
                    outcome: if failed {
                        crate::wire::AskOutcome::AckedFailed
                    } else {
                        crate::wire::AskOutcome::AckedOk
                    },
                    failed_effective: failed,
                }
            }),
            ..Default::default()
        });
    }
    job.worker = Some(peer.into());
    job.assignee_id = Some(peer.into());
    job.relation = Some(if state == "running" {
        crate::wire::JobRelation::Open
    } else if has_ask {
        crate::wire::JobRelation::Closed
    } else {
        crate::wire::JobRelation::NoAsk
    });
    snap.jobs.push(job);
}

fn doing(id: &str, state: &str, since: u64, reason: Option<&str>) -> Peer {
    Peer {
        peer_id: id.into(),
        name: id.into(),
        status: "online".into(),
        activity: Some(Activity {
            state: state.into(),
            since,
            reason: reason.map(Into::into),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn set_live(snap: &mut Snapshot, rows: &[(&str, crate::wire::Liveness)]) {
    for peer in snap.peers.iter_mut().chain(snap.roster.iter_mut()) {
        if let Some((_, kind)) = rows.iter().find(|(id, _)| *id == peer.peer_id) {
            peer.liveness = Some(kind.clone());
        }
    }
}

fn set_progress(snap: &mut Snapshot, id: &str, state: crate::wire::Progress, busy: bool) {
    if let Some(job) = snap.jobs.iter_mut().find(|job| job.job_id == id) {
        job.progress = Some(crate::wire::JobProgress {
            state: Some(state),
            busy,
        });
    }
}

fn set_dispatch(snap: &mut Snapshot, id: &str, state: crate::wire::DispatchState) {
    if let Some(job) = snap.jobs.iter_mut().find(|job| job.job_id == id) {
        job.dispatch_state = Some(state);
    }
}

fn proto(now: u64) -> Snapshot {
    let mut snap = Snapshot {
        captured_at: now,
        ..Default::default()
    };
    snap.capabilities.peer_activity = true;
    snap
}

fn proto_s1() -> Snapshot {
    let mut s = proto(at(13, 42));
    let sent = |short, t| Some((short, "cc", t));
    add(
        &mut s,
        "scope",
        "done",
        &[],
        "cc",
        sent("1f0", at(13, 19)),
        Some(at(13, 20)),
        "5 review dimensions and a shared brief",
        "split the cleanup review into dimensions",
    );
    add(
        &mut s,
        "func",
        "done",
        &["scope"],
        "codex",
        sent("5b1", at(13, 21)),
        Some(at(13, 28)),
        "3 findings: 2 fixed (P2), 1 wontfix",
        "review correctness of the sweep and the delivery filter",
    );
    add(
        &mut s,
        "perf",
        "running",
        &["scope"],
        "codex",
        sent("9a1", at(13, 30)),
        None,
        "",
        "review perf: sweep lock time and inbox filter cost at 100k rows",
    );
    add(
        &mut s,
        "deliv",
        "done",
        &["scope"],
        "pi",
        sent("4c7", at(13, 21)),
        Some(at(13, 25)),
        "filter covers all 3 delivery paths; 1 P3",
        "review delivery and concurrency",
    );
    add(
        &mut s,
        "sec",
        "failed",
        &["scope"],
        "pi",
        sent("2ee1", at(13, 21)),
        Some(at(13, 39)),
        "no fixture for the ws recv path",
        "review security and data retention",
    );
    add(
        &mut s,
        "upg",
        "running",
        &["scope"],
        "pi-3",
        sent("7c2", at(13, 36)),
        None,
        "",
        "review the upgrade path for old state files and mixed versions",
    );
    add(
        &mut s,
        "synth",
        "queued",
        &["func", "perf", "deliv", "sec", "upg"],
        "cc",
        None,
        None,
        "",
        "merge the five reviews into one decision list",
    );
    add(
        &mut s,
        "fix",
        "queued",
        &["synth"],
        "cc",
        None,
        None,
        "",
        "apply the accepted fixes",
    );
    add(
        &mut s,
        "deploy",
        "queued",
        &["fix"],
        "cc",
        None,
        None,
        "",
        "install the build and restart the hub",
    );
    s.peers = vec![
        doing("cc", "work", at(13, 40), None),
        doing("codex", "idle", at(13, 37), None),
        doing("pi", "work", at(13, 40), None),
        doing("pi-3", "work", at(13, 36), None),
    ];
    set_live(
        &mut s,
        &[
            ("cc", crate::wire::Liveness::Work { since: at(13, 40) }),
            ("codex", crate::wire::Liveness::Idle { since: at(13, 37) }),
            ("pi", crate::wire::Liveness::Work { since: at(13, 40) }),
            ("pi-3", crate::wire::Liveness::Work { since: at(13, 36) }),
        ],
    );
    set_progress(
        &mut s,
        "perf",
        crate::wire::Progress::Idle {
            since: at(13, 37),
            why: crate::wire::IdleWhy::TurnEnded,
        },
        false,
    );
    s.jobs
        .iter_mut()
        .find(|j| j.job_id == "perf")
        .unwrap()
        .actions = Some(vec![crate::wire::JobAction::Nudge { to: "codex".into() }]);
    set_progress(
        &mut s,
        "upg",
        crate::wire::Progress::Work { since: at(13, 36) },
        true,
    );
    set_dispatch(
        &mut s,
        "synth",
        crate::wire::DispatchState::Blocked {
            dependencies: vec!["perf".into(), "sec".into(), "upg".into()],
            dependency: "sec".into(),
            reason: crate::wire::BlockReason::Failed,
        },
    );
    set_dispatch(
        &mut s,
        "fix",
        crate::wire::DispatchState::Waiting {
            dependencies: vec!["synth".into()],
        },
    );
    set_dispatch(
        &mut s,
        "deploy",
        crate::wire::DispatchState::Waiting {
            dependencies: vec!["fix".into()],
        },
    );
    s
}

fn proto_s5() -> Snapshot {
    let mut s = proto(at(13, 26));
    add(
        &mut s,
        "scope",
        "done",
        &[],
        "cc",
        Some(("1f0", "cc", at(13, 10))),
        Some(at(13, 11)),
        "10 review dimensions",
        "split the release review",
    );
    /* the mock-up's workers, ask ids and results; upg and tests get workers of their
    own, since the mock-up spins both under one codex and a worker with two running jobs
    shows no spinner, and times are real timestamps where the mock-up's ages disagree */
    let reviews = [
        ("func", "done", "codex"),
        ("perf", "running", "pi"),
        ("deliv", "done", "pi-3"),
        ("sec", "failed", "pi-2"),
        ("upg", "running", "u1"),
        ("docs", "done", "pi"),
        ("api", "done", "pi-3"),
        ("cli", "queued", "pi-2"),
        ("tests", "running", "u2"),
        ("ux", "done", "pi"),
    ];
    for (i, (id, state, peer)) in reviews.iter().enumerate() {
        let short = format!("{i}a{i}");
        let ask = (*state != "queued").then_some((short.as_str(), "cc", at(13, 12)));
        let ended = matches!(*state, "done" | "failed").then_some(at(13, 20));
        let result = if *state == "failed" {
            format!("{id} review could not run its fixture")
        } else {
            format!("{id} review: no blocking findings")
        };
        add(
            &mut s,
            id,
            state,
            &["scope"],
            peer,
            ask,
            ended,
            &result,
            &format!("review the release for {id}"),
        );
    }
    add(
        &mut s,
        "synth",
        "queued",
        &reviews.map(|(id, _, _)| id),
        "cc",
        None,
        None,
        "",
        "merge the ten reviews",
    );
    add(
        &mut s,
        "fix",
        "queued",
        &["synth"],
        "cc",
        None,
        None,
        "",
        "apply the accepted fixes",
    );
    add(
        &mut s,
        "deploy",
        "queued",
        &["fix"],
        "cc",
        None,
        None,
        "",
        "ship the release",
    );
    let sec = s.jobs.iter_mut().find(|j| j.job_id == "sec").unwrap();
    sec.job_id = "job-58851116".into();
    for job in &mut s.jobs {
        for dep in &mut job.depends_on {
            if dep == "sec" {
                *dep = "job-58851116".into();
            }
        }
    }
    s.peers = vec![
        doing("cc", "work", at(13, 25), None),
        doing("codex", "idle", at(13, 20), None),
        doing("pi", "idle", at(13, 22), None),
        doing("pi-3", "idle", at(13, 20), None),
        doing("pi-2", "idle", at(13, 20), None),
        doing("u1", "work", at(13, 12), None),
        doing("u2", "work", at(13, 12), None),
    ];
    set_live(
        &mut s,
        &[
            ("cc", crate::wire::Liveness::Work { since: at(13, 25) }),
            ("codex", crate::wire::Liveness::Idle { since: at(13, 20) }),
            ("pi", crate::wire::Liveness::Idle { since: at(13, 22) }),
            ("pi-3", crate::wire::Liveness::Idle { since: at(13, 20) }),
            ("pi-2", crate::wire::Liveness::Idle { since: at(13, 20) }),
            ("u1", crate::wire::Liveness::Work { since: at(13, 12) }),
            ("u2", crate::wire::Liveness::Work { since: at(13, 12) }),
        ],
    );
    set_progress(
        &mut s,
        "perf",
        crate::wire::Progress::Idle {
            since: at(13, 22),
            why: crate::wire::IdleWhy::TurnEnded,
        },
        false,
    );
    set_progress(
        &mut s,
        "upg",
        crate::wire::Progress::Work { since: at(13, 12) },
        true,
    );
    set_progress(
        &mut s,
        "tests",
        crate::wire::Progress::Work { since: at(13, 12) },
        true,
    );
    if let Some(job) = s.jobs.iter_mut().find(|j| j.job_id == "job-58851116") {
        job.actions = Some(vec![crate::wire::JobAction::Retry {
            needs_assignee: false,
        }]);
    }
    s
}

fn with(mut job: Job, progress: Progress, busy: bool) -> Job {
    job.progress = Some(JobProgress {
        state: Some(progress),
        busy,
    });
    job
}

fn flow(snap: &Snapshot, sel: &str, cols: usize, wide: bool) -> (Grid, HashMap<String, usize>) {
    let (chain, num) = numbered(snap);
    let v = View {
        snap: model::Frame {
            view: snap.clone(),
            ..Default::default()
        },
        chain: &chain,
        num: &num,
        base: 0,
        sel,
    };
    let g = match wide {
        true => layout::horizontal(&v, cols).expect("fits"),
        false => layout::vertical(&v, cols),
    };
    (g, num)
}

fn tone_at(g: &Grid, r: usize, c: usize) -> Tone {
    g.rows[&r][&c].tone
}

/* the row as text with a space for every empty cell, so a char index is its column */
fn cells_text(g: &Grid, r: usize) -> String {
    let row = &g.rows[&r];
    let end = row.keys().max().copied().unwrap_or(0);
    (0..=end)
        .map(|c| row.get(&c).map_or(' ', |cell| cell.ch))
        .collect()
}

fn find(g: &Grid, needle: &str) -> (usize, usize) {
    g.rows
        .keys()
        .find_map(|r| {
            cells_text(g, *r)
                .find(needle)
                .map(|i| (*r, cells_text(g, *r)[..i].chars().count()))
        })
        .unwrap_or_else(|| panic!("no {needle} in\n{}", g.text()))
}

fn peer(id: &str, status: &str, state: Option<&str>) -> Peer {
    Peer {
        peer_id: id.into(),
        name: id.into(),
        status: status.into(),
        activity: state.map(|s| Activity {
            state: s.into(),
            ..Default::default()
        }),
        liveness: state.map(|s| match s {
            "work" => crate::wire::Liveness::Work { since: 0 },
            "wait" => crate::wire::Liveness::Wait {
                since: 0,
                reason: None,
            },
            "idle" => crate::wire::Liveness::Idle { since: 0 },
            _ => crate::wire::Liveness::Unknown,
        }),
        ..Default::default()
    }
}

fn with_roster(mut snap: Snapshot, roster: Vec<Peer>) -> Snapshot {
    snap.capabilities.peer_activity = true;
    snap.capabilities.roster = true;
    snap.roster = roster;
    snap
}

fn joined(pieces: &[(String, layout::Tone)]) -> String {
    pieces.iter().map(|(text, _)| text.as_str()).collect()
}

fn with_events(mut snap: Snapshot, n: u64) -> Snapshot {
    snap.capabilities.event_count = true;
    snap.event_count = n;
    snap
}

const CC: &str = "openhitls-sm2-opt-claude-code";
const CODEX: &str = "openhitls-sm2-opt-codex";
const PI: &str = "openhitls-sm2-opt-pi";
const PI2: &str = "openhitls-sm2-opt-pi-2";

fn lone(cid: &str, from: &str, to: &str, opened_at: Option<u64>) -> Ask {
    Ask {
        correlation_id: cid.into(),
        from_peer: from.into(),
        to_peer: to.into(),
        to_peer_id: to.into(),
        open: true,
        opened_at,
        text: format!("question {cid}"),
        ..Default::default()
    }
}

fn closed(mut ask: Ask, at: u64, by: &str, failed: bool, reply: &str) -> Ask {
    ask.open = false;
    ask.closed_at = Some(at);
    ask.closed_by = (!by.is_empty()).then(|| by.into());
    ask.failed = failed;
    ask.reply = Some(reply.into());
    ask
}

/* asks and the peers they name, who are the view's own peers, as a hub with ask_list sends
them */
fn with_asks(mut snap: Snapshot, asks: Vec<Ask>, peers: Vec<Peer>) -> Snapshot {
    snap.capabilities.ask_list = true;
    snap.capabilities.peer_activity = true;
    snap.capabilities.roster = true;
    snap.asks.extend(asks);
    snap.roster.extend(peers.iter().cloned());
    snap.peers.extend(peers);
    snap
}

fn stamp(
    mut ask: Ask,
    state: crate::wire::AskState,
    actions: Vec<crate::wire::AskAction>,
    just: Option<bool>,
) -> Ask {
    ask.open = matches!(state, crate::wire::AskState::Open { .. });
    ask.state = Some(state);
    ask.actions = Some(actions);
    ask.closed_just_now = just;
    ask
}

/* the fan-out of the design: one question to three peers; codex has answered, pi's turn
ended without an answer */
fn fan_out() -> Snapshot {
    use crate::wire::{AskAction, AskOutcome, AskState, IdleWhy, Liveness, Progress};
    let mut snap = with_asks(
        Snapshot {
            captured_at: 1000,
            ..Default::default()
        },
        vec![
            stamp(
                lone("ask-1223c9e0", CC, PI2, Some(820)),
                AskState::Open {
                    progress: Progress::Work { since: 830 },
                },
                vec![],
                None,
            ),
            stamp(
                lone("ask-a3a07b51", CC, PI, Some(820)),
                AskState::Open {
                    progress: Progress::Idle {
                        since: 940,
                        why: IdleWhy::TurnEnded,
                    },
                },
                vec![AskAction::Nudge { to: PI.into() }],
                None,
            ),
            stamp(
                closed(
                    lone("ask-a904f2d6", CC, CODEX, Some(820)),
                    950,
                    "recipient",
                    false,
                    "2 findings, both fixed",
                ),
                AskState::Closed {
                    outcome: AskOutcome::AckedOk,
                    failed_effective: false,
                },
                vec![],
                Some(false),
            ),
        ],
        vec![
            doing(CC, "idle", 830, None),
            doing(CODEX, "idle", 950, None),
            doing(PI, "idle", 940, None),
            doing(PI2, "work", 830, None),
        ],
    );
    for peer in snap.peers.iter_mut().chain(snap.roster.iter_mut()) {
        peer.liveness = Some(match peer.peer_id.as_str() {
            PI2 => Liveness::Work { since: 830 },
            CODEX => Liveness::Idle { since: 950 },
            PI => Liveness::Idle { since: 940 },
            _ => Liveness::Idle { since: 830 },
        });
    }
    /* every sender here is a peer of the view whose name is its id, as the hub resolves it */
    for ask in snap.asks.iter_mut() {
        ask.from_peer_id = Some(ask.from_peer.clone());
    }
    snap
}

/* the design's busy asks: the fan-out, codex at a permission prompt on a newer ask, pi-2's
question that claude-code has not picked up, an ask to a peer that left, and a failed answer */
fn busy_asks() -> Snapshot {
    use crate::wire::{AskAction, AskOutcome, AskState, IdleWhy, Liveness, Progress};
    let mut snap = fan_out();
    let open = |cid, from, to, at, progress, actions| {
        let ask = lone(cid, from, to, Some(at));
        stamp(ask, AskState::Open { progress }, actions, None)
    };
    let reason = Some("Claude needs your permission to use Bash".to_string());
    let mut perm = open(
        "ask-5d0c11e2",
        CC,
        CODEX,
        960,
        Progress::Wait {
            since: 985,
            reason: reason.clone(),
        },
        vec![],
    );
    perm.text = "run the full sm2 bench on the arm64 box and post the table".into();
    let failed = closed(
        lone("ask-e81f3b07", CC, PI2, Some(600)),
        900,
        "recipient",
        true,
        "build broke: sm2_sign undefined after the rebase",
    );
    snap.asks.extend([
        perm,
        open(
            "ask-77aa0d93",
            PI2,
            CC,
            975,
            Progress::Idle {
                since: 975,
                why: IdleWhy::NotPickedUp,
            },
            vec![AskAction::Nudge { to: CC.into() }],
        ),
        open(
            "ask-0c9e4a18",
            CC,
            "openhitls-sm2-opt-gemini",
            700,
            Progress::Gone,
            vec![AskAction::CloseLeft],
        ),
        stamp(
            failed,
            AskState::Closed {
                outcome: AskOutcome::AckedFailed,
                failed_effective: true,
            },
            vec![],
            Some(false),
        ),
    ]);
    for ask in snap.asks.iter_mut() {
        ask.from_peer_id = Some(ask.from_peer.clone());
    }
    set_live(&mut snap, &[(CODEX, Liveness::Wait { since: 985, reason })]);
    snap
}

fn with_lone_asks(snap: Snapshot) -> Snapshot {
    with_asks(
        snap,
        vec![
            lone("ask-1223c9e0", CC, PI2, Some(820)),
            lone("ask-a3a07b51", CC, PI, Some(820)),
        ],
        vec![doing(PI2, "work", 830, None), doing(PI, "work", 830, None)],
    )
}

fn copy_app() -> App {
    let mut app = app_with(sample());
    app.sel = Some("j".into());
    app
}

/* the screen at `tick`, one string a line */
fn screen_at(app: &mut App, cols: usize, tick: usize) -> Vec<String> {
    app.screen(cols, 40, tick)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn line_with(app: &mut App, cols: usize, tick: usize, needle: &str) -> String {
    let lines = screen_at(app, cols, tick);
    lines
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle}:\n{}", lines.join("\n")))
        .clone()
}

/* an online peer with a push channel or without, and the records waiting for it */
fn reached(id: &str, push: bool, queued: usize) -> Peer {
    Peer {
        push,
        queued,
        ..peer(id, "online", Some("idle"))
    }
}

fn delivering(snap: Snapshot, roster: Vec<Peer>, at: u64) -> Snapshot {
    let mut snap = with_roster(snap, roster);
    snap.capabilities.delivery = true;
    snap.captured_at = at;
    snap
}

fn marked(
    id: &str,
    condition: crate::wire::DeliveryCondition,
    stuck: Option<(usize, u64)>,
) -> Peer {
    let mut peer = reached(id, true, 0);
    peer.liveness = Some(crate::wire::Liveness::Idle { since: 0 });
    peer.delivery = Some(crate::wire::Delivery {
        condition,
        stuck: stuck.map(|(count, since)| crate::wire::Stuck { count, since }),
    });
    peer
}

fn ev(seq: u64, kind: &str, from: &str, to: &str, body: &str) -> model::Event {
    let asked = kind == "ask" || kind == "ack";
    model::Event {
        seq: Some(seq),
        at: Some(1000 + seq),
        kind: kind.into(),
        from_peer: from.into(),
        to_peer: to.into(),
        correlation_id: if asked {
            format!("ask-{seq:08}")
        } else {
            String::new()
        },
        text: if kind == "ask" {
            body.into()
        } else {
            String::new()
        },
        message: if kind == "ask" {
            String::new()
        } else {
            body.into()
        },
        ..Default::default()
    }
}

/* fan_out()'s asks as the hub records them: an event for each recipient, all from one sender
with one text in one second, numbered from `seq` */
fn fan_events(seq: u64) -> Vec<model::Event> {
    fan_out()
        .asks
        .iter()
        .zip(seq..)
        .map(|(ask, n)| model::Event {
            at: Some(1000 + seq),
            correlation_id: ask.correlation_id.clone(),
            ..ev(
                n,
                "ask",
                &ask.from_peer,
                &ask.to_peer,
                "review the SM2 sign path",
            )
        })
        .collect()
}

fn unnumbered(text: &str) -> model::Event {
    model::Event {
        kind: "notify".into(),
        from_peer: "cc".into(),
        to_peer: "pi".into(),
        message: text.into(),
        ..Default::default()
    }
}

fn ask_with(state: crate::wire::AskState, opened: Option<u64>) -> Ask {
    Ask {
        correlation_id: "ask-row".into(),
        from_peer: "amesh-claude-code".into(),
        to_peer_id: "amesh-pi".into(),
        open: matches!(state, crate::wire::AskState::Open { .. }),
        opened_at: opened,
        state: Some(state),
        ..Default::default()
    }
}

fn cell(row: &[(String, layout::Tone)], text: &str) -> layout::Tone {
    row.iter()
        .find(|(t, _)| t.contains(text))
        .unwrap_or_else(|| panic!("no {text} in {row:?}"))
        .1
}

fn b3c_card(job: Job, asks: Vec<Ask>) -> String {
    let snap = Snapshot {
        captured_at: 1000,
        jobs: vec![job],
        asks,
        ..Default::default()
    };
    let (chain, num) = numbered(&snap);
    flat(
        &layout::card(
            &View {
                snap: model::Frame {
                    view: snap,
                    ..Default::default()
                },
                chain: &chain,
                num: &num,
                base: 0,
                sel: "job",
            },
            "job",
            80,
            None,
            Fit::Whole,
        )
        .text(),
    )
}

fn b3c_job(state: &str) -> Job {
    Job {
        job_id: "job".into(),
        title: "title".into(),
        state: state.into(),
        assigned_peer: Some("codex".into()),
        worker: Some("codex".into()),
        assignee_id: Some("codex".into()),
        ..Default::default()
    }
}

fn b3c_scene(job: Job, asks: Vec<Ask>, peers: Vec<Peer>) -> (Snapshot, layout::Grid) {
    let snap = Snapshot {
        captured_at: 1000,
        jobs: vec![job],
        asks,
        peers,
        ..Default::default()
    };
    let (chain, num) = numbered(&snap);
    let grid = layout::card(
        &View {
            snap: model::Frame {
                view: snap.clone(),
                ..Default::default()
            },
            chain: &chain,
            num: &num,
            base: 0,
            sel: "job",
        },
        "job",
        80,
        None,
        Fit::Whole,
    );
    (snap, grid)
}

fn b3c_text(job: Job, asks: Vec<Ask>, peers: Vec<Peer>) -> String {
    flat(&b3c_scene(job, asks, peers).1.text())
}

fn b3c_word_tone(grid: &layout::Grid, word: &str) -> layout::Tone {
    for row in grid.rows.values() {
        let cells: Vec<_> = row.values().collect();
        let text: String = cells.iter().map(|cell| cell.ch).collect();
        if let Some(at) = text.find(word) {
            let chars = text[..at].chars().count();
            return cells[chars].tone;
        }
    }
    panic!("no {word}");
}

fn b3c_ask(from: &str, to_id: &str, to_name: &str) -> Ask {
    Ask {
        correlation_id: "ask-1".into(),
        from_peer: from.into(),
        to_peer: to_name.into(),
        to_peer_id: to_id.into(),
        open: true,
        opened_at: Some(820),
        ..Default::default()
    }
}

fn sample() -> Snapshot {
    Snapshot {
        captured_at: 1000,
        jobs: vec![Job {
            job_id: "j".into(),
            title: "review".into(),
            state: "running".into(),
            assigned_peer: Some("codex".into()),
            worker: Some("codex".into()),
            ask_id: Some("ask-1".into()),
            relation: Some(JobRelation::Open),
            progress: Some(JobProgress {
                state: Some(Progress::Idle {
                    since: 940,
                    why: IdleWhy::TurnEnded,
                }),
                busy: false,
            }),
            prompt: "check the patch".into(),
            actions: Some(vec![
                JobAction::Nudge { to: "codex".into() },
                JobAction::Resend,
            ]),
            ..Default::default()
        }],
        asks: vec![Ask {
            correlation_id: "ask-1".into(),
            from_peer: "boss".into(),
            to_peer: "codex".into(),
            to_peer_id: "codex".into(),
            opened_at: Some(820),
            open: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn badge(g: &Grid, word: &str, tone: Tone, cols: usize) {
    let start = cols - 1 - word.len();
    for (i, ch) in word.chars().enumerate() {
        let cell = &g.rows[&0][&(start + i)];
        assert_eq!((cell.ch, cell.tone), (ch, tone), "badge {word}, cell {i}");
    }
}

/* the first row drawn in rule characters */
fn rule_row(lines: &[String]) -> usize {
    lines
        .iter()
        .position(|l| l.starts_with("────") || l.starts_with("----"))
        .unwrap_or_else(|| panic!("no rule:\n{}", lines.join("\n")))
}

/* an age of about 120 s reads 121s, or 122s on a slow run */
fn aged(row: &str, head: &str, tail: &str) -> bool {
    ["121s", "122s"]
        .iter()
        .any(|age| row.ends_with(&format!("{head}{age}{tail}")))
}

fn held_from(events: Vec<model::Event>) -> Vec<super::feed::Held> {
    events
        .into_iter()
        .map(|e| super::feed::Held(0, Box::new(e)))
        .collect()
}

/* the HOME design's busy circle: busy_asks(), the report chain at work beside a failed lint, and
the events around the fan-out, as the design drew them */
fn home_busy() -> (Snapshot, Vec<model::Event>) {
    let mut snap = busy_asks();
    let mut bench = job("bench-baseline", "done", &[], 900);
    bench.title = "bench baseline".into();
    bench.assigned_peer = Some(CODEX.into());
    bench.finished_at = Some(960);
    let mut tune = job("tune-ladder", "running", &["bench-baseline"], 901);
    tune.title = "tune ladder".into();
    tune.assigned_peer = Some(PI2.into());
    tune.worker = Some(PI2.into());
    let mut report = job("report", "queued", &["tune-ladder"], 902);
    report.dispatch_state = Some(DispatchState::Waiting {
        dependencies: vec!["tune-ladder".into()],
    });
    let mut lint = job("lint", "failed", &[], 903);
    lint.assigned_peer = Some(PI.into());
    lint.result = Some("clippy: 3 warnings".into());
    lint.finished_at = Some(990);
    snap.jobs = vec![bench, tune, report, lint];
    let mut events = vec![model::Event {
        seq: Some(1),
        at: Some(1001),
        kind: "chat".into(),
        peer: CC.into(),
        role: "user".into(),
        text: "fan the timing review out to pi, pi-2 and codex".into(),
        ..Default::default()
    }];
    for (i, to) in [CODEX, PI, PI2].iter().enumerate() {
        let mut b = ev(
            2 + i as u64,
            "broadcast",
            CC,
            to,
            "heads-up: SM2 sign path review starting",
        );
        b.id = "b1".into();
        b.at = Some(1002);
        events.push(b);
    }
    events.extend(fan_events(5));
    events.push(ev(8, "ack", CODEX, CC, "2 findings, both fixed"));
    events.push(model::Event {
        failed: Some(true),
        ..ev(
            9,
            "ack",
            PI2,
            CC,
            "[failed] build broke: sm2_sign undefined after the rebase",
        )
    });
    events.push(ev(
        10,
        "notify",
        "amesh",
        CC,
        "job j-7f3a sm2-perf is running: pi has not acked",
    ));
    events.push(ev(11, "notify", CC, PI, "ask a3a0 waits for your ack"));
    (snap, events)
}

fn home_app((snap, events): (Snapshot, Vec<model::Event>)) -> App {
    let mut app = app_with(snap);
    app.home = true;
    app.take_events(Ok(events));
    app
}

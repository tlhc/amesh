use super::*;

/* boss creates a job for worker in circle one; deputy can take its coordination over, helper
answers to a name that is not its peer id, outsider sits in circle two */
async fn handoff_hub() -> (Router, Arc<Mutex<Hub>>, TempState, String) {
    let (app, hub, state) = test_app_with_hub();
    for (id, circle) in [
        ("boss", "one"),
        ("deputy", "one"),
        ("worker", "one"),
        ("outsider", "two"),
    ] {
        register(app.clone(), id, "pi", circle).await;
    }
    let _ = json_req(
        app.clone(),
        "POST",
        "/peers",
        json!({"name": "helper", "peer_id": "p-helper", "backend": "pi", "circle": "one"}),
    )
    .await;
    let (st, job) = json_req(
        app.clone(),
        "POST",
        "/jobs",
        json!({"title": "build", "assigned_peer": "worker", "from_peer": "boss"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{job}");
    let jid = job["job_id"].as_str().unwrap().to_string();
    (app, hub, state, jid)
}

async fn patch_job(app: &Router, jid: &str, body: Value) -> (StatusCode, Value) {
    json_req(app.clone(), "PATCH", &format!("/jobs/{jid}"), body).await
}

/* the handoff notices a peer holds for one job */
fn handoffs(hub: &Hub, peer: &str, jid: &str) -> Vec<Value> {
    hub.inbox
        .records(peer)
        .iter()
        .filter(|e| {
            e.event()["type"] == "notify"
                && e.event()["topic"] == jid
                && e.event()["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("you coordinate it now"))
        })
        .map(|record| record.event().clone())
        .collect()
}

fn acked_to(hub: &Hub, peer: &str, cid: &str) -> bool {
    hub.inbox
        .records(peer)
        .iter()
        .any(|e| e.event()["type"] == "ack" && e.event()["correlation_id"] == cid)
}

#[tokio::test]
async fn a_handoff_moves_a_running_jobs_ack_to_the_new_coordinator() {
    let (app, hub, _state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    let (st, job) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::OK, "{job}");
    assert_eq!(job["from_peer"], "deputy");
    assert_eq!(job["state"], "running");
    assert_eq!(job["ask_id"], cid.as_str());
    {
        let hub = hub.lock().await;
        assert_eq!(hub.asks[&cid].from_peer, "deputy");
        assert!(hub.asks[&cid].open);
        let got = handoffs(&hub, "deputy", &jid);
        assert_eq!(got.len(), 1, "{got:?}");
        let text = got[0]["message"].as_str().unwrap();
        for part in [
            jid.as_str(),
            "running",
            cid.as_str(),
            "boss",
            "amesh_job_status",
        ] {
            assert!(text.contains(part), "{part} missing from: {text}");
        }
        assert!(handoffs(&hub, "boss", &jid).is_empty());
    }
    let (st, _) = json_req(
        app.clone(),
        "POST",
        "/ack",
        json!({"correlation_id": cid, "from_peer": "worker", "message": "built"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    {
        let mut hub = hub.lock().await;
        assert!(
            acked_to(&hub, "deputy", &cid),
            "the ack reaches the new coordinator"
        );
        assert!(!acked_to(&hub, "boss", &cid), "and not the old one");
        advance_jobs(&mut hub);
        assert_eq!(hub.jobs[&jid].state, "done");
        assert_eq!(hub.jobs[&jid].result_summary.as_deref(), Some("built"));
    }
    let (_, waited) = json_req(
        app,
        "POST",
        &format!("/asks/{cid}/wait"),
        json!({"timeout_seconds": 0}),
    )
    .await;
    assert_eq!(waited["reply"], "built", "a wait on the cid still reads it");
}

#[tokio::test]
async fn an_ack_before_the_handoff_stays_with_the_old_coordinator() {
    let (app, hub, state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    let _ = json_req(
        app.clone(),
        "POST",
        "/ack",
        json!({"correlation_id": cid, "from_peer": "worker", "message": "built"}),
    )
    .await;
    let (st, job) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::OK, "{job}");
    assert_eq!(job["state"], "done", "the handoff settles first");
    assert_eq!(job["result_summary"], "built");
    assert_eq!(job["from_peer"], "deputy");
    let id = {
        let hub = hub.lock().await;
        assert_eq!(
            hub.asks[&cid].from_peer, "boss",
            "a closed ask keeps its sender"
        );
        assert!(
            acked_to(&hub, "boss", &cid),
            "the ack already sent stays queued"
        );
        let got = handoffs(&hub, "deputy", &jid);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0]["message"]
            .as_str()
            .unwrap()
            .contains("result is ready"));
        got[0]["id"].clone()
    };
    let disk = Hub::open(&state).unwrap();
    assert_eq!(disk.jobs[&jid].state, "done");
    assert_eq!(disk.jobs[&jid].from_peer, "deputy");
    let kept = handoffs(&disk, "deputy", &jid);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["id"], id, "the notice was written with the change");
}

#[tokio::test]
async fn a_coordinator_only_update_never_touches_the_jobs_run() {
    let (app, hub, _state, jid) = handoff_hub().await;
    let same = |a: &Job, b: &Job| {
        (
            &a.state,
            &a.result_summary,
            a.finished_at,
            a.nudge_at,
            a.dispatch,
        ) == (
            &b.state,
            &b.result_summary,
            b.finished_at,
            b.nudge_at,
            b.dispatch,
        ) && (&a.assigned_peer, &a.prompt, &a.ask_id) == (&b.assigned_peer, &b.prompt, &b.ask_id)
    };
    /* queued: the next dispatch goes out from the new coordinator */
    let before = hub.lock().await.jobs[&jid].clone();
    let (st, job) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::OK, "{job}");
    assert!(
        same(&before, &hub.lock().await.jobs[&jid]),
        "queued job unchanged"
    );
    let cid = running(&hub, &jid).await;
    {
        let hub = hub.lock().await;
        assert_eq!(hub.asks[&cid].from_peer, "deputy");
        let sent = hub
            .inbox
            .records("worker")
            .iter()
            .find(|e| e.event()["type"] == "ask" && e.event()["correlation_id"] == cid.as_str())
            .expect("the worker got the dispatch");
        /* the ask was queued when the job was created, before this handoff: its copy
        keeps the sender it was written with, while the ask row above carries the new one */
        assert_eq!(sent.event()["from_peer"], "boss");
    }
    /* running: the hub dispatched after the caller last looked; the handoff keeps the run */
    let before = hub.lock().await.jobs[&jid].clone();
    let (st, job) = patch_job(&app, &jid, json!({"coordinator": "boss"})).await;
    assert_eq!(st, StatusCode::OK, "{job}");
    {
        let mut hub = hub.lock().await;
        assert!(same(&before, &hub.jobs[&jid]), "running job unchanged");
        assert!(hub.asks[&cid].open, "the worker's ask stays open");
        advance_jobs(&mut hub);
        let asks = hub
            .asks
            .values()
            .filter(|a| a.to_peer_id == "worker")
            .count();
        assert_eq!(asks, 1, "no second dispatch");
        let told = hub
            .inbox
            .records("worker")
            .iter()
            .filter(|e| e.event()["type"] == "notify")
            .count();
        assert_eq!(told, 0, "the worker hears nothing");
    }
    /* finished: result and times stay */
    let _ = patch_job(&app, &jid, json!({"state": "done", "result_summary": "ok"})).await;
    let before = hub.lock().await.jobs[&jid].clone();
    let (st, _) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::OK);
    let hub = hub.lock().await;
    assert!(same(&before, &hub.jobs[&jid]), "finished job unchanged");
    assert_eq!(hub.jobs[&jid].from_peer, "deputy");
}

#[tokio::test]
async fn a_same_target_handoff_changes_nothing_but_keeps_a_settlement() {
    let (app, hub, state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    let (_, job) = patch_job(&app, &jid, json!({"coordinator": "helper"})).await;
    assert_eq!(
        job["from_peer"], "p-helper",
        "a name is stored as its peer id"
    );
    for again in ["helper", "p-helper"] {
        let (st, job) = patch_job(&app, &jid, json!({"coordinator": again})).await;
        assert_eq!(st, StatusCode::OK, "{job}");
    }
    assert_eq!(
        handoffs(&*hub.lock().await, "p-helper", &jid).len(),
        1,
        "a retry sends no second notice"
    );
    /* the ack is in, the job not yet settled: the no-op still writes the settlement */
    let _ = json_req(
        app.clone(),
        "POST",
        "/ack",
        json!({"correlation_id": cid, "from_peer": "worker", "message": "built"}),
    )
    .await;
    assert_eq!(hub.lock().await.jobs[&jid].state, "running");
    let (_, job) = patch_job(&app, &jid, json!({"coordinator": "helper"})).await;
    assert_eq!(job["state"], "done");
    assert_eq!(handoffs(&*hub.lock().await, "p-helper", &jid).len(), 1);
    let disk = Hub::open(&state).unwrap();
    assert_eq!(disk.jobs[&jid].state, "done", "the settlement reached disk");
    assert_eq!(disk.jobs[&jid].result_summary.as_deref(), Some("built"));
}

#[tokio::test]
async fn a_refused_handoff_changes_nothing() {
    let (app, hub, _state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    for (body, status) in [
        (json!({"coordinator": "nobody"}), StatusCode::NOT_FOUND),
        (json!({"coordinator": "outsider"}), StatusCode::FORBIDDEN),
        (json!({"coordinator": ""}), StatusCode::BAD_REQUEST),
        (
            json!({"coordinator": "deputy", "state": "running"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"coordinator": "deputy", "assigned_peer": "deputy"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"coordinator": "deputy", "result_summary": "x"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"coordinator": "deputy", "prompt": "x"}),
            StatusCode::BAD_REQUEST,
        ),
        (json!({}), StatusCode::BAD_REQUEST),
    ] {
        let (st, out) = patch_job(&app, &jid, body.clone()).await;
        assert_eq!(st, status, "{body} -> {out}");
        let hub = hub.lock().await;
        assert_eq!(hub.jobs[&jid].from_peer, "boss", "{body}");
        assert_eq!(hub.jobs[&jid].state, "running", "{body}");
        assert_eq!(hub.asks[&cid].from_peer, "boss", "{body}");
        assert!(hub.asks[&cid].open, "{body}");
        for peer in ["deputy", "outsider"] {
            assert!(handoffs(&hub, peer, &jid).is_empty(), "{body}");
        }
    }
    let (st, _) = patch_job(&app, "job-nope", json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn closures_after_a_handoff_reach_the_new_coordinator() {
    /* by hand */
    let (app, hub, _state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    let _ = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    let (st, _) = json_req(
        app.clone(),
        "POST",
        &format!("/jobs/{jid}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    {
        let hub = hub.lock().await;
        assert!(acked_to(&hub, "deputy", &cid));
        assert!(!acked_to(&hub, "boss", &cid));
    }
    /* by the hub, for a worker that left */
    let (app, hub, _state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    let _ = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    let mut hub = hub.lock().await;
    hub.peers.get_mut("worker").unwrap().last_seen = 1;
    probe_peers(&mut hub).unwrap();
    assert_eq!(hub.asks[&cid].closed_by.as_deref(), Some("hub"));
    assert!(acked_to(&hub, "deputy", &cid));
    assert!(!acked_to(&hub, "boss", &cid));
}

#[tokio::test]
async fn reminders_and_retries_follow_the_new_coordinator() {
    let (app, hub, _state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    let _ = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    {
        let mut hub = hub.lock().await;
        hub.jobs.get_mut(&jid).unwrap().nudge_at = Some(now_unix() - 1);
        advance_jobs(&mut hub);
        let reminded = |peer: &str| {
            hub.inbox.records(peer).iter().any(|e| {
                e.event()["type"] == "notify"
                    && e.event()["topic"] == jid.as_str()
                    && e.event()["message"]
                        .as_str()
                        .is_some_and(|m| m.contains("has not acked"))
            })
        };
        assert!(
            reminded("deputy"),
            "the reminder goes to the new coordinator"
        );
        assert!(!reminded("boss"));
    }
    let (st, _) = patch_job(&app, &jid, json!({"state": "queued"})).await;
    assert_eq!(st, StatusCode::OK);
    let again = running(&hub, &jid).await;
    assert_ne!(again, cid);
    assert_eq!(hub.lock().await.asks[&again].from_peer, "deputy");
}

#[tokio::test]
async fn a_handoff_is_written_with_its_notice_or_not_at_all() {
    let (app, hub, state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    hub.lock()
        .await
        .db
        .execute_batch("PRAGMA query_only = ON")
        .unwrap();
    let (st, _) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
    {
        let hub = hub.lock().await;
        assert_eq!(
            hub.jobs[&jid].from_peer, "boss",
            "a failed write rolls the senders back"
        );
        assert_eq!(hub.asks[&cid].from_peer, "boss");
        assert!(
            handoffs(&hub, "deputy", &jid).is_empty(),
            "and queues no notice"
        );
    }
    let (_, events) = json_req(app.clone(), "GET", "/events", json!({})).await;
    assert!(
        !events.to_string().contains("you coordinate it now"),
        "nothing was delivered: {events}"
    );
    hub.lock()
        .await
        .db
        .execute_batch("PRAGMA query_only = OFF")
        .unwrap();
    let (st, _) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::OK);
    let id = handoffs(&*hub.lock().await, "deputy", &jid)[0]["id"].clone();
    /* a crash right after the write loses nothing: senders and notice are on disk together */
    let disk = Hub::open(&state).unwrap();
    assert_eq!(disk.jobs[&jid].from_peer, "deputy");
    assert_eq!(disk.asks[&cid].from_peer, "deputy");
    let kept = handoffs(&disk, "deputy", &jid);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["id"], id);
    assert_eq!(
        disk.jobs[&jid].handoff.as_deref(),
        id.as_str(),
        "the job names its current notice on disk"
    );
}

#[tokio::test]
async fn mcp_hands_off_a_job_without_a_state() {
    let (app, hub, _state, jid) = handoff_hub().await;
    let (st, body) = mcp_tool(
        app.clone(),
        "amesh_job_update",
        json!({"from_peer": "boss", "job_id": jid, "coordinator": "deputy"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(tool_json(&body)["from_peer"], "deputy", "{body}");
    assert_eq!(hub.lock().await.jobs[&jid].from_peer, "deputy");
    let (_, list) = json_req(
        app,
        "POST",
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let update = tools
        .iter()
        .find(|t| t["name"] == "amesh_job_update")
        .unwrap();
    assert_eq!(
        update["inputSchema"]["properties"]["coordinator"]["type"],
        "string"
    );
    assert_eq!(update["inputSchema"]["required"], json!(["job_id"]));
}

#[tokio::test]
async fn mcp_refuses_a_mistyped_job_update() {
    let (app, hub, _state, jid) = handoff_hub().await;
    let cid = running(&hub, &jid).await;
    for args in [
        json!({"from_peer": "boss", "job_id": jid, "coordinator": "deputy", "state": 123}),
        json!({"from_peer": "boss", "job_id": jid, "coordinator": 123, "state": "queued"}),
        json!({"from_peer": "boss", "job_id": jid, "state": "queued", "prompt": 7}),
    ] {
        let (st, body) = mcp_tool(app.clone(), "amesh_job_update", args.clone()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{args} -> {body}");
        let hub = hub.lock().await;
        assert_eq!(hub.jobs[&jid].from_peer, "boss", "{args}");
        assert_eq!(hub.jobs[&jid].state, "running", "{args}");
        assert!(hub.asks[&cid].open, "{args}");
        assert_eq!(hub.asks[&cid].from_peer, "boss", "{args}");
    }
}

#[tokio::test]
async fn a_handoff_notice_outlives_the_inbox_cap() {
    let (app, hub, _state, jid) = handoff_hub().await;
    let chatter = |n: usize| {
        (0..n)
            .map(|i| json!({"type": "notify", "id": format!("chat-{i}"), "message": "noise"}))
            .collect::<Vec<_>>()
    };
    queue_inbox(
        &mut *hub.lock().await,
        "deputy",
        chatter(INBOX_MAX),
        now_unix(),
    );
    let (st, _) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::OK);
    queue_inbox(
        &mut *hub.lock().await,
        "deputy",
        chatter(INBOX_MAX),
        now_unix(),
    );
    let hub = hub.lock().await;
    assert_eq!(
        handoffs(&hub, "deputy", &jid).len(),
        1,
        "chatter gives way to the notice, as to an ask"
    );
    assert!(
        hub.inbox.records("deputy").len() <= INBOX_MAX,
        "the cap still holds"
    );
}

#[tokio::test]
async fn superseded_handoff_notices_are_reclaimed() {
    let held = |hub: &Hub, jid: &str| {
        ["boss", "deputy", "worker", "p-helper"]
            .iter()
            .map(|peer| handoffs(hub, peer, jid).len())
            .sum::<usize>()
    };
    /* back and forth: only the current coordinator's notice stays */
    let (app, hub, _state, jid) = handoff_hub().await;
    let _ = running(&hub, &jid).await;
    for (n, to) in ["deputy", "boss", "deputy", "boss", "deputy"]
        .iter()
        .enumerate()
    {
        let (st, _) = patch_job(&app, &jid, json!({"coordinator": to})).await;
        assert_eq!(st, StatusCode::OK, "handoff {n}");
    }
    {
        let hub = hub.lock().await;
        assert_eq!(held(&hub, &jid), 1, "one notice per job");
        assert_eq!(
            handoffs(&hub, "deputy", &jid).len(),
            1,
            "and it is deputy's"
        );
    }
    /* a deleted job takes its notice along */
    let (st, _) = json_req(app.clone(), "DELETE", &format!("/jobs/{jid}"), json!({})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(held(&*hub.lock().await, &jid), 0, "deleted job, no notice");
    /* so does a job the sweep ends */
    let (_, job) = json_req(
        app.clone(),
        "POST",
        "/jobs",
        json!({"title": "old", "assigned_peer": "worker", "from_peer": "boss"}),
    )
    .await;
    let old = job["job_id"].as_str().unwrap().to_string();
    let _ = patch_job(&app, &old, json!({"coordinator": "deputy"})).await;
    let mut hub = hub.lock().await;
    assert_eq!(held(&hub, &old), 1);
    let job = hub.jobs.get_mut(&old).unwrap();
    job.state = "done".into();
    job.finished_at = Some(now_unix() - JOB_KEEP_SECS - 60);
    hub.sweep_at = 0;
    advance_jobs(&mut hub);
    assert!(!hub.jobs.contains_key(&old), "the sweep ended the job");
    assert_eq!(held(&hub, &old), 0, "swept job, no notice");
}

/* deputy is attached over a link that never confirms frames, so its notice sits in the
channel, not the inbox, when the job moves on; the link then drops and hands back what it held */
async fn replay_after(mode: &str) -> usize {
    let (app, hub, state, jid) = handoff_hub().await;
    let (tx, rx) = mpsc::unbounded_channel();
    hub.lock().await.sockets.insert("deputy".into(), (1, tx));
    let (st, _) = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        handoffs(&*hub.lock().await, "deputy", &jid).is_empty(),
        "sent down the link"
    );
    match mode {
        "away" | "back" => {
            let _ = patch_job(&app, &jid, json!({"coordinator": "boss"})).await;
            if mode == "back" {
                let _ = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
            }
        }
        "delete" => {
            let (st, _) = json_req(app.clone(), "DELETE", &format!("/jobs/{jid}"), json!({})).await;
            assert_eq!(st, StatusCode::OK);
        }
        "sweep" => {
            let mut hub = hub.lock().await;
            let job = hub.jobs.get_mut(&jid).unwrap();
            job.state = "done".into();
            job.finished_at = Some(now_unix() - JOB_KEEP_SECS - 60);
            hub.sweep_at = 0;
            advance_jobs(&mut hub);
            assert!(!hub.jobs.contains_key(&jid));
        }
        _ => {}
    }
    let owner = App {
        inner: hub.clone(),
        token: None,
        state_path: state.to_path_buf(),
    };
    close_connection(&owner, "deputy", 1, false, rx, Vec::new()).await;
    let disk = Hub::open(&state).unwrap();
    handoffs(&disk, "deputy", &jid).len()
}

#[tokio::test]
async fn a_dropped_link_hands_back_the_current_notice() {
    assert_eq!(replay_after("current").await, 1);
}

#[tokio::test]
async fn a_dropped_link_hands_back_no_notice_once_the_job_moved_on() {
    assert_eq!(replay_after("away").await, 0);
}

#[tokio::test]
async fn a_dropped_link_hands_back_one_notice_after_the_job_came_back() {
    assert_eq!(replay_after("back").await, 1);
}

#[tokio::test]
async fn a_dropped_link_hands_back_no_notice_for_a_deleted_job() {
    assert_eq!(replay_after("delete").await, 0);
}

#[tokio::test]
async fn a_dropped_link_hands_back_no_notice_for_a_swept_job() {
    assert_eq!(replay_after("sweep").await, 0);
}

/* deputy attached twice: the first link holds an older notice when a second link takes over,
and the links give their frames back in either order; only the newest notice may survive */
async fn two_links(order: &str) -> (Vec<Value>, Value) {
    let (app, hub, state, jid) = handoff_hub().await;
    let (tx1, mut rx1) = mpsc::unbounded_channel();
    hub.lock().await.sockets.insert("deputy".into(), (1, tx1));
    let _ = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    let old = rx1.try_recv().unwrap();
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    {
        let mut hub = hub.lock().await;
        let _ = hub.sockets["deputy"].1.send(Outbound::Control(
            json!({"type": DISPLACED, "peer_id": "deputy"}),
        ));
        hub.sockets.insert("deputy".into(), (2, tx2));
    }
    let owner = App {
        inner: hub.clone(),
        token: None,
        state_path: state.to_path_buf(),
    };
    if order == "old link first" {
        close_connection(&owner, "deputy", 1, false, rx1, vec![old]).await;
        let handed = rx2.try_recv().unwrap();
        let _ = patch_job(&app, &jid, json!({"coordinator": "boss"})).await;
        let _ = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
        let newest = rx2.try_recv().unwrap();
        close_connection(
            &owner,
            "deputy",
            2,
            false,
            rx2,
            vec![handed, newest.clone()],
        )
        .await;
        return (
            handoffs(&Hub::open(&state).unwrap(), "deputy", &jid),
            newest.event().clone(),
        );
    }
    let _ = patch_job(&app, &jid, json!({"coordinator": "boss"})).await;
    let _ = patch_job(&app, &jid, json!({"state": "done"})).await;
    let _ = patch_job(&app, &jid, json!({"coordinator": "deputy"})).await;
    let newest = rx2.try_recv().unwrap();
    assert_ne!(old.event()["id"], newest.event()["id"]);
    assert!(newest.event()["message"]
        .as_str()
        .unwrap()
        .contains("result is ready"));
    if order == "forwarded" {
        close_connection(&owner, "deputy", 1, false, rx1, vec![old]).await;
        close_connection(&owner, "deputy", 2, false, rx2, vec![newest.clone()]).await;
    } else {
        close_connection(&owner, "deputy", 2, false, rx2, vec![newest.clone()]).await;
        close_connection(&owner, "deputy", 1, false, rx1, vec![old]).await;
    }
    (
        handoffs(&Hub::open(&state).unwrap(), "deputy", &jid),
        newest.event().clone(),
    )
}

#[tokio::test]
async fn a_late_old_link_cannot_replace_the_newest_notice() {
    let (kept, newest) = two_links("late old link").await;
    assert_eq!(kept, vec![newest]);
}

#[tokio::test]
async fn an_old_frame_forwarded_to_the_new_link_cannot_replace_the_newest_notice() {
    let (kept, newest) = two_links("forwarded").await;
    assert_eq!(kept, vec![newest]);
}

#[tokio::test]
async fn links_handing_back_in_order_keep_the_newest_notice() {
    let (kept, newest) = two_links("old link first").await;
    assert_eq!(kept, vec![newest]);
}

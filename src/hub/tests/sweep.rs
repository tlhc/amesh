use super::*;

#[tokio::test]
async fn the_sweep_keeps_every_known_peer_and_clears_the_rest() {
    let dir = std::env::temp_dir().join(format!("amesh-sweep-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.db");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    {
        let mut hub = state.inner.lock().await;
        hub.peers.insert(
            "known".into(),
            Peer {
                peer_id: "known".into(),
                name: "known".into(),
                path: "/tmp".into(),
                backend: "pi".into(),
                circle: "default".into(),
                status: "offline".into(),
                description: String::new(),
                session_id: String::new(),
                last_seen: now_unix(),
            },
        );
    }
    let kept = dir.join("hook-ws-known.inbox");
    let gone = dir.join("hook-ws-forgotten.inbox");
    let dead = dir.join("ws-forgotten.pid");
    fs::write(&kept, "sock\n1\n").unwrap();
    fs::write(&gone, "sock\n1\n").unwrap();
    fs::write(&dead, "999999999\n").unwrap();
    fs::write(dir.join("state.db-wal"), "x").unwrap();
    sweep_runtime_files(&state).await;
    assert!(
        kept.exists(),
        "a peer the hub knows, online or not, keeps its stamp"
    );
    assert!(!gone.exists(), "a stamp nobody owns is cleared");
    assert!(!dead.exists(), "a pid file for a dead process is cleared");
    assert!(
        path.exists() && dir.join("state.db-wal").exists(),
        "state files are never touched"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_sweep_deletes_attachments_past_their_keep_time() {
    let dir = std::env::temp_dir().join(format!("amesh-sweep-attach-{}", Uuid::new_v4()));
    let attachments = dir.join("attachments");
    fs::create_dir_all(&attachments).unwrap();
    let path = dir.join("state.db");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    /* a keep time of a minute, which the sweep can only have from the config */
    state.inner.lock().await.config.attachment_keep_secs = 60;
    let (old, young) = (attachments.join("att-old"), attachments.join("att-young"));
    fs::write(&old, "x").unwrap();
    fs::write(&young, "x").unwrap();
    let past = std::time::SystemTime::now() - Duration::from_secs(120);
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(past)
        .unwrap();
    sweep_runtime_files(&state).await;
    assert!(!old.exists(), "an attachment past its keep time is deleted");
    assert!(young.exists(), "a younger one stays");
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_sweep_caps_oversized_logs_in_place() {
    let dir = std::env::temp_dir().join(format!("amesh-sweep-cap-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.db");
    let state = App {
        inner: Arc::new(Mutex::new(Hub::open(&path).unwrap())),
        token: None,
        state_path: path.clone(),
    };
    {
        let mut hub = state.inner.lock().await;
        hub.peers.insert(
            "known".into(),
            Peer {
                peer_id: "known".into(),
                name: "known".into(),
                path: "/tmp".into(),
                backend: "pi".into(),
                circle: "default".into(),
                status: "offline".into(),
                description: String::new(),
                session_id: String::new(),
                last_seen: now_unix(),
            },
        );
    }
    let kept = dir.join("hook-ws-known.log");
    let serve = dir.join("serve.log");
    let gone = dir.join("hook-ws-forgotten.log");
    let launch = dir.join("codex-app-server.log");
    let unknown = dir.join("notes.log");
    let outside = temp_state("elsewhere");
    let elsewhere = outside.with_file_name("kept.txt");
    fs::write(&kept, vec![b'x'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    fs::write(&serve, vec![b'y'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    fs::write(&gone, vec![b'z'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    fs::write(&launch, vec![b'w'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    fs::write(&elsewhere, vec![b'v'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    fs::write(&unknown, vec![b'u'; crate::cli::LOG_CAP as usize + 1]).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("linked.log")).unwrap();
    sweep_runtime_files(&state).await;
    assert_eq!(
        fs::metadata(&launch).unwrap().len(),
        0,
        "a log amesh does not write itself is capped too"
    );
    assert_eq!(
        fs::metadata(&elsewhere).unwrap().len(),
        crate::cli::LOG_CAP + 1,
        "a link named like a log leaves the file it names alone"
    );
    assert_eq!(
        fs::metadata(&unknown).unwrap().len(),
        crate::cli::LOG_CAP + 1,
        "a log amesh knows nothing of may have a writer that is not appending"
    );
    assert!(kept.exists(), "a known peer keeps its log");
    assert_eq!(
        fs::metadata(&kept).unwrap().len(),
        0,
        "oversize live log is truncated in place"
    );
    assert_eq!(
        fs::metadata(&serve).unwrap().len(),
        0,
        "serve.log is truncated in place"
    );
    assert!(
        !gone.exists(),
        "an unowned oversize log is still leftover garbage"
    );
    let _ = fs::remove_dir_all(&dir);
}

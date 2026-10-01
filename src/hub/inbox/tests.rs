use super::*;
use serde_json::json;

#[test]
fn stuck_below_two_seconds_and_at_boundary() {
    let mut inbox = Inbox::default();
    assert_eq!(inbox.stuck("p", 100), None);
    inbox.put("p", vec![]);
    assert_eq!(inbox.stuck("p", 100), None);
    inbox.put("p", vec![QueuedRecord::new(json!({"id": "a"}), 100)]);
    assert_eq!(inbox.stuck("p", 101), None);
    assert_eq!(inbox.stuck("p", 102), Some((1, 100)));
}

#[test]
fn stuck_counts_recent_records_with_oldest() {
    let mut inbox = Inbox::default();
    inbox.put(
        "p",
        vec![
            QueuedRecord::new(json!({}), 100),
            QueuedRecord::new(json!({}), 102),
        ],
    );
    assert_eq!(inbox.stuck("p", 102), Some((2, 100)));
}

#[test]
fn stuck_uses_minimum_timestamp_not_front() {
    let mut inbox = Inbox::default();
    inbox.put(
        "p",
        vec![
            QueuedRecord::new(json!({}), 102),
            QueuedRecord::new(json!({}), 100),
        ],
    );
    assert_eq!(inbox.stuck("p", 102), Some((2, 100)));
}

#[test]
fn stuck_future_timestamp_saturates() {
    let mut inbox = Inbox::default();
    inbox.put("p", vec![QueuedRecord::new(json!({}), 103)]);
    assert_eq!(inbox.stuck("p", 102), None);
}

#[test]
fn enqueue_protects_asks_and_evicts_oldest_chatter() {
    let mut inbox = Inbox::default();
    inbox.enqueue(
        "p",
        [QueuedRecord::new(json!({"type": "ask", "id": "ask"}), 0)],
        false,
    );
    inbox.enqueue(
        "p",
        (0..super::super::INBOX_MAX).map(|i| QueuedRecord::new(json!({"id": i}), 0)),
        false,
    );
    assert_eq!(inbox.count("p"), super::super::INBOX_MAX);
    assert_eq!(inbox.records("p")[0].event()["id"], "ask");
    assert_eq!(inbox.records("p")[1].event()["id"], 1);
}

#[test]
fn owed_ignores_cap() {
    let mut inbox = Inbox::default();
    inbox.enqueue(
        "p",
        (0..super::super::INBOX_MAX + 2).map(|i| QueuedRecord::new(json!({"id": i}), 0)),
        true,
    );
    assert_eq!(inbox.count("p"), super::super::INBOX_MAX + 2);
    assert_eq!(inbox.records("p")[0].event()["id"], 0);
}

#[test]
fn new_handoff_supersedes_same_topic() {
    let mut inbox = Inbox::default();
    inbox.enqueue(
        "p",
        [
            QueuedRecord::new(json!({"handoff": true, "topic": "a", "id": "old"}), 0),
            QueuedRecord::new(json!({"handoff": true, "topic": "b", "id": "other"}), 0),
            QueuedRecord::new(json!({"handoff": true, "topic": "a", "id": "new"}), 0),
        ],
        false,
    );
    assert_eq!(
        inbox.records("p"),
        &[
            QueuedRecord::new(json!({"handoff": true, "topic": "b", "id": "other"}), 0),
            QueuedRecord::new(json!({"handoff": true, "topic": "a", "id": "new"}), 0)
        ]
    );
}

#[test]
fn acknowledge_matches_id_and_cleans_empty_queue() {
    let mut inbox = Inbox::default();
    inbox.enqueue(
        "p",
        [
            QueuedRecord::new(json!({"id": "a"}), 0),
            QueuedRecord::new(json!({"id": "b"}), 0),
        ],
        false,
    );
    assert!(!inbox.acknowledge("p", "unknown"));
    assert!(inbox.acknowledge("p", "a"));
    assert_eq!(
        inbox.records("p"),
        &[QueuedRecord::new(json!({"id": "b"}), 0)]
    );
    assert!(inbox.acknowledge("p", "b"));
    assert_eq!(inbox.iter().count(), 0);
    assert!(!inbox.acknowledge("p", "b"));
}

#[test]
fn transfer_deduplicates_against_target_and_preserves_order() {
    let mut inbox = Inbox::default();
    inbox.put(
        "to",
        vec![
            QueuedRecord::new(json!({"id": "target"}), 90),
            QueuedRecord::new(json!({"id": "duplicate"}), 91),
        ],
    );
    inbox.put(
        "from",
        vec![
            QueuedRecord::new(json!({"id": "duplicate"}), 91),
            QueuedRecord::new(json!({"id": "moved"}), 100),
            QueuedRecord::new(json!({"id": "moved"}), 101),
        ],
    );
    inbox.transfer_owed("from", "to");
    assert_eq!(
        inbox.records("to"),
        &[
            QueuedRecord::new(json!({"id": "target"}), 90),
            QueuedRecord::new(json!({"id": "duplicate"}), 91),
            QueuedRecord::new(json!({"id": "moved"}), 100),
            QueuedRecord::new(json!({"id": "moved"}), 101)
        ]
    );
    assert_eq!(inbox.count("from"), 0);
}

#[test]
fn retain_all_cleans_empty_queues() {
    let mut inbox = Inbox::default();
    inbox.put("drop", vec![QueuedRecord::new(json!({"keep": true}), 100)]);
    inbox.put(
        "keep",
        vec![
            QueuedRecord::new(json!({"keep": true}), 100),
            QueuedRecord::new(json!({"keep": false}), 100),
        ],
    );
    inbox.retain_all(|peer, record| peer == "keep" && record.event()["keep"] == true);
    assert_eq!(inbox.iter().count(), 1);
    assert_eq!(
        inbox.records("keep"),
        &[QueuedRecord::new(json!({"keep": true}), 100)]
    );
}

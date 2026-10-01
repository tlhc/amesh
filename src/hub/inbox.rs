use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{with_event_id, INBOX_MAX};

#[derive(Clone, Debug)]
pub(super) enum Outbound {
    Record(QueuedRecord),
    Control(Value),
}

impl Outbound {
    pub(super) fn event(&self) -> &Value {
        match self {
            Self::Record(record) => record.event(),
            Self::Control(event) => event,
        }
    }
}

const STUCK_SECS: u64 = 2;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct QueuedRecord {
    event: Value,
    queued_at: u64,
}

impl QueuedRecord {
    pub(super) fn new(event: Value, now: u64) -> Self {
        Self {
            event,
            queued_at: now,
        }
    }

    pub(super) fn event(&self) -> &Value {
        &self.event
    }

    pub(super) fn queued_at(&self) -> u64 {
        self.queued_at
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Inbox {
    queues: HashMap<String, Vec<QueuedRecord>>,
}

impl Inbox {
    pub(super) fn records(&self, peer: &str) -> &[QueuedRecord] {
        self.queues.get(peer).map(Vec::as_slice).unwrap_or_default()
    }

    pub(super) fn count(&self, peer: &str) -> usize {
        self.records(peer).len()
    }

    pub(super) fn stuck(&self, peer: &str, now: u64) -> Option<(usize, u64)> {
        let records = self.records(peer);
        let oldest = records.iter().map(QueuedRecord::queued_at).min()?;
        (now.saturating_sub(oldest) >= STUCK_SECS).then_some((records.len(), oldest))
    }

    pub(super) fn enqueue(
        &mut self,
        peer_id: &str,
        events: impl IntoIterator<Item = QueuedRecord>,
        owed: bool,
    ) {
        /* what an acknowledging peer is owed is never evicted: the cap would silently take back
        the at-least-once promise, and pruning the peer is the only bound on that queue */
        let queue = self.queues.entry(peer_id.to_string()).or_default();
        for event in events {
            /* a newer handoff notice for a job replaces the one the queue holds: a link that
            drops hands its frames back oldest first, so the newest is the one kept */
            if event.event()["handoff"] == true {
                let topic = event.event()["topic"].clone();
                queue.retain(|held| {
                    held.event()["handoff"] != true || held.event()["topic"] != topic
                });
            }
            /* only an ask leaves someone blocked on an answer, so chatter gives way to it and a
            queue of nothing but asks is allowed past the cap rather than strand an asker; a
            handoff notice is kept the same way, since it is the new coordinator's only signal
            and a retry of the handoff sends none */
            while !owed && queue.len() >= INBOX_MAX {
                let Some(chatter) = queue.iter().position(|held| {
                    held.event()["type"] != "ask" && held.event()["handoff"] != true
                }) else {
                    break;
                };
                queue.remove(chatter);
            }
            queue.push(event);
        }
        if queue.is_empty() {
            self.queues.remove(peer_id);
        }
    }

    pub(super) fn take(&mut self, peer: &str) -> Vec<QueuedRecord> {
        self.queues.remove(peer).unwrap_or_default()
    }

    pub(super) fn put(&mut self, peer: &str, records: Vec<QueuedRecord>) {
        if records.is_empty() {
            self.queues.remove(peer);
        } else {
            self.queues.insert(peer.to_string(), records);
        }
    }

    pub(super) fn acknowledge(&mut self, peer: &str, id: &str) -> bool {
        let before = self.count(peer);
        self.retain(peer, |held| held.event()["id"].as_str() != Some(id));
        before != self.count(peer)
    }

    pub(super) fn retain(&mut self, peer: &str, predicate: impl FnMut(&QueuedRecord) -> bool) {
        if let Some(queue) = self.queues.get_mut(peer) {
            queue.retain(predicate);
            if queue.is_empty() {
                self.queues.remove(peer);
            }
        }
    }

    pub(super) fn retain_all(&mut self, mut predicate: impl FnMut(&str, &QueuedRecord) -> bool) {
        self.queues.retain(|peer, queue| {
            queue.retain(|record| predicate(peer, record));
            !queue.is_empty()
        });
    }

    pub(super) fn ensure_ids(&mut self, peer: &str) {
        if let Some(queue) = self.queues.get_mut(peer) {
            for record in queue {
                if record.event()["id"]
                    .as_str()
                    .map(str::is_empty)
                    .unwrap_or(true)
                {
                    record.event = with_event_id(record.event.take());
                }
            }
        }
    }

    pub(super) fn transfer_owed(&mut self, from: &str, to: &str) {
        let moved = self.take(from);
        let mut target = self.take(to);
        let present: HashSet<String> = target
            .iter()
            .filter_map(|held| held.event()["id"].as_str().map(str::to_string))
            .collect();
        for mut record in moved {
            record.event = with_event_id(record.event);
            let id = record.event()["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if present.contains(&id) {
                for held in &mut target {
                    if held.event()["id"].as_str() == Some(&id) {
                        held.queued_at = held.queued_at.min(record.queued_at);
                    }
                }
                continue;
            }
            target.push(record);
        }
        self.put(to, target);
    }

    pub(super) fn remove_peer(&mut self, peer: &str) {
        self.queues.remove(peer);
    }

    #[cfg(test)]
    pub(super) fn iter(&self) -> impl Iterator<Item = (&String, &Vec<QueuedRecord>)> {
        self.queues.iter()
    }

    pub(super) fn from_map(mut queues: HashMap<String, Vec<QueuedRecord>>) -> Self {
        queues.retain(|_, records| !records.is_empty());
        Self { queues }
    }

    pub(super) fn to_map(&self) -> HashMap<String, Vec<QueuedRecord>> {
        self.queues.clone()
    }
}

#[cfg(test)]
mod tests {
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
}

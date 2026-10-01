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
mod tests;

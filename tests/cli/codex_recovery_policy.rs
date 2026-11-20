use super::*;

fn record(id: usize) -> Queued {
    Queued {
        owner: Some("thread".into()),
        text: String::new(),
        id: Some(id.to_string()),
        replay: true,
        ask_id: None,
    }
}

#[test]
fn parked_recovery_bounds_bodies_and_preserves_evicted_ids() {
    let mut delivery = CodexDelivery::default();
    delivery.bind("thread");
    for id in 0..33 {
        delivery.park_replay(record(id), false);
    }
    delivery.park_replay(record(33), true);
    assert_eq!(delivery.rechecks.len(), 32);
    assert_eq!(delivery.rechecks[0].record.id.as_deref(), Some("1"));
    assert!(delivery.holds(Some("thread"), "0"));
    assert!(delivery.holds(Some("thread"), "33"));
    assert!(delivery.bind("next"));
    assert!(delivery.rechecks.is_empty());
    assert!(!delivery.holds(Some("next"), "0"));
}

#[test]
fn parked_recheck_delay_doubles_to_ten_minutes() {
    let now = Instant::now();
    let mut item = ParkedReplay {
        record: record(1),
        scan: ReceiptScan::default(),
        retry_at: now,
        delay: Duration::from_secs(30),
    };
    for seconds in [60, 120, 240, 480, 600, 600] {
        item.back_off(now);
        assert_eq!(
            item.retry_at.duration_since(now),
            Duration::from_secs(seconds)
        );
    }
}

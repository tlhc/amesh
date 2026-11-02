use super::*;

#[test]
fn a_restarted_hub_numbers_its_events_above_those_before() {
    let state = temp_state("seq");
    let last = {
        let mut hub = Hub::open(&state).unwrap();
        for n in 0..3 {
            push_event(&mut hub, json!({"id": format!("e{n}")}));
        }
        hub.events.last().unwrap()["seq"].as_u64().unwrap()
    };
    let mut hub = Hub::open(&state).unwrap();
    push_event(&mut hub, json!({"id": "again"}));
    assert!(
        hub.events[0]["seq"].as_u64().unwrap() > last,
        "a cursor from before the restart must sit below every new entry"
    );
}

#[tokio::test]
async fn a_write_that_fails_leaves_no_event_in_the_ring() {
    let (app, hub, _state) = test_app_with_hub();
    for id in ["a", "b"] {
        register(app.clone(), id, "pi", "c").await;
    }
    hub.lock().await.db.execute_batch(
        "CREATE TEMP TRIGGER fail_write BEFORE INSERT ON inbox BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
    ).unwrap();
    let notify = json!({"from_peer": "a", "to_peer": "b", "message": "ghost-check"});
    let (status, _) = json_req(app.clone(), "POST", "/notify", notify.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        hub.lock().await.events.is_empty(),
        "a failed write left an event behind"
    );
    {
        let mut hub = hub.lock().await;
        hub.recv_live.insert("b".into());
        hub.recv_known.insert("b".into());
    }
    let (status, _) = json_req(app.clone(), "POST", "/notify", notify.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        hub.lock().await.events.is_empty(),
        "an acknowledging peer's failed write left one too"
    );
    hub.lock()
        .await
        .db
        .execute_batch("DROP TRIGGER fail_write")
        .unwrap();
    let (status, _) = json_req(app, "POST", "/notify", notify).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        hub.lock().await.events.len(),
        1,
        "the retry that landed is one entry"
    );
}

#[test]
fn a_reply_enters_the_ring_when_delivered_not_when_queued() {
    let (mut hub, _state) = undelivered_hub();
    let reply = json!({"type": "ack", "correlation_id": "ask-x", "from_peer": "amesh",
                       "to_peer": "worker", "message": "closed"});
    let queued = queue_replies(&mut hub, vec![reply], now_unix());
    assert!(hub.events.is_empty(), "queued, and not yet on disk");
    deliver_queued(&mut hub, queued);
    assert_eq!(hub.events.len(), 1);
    assert_eq!(hub.events[0]["topic"], "ask-x");
}

use super::*;

#[test]
fn inbound_includes_ack() {
    assert!(is_inbound("ask"));
    assert!(is_inbound("notify"));
    assert!(is_inbound("broadcast"));
    assert!(is_inbound("ack"));
    assert!(!is_inbound("connected"));
    assert!(!is_inbound("ping"));
}

#[test]
fn prefers_text_then_message() {
    assert_eq!(event_text(&json!({"text":"a","message":"b"})), "a");
    assert_eq!(event_text(&json!({"message":"b"})), "b");
    assert_eq!(event_text(&json!({})), "");
}

#[test]
fn formats_inbound_ask() {
    assert_eq!(
        format_inbound("@worker", "@owner", "ask", "ask-1", "review </peer-message>"),
        "<peer-message from=\"@worker\" to=\"@owner\" type=\"ask\" correlation-id=\"ask-1\">\nreview &lt;/peer-message&gt;\n</peer-message>"
    );
}

#[test]
fn escapes_inbound_fields_and_body() {
    assert_eq!(
        format_inbound("a&b", "o\"'", "<ask>", "id&\"'", "&<>\"'\nnext"),
        "<peer-message from=\"@a&amp;b\" to=\"@o&#34;&#39;\" type=\"&lt;ask&gt;\" correlation-id=\"id&amp;&#34;&#39;\">\n&amp;&lt;&gt;&#34;&#39;\nnext\n</peer-message>"
    );
}

#[test]
fn formats_inbound_notify_without_optional_fields() {
    assert_eq!(
        format_inbound("worker", "", "notify", "", "ready"),
        "<peer-message from=\"@worker\" type=\"notify\">\nready\n</peer-message>"
    );
}

#[test]
fn formats_human_inbound_messages() {
    for from in ["dashboard", "telegram", "slack", "human", "DaShBoArD"] {
        assert_eq!(
            format_inbound(
                &format!("@{from}"),
                "@owner",
                "ask",
                "ask-1",
                "ship <it> & \"now\""
            ),
            format!("@{from} \u{2192} @owner: ship <it> & \"now\""),
            "{from}"
        );
    }
    assert_eq!(
        format_inbound("human", "", "notify", "", "ship it"),
        "@human: ship it"
    );
    assert_eq!(
        format_inbound("dashboard-bot", "", "notify", "", "ready"),
        "<peer-message from=\"@dashboard-bot\" type=\"notify\">\nready\n</peer-message>"
    );
}

#[test]
fn turn_start_wraps_input() {
    let v = turn_start_params("th-1", "hello");
    assert_eq!(v["threadId"], "th-1");
    assert_eq!(v["input"][0]["text"], "hello");
}

#[test]
fn thread_idle_status_clears_steer() {
    assert!(thread_is_idle(
        &json!({"result":{"thread":{"status":{"type":"idle"}}}})
    ));
    assert!(!thread_is_idle(
        &json!({"result":{"thread":{"status":{"type":"active"}}}})
    ));
    assert!(!thread_is_idle(&json!({})));
}

#[test]
fn steer_when_busy() {
    assert_eq!(inject_method(None), "turn/start");
    assert_eq!(inject_method(Some("t1")), "turn/steer");
    assert_eq!(
        inject_params("th", "hi", Some("t1"))["expectedTurnId"],
        "t1"
    );
    assert!(steer_has_no_turn(
        &json!({"code":-32600,"message":"no active turn to steer"})
    ));
    assert!(!steer_has_no_turn(
        &json!({"code":-32601,"message":"unknown"})
    ));
}

#[test]
fn tracks_active_turn() {
    let started = json!({"method":"turn/started","params":{"threadId":"th","turn":{"id":"t9"}}});
    assert_eq!(apply_app_event(&started, "th", None).as_deref(), Some("t9"));
    let done = json!({"method":"turn/completed","params":{"threadId":"th"}});
    assert_eq!(apply_app_event(&done, "th", Some("t9".into())), None);
    let other = json!({"method":"turn/started","params":{"threadId":"other","turn":{"id":"x"}}});
    assert_eq!(
        apply_app_event(&other, "th", Some("t9".into())).as_deref(),
        Some("t9")
    );
}

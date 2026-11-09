use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn is_inbound(kind: &str) -> bool {
    matches!(kind, "ask" | "notify" | "broadcast" | "ack")
}

pub fn format_inbound(
    from: &str,
    to: &str,
    kind: &str,
    correlation_id: &str,
    text: &str,
) -> String {
    let from = from.strip_prefix('@').unwrap_or(from);
    let to = to.strip_prefix('@').unwrap_or(to);
    if matches!(
        from.strip_prefix('@')
            .unwrap_or(from)
            .to_lowercase()
            .as_str(),
        "dashboard" | "telegram" | "slack" | "human"
    ) {
        let to_label = if to.is_empty() {
            String::new()
        } else {
            format!(" \u{2192} @{to}")
        };
        return format!("@{from}{to_label}: {text}");
    }
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('\'', "&#39;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&#34;")
    };
    let mut attrs = format!(" from=\"@{}\"", escape(from));
    if !to.is_empty() {
        attrs.push_str(&format!(" to=\"@{}\"", escape(to)));
    }
    attrs.push_str(&format!(" type=\"{}\"", escape(kind)));
    if !correlation_id.is_empty() {
        attrs.push_str(&format!(" correlation-id=\"{}\"", escape(correlation_id)));
    }
    format!("<peer-message{attrs}>\n{}\n</peer-message>", escape(text))
}

pub fn turn_start_params(thread_id: &str, text: &str) -> Value {
    json!({
        "threadId": thread_id,
        "input": [{"type": "text", "text": text}]
    })
}

pub fn inject_method(active_turn: Option<&str>) -> &'static str {
    if active_turn.is_some() {
        "turn/steer"
    } else {
        "turn/start"
    }
}

pub fn thread_is_idle(read: &Value) -> bool {
    read.pointer("/result/thread/status/type")
        .or_else(|| read.pointer("/result/status/type"))
        .and_then(Value::as_str)
        == Some("idle")
}

/* no turn runs on the thread: idle, or systemError after a turn the server failed (Codex
runs no Stop for either); any other or unknown status is left alone */
pub fn turn_is_over(read: &Value) -> bool {
    thread_is_idle(read)
        || read
            .pointer("/result/thread/status/type")
            .or_else(|| read.pointer("/result/status/type"))
            .and_then(Value::as_str)
            == Some("systemError")
}

pub fn inject_params(thread_id: &str, text: &str, active_turn: Option<&str>) -> Value {
    let mut params = turn_start_params(thread_id, text);
    if let Some(id) = active_turn {
        params["expectedTurnId"] = json!(id);
    }
    params
}

pub fn app_server_socket() -> PathBuf {
    let root = std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".codex"));
    root.join("app-server-control")
        .join("app-server-control.sock")
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    let key = format!("--{name}");
    args.windows(2)
        .find_map(|w| (w[0] == key).then(|| w[1].clone()))
        .or_else(|| {
            args.iter()
                .find_map(|a| a.strip_prefix(&format!("{key}=")).map(str::to_string))
        })
}

pub async fn run(args: &[String]) -> Result<()> {
    let peer_id = parse_flag(args, "peer-id").ok_or("bridge requires --peer-id")?;
    let thread_id = parse_flag(args, "thread-id").ok_or("bridge requires --thread-id")?;
    let name = parse_flag(args, "name").unwrap_or_else(|| peer_id.clone());
    let path = parse_flag(args, "path")
        .unwrap_or_else(|| std::env::current_dir().unwrap().display().to_string());
    let circle =
        parse_flag(args, "circle").unwrap_or_else(|| crate::cli::project_circle(Path::new(&path)));
    /* the thread the bridge serves is the peer's session, so the drainer loop it hands off
    to trusts it as the owner and injects there */
    let registration = json!({
        "peer_id": peer_id, "name": name, "path": path, "backend": "codex",
        "circle": circle, "session_id": thread_id
    });
    crate::cli::hook_ws_loop_with(peer_id, "codex".to_string(), Some(registration)).await
}

#[cfg(test)]
mod tests;

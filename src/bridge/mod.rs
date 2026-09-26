use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::UnixStream;
use tokio::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, connect_async};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn is_inbound(kind: &str) -> bool {
    matches!(kind, "ask" | "notify" | "broadcast" | "ack")
}

pub fn event_text(frame: &Value) -> String {
    frame
        .get("text")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| frame.get("message").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
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

pub fn steer_has_no_turn(error: &Value) -> bool {
    error.to_string().contains("no active turn")
}

pub fn inject_params(thread_id: &str, text: &str, active_turn: Option<&str>) -> Value {
    let mut params = turn_start_params(thread_id, text);
    if let Some(id) = active_turn {
        params["expectedTurnId"] = json!(id);
    }
    params
}

pub fn apply_app_event(msg: &Value, thread_id: &str, active: Option<String>) -> Option<String> {
    let tid = msg
        .pointer("/params/threadId")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !tid.is_empty() && tid != thread_id {
        return active;
    }
    match msg.get("method").and_then(Value::as_str).unwrap_or("") {
        "turn/started" => msg
            .pointer("/params/turn/id")
            .and_then(Value::as_str)
            .map(str::to_string),
        "turn/completed" | "turn/failed" => None,
        _ => active,
    }
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

fn bind() -> String {
    std::env::var("AMESH_BIND").unwrap_or_else(|_| "127.0.0.1:8378".into())
}

fn http_json(method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let mut curl = Command::new("curl");
    curl.args([
        "--disable",
        "--silent",
        "--show-error",
        "--fail-with-body",
        "--noproxy",
        "*",
        "--connect-timeout",
        "2",
        "--max-time",
        "5",
        "--request",
        method,
        "--header",
        "Content-Type: application/json",
    ]);
    if let Ok(token) = std::env::var("AMESH_TOKEN") {
        if !token.is_empty() {
            curl.args(["--header", &format!("Authorization: Bearer {token}")]);
        }
    }
    if body.is_some() {
        curl.args(["--data-binary", "@-"]);
    }
    curl.arg(format!("http://{}{path}", bind()));
    let mut child = curl
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(body) = body {
        use std::io::Write;
        child
            .stdin
            .take()
            .ok_or("curl stdin")?
            .write_all(body.to_string().as_bytes())?;
    } else {
        drop(child.stdin.take());
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(format!(
            "{} {path}: {}",
            method,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&out.stdout)?)
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
    http_json(
        "POST",
        "/peers",
        Some(&json!({
            "peer_id": peer_id, "name": name, "path": path, "backend": "codex", "circle": circle
        })),
    )?;
    let ws_url = format!("ws://{}/ws", bind());
    let (mut mesh, _) = connect_async(&ws_url).await?;
    let mut connect = json!({"type":"connect","peer_id":peer_id,"name":name});
    if let Ok(token) = std::env::var("AMESH_TOKEN") {
        if !token.is_empty() {
            connect["auth_token"] = json!(token);
        }
    }
    mesh.send(Message::Text(connect.to_string().into())).await?;
    let sock = app_server_socket();
    let stream = UnixStream::connect(&sock)
        .await
        .map_err(|e| format!("{}: {e}", sock.display()))?;
    let (mut app, _) = client_async("ws://localhost/", stream).await?;
    let mut rpc_id = 1u64;
    let mut pending = HashMap::new();
    let init = json!({
        "id": rpc_id,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "amesh", "title": "amesh", "version": "0.1.0"},
            "capabilities": {"experimentalApi": true}
        }
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    pending.insert(rpc_id, ("initialize", deadline));
    tokio::time::timeout_at(deadline, app.send(Message::Text(init.to_string().into()))).await??;
    rpc_id += 1;
    let mut active_turn: Option<String> = None;
    let mut last_text = String::new();
    loop {
        let deadline = pending.values().map(|(_, deadline)| *deadline).min();
        tokio::select! {
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                let (id, (method, _)) = pending.iter().min_by_key(|(_, (_, deadline))| *deadline).unwrap();
                return Err(format!("App Server {method} (id {id}) timed out after 15s").into());
            }
            frame = mesh.next(), if pending.is_empty() => {
                let Some(Ok(Message::Text(raw))) = frame else { break; };
                let event: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
                let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
                if !is_inbound(kind) {
                    continue;
                }
                let text = event_text(&event);
                if text.is_empty() {
                    eprintln!("amesh bridge: empty delivery");
                    continue;
                }
                let text = format_inbound(
                    event.get("from_peer").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("unknown"),
                    event.get("to_peer").and_then(Value::as_str).unwrap_or(""),
                    kind,
                    event.get("correlation_id").and_then(Value::as_str).unwrap_or(""),
                    &text,
                );
                last_text = text.clone();
                let method = inject_method(active_turn.as_deref());
                let req = json!({
                    "id": rpc_id,
                    "method": method,
                    "params": inject_params(&thread_id, &text, active_turn.as_deref())
                });
                let deadline = Instant::now() + Duration::from_secs(15);
                pending.insert(rpc_id, (method, deadline));
                rpc_id += 1;
                tokio::time::timeout_at(deadline, app.send(Message::Text(req.to_string().into()))).await??;
            }
            app_frame = app.next() => {
                let raw = match app_frame {
                    Some(Ok(Message::Text(raw))) => raw,
                    Some(Ok(Message::Close(_))) | None => return Err("App Server connection closed".into()),
                    Some(Err(error)) => return Err(error.into()),
                    Some(Ok(_)) => continue,
                };
                if let Ok(msg) = serde_json::from_str::<Value>(&raw) {
                    if msg.get("result").is_some() || msg.get("error").is_some() {
                        if let Some((id, (method, _))) = msg.get("id").and_then(Value::as_u64)
                            .and_then(|id| pending.remove(&id).map(|request| (id, request)))
                        {
                            if let Some(error) = msg.get("error").filter(|error| !error.is_null()) {
                                if method == "turn/steer" && steer_has_no_turn(error) {
                                    active_turn = None;
                                    let req = json!({
                                        "id": rpc_id,
                                        "method": "turn/start",
                                        "params": inject_params(&thread_id, &last_text, None)
                                    });
                                    let deadline = Instant::now() + Duration::from_secs(15);
                                    pending.insert(rpc_id, ("turn/start", deadline));
                                    rpc_id += 1;
                                    tokio::time::timeout_at(deadline, app.send(Message::Text(req.to_string().into()))).await??;
                                    continue;
                                }
                                return Err(format!("App Server {method} (id {id}): {error}").into());
                            }
                            if method == "initialize" {
                                app.send(Message::Text(json!({"method":"initialized","params":{}}).to_string().into())).await?;
                            } else if method == "turn/start" && msg.pointer("/result/turn/status").and_then(Value::as_str) == Some("inProgress") {
                                active_turn = msg.pointer("/result/turn/id").and_then(Value::as_str)
                                    .filter(|id| !id.is_empty()).map(str::to_string);
                            }
                        }
                    }
                    active_turn = apply_app_event(&msg, &thread_id, active_turn);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

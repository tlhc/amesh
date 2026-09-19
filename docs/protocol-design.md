# Protocol (v0.1)

One-page snapshot of the hub in `src/hub/mod.rs`. CLI flags: `amesh --help`. MCP tool list: README.

This is the v0.1-stable surface. Jobs, schedules, attachments, per-peer MCP registry, timeline/transcript may change.

Request bodies name the session field `session_id`.

## Auth

If `AMESH_TOKEN` is set at `serve` start, HTTP needs `Authorization: Bearer <token>` (`check_auth`). Mismatch is 401. Empty token: no check.

`GET /health` is always open: `{"ok":true,"name":"amesh","version":"0.1.0"}`.

## Roster

`POST /peers` registers (alias `POST /peer/register`); `peer_id`, `name` and `circle` must be 1-128 characters of `[A-Za-z0-9._-]`, anything else is 400. An omitted `name` keeps the stored one, else defaults to `peer_id`. `GET /peers` probes sockets, then drops peers with no live WebSocket and `last_seen` older than 30s (`PEER_ONLINE_SECS`), and may persist.

## Ask

`POST /ask` → `{"ok":true,"correlation_id":"ask-…"}`. Cross-circle needs `cross_circle: true` (`require_cross_circle`).

`GET /asks/pending?peer_id=` returns open asks for that peer. If an acknowledging WebSocket is attached, inbox is empty here; only WS `recv` retires those copies.

`POST /asks/{id}/wait` waits on that id.

`POST /ack` with `correlation_id` closes the ask. Omitting `message` still closes. Hub does not check that the caller is the recipient; the primer tells agents to ack only their own. A second ack with a different message is 409.

## Notify and broadcast

`POST /notify` is fire-and-forget. Same `cross_circle` rule.

`POST /broadcast` goes to every other peer in the sender's circle. A different `circle` needs `cross_circle: true`. Do not ack a broadcast.

## Events

`GET /events` is the in-memory ring (last 500, cleared on restart). `?since=<id>` starts after that id. `?circle=NAME` keeps events whose `from_circle` or `to_circle` matches.

## WebSocket

`GET /ws`. First text frame must be `{"type":"connect","peer_id":"…"}` (or `name`). Optional `auth_token`, `recv`. Hub replies `{"type":"connected","peer_id":"…","name":"…"}`.

If `recv` is true, the peer promises `{"type":"recv","id":"…"}` for each event with an id. Hub drops the durable inbox row only on that recv (`acknowledge_event`). The CLI `amesh hook ws` keeps a process-local FIFO as well; restarting that process drops the FIFO.

## MCP

`POST /mcp` is JSON-RPC (`initialize`, `tools/list`, `tools/call`). `amesh mcp` is a stdio shim onto that HTTP route (`src/cli/mod.rs` `mcp()`).

## Aliases

Same handlers, no extra semantics:

| path | same as |
|------|---------|
| `POST /answer` | `POST /ack` |
| `GET /deliveries/pending` | `GET /asks/pending` |
| `POST /questions/ask-blocking` | blocking ask |
| `POST /sessions/resume` | `session_id` in body, then `session_notify` |
| `POST /sessions/{id}/controls/notify` | `session_notify` |
| `POST /sessions/{id}/controls/resume` | `session_notify` |

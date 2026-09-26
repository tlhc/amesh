# Protocol (v0.1)

One-page snapshot of the hub in `src/hub/mod.rs`. CLI flags: `amesh --help`. MCP tool list: README.

- This is the v0.1-stable surface. Jobs, schedules, attachments, per-peer MCP registry, and timeline/transcript may change.
- Request bodies name the session field `session_id`.

## Auth

- If `AMESH_TOKEN` is set at `serve` start, HTTP needs `Authorization: Bearer <token>` (`check_auth`):
  - a mismatch is 401
  - an empty token means no check
- `GET /health` is always open: `{"ok":true,"name":"amesh","version":"0.1.0"}`.

## Roster

- `POST /peers` registers (alias `POST /peer/register`):
  - `peer_id`, `name` and `circle` must be 1-128 characters of `[A-Za-z0-9._-]`; anything else is 400
  - an omitted `name` keeps the stored one, else defaults to `peer_id`
- `GET /peers` probes sockets, then drops peers with no live WebSocket and `last_seen` older than 30s (`PEER_ONLINE_SECS`), and may persist.

## Ask

- `POST /ask` → `{"ok":true,"correlation_id":"ask-…"}`. Cross-circle needs `cross_circle: true` (`require_cross_circle`).
- `GET /asks/pending?peer_id=` returns open asks for that peer. With an acknowledging WebSocket attached, the inbox is empty here; only WS `recv` retires those copies.
- `POST /asks/{id}/wait` waits on that id.
- `POST /ack` with `correlation_id` closes the ask:
  - omitting `message` still closes
  - a body that names `from_peer` must name the recipient, otherwise 403; MCP always names the caller
  - an ack that names nobody is accepted, for operators
  - `failed: true` marks the ask failed, and the asker's copy reads `[failed] <message>`
  - a second ack whose message or `failed` differs from the first is 409

## Jobs

- `POST /jobs` with `assigned_peer` creates a dispatched job:
  - the assignee must resolve (404) and share the job's circle (403)
  - every id in `depends_on` must exist in the same circle (400)
- Once a second the hub:
  - settles running jobs whose ask closed: `done`, or `failed` when the ask is failed
  - sends each queued job whose dependencies are all `done` as an ask from its creator
- That ask quotes each dependency's `result_summary`, up to `job_upstream_chars` (default 2000), after the job's own text:
  - every line starts with `| `
  - any other line break (`\r`, `\v`, `\f`, NEL, U+2028, U+2029) starts a quoted line too
  - control characters are dropped
- `PATCH /jobs/{id}`:
  - first settles a running job whose ask already closed, then applies the change
  - moving a running job elsewhere closes its still-open ask
  - `assigned_peer` reassigns and `prompt` rewrites the next attempt; both are refused (409) while the new state is `running`
  - a dispatched job enters `running` only by being sent (409 otherwise)
- `DELETE /jobs/{id}` is 409 while a queued or running job depends on it.
- Jobs record `created_at` and asks `opened_at` and `closed_by` (`recipient`, `hand` or `hub`: who closed it); rows written before these fields existed leave them empty. A binary from before them rewrites the tables without them, so after a downgrade the times read as empty again.
- MCP job tools that name a caller act only on jobs in the caller's circle unless `cross_circle` is set.
- Cleanup, with both periods set in `config.toml` (default one hour):
  - a chain of jobs whose jobs all ended at least `job_keep_secs` ago is deleted
  - a closed ask is deleted once it has been closed `ask_keep_secs` and no job refers to it
  - copies of closed or deleted asks are never delivered
- `POST /gc`: `{"apply": false}` lists what would go; `{"apply": true}` cleans up now.

## Notify and broadcast

- `POST /notify` is fire-and-forget. Same `cross_circle` rule.
- `POST /broadcast` goes to every other peer in the sender's circle:
  - a different `circle` needs `cross_circle: true`
  - do not ack a broadcast

## Events

- `GET /events` is the in-memory ring (last 500, cleared on restart):
  - `?since=<id>` starts after that id
  - `?circle=NAME` keeps events whose `from_circle` or `to_circle` matches

## Snapshot

- `GET /snapshot` is the read-only view for monitors such as `amesh tui`: the jobs, the asks they point at and the peers they name, copied under one lock.
  - `?circle=NAME` filters the jobs; `?detail=JOB_ID` also returns that job's title, prompt and result in full.
  - Text fields, titles included, are cut to 400 characters, with `*_len` giving the full length; references that no longer resolve are listed under `missing`.
  - An ask's `to_peer_id` is matched by peer id only, so a recipient that left is listed under `missing` even when another peer has taken its name since. Assignees and senders are names, resolved the way the hub resolves them next.
  - Each peer carries its `activity` (below), or null while unknown, and `running`: its running jobs in every circle, counted by recipient (a job run by hand, which has no ask, by its assignee), so a filtered view can tell whether a job is its only one.
  - It never probes peers, settles jobs, drains inboxes, writes the state file or records an event.
  - `hub_epoch` changes when the hub restarts; `capabilities` says which optional fields are filled.

## Activity

- A runtime reports what it is doing: `work`, `idle`, or `wait` (a permission prompt). The hub keeps it in memory only; after a restart every peer is unknown until it reports again.
- `POST /activity` takes `{"session_id", "state", "source"?, "reason"?}` and finds the peer by its session; a `peer_id` names the peer directly.
  - A report from a session that is not the peer's current one gets 409; an unknown peer 404; any other state 400.
  - A repeated state keeps its `since`; `reason` and `source` are cut to 120 and 32 characters and lose control characters.
  - `"check": true` marks a runtime's own look at its state; it yields to another state reported in the last 15 seconds, since a turn that has just reported work may not have started yet.
- `POST /peers` may carry `"activity": {"state", "source"?, "reason"?}`, applied with the registration.
- Activity is cleared when a registration replaces the peer's session, when its socket closes, and when the peer is pruned. A registration's activity applies only once the registration is on disk.
- The hooks report:
  - Claude Code: `UserPromptSubmit` work; `Notification` `permission_prompt` wait with its message, `idle_prompt` idle; `PostToolUse` (async, so Claude never waits for it) work; `Stop` idle unless the hook blocks the stop.
  - Codex: `UserPromptSubmit` work; `Stop` idle unless the hook blocks it. A failed or interrupted turn runs no `Stop`, so the drainer reads its thread every 10 seconds and reports one that is `idle` or in `systemError` (a turn the server failed) as an idle check. The drainer also starts a turn for the next message on such a thread, as on an idle one.
  - pi: `before_agent_start` and `agent_start` work. `agent_settled` reports idle a second later if pi's context still says idle with nothing pending, looking again each second while it does not, then every ten seconds after thirty tries; a run that starts meanwhile cancels it. pi clears its run flag just before `agent_settled` and may start a queued prompt right after it, so the settle alone does not mean idle. `agent_end` reports nothing.

## WebSocket

- `GET /ws`. The first text frame must be `{"type":"connect","peer_id":"…"}` (or `name`), with optional `auth_token` and `recv`. The hub replies `{"type":"connected","peer_id":"…","name":"…"}`.
- If `recv` is true, the peer promises `{"type":"recv","id":"…"}` for each event with an id:
  - the hub drops the durable inbox row only on that recv (`acknowledge_event`)
  - the CLI `amesh hook ws` also keeps a process-local FIFO; restarting that process drops the FIFO

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

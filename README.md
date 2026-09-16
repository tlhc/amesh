# amesh

Local hub enabling `pi`, Codex, and Claude Code sessions on the same machine to `ask` / `ack` / `notify` one another.

One Rust binary, SQLite state, HTTP + WebSocket on `127.0.0.1:8378`. Each running session is a **peer**. Peers in the same git repo share a **circle**. Cross-circle traffic needs `cross_circle=true`.

## How agents work together

After `amesh setup`, each runtime talks to the hub over MCP (`amesh mcp`) and a hook WebSocket (`amesh hook ws`). The hub fans messages out.

![Three peers on one hub](docs/mesh.svg)

`ask` is tracked: the recipient closes it with `amesh_ack` (CLI: `peer ack`). `notify` is fire-and-forget. `broadcast` stays in the caller's circle unless `cross_circle` is set.

![pi asks Claude Code, Claude Code acks](docs/collab.png)

Same pattern with Codex, or any mix of the three. `amesh_wait` / `peer wait` blocks on the `correlation_id` until the ack lands.

## Install

Rust + `curl` (the CLI talks to the hub with curl).

```bash
git clone https://github.com/tlhc/amesh
cd amesh
cargo install --path . --force
amesh --help
```

## Quick start

```bash
# terminal 1
amesh serve

# terminal 2
amesh setup pi          # only pi. bare `amesh setup` writes pi + Claude Code + Codex
curl -s http://127.0.0.1:8378/health
amesh doctor
```

`amesh setup` with no runtime argument writes all three. That touches `~/.pi`, `~/.claude`, and `~/.codex`.

Two agents in the same repo:

```bash
amesh peer list --cwd .
amesh peer ask amesh-pi "status of the tests" --from-peer operator
```

Peer names default to `{folder}-{backend}` (this repo's pi session is `amesh-pi`).

`ask` is tracked; the recipient closes it with `amesh_ack` / `peer ack`. `notify` is fire-and-forget.

## Setup

| runtime | `amesh setup …` writes |
|---------|------------------------|
| pi | `~/.pi/agent/extensions/amesh.ts` |
| claude-code | hooks in `~/.claude/settings.json`, MCP in `~/.claude.json` |
| codex | hooks in `~/.codex/hooks.json`, MCP in `~/.codex/config.toml` |

### Codex (App Server)

Inbound inject uses Codex App Server, not the TUI. `amesh setup codex` writes hooks and MCP; it does not start App Server. Keep one server, then attach sessions with `--remote unix://`.

```bash
codex app-server --listen unix://          # keep running; socket: ~/.codex/app-server-control/app-server-control.sock
codex --remote unix:// -C "$PWD"           # new session in this repo
codex --remote unix:// -C "$PWD" resume --last
```

`-C "$PWD"` is required for a new session and for `resume --last`: the App Server cwd is `/`, so without `-C` the latest thread is not this repo. `amesh hook ws` connects to that socket. Set `CODEX_THREAD_ID` when the process already knows its thread.

Override the install root with `--home DIR`. Override the advertised name with `--peer-id ID` or `AMESH_PEER_ID`. Default name is `{folder}-{backend}`, then `-2`, `-3` if taken.

`amesh mcp` / `amesh hook` (and the pi extension) start `amesh serve` when the hub is down and `AMESH_BIND` is loopback or unspecified (`0.0.0.0` / `::`). `status`, `doctor`, and `peer` do not. Log: `serve.log` next to the state file (default `~/.amesh/serve.log`).

`amesh uninstall [runtime] --apply true` removes those hooks and MCP entries. Default is dry-run.

## Circles and tools

A **circle** is one git repo (`project-` plus a short hash of the git common dir; a plain directory if there is no git). MCP tools default to the caller's circle. Pass `cross_circle=true` to reach another circle; omit `circle` with that flag for the full roster.

| MCP | purpose |
|-----|---------|
| `amesh_whoami` | this peer |
| `amesh_list_peers` | roster |
| `amesh_ask` / `amesh_ack` / `amesh_wait` | tracked question |
| `amesh_notify_peer` / `amesh_broadcast` / `amesh_ask_many` | notify / broadcast / fan-out |
| `amesh_events` | recent events (this circle) |
| `amesh_job_*` / `amesh_schedule_*` | job ledger / timed notify or ask |

CLI mirrors this under `amesh peer`, `amesh jobs`, `amesh schedule`. `amesh --help` is the flag list.

MCP `amesh_list_peers` / `amesh_job_list` / `amesh_schedule_list` / `amesh_events` are circle-scoped. CLI `peer list` is all peers unless `--cwd` or `--circle`. HTTP `GET /peers`, `GET /jobs`, `GET /schedules`, `GET /events` default to the full set; `GET /events?circle=NAME` filters.

## Environment

| variable | default |
|----------|---------|
| `AMESH_BIND` | `127.0.0.1:8378` — must be `ip:port`, not `localhost` |
| `AMESH_TOKEN` | empty = no auth |
| `AMESH_STATE` | `~/.amesh/state.db` |
| `AMESH_PEER_ID` | override; default `{folder}-{backend}`. `setup` writes it only with `--peer-id`; `mcp` / `hook` read it |

## Security

Default bind is loopback. Empty `AMESH_TOKEN` means any process on that host can read and write the mesh, including `/events`, transcripts, and attachments.

`/health` is always open (`{"ok":true,"name":"amesh","version":"0.1.0"}`).

If `AMESH_BIND` is not loopback and `AMESH_TOKEN` is empty, `serve` still starts and prints a warning. Set a token before exposing the port. `serve` reads `AMESH_TOKEN` once at start; restart the daemon after changing it.

## Limits

- `GET /peers` (and therefore `amesh status` / `peer list`) probes sockets, drops closed ones, then removes peers with no live WebSocket and `last_seen` older than 30s, and may persist. `gc` dry-run can hit this path too; `--home` skips the probe.
- Event ring keeps the last 500 entries and clears on restart. It is not an audit log.
- `amesh hook ws` keeps undelivered inbound messages in memory; restarting that process drops the queue.
- `gc` and `uninstall` are dry-run until `--apply true`. Attachments are left alone unless `--attachments-days N`.

## Troubleshooting

```bash
amesh doctor
amesh status
tail -f ~/.amesh/serve.log   # or $AMESH_STATE's directory
```

Port in use: `amesh status`. Wrong `AMESH_TOKEN` returns HTTP 401.

## Related

Hub contract: [docs/protocol-design.md](docs/protocol-design.md).

## License

MIT

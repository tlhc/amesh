import { execFile, spawn } from "node:child_process";
import { closeSync, fstatSync, ftruncateSync, mkdirSync, openSync } from "node:fs";
import http from "node:http";
import net from "node:net";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";

const executable = __AMESH_EXECUTABLE__;
const peerId = __AMESH_PEER_ID__;

function EscapeAttr(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function EscapeBody(value) {
  return String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;");
}

export function ExecError(error, stdout, stderr, timeoutMs) {
  const detail = String(stderr || stdout || "").trim();
  let reason = "";
  if (error.killed) {
    reason = ` (killed by ${error.signal || "timeout"} after ${timeoutMs}ms)`;
  } else if (error.signal) {
    reason = ` (terminated by ${error.signal})`;
  } else if (error.code !== undefined && error.code !== null) {
    reason = ` (exit ${error.code})`;
  }
  return new Error((detail || error.message) + reason);
}

function BindParts() {
  const bind = process.env.AMESH_BIND || "127.0.0.1:8378";
  if (bind.startsWith("[")) {
    const end = bind.indexOf("]");
    return { bind, host: bind.slice(1, end), port: Number(bind.slice(end + 2)) };
  }
  const i = bind.lastIndexOf(":");
  return { bind, host: bind.slice(0, i), port: Number(bind.slice(i + 1)) };
}

function StateFile() {
  const state = process.env.AMESH_STATE;
  let file;
  if (state) {
    file = isAbsolute(state) ? state : resolve(process.cwd(), state);
    if (file.endsWith(".json")) {
      file = `${file.slice(0, -5)}.db`;
    }
  } else {
    file = join(process.env.HOME || homedir(), ".amesh", "state.db");
  }
  return file;
}

function StateDir() {
  return dirname(StateFile());
}

function LocalBind(host) {
  return host === "127.0.0.1" || host === "::1" || host === "0.0.0.0" || host === "::";
}

function ProbeHealth(ms) {
  const { host, port } = BindParts();
  const cap = Math.min(300, Math.max(1, ms));
  return new Promise((resolveHealth) => {
    let done = false;
    const finish = (value) => {
      if (done) {
        return;
      }
      done = true;
      resolveHealth(value);
    };
    const req = http.get({ host, port, path: "/health" }, (res) => {
      let body = "";
      res.on("data", (chunk) => {
        body += chunk;
        if (body.length > 4096) {
          req.destroy();
          finish(false);
        }
      });
      res.on("end", () => {
        try {
          const json = JSON.parse(body);
          finish(json.ok === true && json.name === "amesh");
        } catch {
          finish(false);
        }
      });
    });
    req.on("error", () => finish(false));
    const timer = setTimeout(() => {
      req.destroy();
      finish(false);
    }, cap);
    timer.unref?.();
  });
}

function PortOpen() {
  const { host, port } = BindParts();
  return new Promise((resolve) => {
    const socket = net.connect({ host, port }, () => {
      socket.end();
      resolve(true);
    });
    socket.setTimeout(200, () => {
      socket.destroy();
      resolve(false);
    });
    socket.on("error", () => resolve(false));
  });
}

async function Ensure() {
  const deadline = Date.now() + 2000;
  const left = () => Math.max(0, deadline - Date.now());
  if (await ProbeHealth(left())) {
    return;
  }
  if (left() === 0) {
    return;
  }
  const { host } = BindParts();
  if (!LocalBind(host) || (await PortOpen())) {
    return;
  }
  const file = StateFile();
  const dir = StateDir();
  let log;
  try {
    mkdirSync(dir, { recursive: true });
    log = openSync(join(dir, "serve.log"), "a");
    if (fstatSync(log).size > 1_000_000) {
      ftruncateSync(log, 0);
    }
    const child = spawn(executable, ["serve"], {
      detached: true,
      stdio: ["ignore", log, log],
      cwd: dir,
      env: { ...process.env, AMESH_STATE: file },
    });
    child.on("error", () => {});
    child.unref();
  } catch {
    return;
  } finally {
    if (log !== undefined) {
      try {
        closeSync(log);
      } catch {
        /* already closed */
      }
    }
  }
  while (left() > 0) {
    if (await ProbeHealth(left())) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, Math.min(50, left())));
  }
}

export default function AmeshHooks(pi) {
  let socket;
  let intro = false;
  let selfId = peerId;
  let dead = false;
  let inflight = false;
  let warned = false;
  let localSession;
  let inboundSessionId;
  const pending = [];

  function Cli(args) {
    return new Promise((resolve, reject) => {
      execFile(executable, args, { timeout: 15000 }, (error, stdout, stderr) => {
        if (error) {
          reject(ExecError(error, stdout, stderr, 15000));
          return;
        }
        resolve(String(stdout).trim());
      });
    });
  }

  function Mcp(name, params) {
    return new Promise((resolve, reject) => {
      const args = ["mcp"];
      if (selfId) args.push("--peer-id", selfId);
      const timeout = name === "wait" ? 65000 : 15000;
      const child = execFile(executable, args, { timeout }, (error, stdout, stderr) => {
        if (error) {
          reject(ExecError(error, stdout, stderr, timeout));
          return;
        }
        try {
          const reply = JSON.parse(String(stdout).trim().split("\n").pop());
          if (reply.error) {
            reject(new Error(reply.error.message || JSON.stringify(reply.error)));
            return;
          }
          resolve(reply.result?.content?.[0]?.text ?? "");
        } catch (error) {
          reject(error);
        }
      });
      child.stdin.end(
        `${JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "tools/call",
          params: { name: `amesh_${name}`, arguments: params || {} },
        })}\n`,
      );
    });
  }

  function FromPeer() {
    return selfId ? ["--from-peer", selfId] : [];
  }

  function UseSession(ctx) {
    const session = ctx.sessionManager.getSessionId();
    if (localSession !== session) {
      KeepSession(session);
      localSession = session;
    }
    return session;
  }

  async function Invoke(event, ctx) {
    const session = UseSession(ctx);
    const payload = JSON.stringify({ session_id: session, cwd: ctx.cwd });
    return new Promise((resolve, reject) => {
      const args = ["hook", event, "--backend=pi"];
      if (peerId) args.push("--peer-id", peerId);
      const child = execFile(executable, args, { timeout: 12000 }, (error, stdout) => {
        if (error) { reject(error); return; }
        try { resolve({ ...JSON.parse(stdout), session_id: session }); } catch (error) { reject(error); }
      });
      child.stdin.end(payload);
    });
  }

  function Drop() {
    dead = true;
    inflight = false;
    warned = false;
    pending.length = 0;
    clearTimeout(retry);
    retry = null;
    clearInterval(beat);
    beat = null;
    if (socket) {
      socket.close();
      socket = null;
    }
  }

  function Take(items) {
    const batch = [];
    let size = 0;
    const limit = 256 * 1024;
    while (items.length) {
      const next = items[0];
      const n = Buffer.byteLength(next.text, "utf8");
      if (batch.length && size + n > limit) {
        break;
      }
      batch.push(items.shift());
      size += n;
    }
    return batch;
  }

  function Flush() {
    if (dead || !intro || inflight || pending.length === 0 || inboundSessionId === "") {
      return;
    }
    const steers = [];
    const follows = [];
    const held = pending.findIndex((item) => item.session && localSession && item.session !== localSession);
    if (held === 0) return;
    for (const item of pending.splice(0, held < 0 ? pending.length : held)) {
      if (item.type === "broadcast") {
        follows.push(item);
      } else {
        steers.push(item);
      }
    }
    if (steers.length) {
      const batch = Take(steers);
      pending.unshift(...follows, ...steers);
      if (pending.length <= 200) {
        warned = false;
      }
      Start(batch, "steer");
      return;
    }
    const batch = Take(follows);
    pending.unshift(...follows);
    if (pending.length <= 200) {
      warned = false;
    }
    Start(batch, "broadcast");
  }

  function Start(batch, type) {
    inflight = true;
    try {
      pi.sendUserMessage(batch.map((item) => item.text).join("\n\n"), { deliverAs: type === "broadcast" ? "followUp" : "steer" });
    } catch (error) {
      inflight = false;
      pending.unshift(...batch);
      console.error(`[amesh] ${error.message}`);
    }
  }

  function Inject(text, type, session) {
    if (dead) {
      return false;
    }
    pending.push({ text, type, session });
    if (pending.length > 200) {
      if (!warned) {
        console.error("[amesh] inbound queue depth " + pending.length);
        warned = true;
      }
    } else {
      warned = false;
    }
    return true;
  }

  function KeepSession(session) {
    for (let i = pending.length - 1; i >= 0; i--) {
      if (!session || pending[i].session !== session) pending.splice(i, 1);
    }
    if (pending.length <= 200) warned = false;
  }

  function Receive(body, session = inboundSessionId) {
    if (["connected", "replaced", "bound"].includes(body?.type)) {
      const next = typeof body.session_id === "string" ? body.session_id : undefined;
      inboundSessionId = next;
      if (body.type === "replaced") {
        KeepSession(next);
      } else if (next) {
        for (const item of pending) {
          if (!item.session) item.session = next;
        }
        KeepSession(next);
      }
      Flush();
      return false;
    }
    if (!body || !["ask", "notify", "broadcast", "ack"].includes(body.type)) {
      return false;
    }
    if (session && localSession && session !== localSession) return true;
    const text = body.text || body.message || "";
    return Inject(FormatPeer(body.from_peer || "unknown", body.type, text, body.correlation_id), body.type, session);
  }

  function EnqueueInbox(result) {
    selfId = result.peer_id || selfId;
    for (const body of Array.isArray(result.inbox) ? result.inbox : []) {
      Receive(body, result.session_id);
    }
  }

  function FormatPeer(from, type, text, correlationId) {
    const corr = correlationId ? ` correlation-id="${EscapeAttr(correlationId)}"` : "";
    return `<peer-message from="@${EscapeAttr(from)}" to="@${EscapeAttr(selfId || "")}" type="${EscapeAttr(type)}"${corr}>\n${EscapeBody(text)}\n</peer-message>`;
  }

  function RegisterTools() {
    if (typeof pi.registerTool !== "function") {
      return;
    }
    const tool = (name, description, properties, required, run) => {
      pi.registerTool({
        name: `amesh_${name}`,
        label: `amesh: ${name}`,
        description,
        parameters: { type: "object", properties, required },
        async execute(_id, params) {
          return { content: [{ type: "text", text: await run(params) }] };
        },
      });
    };
    tool(
      "whoami",
      "Show this amesh peer identity.",
      {},
      [],
      () => Cli(selfId ? ["peer", "whoami", "--peer-id", selfId] : ["peer", "whoami"]),
    );
    tool(
      "ask",
      "Open a tracked ask. Same-circle by default; set cross_circle for another circle. Close with amesh_ack.",
      {
        peer_name: { type: "string", description: "Target peer_id or name" },
        query: { type: "string", description: "Ask text" },
        cross_circle: { type: "boolean", description: "Required when target is in another circle" },
      },
      ["peer_name", "query"],
      (params) => {
        const args = ["peer", "ask", params.peer_name, params.query, ...FromPeer()];
        if (params.cross_circle) args.push("--cross-circle", "true");
        return Cli(args);
      },
    );
    tool(
      "ack",
      "Close an ask. Bare: amesh_ack(corr_id). Reply: amesh_ack(corr_id, message).",
      {
        correlation_id: { type: "string" },
        message: { type: "string", description: "Optional reply to the asker" },
      },
      ["correlation_id"],
      (params) => {
        const args = ["peer", "ack", params.correlation_id];
        if (params.message) args.push("--message", params.message);
        return Cli(args);
      },
    );
    tool(
      "notify_peer",
      "Fire-and-forget notify. Same-circle by default; set cross_circle for another circle.",
      {
        peer_name: { type: "string" },
        message: { type: "string" },
        cross_circle: { type: "boolean", description: "Required when target is in another circle" },
      },
      ["peer_name", "message"],
      (params) => {
        const args = ["peer", "notify", params.peer_name, params.message, ...FromPeer()];
        if (params.cross_circle) args.push("--cross-circle", "true");
        return Cli(args);
      },
    );
    const extra = [
      ["job_create", "Create a job ledger row", {
        title: { type: "string" },
        prompt: { type: "string" },
        path: { type: "string" },
        backend: { type: "string" },
        assigned_peer: { type: "string" },
      }, []],
      ["job_list", "List jobs in your circle; cross_circle without circle lists all", {
        circle: { type: "string" },
        cross_circle: { type: "boolean" },
      }, []],
      ["job_status", "Show a job", { job_id: { type: "string" } }, ["job_id"]],
      ["job_update", "Update job state", {
        job_id: { type: "string" },
        state: { type: "string" },
        result_summary: { type: "string" },
      }, ["job_id", "state"]],
      ["job_cancel", "Cancel a job", { job_id: { type: "string" } }, ["job_id"]],
      ["job_delete", "Delete a job", { job_id: { type: "string" } }, ["job_id"]],
      ["schedule_create", "Create a schedule", {
        to_peer: { type: "string" },
        peer_name: { type: "string" },
        text: { type: "string" },
        kind: { type: "string" },
        in_seconds: { type: "integer" },
        fire_at: { type: "integer" },
        every_seconds: { type: "integer" },
      }, []],
      ["schedule_list", "List schedules in your circle; cross_circle without circle lists all", {
        circle: { type: "string" },
        cross_circle: { type: "boolean" },
      }, []],
      ["schedule_delete", "Delete a schedule", { schedule_id: { type: "string" } }, ["schedule_id"]],
      ["ask_many", "Ask many peers", {
        to_peers: { type: "array", items: { type: "string" } },
        text: { type: "string" },
      }, ["to_peers", "text"]],
      ["wait", "Wait for an ask ack. timeout_seconds is capped at 50. When the result carries a hint, the ack is normally pushed to you as a peer-message: keep working or end the turn, and wait again only if it never arrives. The recipient acks once.", {
        correlation_id: { type: "string" },
        timeout_seconds: { type: "integer" },
      }, ["correlation_id"]],
      ["list_peers", "List peers in your circle; cross_circle without circle lists all", {
        circle: { type: "string" },
        cross_circle: { type: "boolean" },
      }, []],
      ["events", "List recent events in your circle; cross_circle without circle lists all. In-memory ring cleared on hub restart; newest 20 by default, limit up to 50, text trimmed to 200 chars", {
        since: { type: "string" },
        limit: { type: "integer" },
        circle: { type: "string" },
        cross_circle: { type: "boolean" },
      }, []],
      ["broadcast", "Broadcast in a circle. Default: caller's circle. Other circle: set circle and cross_circle.", {
        message: { type: "string" },
        circle: { type: "string" },
        cross_circle: { type: "boolean" },
      }, ["message"]],
    ];
    for (const [name, description, properties, required] of extra) {
      tool(name, description, properties, required, (params) => Mcp(name, params));
    }
  }

  let retry;
  let beat;
  let connectLogged = false;
  let connectTries = 0;
  function Connect(id) {
    if (dead || typeof WebSocket === "undefined" || process.env.AMESH_SKIP_WS) {
      return;
    }
    if (socket && (socket.readyState === 0 || socket.readyState === 1)) {
      return;
    }
    clearTimeout(retry);
    void Ensure().then(() => {
    if (dead || (socket && (socket.readyState === 0 || socket.readyState === 1))) {
      return;
    }
    const bind = process.env.AMESH_BIND || "127.0.0.1:8378";
    socket = new WebSocket(`ws://${bind}/ws`);
    socket.addEventListener("open", () => {
      connectLogged = false;
      connectTries = 0;
      const frame = { type: "connect", peer_id: id, recv: true };
      if (process.env.AMESH_TOKEN) {
        frame.auth_token = process.env.AMESH_TOKEN;
      }
      socket.send(JSON.stringify(frame));
    });
    socket.addEventListener("message", (event) => {
      if (dead) {
        return;
      }
      try {
        let body;
        try {
          body = JSON.parse(event.data);
        } catch {
          return;
        }
        if (!Receive(body)) {
          return;
        }
        if (body.id && socket.readyState === 1) {
          socket.send(JSON.stringify({ type: "recv", id: body.id }));
        }
        Flush();
      } catch (error) {
        console.error(`[amesh] ${error.message}`);
      }
    });
    socket.addEventListener("close", () => {
      socket = null;
      clearInterval(beat);
      if (dead) {
        return;
      }
      retry = setTimeout(() => Connect(id), 1000);
      retry.unref?.();
    });
    beat = setInterval(() => {
      if (socket?.readyState === 1) {
        socket.send(JSON.stringify({ type: "ping" }));
      }
    }, 10000);
    beat.unref?.();
    }).catch((error) => {
      if (!connectLogged) {
        console.error(`[amesh] ${error.message}`);
        connectLogged = true;
      }
      if (dead) {
        return;
      }
      if (connectTries >= 5) {
        console.error("[amesh] connect gave up after 5 retries; /reload to retry");
        return;
      }
      connectTries += 1;
      retry = setTimeout(() => Connect(id), 1000);
      retry.unref?.();
    });
  }

  RegisterTools();

  pi.on("session_start", async (_event, ctx) => {
    UseSession(ctx);
    try {
      await Ensure();
    } catch (error) {
      console.error(`[amesh] ${error.message}`);
    }
    if (dead) {
      return;
    }
    try {
      const result = await Invoke("session", ctx);
      EnqueueInbox(result);
      Connect(result.peer_id);
      if (!intro) {
        const boundary = /\n(?:Pending asks|Inbox):/;
        /* the roster block is a marker line plus TSV rows; peers coming and going must not
        re-send the rules, while any change to the rules themselves still does */
        const roster = /\nPeers in your circle[^\n]*(?:\n[^\t\n]+\t[^\t\n]+)*(?:\n\.\.\. \d+ more[^\n]*)?/g;
        const rules = (text) => text.replace(roster, "");
        const line = String(result.context || "").split(boundary)[0];
        const previous = (ctx.sessionManager.buildContextEntries?.() ?? []).findLast(
          (entry) => entry.type === "custom_message" && entry.customType === "amesh",
        );
        const previousLine = typeof previous?.content === "string" ? previous.content.split(boundary)[0] : undefined;
        if (previousLine === undefined || rules(previousLine) !== rules(line)) {
          pi.sendMessage({ customType: "amesh", content: line, display: true });
        }
        intro = true;
      }
    } catch (error) {
      console.error(`[amesh] ${error.message}`);
      if (!dead) {
        Connect(selfId);
      }
    }
    Flush();
  });

  pi.on("before_agent_start", async (_event, ctx) => {
    try {
      EnqueueInbox(await Invoke("prompt", ctx));
    } catch (error) {
      console.error(`[amesh] ${error.message}`);
    }
  });

  pi.on("agent_end", async (_event, ctx) => {
    try {
      EnqueueInbox(await Invoke("stop", ctx));
    } catch (error) {
      console.error(`[amesh] ${error.message}`);
    }
  });

  pi.on("agent_settled", () => {
    inflight = false;
    Flush();
  });

  pi.on("session_shutdown", () => {
    Drop();
  });
}

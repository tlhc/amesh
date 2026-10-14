import assert from "node:assert/strict";
import childProcess, { execFileSync } from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
const [pluginPath, executable, cwd] = process.argv.slice(2);
const source = await readFile(pluginPath, "utf8");
const commands = [];
const originalExecFile = childProcess.execFile;
childProcess.execFile = (...args) => {
  commands.push(args[1]);
  return originalExecFile(...args);
};
syncBuiltinESMExports();
const plugin = (
  await import(
    `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
  )
).default;
const logs = [];
const originalError = console.error;
console.error = (...values) => {
  logs.push(values.join(" "));
  originalError(...values);
};
const tools = new Map(),
  hooks = new Map(),
  sessions = new Map(),
  admitted = [];
const location = { directory: cwd };
for (const id of ["ses_a", "ses_b"]) sessions.set(id, { id, location });
sessions.set("ses_child", { id: "ses_child", parentID: "ses_a", location });
sessions.set("ses_cycle", { id: "ses_cycle", parentID: "ses_cycle", location });
sessions.set("ses_other", { id: "ses_other", location: { directory: "/tmp" } });
sessions.set("ses_gone_directory", {
  id: "ses_gone_directory",
  location: { directory: cwd + "/missing-location" },
});
const sockets = [];
const NativeWebSocket = globalThis.WebSocket;
globalThis.WebSocket = class extends NativeWebSocket {
  constructor(url) {
    super(url);
    this.created = Date.now();
    this.frames = [];
    sockets.push(this);
  }
  send(text) {
    const frame = JSON.parse(text);
    this.frames.push(frame);
    if (this.dropReceipt && frame.type === "recv") {
      this.dropReceipt = false;
      queueMicrotask(() => this.close());
      return;
    }
    super.send(text);
  }
};
let queue = [],
  wake,
  ended = false,
  failAdmit = false;
let admissionError, admissionGate;
const attempts = [];
const stored = new Map();
const Emit = (event) => {
  queue.push(event);
  wake?.();
};
const rpcHandlers = new Map();
const ctx = {
  rpc: {
    async register(definition, handlers) {
      rpcHandlers.set(definition.id, handlers);
      return {
        async dispose() {
          rpcHandlers.delete(definition.id);
        },
      };
    },
  },
  location,
  tool: {
    async transform(callback) {
      callback({
        add(tool) {
          tools.set(tool.name, tool);
        },
      });
      return { async dispose() {} };
    },
  },
  session: {
    async get({ sessionID }) {
      const info = sessions.get(sessionID);
      if (!info)
        throw Object.assign(new Error(""), {
          _tag: "Session.NotFoundError",
          name: "Session.NotFoundError",
        });
      return info;
    },
    async hook(name, callback) {
      hooks.set(name, callback);
      return {
        async dispose() {
          hooks.delete(name);
        },
      };
    },
    async synthetic(input) {
      attempts.push(input);
      if (admissionGate) await admissionGate;
      if (admissionError) throw admissionError;
      if (failAdmit) throw new Error("admission unavailable");
      if (stored.has(input.id)) return stored.get(input.id);
      admitted.push(input);
      stored.set(input.id, { id: input.id });
      return stored.get(input.id);
    },
  },
  event: {
    async *subscribe({ signal }) {
      signal.addEventListener(
        "abort",
        () => {
          ended = true;
          wake?.();
        },
        { once: true },
      );
      while (!ended) {
        while (queue.length) yield queue.shift();
        await new Promise((done) => {
          wake = done;
          if (ended || queue.length) done();
        });
      }
    },
  },
};
const Cli = (args, input) =>
  JSON.parse(
    execFileSync(executable, args, {
      cwd,
      env: process.env,
      input: input ? JSON.stringify(input) + "\n" : "",
      encoding: "utf8",
    }),
  );
const Invoke = async (name, sessionID, args = {}) =>
  tools
    .get(name)
    .execute(args, { sessionID, signal: new AbortController().signal });
const Text = (result) =>
  typeof result.content === "string" ? result.content : result.content[0].text;
async function Until(predicate, message, ms = 8000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await predicate()) return;
    await new Promise((done) => setTimeout(done, 25));
  }
  assert.fail(message);
}
const peers = () => Cli(["peer", "list"]);
const Details = async () =>
  JSON.parse(Text(await Invoke("amesh_list_peers", "ses_a", { detail: true })));
let cleanup;
await test("OpenCode plugin contracts against an isolated hub", async (t) => {
  try {
    cleanup = await plugin.setup(ctx);
    await t.test("catalog is directly visible with JSON schemas", () => {
      assert.equal(tools.size, 18);
      for (const tool of tools.values()) {
        assert.equal(tool.options.codemode, false);
        assert.equal(tool.input.type, "object");
      }
      assert.equal(
        tools.get("amesh_list_peers").input.properties.detail.type,
        "boolean",
      );
    });
    await t.test("context and child prompt do not enroll", async () => {
      await hooks.get("context")({ sessionID: "ses_a", system: [] });
      await hooks.get("prompt")({ sessionID: "ses_child" });
      assert.equal(peers().length, 0);
    });
    let a, b;
    await t.test(
      "two roots remain distinct and child shares root",
      async () => {
        [a, b] = await Promise.all([
          Invoke("amesh_whoami", "ses_a"),
          Invoke("amesh_whoami", "ses_b"),
        ]);
        a = Text(a);
        b = Text(b);
        assert.notEqual(a, b);
        assert.equal(Text(await Invoke("amesh_whoami", "ses_child")), a);
        assert.equal(peers().length, 2);
        assert(
          peers().every((peer) => peer.circle !== "inherited-wrong-circle"),
        );
      },
    );
    await t.test("baked catalog matches current MCP definitions", () => {
      const reply = Cli(["mcp", "--peer-id", a], {
        jsonrpc: "2.0",
        id: 1,
        method: "tools/list",
        params: {},
      });
      assert.equal(tools.size, reply.result.tools.length);
      for (const definition of reply.result.tools) {
        const tool = tools.get(definition.name);
        assert.equal(tool.description, definition.description);
        assert.deepEqual(tool.input, definition.inputSchema);
      }
    });
    await t.test("foreign location rejected before registration", async () => {
      await assert.rejects(Invoke("amesh_whoami", "ses_other"), /location/i);
      assert.equal(peers().length, 2);
    });
    await t.test("cyclic parent chain is rejected", async () => {
      await assert.rejects(Invoke("amesh_whoami", "ses_cycle"), /parent chain/);
    });
    await t.test(
      "opening a root enrolls without a prompt or model request",
      async () => {
        const handlers = rpcHandlers.get("amesh");
        assert(handlers, "session-open RPC must be registered");
        sessions.set("ses_open", { id: "ses_open", location });
        const result = await handlers.join({
          sessionID: "ses_open",
          state: "idle",
        });
        assert(result.peerID);
        await Until(
          () =>
            sockets.some((socket) =>
              socket.frames.some(
                (frame) =>
                  frame.type === "connect" && frame.peer_id === result.peerID,
              ),
            ),
          "opened root connects",
        );
        assert.equal(admitted.length, 0);
        const peer = (await Details()).find(
          (peer) => peer.peer_id === result.peerID,
        );
        assert.equal(peer.activity.state, "idle");
        await new Promise((resolve) => setTimeout(resolve, 100));
        const reports = () =>
          commands.filter((args) => args[0] === "hook" && args[1] === "idle")
            .length;
        const beforeRefresh = reports();
        await handlers.join({ sessionID: "ses_open", state: "idle" });
        await handlers.join({ sessionID: "ses_open", state: "idle" });
        assert.equal(
          reports(),
          beforeRefresh,
          "healthy foreground refresh must not spawn activity hooks",
        );
        assert.equal(
          (await handlers.join({ sessionID: "ses_child", state: "idle" }))
            .peerID,
          a,
        );
        const rejected = await handlers.join(
          { sessionID: "ses_other", state: "idle" },
          { error: (type, message, data) => ({ type, message, data }) },
        );
        assert.equal(rejected.type, "location");
      },
    );
    await t.test("child context describes shared identity", async () => {
      const input = { sessionID: "ses_child", system: [] };
      await hooks.get("context")(input);
      assert(
        input.system.some(
          (part) =>
            part.type === "text" &&
            part.text.includes(a) &&
            part.text.includes("ses_a"),
        ),
      );
    });
    await t.test(
      "admission failure retains notification until replay",
      async () => {
        failAdmit = true;
        const socketStart = sockets.length;
        await Invoke("amesh_notify_peer", "ses_b", {
          peer_name: a,
          message: "retry-admission",
        });
        await new Promise((done) => setTimeout(done, 6500));
        assert.equal(admitted.length, 0);
        const reconnects = sockets
          .slice(socketStart)
          .filter((socket) =>
            socket.frames.some((frame) => frame.peer_id === a),
          );
        assert(
          reconnects.length >= 2 && reconnects.length <= 3,
          "bounded reconnections while admission fails",
        );
        assert(
          reconnects[1].created - reconnects[0].created >= 2800,
          "failure backoff grows across successful handshakes",
        );
        failAdmit = false;
        await Until(
          () => admitted.some((item) => item.text.includes("retry-admission")),
          "admission must retry",
        );
        assert.equal(admitted[0].sessionID, "ses_a");
        assert.equal(admitted[0].delivery, "steer");
        assert.equal(
          admitted[0].description,
          `amesh notify from @${b}`,
          "collaboration message is visible in the TUI",
        );
      },
    );
    await t.test("revert defers durable admission", async () => {
      sessions.get("ses_a").revert = { messageID: "msg_boundary" };
      const count = admitted.length;
      await Invoke("amesh_notify_peer", "ses_b", {
        peer_name: a,
        message: "after-revert",
      });
      await new Promise((done) => setTimeout(done, 150));
      assert.equal(admitted.length, count);
      delete sessions.get("ses_a").revert;
      await Until(
        () => admitted.some((item) => item.text.includes("after-revert")),
        "revert recovery",
      );
    });
    await t.test(
      "broadcast uses queue and incoming text is escaped",
      async () => {
        await Invoke("amesh_broadcast", "ses_b", { message: "<unsafe>" });
        await Until(
          () => admitted.some((item) => item.text.includes("&lt;unsafe>")),
          "broadcast",
        );
        assert.equal(
          admitted.find((item) => item.text.includes("&lt;unsafe>")).delivery,
          "queue",
        );
      },
    );
    await t.test(
      "child approvals hold root in wait until all replied",
      async () => {
        Emit({
          type: "permission.asked",
          location,
          data: { sessionID: "ses_child", id: "per_1", action: "bash" },
        });
        Emit({
          type: "permission.asked",
          location,
          data: { sessionID: "ses_a", id: "per_2", action: "edit" },
        });
        await Until(
          async () =>
            (await Details()).find((peer) => peer.peer_id === a)?.activity
              ?.state === "wait",
          "wait activity",
        );
        Emit({
          type: "permission.replied",
          location,
          data: { sessionID: "ses_child", requestID: "per_1" },
        });
        await new Promise((done) => setTimeout(done, 100));
        assert.equal(
          (await Details()).find((peer) => peer.peer_id === a).activity.state,
          "wait",
        );
        Emit({
          type: "permission.replied",
          location,
          data: { sessionID: "ses_a", requestID: "per_2" },
        });
        await Until(
          async () =>
            (await Details()).find((peer) => peer.peer_id === a)?.activity
              ?.state === "work",
          "work activity",
        );
      },
    );
    await t.test("foreign and missing session events are silent", async () => {
      const before = logs.length;
      Emit({
        type: "session.execution.started",
        data: { sessionID: "ses_other" },
      });
      Emit({
        type: "permission.asked",
        data: { sessionID: "ses_other", id: "foreign", action: "bash" },
      });
      Emit({
        type: "session.execution.started",
        data: { sessionID: "ses_missing" },
      });
      Emit({
        type: "session.execution.started",
        data: { sessionID: "ses_gone_directory" },
      });
      Emit({
        type: "session.execution.succeeded",
        data: { sessionID: "ses_child" },
      });
      await new Promise((done) => setTimeout(done, 100));
      assert.equal(logs.length, before);
      assert.equal(
        (await Details()).find((peer) => peer.peer_id === a).activity.state,
        "work",
      );
    });
    await t.test("tool errors propagate", async () => {
      await assert.rejects(
        Invoke("amesh_ack", "ses_a", {
          correlation_id: "missing",
          message: "bad",
        }),
        /./,
      );
    });
    await t.test("recv waits for durable admission", async () => {
      let release;
      admissionGate = new Promise((done) => (release = done));
      const socket = sockets.find(
        (socket) =>
          socket.readyState === 1 &&
          socket.frames.some((frame) => frame.peer_id === a),
      );
      const count = attempts.length;
      await Invoke("amesh_notify_peer", "ses_b", {
        peer_name: a,
        message: "receipt-barrier",
      });
      await Until(() => attempts.length > count, "admission started");
      const eventID = attempts.at(-1).metadata.ameshEventID;
      assert(
        !socket.frames.some(
          (frame) => frame.type === "recv" && frame.id === eventID,
        ),
      );
      release();
      admissionGate = undefined;
      await Until(
        () =>
          socket.frames.some(
            (frame) => frame.type === "recv" && frame.id === eventID,
          ),
        "recv after admission",
      );
    });
    await t.test(
      "successful admission survives lost recv with one stored message",
      async () => {
        const socket = sockets.find(
          (socket) =>
            socket.readyState === 1 &&
            socket.frames.some((frame) => frame.peer_id === a),
        );
        socket.dropReceipt = true;
        await Invoke("amesh_notify_peer", "ses_b", {
          peer_name: a,
          message: "lost-receipt",
        });
        await Until(
          () =>
            attempts.filter((item) => item.text.includes("lost-receipt"))
              .length >= 2,
          "successful admission replay",
        );
        const replay = attempts.filter((item) =>
          item.text.includes("lost-receipt"),
        );
        assert.equal(replay[0].id, replay[1].id);
        assert.equal(
          admitted.filter((item) => item.text.includes("lost-receipt")).length,
          1,
        );
      },
    );
    await t.test(
      "409 remains unconfirmed and later frames proceed",
      async () => {
        const socket = sockets.find(
          (socket) =>
            socket.readyState === 1 &&
            socket.frames.some((frame) => frame.peer_id === a),
        );
        admissionError = Object.assign(new Error(""), {
          _tag: "Session.SyntheticConflictError",
          name: "Session.SyntheticConflictError",
        });
        const before = attempts.length;
        await Invoke("amesh_notify_peer", "ses_b", {
          peer_name: a,
          message: "conflict-replay",
        });
        await Until(() => attempts.length >= before + 2, "one delayed retry");
        const failed = attempts.slice(before);
        assert.equal(failed.length, 2);
        assert.equal(failed[0].id, failed[1].id);
        assert(
          !socket.frames.some(
            (frame) =>
              frame.type === "recv" &&
              frame.id === failed[0].metadata.ameshEventID,
          ),
        );
        assert(
          logs.some(
            (line) =>
              line.includes("Session.SyntheticConflictError") &&
              line.includes("ses_a/"),
          ),
          "conflict diagnostic identifies session and event",
        );
        admissionError = undefined;
        await Invoke("amesh_notify_peer", "ses_b", {
          peer_name: a,
          message: "after-conflict",
        });
        await Until(
          () => admitted.some((item) => item.text.includes("after-conflict")),
          "later frame",
        );
        socket.close();
        await Until(
          () => admitted.some((item) => item.text.includes("conflict-replay")),
          "conflict reconnect replay",
        );
        assert.equal(
          admitted.find((item) => item.text.includes("conflict-replay")).id,
          failed[0].id,
        );
      },
    );
    await t.test("old generation control frame is ignored", async () => {
      const old = sockets.find(
        (socket) =>
          socket.readyState === 3 &&
          socket.frames.some((frame) => frame.peer_id === a),
      );
      assert(old);
      old.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({ type: "displaced", peer_id: a }),
        }),
      );
      assert.equal(Text(await Invoke("amesh_whoami", "ses_a")), a);
    });
    await t.test("reconnect reports last idle state", async () => {
      Emit({
        type: "session.execution.succeeded",
        data: { sessionID: "ses_a" },
      });
      await Until(
        async () =>
          (await Details()).find((peer) => peer.peer_id === a)?.activity
            ?.state === "idle",
        "recorded idle",
      );
      const socket = sockets.find(
        (socket) =>
          socket.readyState === 1 &&
          socket.frames.some((frame) => frame.peer_id === a),
      );
      const count = sockets.length;
      socket.close();
      await Until(() => sockets.length > count, "new connection");
      await Until(
        async () =>
          (await Details()).find((peer) => peer.peer_id === a)?.activity
            ?.state === "idle",
        "restored idle",
      );
    });
    await t.test(
      "transferred backlog keeps original recipient spelling",
      async () => {
        const socket = sockets.find(
          (socket) =>
            socket.readyState === 1 &&
            socket.frames.some((frame) => frame.peer_id === a),
        );
        socket.dispatchEvent(
          new MessageEvent("message", {
            data: JSON.stringify({
              type: "notify",
              id: "evt_oldtarget",
              to_peer: "previous-peer-name",
              message: "transferred backlog",
            }),
          }),
        );
        await Until(
          () =>
            admitted.some(
              (item) => item.metadata.ameshEventID === "evt_oldtarget",
            ),
          "backlog with old name admitted",
        );
        assert(
          socket.frames.some(
            (frame) => frame.type === "recv" && frame.id === "evt_oldtarget",
          ),
        );
      },
    );
    await t.test(
      "display sender rejects terminal control characters",
      async () => {
        const socket = sockets.find(
          (socket) =>
            socket.readyState === 1 &&
            socket.frames.some((frame) => frame.peer_id === a),
        );
        socket.dispatchEvent(
          new MessageEvent("message", {
            data: JSON.stringify({
              type: "notify",
              id: "evt_control_sender",
              from_peer: "evil\u001b[2J\u001b]0;pwned\u0007",
              message: "payload",
            }),
          }),
        );
        await Until(
          () =>
            admitted.some(
              (item) => item.metadata.ameshEventID === "evt_control_sender",
            ),
          "control sender admitted",
        );
        assert.equal(
          admitted.find(
            (item) => item.metadata.ameshEventID === "evt_control_sender",
          ).description,
          "amesh notify from @anonymous",
        );
      },
    );
    await t.test("queue capacity includes the active admission", async () => {
      let release;
      admissionGate = new Promise((done) => (release = done));
      const socket = sockets.find(
        (socket) =>
          socket.readyState === 1 &&
          socket.frames.some((frame) => frame.peer_id === a),
      );
      try {
        for (let n = 0; n < 129; n++)
          socket.dispatchEvent(
            new MessageEvent("message", {
              data: JSON.stringify({
                type: "notify",
                id: `evt_capacity_${n}`,
                to_peer: a,
                message: "capacity",
              }),
            }),
          );
        assert(
          socket.readyState >= 2,
          "129 total frames exceed the 128 frame budget",
        );
      } finally {
        release();
        admissionGate = undefined;
      }
      await Until(
        () =>
          sockets.some(
            (socket) =>
              socket.readyState === 1 &&
              socket.frames.some((frame) => frame.peer_id === a) &&
              socket !== sockets.at(0),
          ),
        "capacity reconnect",
      );
    });
    await t.test(
      "aggregate byte budget closes connection before admission",
      async () => {
        let release;
        admissionGate = new Promise((done) => (release = done));
        const socket = sockets.find(
          (socket) =>
            socket.readyState === 1 &&
            socket.frames.some((frame) => frame.peer_id === b),
        );
        try {
          for (let n = 0; n < 3; n++)
            socket.dispatchEvent(
              new MessageEvent("message", {
                data: JSON.stringify({
                  type: "notify",
                  id: `evt_bytes_${n}`,
                  message: "x".repeat(12 * 1024 * 1024),
                }),
              }),
            );
          assert(
            socket.readyState >= 2,
            "36 MiB exceeds the 32 MiB aggregate budget",
          );
        } finally {
          release();
          admissionGate = undefined;
        }
        await Until(
          () =>
            sockets.some(
              (value) =>
                value !== socket &&
                value.readyState === 1 &&
                value.frames.some((frame) => frame.peer_id === b),
            ),
          "byte budget reconnect",
        );
      },
    );
    for (const kind of ["oversized", "identity", "replaced"]) {
      await t.test(`${kind} frame terminates only its root`, async () => {
        const id = "ses_" + kind;
        sessions.set(id, { id, location });
        const peer = Text(await Invoke("amesh_whoami", id));
        await Until(
          () =>
            sockets.some(
              (socket) =>
                socket.readyState === 1 &&
                socket.frames.some((frame) => frame.peer_id === peer),
            ),
          "root socket",
        );
        const socket = sockets.find(
          (socket) =>
            socket.readyState === 1 &&
            socket.frames.some((frame) => frame.peer_id === peer),
        );
        const event =
          kind === "oversized"
            ? {
                type: "notify",
                id: "evt_oversized",
                message: "x".repeat(16 * 1024 * 1024),
              }
            : kind === "replaced"
              ? { type: "replaced" }
              : {
                  type: "connected",
                  peer_id: peer,
                  session_id: "opencode:wrong",
                };
        socket.dispatchEvent(
          new MessageEvent("message", { data: JSON.stringify(event) }),
        );
        await assert.rejects(Invoke("amesh_whoami", id), /reload/);
        assert(!socket.frames.some((frame) => frame.type === "recv"));
        assert.equal(Text(await Invoke("amesh_whoami", "ses_b")), b);
      });
    }
    await t.test(
      "subprocess cancellation rejects an in-flight wait",
      async () => {
        const ask = JSON.parse(
          Text(
            await Invoke("amesh_ask", "ses_a", {
              peer_name: b,
              query: "leave this unanswered",
            }),
          ),
        );
        const abort = new AbortController();
        const waiting = tools
          .get("amesh_wait")
          .execute(
            { correlation_id: ask.correlation_id, timeout_seconds: 50 },
            { sessionID: "ses_a", signal: abort.signal },
          );
        setTimeout(() => abort.abort(), 100);
        await assert.rejects(waiting, /abort|unknown/i);
      },
    );
    await t.test(
      "displaced root stops while other root still works",
      async () => {
        const socket = new WebSocket(`ws://${process.env.AMESH_BIND}/ws`);
        await new Promise((resolve, reject) => {
          socket.addEventListener("open", () =>
            socket.send(
              JSON.stringify({
                type: "connect",
                peer_id: a,
                recv: true,
                auth_token: process.env.AMESH_TOKEN,
              }),
            ),
          );
          socket.addEventListener("message", () => resolve(), { once: true });
          socket.addEventListener("error", reject, { once: true });
        });
        try {
          await Until(async () => {
            try {
              await Invoke("amesh_whoami", "ses_a");
              return false;
            } catch (error) {
              return /reload/.test(error.message);
            }
          }, "displaced terminates root");
          const stopped = await rpcHandlers
            .get("amesh")
            .join(
              { sessionID: "ses_a", state: "idle" },
              { error: (type, message, data) => ({ type, message, data }) },
            );
          assert.equal(stopped.type, "stopped");
          assert.match(stopped.message, /opencode reload/);
          assert.equal(Text(await Invoke("amesh_whoami", "ses_b")), b);
        } finally {
          socket.close();
        }
      },
    );
    await t.test(
      "stopped and missing sessions do not reject user prompts",
      async () => {
        await assert.doesNotReject(hooks.get("prompt")({ sessionID: "ses_a" }));
        await assert.doesNotReject(
          hooks.get("prompt")({ sessionID: "ses_missing" }),
        );
      },
    );
    for (const type of [
      "session.deleted",
      "session.moved",
      "Session.NotFoundError",
    ]) {
      await t.test(`${type} terminates only its root`, async () => {
        const id = "ses_" + type.replaceAll(".", "_");
        sessions.set(id, { id, location });
        const peer = Text(await Invoke("amesh_whoami", id));
        if (type === "Session.NotFoundError") {
          admissionError = Object.assign(new Error(""), {
            _tag: type,
            name: type,
          });
          await Invoke("amesh_notify_peer", "ses_b", {
            peer_name: peer,
            message: "missing-session-admission",
          });
        } else Emit({ type, data: { sessionID: id } });
        await Until(async () => {
          try {
            await Invoke("amesh_whoami", id);
            return false;
          } catch (error) {
            return /reload/.test(error.message);
          }
        }, "root terminated");
        admissionError = undefined;
        const socket = sockets.find((socket) =>
          socket.frames.some((frame) => frame.peer_id === peer),
        );
        assert(socket);
        assert(!socket.frames.some((frame) => frame.type === "recv"));
        await assert.doesNotReject(hooks.get("prompt")({ sessionID: id }));
        assert.equal(Text(await Invoke("amesh_whoami", "ses_b")), b);
      });
    }
    await t.test(
      "location shutdown cleans up only the matching instance",
      async () => {
        Emit({
          type: "location.shutdown",
          location: { directory: "/tmp" },
          data: {},
        });
        await new Promise((done) => setTimeout(done, 100));
        assert.equal(Text(await Invoke("amesh_whoami", "ses_b")), b);
        Emit({ type: "location.shutdown", location, data: {} });
        await Until(() => hooks.size === 0, "shutdown disposes registrations");
        await assert.rejects(Invoke("amesh_whoami", "ses_b"), /stopped|reload/);
      },
    );
  } finally {
    await cleanup?.();
  }
  await t.test("cleanup removes hook registrations", () => {
    assert.equal(hooks.size, 0);
    assert(ended);
    assert(sockets.every((socket) => socket.readyState >= 2));
  });
  await t.test("failed setup aborts its event subscription", async () => {
    let signal;
    const broken = {
      ...ctx,
      tool: {
        async transform() {
          throw new Error("registration failed");
        },
      },
      event: {
        async *subscribe(options) {
          signal = options.signal;
          await new Promise((done) =>
            signal.addEventListener("abort", done, { once: true }),
          );
        },
      },
    };
    await assert.rejects(plugin.setup(broken), /registration failed/);
    assert(signal.aborted, "setup failure must release its event iterator");
  });
  await t.test(
    "cleanup is bounded when an in-process session read stalls",
    async () => {
      let release,
        entered = false;
      const pending = new Promise((done) => (release = done));
      const stopCleanup = await plugin.setup({
        ...ctx,
        session: {
          ...ctx.session,
          async get() {
            entered = true;
            await pending;
            return sessions.get("ses_b");
          },
        },
        event: {
          async *subscribe() {
            yield {
              type: "session.execution.started",
              data: { sessionID: "ses_b" },
            };
          },
        },
      });
      await Until(() => entered, "event read started");
      const started = Date.now();
      try {
        await Promise.race([
          stopCleanup(),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error("cleanup exceeded 3s")), 3000),
          ),
        ]);
        assert(Date.now() - started < 3000);
      } finally {
        release();
      }
    },
  );

  await t.test(
    "two complete plugin instances do not fight for one root",
    async () => {
      const id = "ses_contended";
      sessions.set(id, { id, location });
      async function Instance() {
        const definitions = new Map();
        const cleanup = await plugin.setup({
          ...ctx,
          tool: {
            async transform(callback) {
              callback({ add: (tool) => definitions.set(tool.name, tool) });
              return { dispose() {} };
            },
          },
          session: {
            ...ctx.session,
            async hook() {
              return { dispose() {} };
            },
          },
          event: {
            async *subscribe({ signal }) {
              await new Promise((done) =>
                signal.addEventListener("abort", done, { once: true }),
              );
            },
          },
        });
        return {
          cleanup,
          invoke: () =>
            definitions
              .get("amesh_whoami")
              .execute(
                {},
                { sessionID: id, signal: new AbortController().signal },
              ),
        };
      }
      const first = await Instance(),
        second = await Instance();
      try {
        const peer = Text(await first.invoke());
        assert.equal(Text(await second.invoke()), peer);
        await Until(async () => {
          try {
            await first.invoke();
            return false;
          } catch (error) {
            return /reload/.test(error.message);
          }
        }, "first instance terminates");
        const count = sockets.length;
        await new Promise((done) => setTimeout(done, 1200));
        assert.equal(
          sockets.length,
          count,
          "terminated instance stays stopped",
        );
        assert.equal(Text(await second.invoke()), peer);
      } finally {
        await first.cleanup();
        await second.cleanup();
      }
    },
  );
});

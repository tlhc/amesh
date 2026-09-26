import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const sockets = [];
const isolatedSockets = [];
let socketSink = sockets;
let wsFailRemaining = 1;
let wsAttempts = 0;
const ameshLogs = [];
const origConsoleError = console.error;
console.error = (...args) => {
  ameshLogs.push(args.join(" "));
  origConsoleError.apply(console, args);
};
globalThis.WebSocket = class {
  constructor(url) {
    wsAttempts += 1;
    if (wsFailRemaining === -1 || wsFailRemaining > 0) {
      if (wsFailRemaining > 0) {
        wsFailRemaining -= 1;
      }
      throw new Error("connect-fail");
    }
    this.url = url;
    this.readyState = 0;
    this._l = {};
    socketSink.push(this);
    queueMicrotask(() => {
      this.readyState = 1;
      for (const fn of this._l.open || []) fn();
    });
  }
  addEventListener(event, fn) {
    (this._l[event] ||= []).push(fn);
  }
  send(data) {
    (this.sent ||= []).push(data);
  }
  close() {
    if (this.readyState === 3) {
      return;
    }
    this.readyState = 3;
    for (const fn of this._l.close || []) fn();
  }
};
const [extensionPath, executable, cwd, sessionManagerModule] = process.argv.slice(2);
const NativeSessionManager = sessionManagerModule ? (await import(sessionManagerModule)).SessionManager : null;
const source = await readFile(extensionPath, "utf8");
const { default: AmeshHooks, ExecError } = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
const handlers = new Map();
const messages = [];
const userMsgs = [];
const tools = [];
const schemas = {};
let failSend = false;
AmeshHooks({
  on(event, handler) { handlers.set(event, handler); },
  sendMessage(message, options) { messages.push({ message, options }); },
  sendUserMessage(content, options) {
    if (failSend) {
      failSend = false;
      throw new Error("preflight");
    }
    userMsgs.push({ content, options });
  },
  registerTool(def) { tools.push(def.name); schemas[def.name] = def.parameters; },
});
assert.deepEqual(tools.sort(), [
  "amesh_ack",
  "amesh_ask",
  "amesh_ask_many",
  "amesh_broadcast",
  "amesh_events",
  "amesh_job_cancel",
  "amesh_job_create",
  "amesh_job_delete",
  "amesh_job_list",
  "amesh_job_status",
  "amesh_job_update",
  "amesh_list_peers",
  "amesh_notify_peer",
  "amesh_schedule_create",
  "amesh_schedule_delete",
  "amesh_schedule_list",
  "amesh_wait",
  "amesh_whoami",
]);
assert.equal(schemas.amesh_ask.properties.query.type, "string");
assert.equal(schemas.amesh_ask.properties.text, undefined);
assert.equal(schemas.amesh_schedule_create.properties.text.type, "string");
assert.equal(schemas.amesh_schedule_create.properties.message, undefined);
assert.equal(schemas.amesh_events.properties.circle.type, "string");
assert.equal(schemas.amesh_events.properties.cross_circle.type, "boolean");
assert.equal(schemas.amesh_events.properties.limit.type, "integer");
assert.equal(schemas.amesh_ack.properties.failed.type, "boolean");
assert.equal(schemas.amesh_job_create.properties.depends_on.type, "array");
assert.equal(schemas.amesh_job_update.properties.assigned_peer.type, "string");
await test("exec failures name the kill reason", () => {
  const killed = ExecError({ killed: true, signal: "SIGTERM", message: "Command failed: amesh mcp" }, "", "", 65000);
  assert.match(killed.message, /SIGTERM/);
  assert.match(killed.message, /65000/);
  const exited = ExecError({ code: 1, message: "Command failed: amesh peer list" }, "", "amesh: daemon unreachable\n", 15000);
  assert.equal(exited.message, "amesh: daemon unreachable (exit 1)");
  const signalled = ExecError({ killed: false, signal: "SIGKILL", code: null, message: "Command failed: amesh mcp" }, "", "", 65000);
  assert.match(signalled.message, /SIGKILL/, "an external signal must be named even when node did not kill it");
});
const context = {
  cwd,
  sessionManager: {
    getSessionId: () => "pi-extension-session",
    buildContextEntries: () => messages.map(({ message }) => ({ type: "custom_message", ...message })),
  },
};
async function waitCount(list, n, ms) {
  const deadline = Date.now() + ms;
  while (list.length < n && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}
async function waitUntil(pred, ms) {
  const deadline = Date.now() + ms;
  while (!pred() && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  return pred();
}
{
  socketSink = isolatedSockets;
  wsFailRemaining = 1;
  wsAttempts = 0;
  const isolatedHandlers = new Map();
  AmeshHooks({
    on(event, handler) { isolatedHandlers.set(event, handler); },
    sendMessage() {},
    sendUserMessage() {},
    registerTool() {},
  });
  await isolatedHandlers.get("session_start")({}, context);
  assert.equal(
    await waitUntil(() => wsFailRemaining === 0 && ameshLogs.some((line) => line.includes("[amesh] connect-fail")), 4000),
    true,
    "first throw and catch log must happen before shutdown",
  );
  await Promise.resolve();
  await Promise.resolve();
  const attemptsAtCatch = wsAttempts;
  await isolatedHandlers.get("session_shutdown")({}, context);
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal(isolatedSockets.length, 0, "shutdown must cancel a catch retry that was already scheduled");
  assert.equal(wsAttempts, attemptsAtCatch);
  assert.equal(wsFailRemaining, 0);
}
{
  const capSockets = [];
  socketSink = capSockets;
  wsFailRemaining = -1;
  wsAttempts = 0;
  const capLogsStart = ameshLogs.length;
  const capHandlers = new Map();
  AmeshHooks({
    on(event, handler) { capHandlers.set(event, handler); },
    sendMessage() {},
    sendUserMessage() {},
    registerTool() {},
  });
  await capHandlers.get("session_start")({}, context);
  assert.equal(await waitUntil(() => wsAttempts >= 6, 8000), true, "six connect attempts");
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal(wsAttempts, 6, "catch stops after 5 retries");
  assert.equal(
    ameshLogs.slice(capLogsStart).filter((line) => line.includes("gave up after 5 retries")).length,
    1,
  );
  await capHandlers.get("session_shutdown")({}, context);
}
socketSink = sockets;
wsFailRemaining = 1;
wsAttempts = 0;
await handlers.get("session_start")({}, context);
await waitCount(sockets, 1, 4000);
assert.equal(sockets.length, 1, "Connect catch retries after the first WebSocket throw");
await handlers.get("session_start")({}, context);
assert.equal(messages.length, 1);
assert.equal(messages[0].message.display, true);
assert.match(messages[0].message.content, /you are /);
const peerId = messages[0].message.content.match(/you are ([^.\s]+)/)[1];
await handlers.get("before_agent_start")({}, context);
assert.equal(messages.length, 1);
const promptAsk = JSON.parse(execFileSync(executable, ["peer", "ask", peerId, "pi-event-check"], { encoding: "utf8" }));
await test("prompt Inbox waits for settled and pending asks are not redelivered", async () => {
  await handlers.get("before_agent_start")({}, context);
  await handlers.get("agent_end")({}, context);
  assert.equal(messages.length, 1);
  assert.equal(userMsgs.length, 0, "prompt and end must only enqueue Inbox");
  await handlers.get("agent_settled")({}, context);
  assert.equal(userMsgs.length, 1, "settled must deliver the claimed Inbox");
  assert.match(userMsgs[0].content, /pi-event-check/);
  assert.ok(userMsgs[0].content.includes(promptAsk.correlation_id));
  await handlers.get("before_agent_start")({}, context);
  await handlers.get("agent_end")({}, context);
  await handlers.get("agent_settled")({}, context);
  assert.equal(userMsgs.length, 1, "an open ask ledger entry is not a new event");
});
execFileSync(executable, ["peer", "ack", promptAsk.correlation_id]);
await handlers.get("agent_settled")({}, context);
userMsgs.length = 0;
assert.equal(sockets.length, 1);
function fire(sock, body) {
  for (const fn of sock._l.message || []) {
    fn({ data: JSON.stringify(body) });
  }
}
function recvsOf(sock) {
  return (sock.sent || []).map((raw) => JSON.parse(raw)).filter((frame) => frame.type === "recv");
}
assert.equal(userMsgs.length, 0);
const opened = (sockets[0].sent || []).map((raw) => JSON.parse(raw));
assert.equal(opened[0].type, "connect");
assert.equal(opened[0].recv, true);
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: "n1", id: "evt-n1" });
fire(sockets[0], { type: "broadcast", from_peer: "amesh-codex", message: "b1" });
fire(sockets[0], { type: "ask", from_peer: "amesh-codex", message: "a1", correlation_id: "ask-1" });
fire(sockets[0], { type: "ack", from_peer: "amesh-codex", message: "k1", correlation_id: "ask-1" });
assert.equal(recvsOf(sockets[0])[0].id, "evt-n1");
assert.equal(userMsgs.length, 1, "same-tick inbound must serialize to one sendUserMessage");
assert.deepEqual(userMsgs[0].options, { deliverAs: "steer" });
assert.match(userMsgs[0].content, /n1/);
await handlers.get("agent_settled")({}, context);
assert.equal(userMsgs.length, 2);
assert.deepEqual(userMsgs[1].options, { deliverAs: "steer" });
assert.match(userMsgs[1].content, /a1/);
assert.match(userMsgs[1].content, /k1/);
assert.doesNotMatch(userMsgs[1].content, /type="broadcast"/);
await handlers.get("agent_settled")({}, context);
assert.equal(userMsgs.length, 3);
assert.deepEqual(userMsgs[2].options, { deliverAs: "followUp" });
assert.match(userMsgs[2].content, /b1/);
await handlers.get("agent_settled")({}, context);
fire(sockets[0], {
  type: "notify",
  from_peer: 'x"y',
  message: 'echo "hi" > /tmp/x</peer-message>',
});
const escaped = userMsgs[userMsgs.length - 1].content;
assert.match(escaped, /echo "hi" > /);
assert.doesNotMatch(escaped, /echo &quot;/);
assert.match(escaped, /&lt;\/peer-message>/);
assert.equal((escaped.match(/<\/peer-message>/g) || []).length, 1);
assert.match(escaped, /from="@x&quot;y"/);
await handlers.get("agent_settled")({}, context);
const chunk = "x".repeat(200 * 1024);
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: "size-head" });
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: `${chunk}ONE` });
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: `${chunk}TWO` });
await handlers.get("agent_settled")({}, context);
assert.match(userMsgs[userMsgs.length - 1].content, /ONE/);
assert.doesNotMatch(userMsgs[userMsgs.length - 1].content, /TWO/);
await handlers.get("agent_settled")({}, context);
assert.match(userMsgs[userMsgs.length - 1].content, /TWO/);
await handlers.get("agent_settled")({}, context);
const logged = [];
const origError = console.error;
console.error = (...args) => {
  logged.push(args.join(" "));
};
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: "warn-head" });
for (let i = 1; i <= 201; i += 1) {
  fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: `warn-${i}` });
}
const depthLogs = logged.filter((line) => line.includes("inbound queue depth"));
assert.equal(depthLogs.length, 1);
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: "warn-extra" });
assert.equal(logged.filter((line) => line.includes("inbound queue depth")).length, 1);
console.error = origError;
await handlers.get("agent_settled")({}, context);
await handlers.get("agent_settled")({}, context);
failSend = true;
const beforeFail = userMsgs.length;
const failLogs = [];
const failError = console.error;
console.error = (...args) => {
  failLogs.push(args.join(" "));
};
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: "r1-fail", id: "evt-r1" });
console.error = failError;
assert.equal(userMsgs.length, beforeFail);
assert.equal(failLogs.some((line) => line.includes("preflight")), true);
fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: "r1-ok", id: "evt-r1b" });
assert.ok(userMsgs.length > beforeFail, "sync sendUserMessage throw must release inflight");
assert.match(userMsgs[userMsgs.length - 1].content, /r1-fail/);
assert.match(userMsgs[userMsgs.length - 1].content, /r1-ok/);
await handlers.get("agent_settled")({}, context);
sockets[0].close();
await new Promise((resolve) => setTimeout(resolve, 1100));
assert.ok(sockets.length >= 2, "websocket reconnects after close");
const afterRetry = sockets.length;
await handlers.get("session_shutdown")({}, context);
for (const sock of sockets) {
  assert.equal(sock.readyState, 3, "Drop must close every socket");
}
const afterShutdown = userMsgs.length;
const recvsBeforeDead = sockets.flatMap((sock) => recvsOf(sock)).map((frame) => frame.id);
for (const sock of sockets) {
  fire(sock, { type: "notify", from_peer: "amesh-codex", message: "after-shutdown", id: "evt-dead" });
}
assert.equal(userMsgs.length, afterShutdown, "shutdown instance must not inject on a leftover socket");
assert.deepEqual(
  sockets.flatMap((sock) => recvsOf(sock)).map((frame) => frame.id),
  recvsBeforeDead,
);
assert.ok(!recvsBeforeDead.includes("evt-dead"));
await new Promise((resolve) => setTimeout(resolve, 1100));
assert.equal(sockets.length, afterRetry, "shutdown must cancel reconnect");
console.log("Pi session_start, before_agent_start and agent_end passed against the daemon");

const primer = { type: "custom_message", ...messages[0].message };
const oldRules = { ...primer, content: `${primer.content}\nOld rules.` };
const oldPeer = { ...primer, content: primer.content.replace(peerId, "previous-peer") };
const oldCircle = { ...primer, content: primer.content.replace(/in circle [^.]+/, "in circle previous-circle") };
const unrelated = { ...primer, customType: "another-extension", content: `${primer.content}\nOther extension.` };
const compaction = { type: "compaction", summary: "Earlier conversation summarized." };
for (const [name, reason, entries, expected, history = entries] of [
  ["reopen keeps an identical primer", "startup", [primer], 0],
  ["resume keeps an identical primer", "resume", [primer], 0],
  ["reload keeps an identical primer", "reload", [primer], 0],
  ["retained compaction tail keeps its primer", "startup", [compaction, primer], 0, [primer, compaction]],
  ["latest matching primer wins over older rules and other extensions", "reload", [oldRules, primer, unrelated], 0],
  ["new session receives a primer", "new", [], 1],
  ["summarized-away primer is restored", "startup", [compaction], 1, [primer, compaction]],
  ["primer on another branch does not suppress injection", "fork", [], 1, [primer]],
  ["peer change refreshes the primer", "resume", [oldPeer], 1],
  ["circle change refreshes the primer", "resume", [oldCircle], 1],
  ["rule change refreshes the primer", "reload", [oldRules], 1],
  ["latest changed primer wins over an older match", "reload", [primer, oldRules], 1],
]) {
  await test(name, async () => {
    const freshHandlers = new Map();
    const freshMessages = [];
    const freshSockets = [];
    socketSink = freshSockets;
    AmeshHooks({
      on(event, handler) { freshHandlers.set(event, handler); },
      sendMessage(message) { freshMessages.push(message); },
      sendUserMessage() {},
      registerTool() {},
    });
    const freshContext = {
      cwd,
      sessionManager: {
        getSessionId: context.sessionManager.getSessionId,
        getEntries: () => history,
        getBranch: () => entries.includes(compaction) ? history : entries,
        buildContextEntries: () => entries,
      },
    };
    try {
      await freshHandlers.get("session_start")({ reason }, freshContext);
      await waitCount(freshSockets, 1, 2000);
      assert.equal(freshSockets.length, 1, "fresh instances must still connect");
      const connect = JSON.parse(freshSockets[0].sent[0]);
      assert.equal(connect.peer_id, peerId, "registration must retain the session identity");
      assert.equal(freshMessages.length, expected, "number of newly persisted primers");
      if (expected) {
        assert.deepEqual(freshMessages[0], messages[0].message);
      }
    } finally {
      await freshHandlers.get("session_shutdown")({}, freshContext);
    }
  });
}

let inboxSessionIndex = 0;
function InboxSession() {
  const entries = [];
  const sessionId = `pi-inbox-${++inboxSessionIndex}`;
  const sessionManager = NativeSessionManager ? NativeSessionManager.inMemory(cwd) : {
    getSessionId: () => sessionId,
    buildContextEntries: () => entries,
    appendCustomMessageEntry(customType, content, display) {
      entries.push({ type: "custom_message", customType, content, display });
    },
  };
  const ctx = { cwd, sessionManager };
  const registered = JSON.parse(execFileSync(executable, ["hook", "session", "--backend=pi"], {
    input: JSON.stringify({ cwd, session_id: sessionManager.getSessionId() }),
    encoding: "utf8",
  }));
  return { ctx, id: registered.peer_id, primer: registered.context };
}

function InboxInstance(state, failPrimer = false, hooks = AmeshHooks) {
  const handlers = new Map();
  const primers = [];
  const users = [];
  const order = [];
  const sockets = [];
  let failUser = false;
  socketSink = sockets;
  hooks({
    on(event, handler) { handlers.set(event, handler); },
    sendMessage(message) {
      if (failPrimer) {
        failPrimer = false;
        throw new Error("primer-write-fault");
      }
      primers.push(message);
      order.push("primer");
      state.ctx.sessionManager.appendCustomMessageEntry(message.customType, message.content, message.display);
    },
    sendUserMessage(content, options) {
      if (failUser) {
        failUser = false;
        throw new Error("user-write-fault");
      }
      users.push({ content, options });
      order.push("user");
    },
    registerTool() {},
  });
  return { handlers, primers, users, order, sockets, failNextUser() { failUser = true; } };
}

async function PinnedInbox(state) {
  const pinned = source.replace(/^const peerId = .*;$/m, `const peerId = ${JSON.stringify(state.id)};`);
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(pinned).toString("base64")}`);
  return InboxInstance(state, false, hooks);
}

function SendInbox(state, command, message) {
  return JSON.parse(execFileSync(executable, ["peer", command, state.id, message, "--from-peer", peerId], {
    encoding: "utf8",
  }));
}

await test("replacement drops pending messages and keeps the socket", async () => {
  const state = InboxSession();
  const instance = InboxInstance(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const socket = instance.sockets[0];
    fire(socket, { type: "notify", message: "already accepted" });
    fire(socket, { type: "ask", message: "old queued ask", correlation_id: "old-ask" });
    fire(socket, { type: "notify", message: "old queued notify" });
    assert.equal(instance.users.length, 1);
    fire(socket, { type: "replaced", peer_id: state.id });
    fire(socket, { type: "notify", message: "new session message" });
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(instance.users.length, 2);
    assert.match(instance.users[1].content, /new session message/);
    assert.doesNotMatch(instance.users[1].content, /old queued/);
    assert.equal(socket.readyState, 1);
    assert.equal(instance.sockets.length, 1);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("reconnect keeps the new session's HTTP inbox", async () => {
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    fire(instance.sockets[0], { type: "notify", message: "A accepted", id: "accept-a" });
    fire(instance.sockets[0], { type: "notify", message: "A queued", id: "queued-a" });
    instance.sockets[0].close();
    execFileSync(executable, ["hook", "session", "--backend=pi", "--peer-id", a.id], {
      input: JSON.stringify({ cwd, session_id: b.ctx.sessionManager.getSessionId() }),
    });
    SendInbox(a, "notify", "B HTTP inbox");
    await instance.handlers.get("session_start")({}, b.ctx);
    await waitCount(instance.sockets, 2, 2000);
    fire(instance.sockets[1], { type: "connected", session_id: b.ctx.sessionManager.getSessionId() });
    await instance.handlers.get("agent_settled")({}, b.ctx);
    assert.equal(instance.users.length, 2);
    assert.match(instance.users[1].content, /B HTTP inbox/);
    assert.doesNotMatch(instance.users[1].content, /A queued/);
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("a delayed old frame cannot enter the new local session", async () => {
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const socket = instance.sockets[0];
    fire(socket, { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    const starting = instance.handlers.get("session_start")({}, b.ctx);
    fire(socket, { type: "notify", message: "A during startup", id: "startup-a" });
    const duringStartup = instance.users.length;
    await starting;
    assert.equal(duringStartup, 0);
    fire(socket, { type: "notify", message: "A in transit", id: "late-a" });
    assert.equal(instance.users.length, 0);
    fire(socket, { type: "replaced", session_id: b.ctx.sessionManager.getSessionId() });
    fire(socket, { type: "notify", message: "B current", id: "current-b" });
    assert.equal(instance.users.length, 1);
    assert.match(instance.users[0].content, /B current/);
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

for (const kind of ["connected", "bound"]) {
  await test(`an unbound queue waits for ${kind} and is then delivered`, async () => {
    const state = InboxSession();
    const instance = InboxInstance(state);
    try {
      await instance.handlers.get("session_start")({}, state.ctx);
      await waitCount(instance.sockets, 1, 2000);
      let socket = instance.sockets[0];
      const session = state.ctx.sessionManager.getSessionId();
      fire(socket, { type: "connected", session_id: session });
      fire(socket, { type: "notify", message: "in flight" });
      fire(socket, { type: "connected", session_id: "" });
      fire(socket, { type: "notify", message: "unbound pending", id: "unbound" });
      await instance.handlers.get("agent_settled")({}, state.ctx);
      assert.equal(instance.users.length, 1, "unbound messages must wait");
      if (kind === "connected") {
        socket.close();
        await waitCount(instance.sockets, 2, 2500);
        socket = instance.sockets[1];
      }
      fire(socket, { type: kind, session_id: session });
      assert.equal(instance.users.length, 2, "binding must flush the retained message");
      assert.match(instance.users[1].content, /unbound pending/);
    } finally {
      await instance.handlers.get("session_shutdown")({}, state.ctx);
    }
  });
}

await test("a legacy greeting clears the previous inbound session", async () => {
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    await instance.handlers.get("session_start")({}, b.ctx);
    instance.sockets[0].close();
    await waitCount(instance.sockets, 2, 2500);
    fire(instance.sockets[1], { type: "connected" });
    fire(instance.sockets[1], { type: "notify", message: "B legacy message", id: "legacy-b" });
    assert.equal(instance.users.length, 1, "the legacy message must reach the current session");
    assert.match(instance.users[0].content, /B legacy message/);
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("replacement keeps a new session's earlier HTTP inbox", async () => {
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const socket = instance.sockets[0];
    fire(socket, { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    fire(socket, { type: "notify", message: "A in flight" });
    fire(socket, { type: "notify", message: "A stale pending" });
    execFileSync(executable, ["hook", "session", "--backend=pi", "--peer-id", a.id], {
      input: JSON.stringify({ cwd, session_id: b.ctx.sessionManager.getSessionId() }),
    });
    SendInbox(a, "notify", "B HTTP before replacement");
    await instance.handlers.get("session_start")({}, b.ctx);
    fire(socket, { type: "replaced", session_id: b.ctx.sessionManager.getSessionId() });
    await instance.handlers.get("agent_settled")({}, b.ctx);
    assert.equal(instance.users.length, 2);
    assert.match(instance.users[1].content, /B HTTP before replacement/);
    assert.doesNotMatch(instance.users[1].content, /A stale pending/);
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("a local session switch does not claim the earlier unbound queue", async () => {
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const socket = instance.sockets[0];
    fire(socket, { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    fire(socket, { type: "notify", message: "A in flight" });
    fire(socket, { type: "connected", session_id: "" });
    fire(socket, { type: "notify", message: "old unbound message" });
    await instance.handlers.get("session_start")({}, b.ctx);
    fire(socket, { type: "bound", session_id: b.ctx.sessionManager.getSessionId() });
    await instance.handlers.get("agent_settled")({}, b.ctx);
    assert.equal(instance.users.length, 1, "the local switch must discard the old unbound message");
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("an unbound queue claimed for another session waits for that local session", async () => {
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const socket = instance.sockets[0];
    fire(socket, { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    fire(socket, { type: "notify", message: "A in flight" });
    fire(socket, { type: "connected", session_id: "" });
    fire(socket, { type: "notify", message: "message for the next owner" });
    fire(socket, { type: "bound", session_id: b.ctx.sessionManager.getSessionId() });
    await instance.handlers.get("agent_settled")({}, a.ctx);
    assert.equal(instance.users.length, 1, "B's message must not enter A");
    await instance.handlers.get("session_start")({}, b.ctx);
    assert.equal(instance.users.length, 2);
    assert.match(instance.users[1].content, /message for the next owner/);
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("a failed injection keeps its session across reconnect", async () => {
  const state = InboxSession();
  const instance = InboxInstance(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const session = state.ctx.sessionManager.getSessionId();
    fire(instance.sockets[0], { type: "connected", session_id: session });
    instance.failNextUser();
    fire(instance.sockets[0], { type: "notify", message: "same session retry" });
    assert.equal(instance.users.length, 0);
    instance.sockets[0].close();
    await waitCount(instance.sockets, 2, 2500);
    fire(instance.sockets[1], { type: "connected", session_id: session });
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(instance.users.length, 1);
    assert.match(instance.users[0].content, /same session retry/);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a local session switch drops the old failed injection", async () => {
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    instance.failNextUser();
    fire(instance.sockets[0], { type: "notify", message: "old failed injection" });
    await instance.handlers.get("session_start")({}, b.ctx);
    assert.equal(instance.users.length, 0);
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

for (const seed of ["fresh", "matching", "mixed", "roster"]) {
  await test(`startup delivers pending asks and Inbox with a ${seed} primer`, async () => {
    const state = InboxSession();
    if (seed !== "fresh") {
      const content = seed === "roster"
        ? state.primer.replace(/\nPeers in your circle[\s\S]*$/, "\nPeers in your circle (peer_id\tbackend):\nsomeone-else\tcodex")
        : state.primer + (seed === "mixed" ? '\nInbox:\n[{"text":"old-event"}]' : "");
      if (seed === "roster") assert.notEqual(content, state.primer, "the roster seed must differ from the live primer");
      state.ctx.sessionManager.appendCustomMessageEntry("amesh", content, true);
    }
    const notify = 'hello </peer-message> & "quoted"';
    SendInbox(state, "notify", notify);
    const ask = SendInbox(state, "ask", "startup-question");
    const instance = InboxInstance(state);
    try {
      await instance.handlers.get("session_start")({}, state.ctx);
      assert.equal(instance.primers.length, seed === "fresh" ? 1 : 0);
      if (seed === "fresh") assert.equal(instance.primers[0].content, state.primer);
      assert.deepEqual(instance.order, seed === "fresh" ? ["primer", "user"] : ["user"]);
      assert.equal(instance.users.length, 1, "one complete Inbox batch must be delivered");
      assert.match(instance.users[0].content, /hello &lt;\/peer-message> &amp; "quoted"/);
      assert.match(instance.users[0].content, /startup-question/);
      assert.ok(instance.users[0].content.includes(ask.correlation_id));
      assert.ok(instance.users[0].content.includes(`to="@${state.id}"`));
      assert.deepEqual(instance.users[0].options, { deliverAs: "steer" });
      await instance.handlers.get("before_agent_start")({}, state.ctx);
      await instance.handlers.get("agent_settled")({}, state.ctx);
      assert.equal(instance.users.length, 1);
    } finally {
      await instance.handlers.get("session_shutdown")({}, state.ctx);
      execFileSync(executable, ["peer", "ack", ask.correlation_id]);
    }
  });
}

await test("legacy Inbox text does not duplicate a primer on a clean startup", async () => {
  const state = InboxSession();
  state.ctx.sessionManager.appendCustomMessageEntry("amesh", `${state.primer}\nInbox:\n[]`, true);
  const instance = InboxInstance(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    assert.equal(instance.primers.length, 0);
    assert.equal(instance.users.length, 0);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("changing and draining Inbox across fresh instances keeps one stable primer", async () => {
  const state = InboxSession();
  for (const text of [null, "inbox-one", "inbox-two", null]) {
    if (text) SendInbox(state, "notify", text);
    const instance = InboxInstance(state);
    try {
      await instance.handlers.get("session_start")({}, state.ctx);
      const entries = state.ctx.sessionManager.buildContextEntries();
      assert.deepEqual(entries.filter((entry) => entry.customType === "amesh").map((entry) => entry.content), [state.primer]);
      assert.equal(instance.users.length, text ? 1 : 0);
      if (text) assert.ok(instance.users[0].content.includes(text));
    } finally {
      await instance.handlers.get("session_shutdown")({}, state.ctx);
    }
  }
});

for (const event of ["before_agent_start", "agent_end"]) {
  await test(`${event} delivers claimed Inbox only after settled`, async () => {
    const state = InboxSession();
    const instance = InboxInstance(state);
    let ask;
    try {
      await instance.handlers.get("session_start")({}, state.ctx);
      ask = SendInbox(state, "ask", `${event}-question`);
      SendInbox(state, "notify", `${event}-notify`);
      await instance.handlers.get(event)({}, state.ctx);
      assert.equal(instance.users.length, 0);
      await instance.handlers.get("agent_settled")({}, state.ctx);
      assert.equal(instance.users.length, 1);
      assert.ok(instance.users[0].content.includes(`${event}-question`));
      assert.ok(instance.users[0].content.includes(`${event}-notify`));
      assert.equal(instance.primers.length, 1);
      await instance.handlers.get(event)({}, state.ctx);
      await instance.handlers.get("agent_settled")({}, state.ctx);
      assert.equal(instance.users.length, 1, "pending ledger must not redeliver an ask");
    } finally {
      await instance.handlers.get("session_shutdown")({}, state.ctx);
      if (ask) execFileSync(executable, ["peer", "ack", ask.correlation_id]);
    }
  });
}

await test("same-instance session_start consumes Inbox after intro was sent", async () => {
  const state = InboxSession();
  const instance = InboxInstance(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    SendInbox(state, "notify", "second-start");
    await instance.handlers.get("session_start")({}, state.ctx);
    assert.equal(instance.primers.length, 1);
    assert.equal(instance.users.length, 1);
    assert.match(instance.users[0].content, /second-start/);
    await instance.handlers.get("agent_settled")({}, state.ctx);
    await instance.handlers.get("session_start")({}, state.ctx);
    assert.equal(instance.users.length, 1);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("HTTP Inbox batches preserve notify-before-broadcast priority", async () => {
  const state = InboxSession();
  execFileSync(executable, ["peer", "broadcast", "inbox-broadcast", "--from-peer", peerId]);
  SendInbox(state, "notify", "inbox-priority");
  const instance = InboxInstance(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    assert.equal(instance.primers[0].content, state.primer);
    assert.equal(instance.users.length, 1);
    assert.match(instance.users[0].content, /inbox-priority/);
    assert.doesNotMatch(instance.users[0].content, /inbox-broadcast/);
    assert.deepEqual(instance.users[0].options, { deliverAs: "steer" });
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(instance.users.length, 2);
    assert.match(instance.users[1].content, /inbox-broadcast/);
    assert.deepEqual(instance.users[1].options, { deliverAs: "followUp" });
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("HTTP Inbox delivers ack replies with their correlation ID", async () => {
  const state = InboxSession();
  const ask = JSON.parse(execFileSync(executable, ["peer", "ask", peerId, "reply-question", "--from-peer", state.id], { encoding: "utf8" }));
  execFileSync(executable, ["peer", "ack", ask.correlation_id, "--message", "inbox-answer"]);
  const instance = InboxInstance(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    assert.equal(instance.users.length, 1);
    assert.match(instance.users[0].content, /type="ack"/);
    assert.match(instance.users[0].content, /inbox-answer/);
    assert.ok(instance.users[0].content.includes(ask.correlation_id));
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

for (const fault of ["context-lookup", "primer-write"]) {
  await test(`startup retains Inbox through ${fault} failure and retries`, async () => {
    const state = InboxSession();
    const manager = state.ctx.sessionManager;
    const originalBuild = manager.buildContextEntries;
    if (fault === "context-lookup") manager.buildContextEntries = () => { throw new Error("context-lookup-fault"); };
    SendInbox(state, "notify", "retained-http-inbox");
    const instance = InboxInstance(state, fault === "primer-write");
    try {
      await instance.handlers.get("session_start")({}, state.ctx);
      assert.equal(instance.primers.length, 0);
      assert.equal(instance.users.length, 0, "startup must wait for a ready primer");
      await instance.handlers.get("agent_settled")({}, state.ctx);
      assert.equal(instance.users.length, 0, "settled must preserve the queued Inbox");
      await waitCount(instance.sockets, 1, 2000);
      assert.equal(instance.sockets.length, 1);
      fire(instance.sockets[0], { id: "early-ws", type: "notify", text: "retained-ws-inbox", from_peer: peerId });
      assert.equal(instance.users.length, 0, "WebSocket must also wait for the primer");
      manager.buildContextEntries = originalBuild;
      await instance.handlers.get("session_start")({}, state.ctx);
      assert.deepEqual(instance.order, ["primer", "user"]);
      assert.equal(instance.primers[0].content, state.primer);
      assert.match(instance.users[0].content, /retained-http-inbox/);
      assert.match(instance.users[0].content, /retained-ws-inbox/);
      await instance.handlers.get("agent_settled")({}, state.ctx);
      await instance.handlers.get("session_start")({}, state.ctx);
      assert.equal(instance.primers.length, 1);
      assert.equal(instance.users.length, 1);
    } finally {
      manager.buildContextEntries = originalBuild;
      await instance.handlers.get("session_shutdown")({}, state.ctx);
    }
  });
}

await test("startup delivers Inbox when the context query API is absent", async () => {
  const state = InboxSession();
  state.ctx.sessionManager.buildContextEntries = undefined;
  SendInbox(state, "notify", "legacy-context-api");
  const instance = InboxInstance(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    assert.equal(instance.primers.length, 1);
    assert.equal(instance.primers[0].content, state.primer);
    assert.deepEqual(instance.order, ["primer", "user"]);
    assert.match(instance.users[0].content, /legacy-context-api/);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

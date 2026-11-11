import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { test as nodeTest, after } from "node:test";

if (!process.env.AMESH_STATE || !process.env.AMESH_BIND) {
  console.error(
    "amesh harness: AMESH_STATE and AMESH_BIND must both point at a sandbox, or a hand-started run writes into your home or reaches your daemon on the default bind; run this harness through cargo test",
  );
  process.exit(1);
}

/* AMESH_PI_DAEMON_ONLY keeps the session that talks to the daemon, for the run where
session_start has to start it; the socket retry checks before it are left out and the
subtests after it skipped, never cut off, so a failure still fails the run */
const childEnv = JSON.parse(execFileSync(process.execPath, ["-e", "console.log(JSON.stringify({HOME:process.env.HOME,AMESH_BIND:process.env.AMESH_BIND,AMESH_STATE:process.env.AMESH_STATE}))"], { encoding: "utf8" }));
assert.deepEqual(childEnv, { HOME: process.env.HOME, AMESH_BIND: process.env.AMESH_BIND, AMESH_STATE: process.env.AMESH_STATE });
assert.notEqual(childEnv.AMESH_BIND, "127.0.0.1:8378");

const daemonOnly = process.env.AMESH_PI_DAEMON_ONLY === "1";
let test = nodeTest;

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
const NativeWebSocket = globalThis.WebSocket;
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
/* the accept log lives under the agent directory, so every instance in this run writes into
the sandbox rather than the home that started the harness */
process.env.PI_CODING_AGENT_DIR = join(cwd, `.pi-agent-${process.pid}`);
mkdirSync(process.env.PI_CODING_AGENT_DIR, { recursive: true });
const NativeSessionManager = sessionManagerModule ? (await import(sessionManagerModule)).SessionManager : null;
const source = await readFile(extensionPath, "utf8");
const { default: AmeshHooks, ExecError } = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
const handlers = new Map();
const messages = [];
const userMsgs = [];
const tools = [];
const schemas = {};
const defs = {};
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
  registerTool(def) { tools.push(def.name); schemas[def.name] = def.parameters; defs[def.name] = def; },
});
/* the four knowledge tools register last: the original eighteen keep their order, so a
runtime that presents tools in registration order does not reshuffle what existed */
const registrationOrder = [...tools];
assert.deepEqual(registrationOrder.slice(0, 18), [
  "amesh_whoami",
  "amesh_ask",
  "amesh_ack",
  "amesh_notify_peer",
  "amesh_job_create",
  "amesh_job_list",
  "amesh_job_status",
  "amesh_job_update",
  "amesh_job_cancel",
  "amesh_job_delete",
  "amesh_schedule_create",
  "amesh_schedule_list",
  "amesh_schedule_delete",
  "amesh_ask_many",
  "amesh_wait",
  "amesh_list_peers",
  "amesh_events",
  "amesh_broadcast",
]);
assert.deepEqual(registrationOrder.slice(18), [
  "amesh_knowledge_publish",
  "amesh_knowledge_search",
  "amesh_knowledge_get",
  "amesh_knowledge_revoke",
  "amesh_knowledge_delete",
  "amesh_watch",
]);
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
  "amesh_knowledge_delete",
  "amesh_knowledge_get",
  "amesh_knowledge_publish",
  "amesh_knowledge_revoke",
  "amesh_knowledge_search",
  "amesh_list_peers",
  "amesh_notify_peer",
  "amesh_schedule_create",
  "amesh_schedule_delete",
  "amesh_schedule_list",
  "amesh_wait",
  "amesh_watch",
  "amesh_whoami",
]);
assert.equal(schemas.amesh_ask.properties.query.type, "string");
assert.equal(schemas.amesh_ask.properties.text, undefined);
assert.equal(schemas.amesh_schedule_create.properties.text.type, "string");
assert.equal(schemas.amesh_schedule_create.properties.message, undefined);
assert.equal(schemas.amesh_schedule_create.properties.cross_circle.type, "boolean");
assert.equal(schemas.amesh_ask_many.properties.cross_circle.type, "boolean");
for (const name of ["amesh_ask", "amesh_ask_many", "amesh_job_create", "amesh_job_update"]) {
  assert.equal(schemas[name].properties.watch.type, "boolean", `${name} takes watch`);
  assert.equal(
    schemas[name].properties.deadline_seconds.type,
    "integer",
    `${name} takes deadline_seconds`,
  );
}
assert.equal(schemas.amesh_list_peers.properties.detail?.type, "boolean", "installed Pi peer detail schema");
assert.equal(schemas.amesh_events.properties.circle.type, "string");
assert.equal(schemas.amesh_events.properties.cross_circle.type, "boolean");
assert.equal(schemas.amesh_events.properties.limit.type, "integer");
assert.equal(schemas.amesh_ack.properties.failed.type, "boolean");
assert.equal(schemas.amesh_job_create.properties.depends_on.type, "array");
assert.equal(schemas.amesh_job_update.properties.assigned_peer.type, "string");
assert.equal(schemas.amesh_job_update.properties.coordinator.type, "string");
assert.deepEqual(schemas.amesh_job_update.required, ["job_id"]);
assert.deepEqual(schemas.amesh_knowledge_publish.required, ["title", "body", "applies_to", "evidence"]);
assert.equal(schemas.amesh_knowledge_publish.properties.ttl_seconds.type, "integer");
assert.equal(schemas.amesh_knowledge_publish.properties.replaces.type, "string");
assert.deepEqual(schemas.amesh_knowledge_search.required, ["query"]);
assert.equal(schemas.amesh_knowledge_search.properties.limit.type, "integer");
assert.deepEqual(schemas.amesh_knowledge_get.required, ["id"]);
assert.deepEqual(schemas.amesh_knowledge_revoke.required, ["id", "reason"]);
assert.deepEqual(schemas.amesh_knowledge_delete.required, ["id"]);
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
if (!daemonOnly) {
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
if (!daemonOnly) {
  const capSockets = [];
  socketSink = capSockets;
  wsFailRemaining = -1;
  wsAttempts = 0;
  const capLogsStart = ameshLogs.length;
  /* the retry delays grow, so a five-retry run is only quick with a small base */
  process.env.AMESH_CONNECT_BACKOFF_MS = "10";
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
  delete process.env.AMESH_CONNECT_BACKOFF_MS;
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
const catalog = JSON.parse(execFileSync(executable, ["mcp", "--peer-id", peerId], {
  input: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list", params: {} }) + "\n",
  encoding: "utf8",
})).result.tools;
/* the extension fills from_peer itself, takes peer_name in place of to_peer, and its
whoami answers for its own peer */
const piOmits = { amesh_whoami: ["peer_id"] };
await test("every pi tool copies the hub catalogue", () => {
  const hub = new Map(catalog.map((entry) => [entry.name, entry]));
  for (const name of hub.keys()) {
    assert.ok(defs[name], `R1 ${name}: the hub has it, the pi copy does not`);
  }
  for (const name of Object.keys(defs)) {
    assert.ok(hub.has(name), `R1 ${name}: the pi copy has it, the hub does not`);
  }
  for (const [name, entry] of hub) {
    const schema = schemas[name];
    assert.equal(
      defs[name].description,
      entry.description,
      `R2 ${name}: description differs from the hub`,
    );
    const hubProperties = entry.inputSchema.properties ?? {};
    const piProperties = schema.properties ?? {};
    for (const [key, property] of Object.entries(piProperties)) {
      assert.ok(key in hubProperties, `R3 ${name}.${key}: only the pi copy declares it`);
      assert.deepEqual(
        property,
        hubProperties[key],
        `R3 ${name}.${key}: differs from the hub`,
      );
    }
    const omitted = new Set(["from_peer", "to_peer", ...(piOmits[name] ?? [])]);
    for (const key of Object.keys(hubProperties)) {
      if (omitted.has(key)) continue;
      assert.ok(key in piProperties, `R4 ${name}.${key}: the hub declares it, the pi copy does not`);
    }
    for (const key of entry.inputSchema.required ?? []) {
      assert.ok(
        (schema.required ?? []).includes(key),
        `R5 ${name}.${key}: the hub requires it, the pi copy does not`,
      );
    }
  }
});
await test("the ask tool forwards watch and deadline flags", async () => {
  const ask = JSON.parse(
    (
      await defs.amesh_ask.execute("ask-watch-flags", {
        peer_name: peerId,
        query: "watch flags",
        watch: false,
        deadline_seconds: 30,
      })
    ).content[0].text,
  );
  assert.equal(ask.ok, true, JSON.stringify(ask));
  const badDeadline = await defs.amesh_ask
    .execute("ask-watch-bad-deadline", {
      peer_name: peerId,
      query: "bad deadline",
      deadline_seconds: "30x",
    })
    .then(() => null, (error) => error);
  assert.match(
    String(badDeadline),
    /--deadline-seconds requires an unsigned integer/,
    "a bad deadline reaches the CLI's validation",
  );
  const badWatch = await defs.amesh_ask
    .execute("ask-watch-bad-watch", {
      peer_name: peerId,
      query: "bad watch",
      watch: "maybe",
    })
    .then(() => null, (error) => error);
  assert.match(
    String(badWatch),
    /--watch requires true or false/,
    "a bad watch reaches the CLI's validation",
  );
});
await test("knowledge tools round-trip against the daemon", async () => {
  const call = async (name, params) => {
    const result = await defs[name].execute("knowledge-round-trip", params);
    return JSON.parse(result.content[0].text);
  };
  const published = await call("amesh_knowledge_publish", {
    title: "pi tool round trip",
    body: "the four knowledge tools reach the hub",
    applies_to: "tests/pi_hook.mjs",
    evidence: "node harness against a running daemon",
  });
  assert.equal(published.ok, true, JSON.stringify(published));
  const id = published.knowledge.id;
  assert.equal(published.knowledge.status, "active");
  assert.equal(published.knowledge.publisher_peer, peerId);
  assert.equal(published.knowledge.title, "pi tool round trip");
  assert.equal(published.knowledge.body, "the four knowledge tools reach the hub");
  assert.equal(published.knowledge.applies_to, "tests/pi_hook.mjs");
  assert.equal(published.knowledge.evidence, "node harness against a running daemon");
  const found = await call("amesh_knowledge_search", { query: "round trip" });
  assert.equal(found.items[0].id, id, JSON.stringify(found));
  assert.equal(found.items[0].status, "active");
  const got = await call("amesh_knowledge_get", { id });
  assert.equal(got.knowledge.title, "pi tool round trip");
  assert.equal(got.knowledge.body, "the four knowledge tools reach the hub");
  assert.equal(got.knowledge.applies_to, "tests/pi_hook.mjs");
  assert.equal(got.knowledge.evidence, "node harness against a running daemon");
  const revoked = await call("amesh_knowledge_revoke", { id, reason: "harness done" });
  assert.equal(revoked.ok, true);
  assert.equal(revoked.id, id);
  const after = await call("amesh_knowledge_search", { query: "round trip" });
  assert.deepEqual(after.items, [], "a revoked entry leaves search");
  const withdrawn = await call("amesh_knowledge_get", { id });
  assert.equal(withdrawn.knowledge.status, "revoked");
  assert.equal(withdrawn.knowledge.revoked_by_peer, peerId);
  const bad = await defs.amesh_knowledge_search.execute("too-many-words", {
    query: "one two three four five six seven eight nine",
  }).then(() => null, (error) => error);
  assert.match(String(bad), /8 words/, "a rejected query is an error, never an empty list");
  /* a query that looks like a flag must reach the hub as a term instead of being parsed */
  const flagged = await call("amesh_knowledge_search", { query: "--timeout" });
  assert.ok(Array.isArray(flagged.items), JSON.stringify(flagged));
  /* a body that starts with -- travels inline, so the value is not taken for a flag either */
  const dashed = await call("amesh_knowledge_publish", {
    title: "dash body",
    body: "-- keep\nnotes",
    applies_to: "tests/pi_hook.mjs",
    evidence: "node harness against a running daemon",
  });
  assert.equal(dashed.ok, true, JSON.stringify(dashed));
  assert.equal(dashed.knowledge.body, "-- keep\nnotes");
});
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
await test("a settled agent reports idle when pi still says so a second later", async (t) => {
  execFileSync(executable, ["jobs", "create", "pi-activity", "--from-peer", peerId]);
  const activity = async () => {
    const response = await fetch(`http://${process.env.AMESH_BIND}/snapshot`, {
      headers: { authorization: `Bearer ${process.env.AMESH_TOKEN}` },
    });
    const snapshot = await response.json();
    return snapshot.peers.find((peer) => peer.peer_id === peerId)?.activity?.state;
  };
  /* a loaded machine can take seconds to spawn the hook and its curl */
  const reaches = async (want) => {
    for (let i = 0; i < 300 && (await activity()) !== want; i++) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return activity();
  };
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  /* a fresh instance per scenario: the earlier tests leave the shared one mid-run. Each holds
  its log owner child and its idle check until session_shutdown, and either keeps node alive,
  so every instance is shut down whether or not the scenario passes */
  let idleIndex = 0;
  const instances = [];
  t.after(async () => {
    for (const call of instances) {
      await call("session_shutdown");
    }
  });
  const instance = async (bin) => {
    process.env.PI_CODING_AGENT_DIR = join(cwd, `pi-idle-owner-${++idleIndex}`);
    const text = source.split(JSON.stringify(executable)).join(JSON.stringify(bin));
    if (bin !== executable) {
      assert.notEqual(text, source, "the extension names its executable as a JSON string");
    }
    const { default: Hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
    const on = new Map();
    Hooks({ on(event, handler) { on.set(event, handler); }, sendMessage() {}, sendUserMessage() {}, registerTool() {} });
    const call = (event, ctx = context) => on.get(event)({}, ctx);
    instances.push(call);
    return call;
  };
  /* pi's own word on whether the agent streams or has prompts waiting */
  const busy = { ...context, isIdle: () => false, hasPendingMessages: () => false };
  const done = { ...context, isIdle: () => true, hasPendingMessages: () => false };
  const waiting = { ...context, isIdle: () => true, hasPendingMessages: () => true };
  const { writeFileSync, chmodSync, existsSync, readFileSync, rmSync } = await import("node:fs");
  const real = JSON.stringify(executable);
  const wrapper = (name, body) => {
    const path = `${cwd}/${name}`;
    writeFileSync(path, `#!/bin/sh\n${body}\nexec ${real} "$@"\n`);
    chmodSync(path, 0o755);
    return path;
  };

  const live = await instance(executable);
  await live("before_agent_start");
  assert.equal(await activity(), "work");
  await live("agent_end");
  assert.equal(await activity(), "work", "agent_end can still have steers queued");
  await live("agent_settled", done);
  assert.equal(await reaches("idle"), "idle");
  await live("before_agent_start");
  await live("before_agent_start");
  await live("agent_settled", busy);
  await live("agent_settled", waiting);
  await sleep(1500);
  assert.equal(await activity(), "work", "a settle while pi streams or has prompts waiting is not idle");
  await live("agent_end");
  await live("agent_settled", done);
  assert.equal(await reaches("idle"), "idle", "idle once pi says so, however starts and ends paired");

  /* measured on pi: its run flag is already down at agent_settled and a queued prompt starts
  a run right after, so isIdle() alone reads idle mid-work */
  const idles = `${cwd}/idle-reports`;
  const counted = await instance(wrapper("counting-amesh", `[ "$2" = idle ] && echo idle >> ${JSON.stringify(idles)}`));
  await counted("before_agent_start");
  await counted("agent_settled", done);
  await counted("agent_start");
  await sleep(1500);
  assert.ok(!existsSync(idles) || readFileSync(idles, "utf8") === "", "a run that starts within the second cancels the idle report");
  assert.equal(await activity(), "work");
  await counted("agent_settled", done);
  assert.equal(await reaches("idle"), "idle");
  await counted("agent_start");
  assert.equal(await reaches("work"), "work", "a queued prompt's run, starting after the idle, reports work");

  /* the idle report is held back and leaves a mark when it has landed, so the order is
  checked after both reports, however slow the machine */
  const landed = `${cwd}/slow-idle-landed`;
  const late = await instance(
    wrapper("slow-amesh", `if [ "$2" = idle ]; then sleep 0.4; ${real} "$@"; s=$?; touch ${JSON.stringify(landed)}; exit $s; fi`),
  );
  await late("before_agent_start");
  await late("agent_end");
  await late("agent_settled", done);
  await sleep(1100);
  await late("before_agent_start");
  for (let i = 0; i < 300 && !existsSync(landed); i++) {
    await sleep(50);
  }
  assert.ok(existsSync(landed), "the held idle report ran");
  assert.equal(await activity(), "work", "a slow idle report still lands before the next prompt's");

  /* a run that starts while the idle report is landing waits for it; a shutdown meanwhile
  ends the session, so the run reports nothing */
  const after = `${cwd}/reports-after-shutdown`;
  for (const start of ["agent_start", "before_agent_start"]) {
    const closing = await instance(
      wrapper(`closing-${start}`, `[ "$2" = idle ] && sleep 0.7; [ "$2" != idle ] && echo "$2" >> ${JSON.stringify(after)}`),
    );
    await closing("before_agent_start");
    await closing("agent_settled", done);
    await sleep(1100);
    rmSync(after, { force: true });
    const run = closing(start);
    await closing("session_shutdown");
    await run;
    assert.ok(!existsSync(after), `${start} reports nothing after the session shut down`);
  }
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
assert.match(opened[0].build, /^\d+:\d+:\d+:\d+\.\d{9}$/, "the connect frame names the build that rendered the extension");
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
for (let i = 0; i < 2; i++) {
  fire(sockets[0], { type: "notify", from_peer: "amesh-codex", message: "replayed record", id: "evt-replayed" });
}
await handlers.get("agent_settled")({}, context);
const injected = userMsgs.map((msg) => (msg.content.match(/replayed record/g) || []).length).reduce((a, b) => a + b, 0);
const acked = recvsOf(sockets[0]).filter((frame) => frame.id === "evt-replayed").length;
assert.equal(`${injected},${acked}`, "1,2", "a record the hub replays after a lost recv is acknowledged again and injected once");
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
assert.ok(await waitUntil(() => sockets.length >= 2, 5000), "websocket reconnects after close");
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
if (daemonOnly) {
  test = (name, body) => nodeTest(name, { skip: true }, body);
}

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

const inboxInstances = [];
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
  inboxInstances.push({ handlers, ctx: state.ctx });
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

await test("a replacement without a session keeps the queued messages", async () => {
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
    assert.equal(instance.users.length, 2, "a replacement that names no session discards nothing");
    assert.match(instance.users[1].content, /old queued ask/);
    assert.match(instance.users[1].content, /old queued notify/);
    assert.match(instance.users[1].content, /new session message/);
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
    process.env.PI_CODING_AGENT_DIR = join(cwd, `.pi-agent-unbound-${kind}`);
    mkdirSync(process.env.PI_CODING_AGENT_DIR, { recursive: true });
    const state = InboxSession();
    const instance = await PinnedInbox(state);
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

await test("a local session switch leaves an unbound queue to the hub's binding", async () => {
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
    assert.equal(instance.users.length, 2, "the hub's binding delivers the unbound message");
    assert.match(instance.users[1].content, /old unbound message/);
    assert.doesNotMatch(instance.users[1].content, /A in flight/, "A's record never enters B");
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

await test("every connect reports activity again, so an ask to an idle pi reads not picked up", async () => {
  const state = InboxSession();
  let busy = false;
  const ctx = { ...state.ctx, isIdle: () => !busy, hasPendingMessages: () => false };
  const idle = { state: "idle", source: "pi-idle" };
  const work = { state: "work", source: "pi-work" };
  const real = [];
  const fake = globalThis.WebSocket;
  globalThis.WebSocket = class extends NativeWebSocket {
    constructor(url) {
      super(url);
      real.push(this);
    }
  };
  const view = async () => (await fetch(`http://${process.env.AMESH_BIND}/snapshot`, {
    headers: { authorization: `Bearer ${process.env.AMESH_TOKEN}` },
  })).json();
  const record = async () => {
    const found = (await view()).roster.find((peer) => peer.peer_id === state.id)?.activity;
    return found ? { state: found.state, source: found.source } : null;
  };
  /* a loaded machine can take seconds to spawn the hook */
  const reaches = async (want) => {
    for (let i = 0; i < 300 && JSON.stringify(await record()) !== JSON.stringify(want); i++) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return record();
  };
  /* the hub drops a closed socket's activity, as a restarted hub has none; the extension
  connects again a second later */
  const drop = async () => {
    real.at(-1).close();
    assert.equal(await reaches(null), null, "the hub forgets activity with the socket");
  };
  const instance = InboxInstance(state);
  let ask;
  try {
    await instance.handlers.get("session_start")({}, ctx);
    assert.deepEqual(await reaches(idle), idle, "the first connect reports an idle pi");
    await drop();
    assert.deepEqual(await reaches(idle), idle, "a reconnect reports idle again");
    busy = true;
    await drop();
    assert.deepEqual(await reaches(work), work, "a reconnect while pi runs reports work");
    busy = false;
    assert.deepEqual(await reaches(idle), idle, "then idle once pi is idle");
    /* the hub stamps whole seconds, and an ask opened in the idle report's second reads
    ask_still_open */
    await new Promise((resolve) => setTimeout(resolve, 1100));
    ask = SendInbox(state, "ask", "reconnect-question");
    let progress;
    for (let i = 0; i < 300 && progress?.kind !== "idle"; i++) {
      await new Promise((resolve) => setTimeout(resolve, 50));
      progress = (await view()).asks.find((row) => row.correlation_id === ask.correlation_id)?.state?.progress;
    }
    assert.deepEqual([progress?.kind, progress?.why], ["idle", "not_picked_up"], "an ask the idle pi leaves alone");
  } finally {
    await instance.handlers.get("session_shutdown")({}, ctx);
    globalThis.WebSocket = fake;
    if (ask) execFileSync(executable, ["peer", "ack", ask.correlation_id]);
  }
});

await test("a report that fails on connect is logged, never thrown", async () => {
  const state = InboxSession();
  const { writeFileSync, chmodSync } = await import("node:fs");
  const failing = `${cwd}/failing-report-amesh`;
  writeFileSync(failing, `#!/bin/sh\n[ "$2" = work ] && { echo report-refused >&2; exit 1; }\nexec ${JSON.stringify(executable)} "$@"\n`);
  chmodSync(failing, 0o755);
  const text = source.split(JSON.stringify(executable)).join(JSON.stringify(failing));
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
  const instance = InboxInstance(state, false, hooks);
  const busy = { ...state.ctx, isIdle: () => false, hasPendingMessages: () => false };
  const from = ameshLogs.length;
  try {
    await instance.handlers.get("session_start")({}, busy);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "connected", session_id: state.ctx.sessionManager.getSessionId() });
    assert.ok(
      await waitUntil(() => ameshLogs.slice(from).some((line) => line.includes("report-refused")), 5000),
      "the refused work report is logged",
    );
  } finally {
    await instance.handlers.get("session_shutdown")({}, busy);
  }
});

await test("a hook child that exits before reading its input is logged, never thrown", async () => {
  const state = InboxSession();
  const { writeFileSync, chmodSync } = await import("node:fs");
  const early = `${cwd}/early-hook-amesh`;
  writeFileSync(early, `#!/bin/sh\n[ "$1" = hook ] && [ "$2" != hold ] && { echo hook-refused >&2; exit 1; }\nexec ${JSON.stringify(executable)} "$@"\n`);
  chmodSync(early, 0o755);
  const text = source.split(JSON.stringify(executable)).join(JSON.stringify(early));
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
  const instance = InboxInstance(state, false, hooks);
  /* a cwd larger than a pipe buffer keeps the hook input unwritten until the child is gone */
  const wide = { ...state.ctx, cwd: `${cwd}/${"x".repeat(256 * 1024)}` };
  const from = ameshLogs.length;
  try {
    await instance.handlers.get("session_start")({}, wide);
    assert.ok(
      ameshLogs.slice(from).some((line) => line.includes("hook-refused")),
      "the refused session report is logged",
    );
  } finally {
    await instance.handlers.get("session_shutdown")({}, wide);
  }
});

await test("a tool child that exits before reading its input rejects, never throws", async () => {
  const { writeFileSync, chmodSync } = await import("node:fs");
  const early = `${cwd}/early-mcp-amesh`;
  writeFileSync(early, `#!/bin/sh\n[ "$1" = mcp ] && { echo mcp-refused >&2; exit 3; }\nexec ${JSON.stringify(executable)} "$@"\n`);
  chmodSync(early, 0o755);
  const text = source.split(JSON.stringify(executable)).join(JSON.stringify(early));
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
  const registered = {};
  hooks({ on() {}, sendMessage() {}, sendUserMessage() {}, registerTool(def) { registered[def.name] = def; } });
  /* a request larger than a pipe buffer is still being written when the child exits */
  const call = registered.amesh_job_create.execute("early", { title: "x".repeat(256 * 1024) });
  await assert.rejects(call, /mcp-refused/);
});

await test("a consumed steer lets the next batch out inside the same run", async () => {
  const state = InboxSession();
  wsFailRemaining = 0;
  const instance = InboxInstance(state);
  const delivered = (content, role = "user") =>
    instance.handlers.get("message_start")({ type: "message_start", message: { role, content } });
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 4000);
    fire(instance.sockets[0], { type: "connected", session_id: state.ctx.sessionManager.getSessionId() });
    fire(instance.sockets[0], { type: "notify", from_peer: "amesh-codex", message: "m1-one", id: "m1-e1" });
    assert.equal(instance.users.length, 1, "the first batch goes out at once");
    fire(instance.sockets[0], { type: "notify", from_peer: "amesh-codex", message: "m1-two", id: "m1-e2" });
    assert.equal(instance.users.length, 1, "the second batch waits while the first is in flight");
    const first = instance.users[0].content;
    await delivered(first, "assistant");
    assert.equal(instance.users.length, 1, "an assistant message must not open the gate");
    await delivered("some other user text");
    assert.equal(instance.users.length, 1, "a different user text must not open the gate");
    await delivered([{ type: "text", text: first }]);
    assert.equal(instance.users.length, 2, "the consumed batch opens the gate inside the run");
    assert.match(instance.users[1].content, /m1-two/);
    await delivered([{ type: "text", text: first }]);
    assert.equal(instance.users.length, 2, "one batch opens the gate once");
    fire(instance.sockets[0], { type: "notify", from_peer: "amesh-codex", message: "m1-three", id: "m1-e3" });
    assert.equal(instance.users.length, 2, "the new batch is in flight again");
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(instance.users.length, 3, "the settle fallback delivers what was held");
    assert.match(instance.users[2].content, /m1-three/);
    fire(instance.sockets[0], { type: "notify", from_peer: "amesh-codex", message: "m1-four", id: "m1-e4" });
    fire(instance.sockets[0], { type: "notify", from_peer: "amesh-codex", message: "m1-five", id: "m1-e5" });
    assert.equal(instance.users.length, 3, "both wait behind the batch the settle just delivered");
    const other = {
      ...state.ctx,
      sessionManager: { ...state.ctx.sessionManager, getSessionId: () => "pi-other-session" },
    };
    await instance.handlers.get("before_agent_start")({}, other);
    await delivered([{ type: "text", text: instance.users[2].content }]);
    assert.equal(instance.users.length, 3, "a session change drops the in-flight record");
    await instance.handlers.get("agent_settled")({}, other);
    assert.equal(instance.users.length, 3, "a batch queued for the old session is gone");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
    wsFailRemaining = 1;
  }
});

await test("a settle reminder fires once per chain and only for a completed run", async () => {
  const state = InboxSession();
  const instance = InboxInstance(state);
  const boundary = (over = {}) => ({
    type: "agent_before_settle",
    outcome: "completed",
    entries: [],
    context: { canContinue: false },
    ...over,
  });
  let ask = SendInbox(state, "ask", "m2-pending");
  try {
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    const first = await instance.handlers.get("agent_before_settle")(boundary(), state.ctx);
    assert.equal(first?.continue, true, "an open ask continues the run");
    assert.equal(first.entries.length, 1, "exactly one draft is added");
    assert.equal(first.entries[0].type, "custom_message");
    assert.match(first.entries[0].content, new RegExp(ask.correlation_id));
    await instance.handlers.get("agent_start")({}, state.ctx);
    const again = await instance.handlers.get("agent_before_settle")(boundary(), state.ctx);
    assert.equal(again, undefined, "one reminder per chain, not once per continuation");
    const kept = await instance.handlers.get("agent_before_settle")(
      boundary({ entries: [{ type: "custom_message", customType: "other", content: "keep", display: true }] }),
      state.ctx,
    );
    assert.equal(kept, undefined, "a reminded chain adds nothing after the reminder");
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    const next = await instance.handlers.get("agent_before_settle")(
      boundary({ entries: [{ type: "custom_message", customType: "other", content: "keep", display: true }] }),
      state.ctx,
    );
    assert.equal(next?.continue, true, "a new chain reminds again");
    assert.equal(next.entries.length, 2, "existing drafts are kept");
    assert.equal(next.entries[0].content, "keep");
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    const aborted = await instance.handlers.get("agent_before_settle")(boundary({ outcome: "aborted" }), state.ctx);
    assert.equal(aborted, undefined, "an aborted run is not reminded");
    const afterAbort = await instance.handlers.get("agent_before_settle")(boundary(), state.ctx);
    assert.equal(afterAbort?.continue, true, "the aborted check did not consume the reminder");
    execFileSync(executable, ["peer", "ack", ask.correlation_id], { encoding: "utf8" });
    ask = null;
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    const closed = await instance.handlers.get("agent_before_settle")(boundary(), state.ctx);
    assert.equal(closed, undefined, "no open ask, no reminder");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
    if (ask) execFileSync(executable, ["peer", "ack", ask.correlation_id], { encoding: "utf8" });
  }
});

await test("the reminder re-reads the asks and keeps the inbox it took", async () => {
  const state = InboxSession();
  wsFailRemaining = 0;
  const instance = InboxInstance(state);
  let stale = SendInbox(state, "ask", "m2-stale");
  let fresh;
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 4000);
    fire(instance.sockets[0], { type: "connected", session_id: state.ctx.sessionManager.getSessionId() });
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    execFileSync(executable, ["peer", "ack", stale.correlation_id], { encoding: "utf8" });
    stale = null;
    fresh = SendInbox(state, "ask", "m2-fresh");
    const reminder = await instance.handlers.get("agent_before_settle")(
      { type: "agent_before_settle", outcome: "completed", entries: [], context: { canContinue: false } },
      state.ctx,
    );
    assert.equal(reminder?.continue, true, "the fresh ask is still reminded");
    assert.match(reminder.entries[0].content, new RegExp(fresh.correlation_id));
    const deliveredBefore = instance.users.length;
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    SendInbox(state, "notify", "m2-inbox");
    const late = await instance.handlers.get("agent_before_settle")(
      { type: "agent_before_settle", outcome: "completed", entries: [], context: { canContinue: false } },
      state.ctx,
    );
    assert.equal(late?.continue, true, "the still-open ask reminds again on the new chain");
    await instance.handlers.get("agent_settled")({}, state.ctx);
    const delivered = instance.users.slice(deliveredBefore).map((item) => item.content).join("\n");
    assert.match(delivered, /m2-inbox/, "the inbox the second read took is still delivered");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
    wsFailRemaining = 1;
    if (stale) execFileSync(executable, ["peer", "ack", stale.correlation_id], { encoding: "utf8" });
    if (fresh) execFileSync(executable, ["peer", "ack", fresh.correlation_id], { encoding: "utf8" });
  }
});

await test("a late ack drops the reminder but keeps the inbox the read took", async () => {
  const state = InboxSession();
  wsFailRemaining = 0;
  const instance = InboxInstance(state);
  let ask = SendInbox(state, "ask", "r2-late-ack");
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 4000);
    fire(instance.sockets[0], { type: "connected", session_id: state.ctx.sessionManager.getSessionId() });
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    execFileSync(executable, ["peer", "ack", ask.correlation_id], { encoding: "utf8" });
    ask = null;
    SendInbox(state, "notify", "r2-late-inbox");
    const result = await instance.handlers.get("agent_before_settle")(
      { type: "agent_before_settle", outcome: "completed", entries: [], context: { canContinue: false } },
      state.ctx,
    );
    assert.equal(result, undefined, "a late ack prevents a stale reminder");
    await instance.handlers.get("agent_settled")({}, state.ctx);
    const delivered = instance.users.map((item) => item.content).join("\n");
    assert.match(delivered, /r2-late-inbox/, "a now-empty pending read still delivers its inbox");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
    wsFailRemaining = 1;
    if (ask) execFileSync(executable, ["peer", "ack", ask.correlation_id], { encoding: "utf8" });
  }
});

await test("a session switch during the re-read keeps the reminder out", async () => {
  const { writeFileSync, chmodSync, existsSync, rmSync } = await import("node:fs");
  const statePath = `${cwd}/parity-stop-state.json`;
  const fake = `${cwd}/parity-stop-amesh`;
  const started = `${cwd}/parity-stop-started`;
  const release = `${cwd}/parity-stop-release`;
  rmSync(started, { force: true });
  rmSync(release, { force: true });
  writeFileSync(
    fake,
    `#!/usr/bin/env node
const fs = require("node:fs");
const state = JSON.parse(fs.readFileSync(${JSON.stringify(statePath)}, "utf8"));
const verb = process.argv[3];
if (verb === "stop" && state.gate) {
  fs.writeFileSync(${JSON.stringify(started)}, "");
  const end = Date.now() + 10000;
  while (!fs.existsSync(${JSON.stringify(release)}) && Date.now() < end) {}
  if (!fs.existsSync(${JSON.stringify(release)})) {
    process.exit(3);
  }
}
process.stdout.write(JSON.stringify(state[verb] ?? {}));
`,
  );
  chmodSync(fake, 0o755);
  const answers = {
    stop: { peer_id: "parity-fake", context: "reminder text", pending: [{ correlation_id: "ask-fake" }], inbox: [] },
    prompt: { peer_id: "parity-fake", context: "", inbox: [] },
    work: {},
    idle: {},
  };
  writeFileSync(statePath, JSON.stringify(answers));
  const patched = source.split(JSON.stringify(executable)).join(JSON.stringify(fake));
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(patched).toString("base64")}`);
  const state = InboxSession();
  const instance = InboxInstance(state, false, hooks);
  const boundary = { type: "agent_before_settle", outcome: "completed", entries: [], context: { canContinue: false } };
  const other = {
    ...state.ctx,
    sessionManager: { ...state.ctx.sessionManager, getSessionId: () => "parity-other-session" },
  };
  try {
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_end")({}, state.ctx);
    writeFileSync(statePath, JSON.stringify({ ...answers, gate: true }));
    const pending = instance.handlers.get("agent_before_settle")(boundary, state.ctx);
    assert.equal(
      await waitUntil(() => existsSync(started), 10000),
      true,
      "the re-read is in flight before the session switch",
    );
    await instance.handlers.get("before_agent_start")({}, other);
    writeFileSync(release, "");
    const result = await pending;
    assert.equal(result, undefined, "a session switch during the re-read drops the reminder");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
    rmSync(started, { force: true });
    rmSync(release, { force: true });
  }
});

/* the accept log must survive a crash between the socket ack and the injection, and must be
the one place both ingresses dedupe against */
let inboxLogIndex = 0;
function InboxLog(cwd) {
  const dir = join(cwd, `.pi-agent-log-${process.pid}-${++inboxLogIndex}`);
  mkdirSync(dir, { recursive: true });
  process.env.PI_CODING_AGENT_DIR = dir;
  return dir;
}

function InboxLogFile(dir, peer) {
  return join(dir, "amesh-inbox", `${peer}.jsonl`);
}

function Injected(instance, text) {
  return instance.users.filter((message) => message.content.includes(text)).length;
}

function AskVisible(peer, correlationId) {
  try {
    const out = execFileSync(executable, ["peer", "asks", "--peer-id", peer], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    return out.includes(correlationId);
  } catch {
    return false;
  }
}

await test("a record acknowledged before the crash is replayed from the accept log exactly once", async () => {
  InboxLog(cwd);
  const state = InboxSession();
  const first = await PinnedInbox(state);
  await first.handlers.get("session_start")({}, state.ctx);
  await waitCount(first.sockets, 1, 2000);
  fire(first.sockets[0], { type: "connected", session_id: state.ctx.sessionManager.getSessionId() });
  first.failNextUser();
  fire(first.sockets[0], { type: "notify", message: "survives the crash", id: "evt-crash" });
  assert.equal(Injected(first, "survives the crash"), 0, "the crash window: acknowledged, not injected");
  assert.equal(
    recvsOf(first.sockets[0]).filter((frame) => frame.id === "evt-crash").length,
    1,
    "the recv went out before the crash",
  );
  await first.handlers.get("session_shutdown")({}, state.ctx);
  const second = await PinnedInbox(state);
  await second.handlers.get("session_start")({}, state.ctx);
  await waitCount(second.sockets, 1, 2000);
  assert.equal(Injected(second, "survives the crash"), 1, "the log replays the record once");
  await second.handlers.get("session_shutdown")({}, state.ctx);
  const third = await PinnedInbox(state);
  await third.handlers.get("session_start")({}, state.ctx);
  await waitCount(third.sockets, 1, 2000);
  await third.handlers.get("agent_settled")({}, state.ctx);
  assert.equal(Injected(third, "survives the crash"), 0, "a completed record never replays");
  await third.handlers.get("session_shutdown")({}, state.ctx);
  await second.handlers.get("session_shutdown")({}, state.ctx);
  await first.handlers.get("session_shutdown")({}, state.ctx);
});

await test("one record drained over HTTP and pushed over the socket is injected once", async () => {
  InboxLog(cwd);
  const state = InboxSession();
  const instance = await PinnedInbox(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const sent = SendInbox(state, "notify", "one ingress");
    await instance.handlers.get("before_agent_start")({}, state.ctx);
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(Injected(instance, "one ingress"), 1, "the HTTP drain delivers the record");
    fire(instance.sockets[0], { type: "notify", message: "one ingress", id: sent.id });
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(Injected(instance, "one ingress"), 1, "the socket copy of the same id must not inject twice");
    assert.equal(
      recvsOf(instance.sockets[0]).filter((frame) => frame.id === sent.id).length,
      1,
      "the socket copy is still acknowledged",
    );
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a record for another session is held and injected when that session returns", async () => {
  InboxLog(cwd);
  const a = InboxSession();
  const b = InboxSession();
  const instance = await PinnedInbox(a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "connected", session_id: a.ctx.sessionManager.getSessionId() });
    instance.failNextUser();
    fire(instance.sockets[0], { type: "notify", message: "held for A", id: "held-a" });
    assert.equal(Injected(instance, "held for A"), 0);
    await instance.handlers.get("session_start")({}, b.ctx);
    await instance.handlers.get("agent_settled")({}, b.ctx);
    assert.equal(Injected(instance, "held for A"), 0, "A's record never enters B");
    await instance.handlers.get("session_start")({}, a.ctx);
    assert.equal(Injected(instance, "held for A"), 1, "returning to A injects the held record");
  } finally {
    await instance.handlers.get("session_shutdown")({}, a.ctx);
  }
});

await test("a held ask past the retention window is closed with a failed ack", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const ask = SendInbox(state, "ask", "held ask");
  assert.equal(
    await waitUntil(() => AskVisible(state.id, ask.correlation_id), 4000),
    true,
    "the ask is open before the replay",
  );
  const file = InboxLogFile(dir, state.id);
  mkdirSync(dirname(file), { recursive: true });
  const stale = Math.floor(Date.now() / 1000) - 2 * 24 * 3600;
  const row = {
    t: "a",
    id: "evt-stale",
    at: stale,
    session: state.ctx.sessionManager.getSessionId(),
    type: "ask",
    cid: ask.correlation_id,
    text: "held ask",
  };
  writeFileSync(file, `${JSON.stringify(row)}\n`);
  const instance = await PinnedInbox(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(Injected(instance, "held ask"), 0, "a record past the window is never injected");
    assert.ok(
      ameshLogs.some((line) => line.includes("dropped 1 held message")),
      "one log line names the drop",
    );
    assert.equal(
      await waitUntil(() => !AskVisible(state.id, ask.correlation_id), 6000),
      true,
      "the ask is closed",
    );
    const closed = JSON.parse(
      execFileSync(executable, ["peer", "wait", ask.correlation_id, "--timeout-seconds", "1"], { encoding: "utf8" }),
    );
    assert.deepEqual([closed.open, closed.failed], [false, true], "the ask is closed as failed");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("connect retries back off up to a cap instead of every second", async () => {
  InboxLog(cwd);
  process.env.AMESH_CONNECT_BACKOFF_MS = "40";
  process.env.AMESH_CONNECT_BACKOFF_MAX_MS = "160";
  const stamps = [];
  const Native = globalThis.WebSocket;
  globalThis.WebSocket = class {
    constructor() {
      stamps.push(Date.now());
      throw new Error("connect-backoff-probe");
    }
  };
  const handlers = new Map();
  try {
    AmeshHooks({
      on(event, handler) { handlers.set(event, handler); },
      sendMessage() {},
      sendUserMessage() {},
      registerTool() {},
    });
    await handlers.get("session_start")({}, { ...context, cwd });
    assert.equal(
      await waitUntil(() => stamps.length >= 4, 5000),
      true,
      "the retry loop keeps trying",
    );
    const gaps = stamps.slice(1).map((at, i) => at - stamps[i]);
    assert.ok(gaps[0] <= 60, `the first retry waits for the backoff base (${gaps.join(",")})`);
    assert.ok(gaps[1] >= gaps[0] * 0.9, `the second wait grows (${gaps.join(",")})`);
    assert.ok(gaps.length < 3 || gaps[2] >= gaps[1] * 0.9, `the third wait grows (${gaps.join(",")})`);
    assert.ok(Math.max(...gaps) <= 200, `the wait stays capped (${gaps.join(",")})`);
    await handlers.get("session_shutdown")({}, { ...context, cwd });
  } finally {
    globalThis.WebSocket = Native;
    delete process.env.AMESH_CONNECT_BACKOFF_MS;
    delete process.env.AMESH_CONNECT_BACKOFF_MAX_MS;
  }
});

await test("a socket connect without an identity names it once", async () => {
  InboxLog(cwd);
  const handlers = new Map();
  const missing = join(cwd, "missing-cwd-for-identity");
  const before = ameshLogs.length;
  AmeshHooks({
    on(event, handler) { handlers.set(event, handler); },
    sendMessage() {},
    sendUserMessage() {},
    registerTool() {},
  });
  await handlers.get("session_start")({}, { ...context, cwd: missing, sessionManager: { getSessionId: () => "pi-identity-probe", buildContextEntries: () => [] } });
  assert.equal(
    await waitUntil(() => ameshLogs.slice(before).some((line) => line.includes("no peer id")), 4000),
    true,
    "the missing identity is reported",
  );
  await handlers.get("session_shutdown")({}, context);
  assert.equal(
    ameshLogs.slice(before).filter((line) => line.includes("no peer id")).length,
    1,
    "the line is not repeated on every retry",
  );
});

/* the extension with a writeSync that fails like a full disk on writes carrying marker,
every time or only the first time */
async function FailingWrite(marker, once) {
  const text = source.replace("  writeSync,", "  writeSync as NativeWriteSync,") + `
let failing = true;
function writeSync(fd, text, ...rest) {
  if (failing && typeof text === "string" && text.includes(${JSON.stringify(marker)})) {
    failing = ${!once};
    throw Object.assign(new Error("full log"), { code: "ENOSPC" });
  }
  return NativeWriteSync(fd, text, ...rest);
}`;
  return (await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`)).default;
}

await test("a record whose log write fails is not acknowledged", async () => {
  InboxLog(cwd);
  const hooks = await FailingWrite('"id":"evt-nolog"', false);
  const state = InboxSession();
  const instance = InboxInstance(state, false, hooks);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "notify", message: "cannot be logged", id: "evt-nolog" });
    assert.equal(
      recvsOf(instance.sockets[0]).filter((frame) => frame.id === "evt-nolog").length,
      0,
      "the recv must not go out before the record is on disk",
    );
    assert.equal(Injected(instance, "cannot be logged"), 0);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

/* the accept log is the only copy between the socket ack and the injection, so its own
failure modes are part of the delivery contract */
function InboxRows(dir, peer) {
  const file = InboxLogFile(dir, peer);
  const raw = existsSync(file) ? readFileSync(file, "utf8") : "";
  return raw
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch {
        return { t: "torn" };
      }
    });
}

function InboxSessionAt(directory) {
  mkdirSync(directory, { recursive: true });
  const entries = [];
  const sessionId = `pi-inbox-${++inboxSessionIndex}`;
  const sessionManager = {
    getSessionId: () => sessionId,
    buildContextEntries: () => entries,
    appendCustomMessageEntry(customType, content, display) {
      entries.push({ type: "custom_message", customType, content, display });
    },
  };
  const registered = JSON.parse(execFileSync(executable, ["hook", "session", "--backend=pi"], {
    input: JSON.stringify({ cwd: directory, session_id: sessionId }),
    encoding: "utf8",
  }));
  return { ctx: { cwd: directory, sessionManager }, id: registered.peer_id };
}

let inboxExecIndex = 0;
function InboxExec(marker) {
  const path = join(cwd, `pi-exec-${process.pid}-${++inboxExecIndex}.sh`);
  writeFileSync(path, `#!/bin/sh\n[ "$2" = session ] && echo session >> '${marker}'\nexec '${executable}' "$@"\n`);
  chmodSync(path, 0o755);
  return path;
}

async function InboxWithExec(bin, state, failPrimer = false) {
  const text = source.split(JSON.stringify(executable)).join(JSON.stringify(bin));
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
  return InboxInstance(state, failPrimer, hooks);
}

await test("a torn tail does not swallow the record accepted after it", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const file = InboxLogFile(dir, state.id);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, `{"t":"a","id":"evt-torn","at":${Math.floor(Date.now() / 1000)}`);
  const first = await PinnedInbox(state);
  await first.handlers.get("session_start")({}, state.ctx);
  await waitCount(first.sockets, 1, 2000);
  first.failNextUser();
  fire(first.sockets[0], { type: "notify", message: "after the torn tail", id: "evt-after" });
  assert.equal(
    recvsOf(first.sockets[0]).filter((frame) => frame.id === "evt-after").length,
    1,
    "the record is acknowledged before the crash",
  );
  await first.handlers.get("session_shutdown")({}, state.ctx);
  const second = await PinnedInbox(state);
  await second.handlers.get("session_start")({}, state.ctx);
  await waitCount(second.sockets, 1, 2000);
  assert.equal(Injected(second, "after the torn tail"), 1, "the acknowledged record replays after the torn tail");
  await second.handlers.get("session_shutdown")({}, state.ctx);
  await first.handlers.get("session_shutdown")({}, state.ctx);
});

await test("a new peer id writes its own log and keeps it across a restart", async () => {
  const dir = InboxLog(cwd);
  const a = await InboxSessionAt(join(cwd, "pi-identity-a"));
  const b = await InboxSessionAt(join(cwd, "pi-identity-b"));
  assert.notEqual(a.id, b.id, "two folders are two peers");
  /* unpinned, so the hub names this session by the folder it runs in */
  const first = await InboxWithExec(executable, a);
  await first.handlers.get("session_start")({}, a.ctx);
  await waitCount(first.sockets, 1, 2000);
  await first.handlers.get("session_start")({}, b.ctx);
  assert.equal(
    existsSync(InboxLogFile(dir, b.id)),
    true,
    "the new identity has a log of its own",
  );
  await waitCount(first.sockets, 2, 2000);
  first.failNextUser();
  fire(first.sockets.at(-1), { type: "notify", message: "accepted as B", id: "evt-carried" });
  assert.equal(
    recvsOf(first.sockets.at(-1)).filter((frame) => frame.id === "evt-carried").length,
    1,
    "the record is acknowledged under the new identity",
  );
  assert.equal(
    InboxRows(dir, a.id).some((row) => row.id === "evt-carried"),
    false,
    "the old identity's log does not take it",
  );
  await first.handlers.get("session_shutdown")({}, b.ctx);
  const second = await PinnedInbox(b);
  await second.handlers.get("session_start")({}, b.ctx);
  await waitCount(second.sockets, 1, 2000);
  assert.equal(Injected(second, "accepted as B"), 1, "the new identity replays its record after the restart");
  await second.handlers.get("session_shutdown")({}, b.ctx);
  await first.handlers.get("session_shutdown")({}, b.ctx);
});

await test("a session claimed before a crash stays that session's", async () => {
  InboxLog(cwd);
  const a = InboxSession();
  const b = InboxSession();
  const first = await PinnedInbox(a);
  await first.handlers.get("session_start")({}, a.ctx);
  await waitCount(first.sockets, 1, 2000);
  fire(first.sockets[0], { type: "connected", session_id: "" });
  fire(first.sockets[0], { type: "notify", message: "claimed once", id: "evt-claim" });
  first.failNextUser();
  fire(first.sockets[0], { type: "bound", session_id: a.ctx.sessionManager.getSessionId() });
  assert.equal(Injected(first, "claimed once"), 0, "the injection failed, so the record is still held");
  await first.handlers.get("session_shutdown")({}, a.ctx);
  const second = await PinnedInbox(a);
  await second.handlers.get("session_start")({}, b.ctx);
  await waitCount(second.sockets, 1, 2000);
  fire(second.sockets[1] ?? second.sockets[0], { type: "bound", session_id: b.ctx.sessionManager.getSessionId() });
  await second.handlers.get("agent_settled")({}, b.ctx);
  assert.equal(Injected(second, "claimed once"), 0, "the record belongs to the session that claimed it");
  await second.handlers.get("session_start")({}, a.ctx);
  assert.equal(Injected(second, "claimed once"), 1, "and it is delivered when that session returns");
  await second.handlers.get("session_shutdown")({}, a.ctx);
  await first.handlers.get("session_shutdown")({}, a.ctx);
});

await test("an unknown peer puts the registration back", async () => {
  InboxLog(cwd);
  const marker = join(cwd, `pi-exec-${process.pid}.marker`);
  writeFileSync(marker, "");
  const state = InboxSession();
  const instance = await InboxWithExec(InboxExec(marker), state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const before = readFileSync(marker, "utf8").split("\n").filter(Boolean).length;
    fire(instance.sockets[0], { error: "unknown peer", peers: [] });
    assert.equal(
      await waitUntil(() => readFileSync(marker, "utf8").split("\n").filter(Boolean).length > before, 4000),
      true,
      "the drainer must re-register without a manual hook",
    );
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a done row alone keeps its record out", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const file = InboxLogFile(dir, state.id);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, `${JSON.stringify({ t: "d", id: "evt-done-only", at: Math.floor(Date.now() / 1000) })}\n`);
  const instance = await PinnedInbox(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "notify", message: "must not reappear", id: "evt-done-only" });
    assert.equal(
      recvsOf(instance.sockets[0]).filter((frame) => frame.id === "evt-done-only").length,
      1,
      "a tombstone still acknowledges the replay",
    );
    await instance.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(Injected(instance, "must not reappear"), 0, "a completed id is never injected again");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a stale staged file is never renamed into the log", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const file = InboxLogFile(dir, state.id);
  mkdirSync(dirname(file), { recursive: true });
  const stale = { t: "a", id: "evt-stale", at: Math.floor(Date.now() / 1000), type: "notify", text: "stale" };
  writeFileSync(`${file}.tmp`, `${JSON.stringify(stale)}\n`);
  const old = Math.floor(Date.now() / 1000) - 8 * 24 * 3600;
  writeFileSync(file, `${" ".repeat(8 * 1024 * 1024)}\n${JSON.stringify({ t: "d", id: "evt-old", at: old })}\n`);
  const instance = await PinnedInbox(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const rows = InboxRows(dir, state.id);
    assert.equal(rows.some((row) => row.id === "evt-stale"), false, "the stale snapshot must not become the log");
    assert.ok(statSync(file).size < 8 * 1024 * 1024, "the rewritten log is not the old oversized one");
    assert.equal(Injected(instance, "stale"), 0);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a migration replaces the target by rename and keeps what it held", async () => {
  const dir = InboxLog(cwd);
  const a = await InboxSessionAt(join(cwd, "pi-f2-rename-a"));
  const b = await InboxSessionAt(join(cwd, "pi-f2-rename-b"));
  const instance = await InboxWithExec(executable, a);
  try {
    await instance.handlers.get("session_start")({}, a.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const target = InboxLogFile(dir, b.id);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, `${JSON.stringify({ t: "d", id: "evt-target-done", at: Math.floor(Date.now() / 1000) })}\n`);
    const before = statSync(target).ino;
    await instance.handlers.get("session_start")({}, b.ctx);
    assert.notEqual(
      statSync(target).ino,
      before,
      "the target is replaced by rename, never truncated in place",
    );
    assert.ok(
      InboxRows(dir, b.id).some((row) => row.id === "evt-target-done"),
      "the target's own row survives the move",
    );
  } finally {
    await instance.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("a rewrite never leaves more done ids than the bound", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const file = InboxLogFile(dir, state.id);
  mkdirSync(dirname(file), { recursive: true });
  const at = Math.floor(Date.now() / 1000);
  const rows = [];
  for (let i = 0; i < 600; i += 1) {
    rows.push(JSON.stringify({ t: "d", id: `evt-done-${i}`, at }));
  }
  writeFileSync(file, `${rows.join("\n")}\n`);
  const instance = await PinnedInbox(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const done = InboxRows(dir, state.id).filter((row) => row.t === "d").length;
    assert.ok(done <= 512, `the rewritten file holds the bound, not the merge: ${done} done ids`);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a claim that cannot be written leaves the record unbound", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const hooks = await FailingWrite('"t":"s"', true);
  const instance = InboxInstance(state, false, hooks);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    const session = state.ctx.sessionManager.getSessionId();
    fire(instance.sockets[0], { type: "connected", session_id: "" });
    fire(instance.sockets[0], { type: "notify", message: "owned by A", id: "evt-claim" });
    fire(instance.sockets[0], { type: "bound", session_id: session });
    assert.equal(Injected(instance, "owned by A"), 0, "an unwritten claim must not deliver");
    assert.equal(
      InboxRows(dir, state.id).some((row) => row.id === "evt-claim" && row.t === "s"),
      false,
      "and leaves no claim on disk",
    );
    fire(instance.sockets[0], { type: "bound", session_id: session });
    assert.equal(Injected(instance, "owned by A"), 1, "the retry claims it and only then delivers it");
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a re-registration connects at once instead of waiting out the backoff", async () => {
  InboxLog(cwd);
  process.env.AMESH_CONNECT_BACKOFF_MS = "12000";
  process.env.AMESH_CONNECT_BACKOFF_MAX_MS = "12000";
  const state = InboxSession();
  const instance = await PinnedInbox(state);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { error: "unknown peer", peers: [] });
    instance.sockets[0].close();
    assert.equal(
      await waitUntil(() => instance.sockets.length >= 2, 2000),
      true,
      "the fresh registration is used by a socket at once",
    );
  } finally {
    delete process.env.AMESH_CONNECT_BACKOFF_MS;
    delete process.env.AMESH_CONNECT_BACKOFF_MAX_MS;
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

function OwnerSleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

await test("a second log instance waits for the owner and replays after release", async () => {
  InboxLog(cwd);
  const state = InboxSession();
  const one = await PinnedInbox(state);
  let two;
  try {
    await one.handlers.get("session_start")({}, state.ctx);
    await waitCount(one.sockets, 1, 2000);
    one.failNextUser();
    fire(one.sockets[0], { type: "notify", message: "owner owed", id: "evt-owner-owed" });
    two = await PinnedInbox(state);
    await two.handlers.get("session_start")({}, state.ctx);
    await OwnerSleep(150);
    assert.equal(two.sockets.length, 0, "a non-owner must not connect or displace the owner");
    assert.equal(Injected(two, "owner owed"), 0, "a non-owner cannot inject the owner's record");
    await one.handlers.get("session_shutdown")({}, state.ctx);
    await waitCount(two.sockets, 1, 4000);
    await two.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(Injected(two, "owner owed"), 1, "takeover loads the durable owed record exactly once");
    fire(two.sockets[0], { type: "notify", message: "owner owed", id: "evt-owner-owed" });
    await two.handlers.get("agent_settled")({}, state.ctx);
    assert.equal(Injected(two, "owner owed"), 1);
  } finally {
    await one.handlers.get("session_shutdown")({}, state.ctx);
    await two?.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a slow live owner keeps its log past the old stale timeout", async () => {
  InboxLog(cwd);
  const state = InboxSession();
  const one = await PinnedInbox(state);
  let two;
  try {
    await one.handlers.get("session_start")({}, state.ctx);
    await waitCount(one.sockets, 1, 2000);
    two = await PinnedInbox(state);
    await two.handlers.get("session_start")({}, state.ctx);
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10100);
    await OwnerSleep(50);
    assert.equal(two.sockets.length, 0, "a paused live owner is never replaced on age");
    fire(one.sockets[0], { type: "notify", message: "still owned", id: "evt-still-owned" });
    assert.equal(recvsOf(one.sockets[0]).filter((r) => r.id === "evt-still-owned").length, 1);
  } finally {
    await one.handlers.get("session_shutdown")({}, state.ctx);
    await two?.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("an identity change waits for B ownership before writing as B", async () => {
  const dir = InboxLog(cwd);
  const a = InboxSessionAt(join(cwd, "pi-owner-a"));
  const b = InboxSessionAt(join(cwd, "pi-owner-b"));
  let one;
  const two = await PinnedInbox(b);
  try {
    await two.handlers.get("session_start")({}, b.ctx);
    await waitCount(two.sockets, 1, 2000);
    one = await InboxWithExec(executable, a);
    await one.handlers.get("session_start")({}, a.ctx);
    await waitCount(one.sockets, 1, 2000);
    one.failNextUser();
    fire(one.sockets[0], { type: "notify", message: "A held", id: "evt-A-held" });
    await one.handlers.get("session_start")({}, b.ctx);
    fire(one.sockets[0], { type: "notify", message: "arrived as B", id: "evt-as-B" });
    assert.equal(InboxRows(dir, a.id).some((r) => r.id === "evt-as-B"), false,
      "B traffic must never enter A's log while B is owned elsewhere");
    assert.ok(InboxRows(dir, a.id).some((r) => r.id === "evt-A-held"), "A keeps its record until migration");
    assert.equal(Injected(one, "A held"), 0, "A's held record stays outside B's session");
    assert.equal(Injected(two, "A held"), 0);
    assert.equal(one.sockets[0].readyState, 3, "the switching non-owner releases its old socket");
    await two.handlers.get("session_shutdown")({}, b.ctx);
    assert.equal(await waitUntil(() => InboxRows(dir, b.id).some((r) => r.id === "evt-A-held"), 4000), true);
    assert.equal(InboxRows(dir, a.id).some((r) => r.id === "evt-A-held"), false);
  } finally {
    await one?.handlers.get("session_shutdown")({}, b.ctx);
    await two.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("a failed accept recovers on its own while the hub remains healthy", async () => {
  InboxLog(cwd);
  const state = InboxSession();
  const hooks = await FailingWrite('"id":"evt-io-retry"', true);
  const instance = InboxInstance(state, false, hooks);
  const FakeSocket = globalThis.WebSocket;
  globalThis.WebSocket = class extends FakeSocket {
    constructor(url) {
      super(url);
      queueMicrotask(() => {
        if (instance.sockets.length > 1) {
          fire(this, { type: "connected", session_id: state.ctx.sessionManager.getSessionId() });
          fire(this, { type: "notify", message: "autonomous replay", id: "evt-io-retry" });
        }
      });
    }
  };
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await waitCount(instance.sockets, 1, 2000);
    fire(instance.sockets[0], { type: "notify", message: "autonomous replay", id: "evt-io-retry" });
    assert.equal(await waitUntil(() => Injected(instance, "autonomous replay") === 1, 4000), true,
      "a failed durable accept must trigger replay without another manual frame");
    assert.equal(instance.sockets.flatMap(recvsOf).filter((r) => r.id === "evt-io-retry").length, 1);
  } finally {
    await instance.handlers.get("session_shutdown")({}, state.ctx);
    globalThis.WebSocket = FakeSocket;
  }
});

await test("a standby instance cannot erase the owner's durable claim", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const one = await PinnedInbox(state);
  let two;
  try {
    await one.handlers.get("session_start")({}, state.ctx);
    await waitCount(one.sockets, 1, 2000);
    fire(one.sockets[0], { type: "connected", session_id: "" });
    fire(one.sockets[0], { type: "notify", message: "claimed by owner", id: "evt-owner-claim" });
    const text = source.replace("  RegisterTools();", "  globalThis.OwnerRewrite = Rewrite;\n  RegisterTools();");
    const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
    two = InboxInstance(state, true, hooks);
    await two.handlers.get("session_start")({}, state.ctx);
    one.failNextUser();
    const session = state.ctx.sessionManager.getSessionId();
    fire(one.sockets[0], { type: "bound", session_id: session });
    const before = readFileSync(InboxLogFile(dir, state.id), "utf8");
    globalThis.OwnerRewrite();
    assert.equal(readFileSync(InboxLogFile(dir, state.id), "utf8"), before,
      "the standby cannot compact stale memory over the owner's claim");
    assert.ok(InboxRows(dir, state.id).some((r) => r.t === "s" && r.session === session));
  } finally {
    await one.handlers.get("session_shutdown")({}, state.ctx);
    await two?.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("a failed owner helper keeps the socket closed and retries", async () => {
  InboxLog(cwd);
  const state = InboxSession();
  const marker = join(cwd, `pi-owner-allow-${process.pid}`);
  const bin = join(cwd, `pi-owner-retry-${process.pid}.sh`);
  writeFileSync(bin, `#!/bin/sh\nif [ "$2" = hold ] && [ ! -e '${marker}' ]; then exit 1; fi\nexec '${executable}' "$@"\n`);
  chmodSync(bin, 0o755);
  const text = source.split(JSON.stringify(executable)).join(JSON.stringify(bin))
    .replace("  RegisterTools();", "  globalThis.OwnerChild = () => owner?.child;\n  RegisterTools();");
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
  const instance = InboxInstance(state, false, hooks);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    await OwnerSleep(150);
    assert.equal(instance.sockets.length, 0, "failed ownership cannot open a hub socket");
    writeFileSync(marker, "go");
    await waitCount(instance.sockets, 1, 5000);
    fire(instance.sockets[0], { type: "connected", session_id: state.ctx.sessionManager.getSessionId() });
    fire(instance.sockets[0], { type: "notify", message: "helper recovered", id: "evt-helper-recovered" });
    assert.equal(recvsOf(instance.sockets[0]).filter((r) => r.id === "evt-helper-recovered").length, 1);
    const child = globalThis.OwnerChild();
    assert.equal(child.spawnfile, bin);
    assert.equal(child.exitCode, null);
    child.kill("SIGTERM");
    await waitCount(instance.sockets, 2, 5000);
    assert.equal(instance.sockets[0].readyState, 3, "losing the helper closes the old socket");
    fire(instance.sockets.at(-1), { type: "notify", message: "helper recovered", id: "evt-helper-recovered" });
    assert.equal(Injected(instance, "helper recovered"), 1, "a new holder reloads the tombstone");
  } finally {
    delete globalThis.OwnerChild;
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

await test("an in-flight connect rechecks ownership after an identity change", async () => {
  InboxLog(cwd);
  const a = InboxSessionAt(join(cwd, "pi-connect-owner-a"));
  const b = InboxSessionAt(join(cwd, "pi-connect-owner-b"));
  const two = await PinnedInbox(b);
  let one;
  const release = [];
  globalThis.OwnerConnectBarrier = () => new Promise((resolve) => release.push(resolve));
  try {
    await two.handlers.get("session_start")({}, b.ctx);
    await waitCount(two.sockets, 1, 2000);
    const text = source.replace("void Ensure().then(() => {", "void Ensure().then(async () => { await globalThis.OwnerConnectBarrier?.();");
    const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
    one = InboxInstance(a, false, hooks);
    await one.handlers.get("session_start")({}, a.ctx);
    assert.equal(await waitUntil(() => release.length > 0, 2000), true);
    await one.handlers.get("session_start")({}, b.ctx);
    for (const resolve of release.splice(0)) resolve();
    await OwnerSleep(50);
    assert.equal(one.sockets.length, 0, "a connect resumed after switching to an unowned log must stay closed");
  } finally {
    delete globalThis.OwnerConnectBarrier;
    for (const resolve of release.splice(0)) resolve();
    await one?.handlers.get("session_shutdown")({}, b.ctx);
    await two.handlers.get("session_shutdown")({}, b.ctx);
  }
});

await test("a log read failure retries before the log becomes authoritative", async () => {
  const dir = InboxLog(cwd);
  const state = InboxSession();
  const file = InboxLogFile(dir, state.id);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, `${JSON.stringify({ t: "a", id: "evt-read-retry", at: Math.floor(Date.now() / 1000), session: state.ctx.sessionManager.getSessionId(), type: "notify", text: "read recovery" })}\n`);
  globalThis.OwnerReadFailure = true;
  const text = source.replace(/^const peerId = .*;$/m, `const peerId = ${JSON.stringify(state.id)};`)
    .replace("  readFileSync,", "  readFileSync as NativeReadFileSync,") + `
function readFileSync(path, ...rest) {
  if (globalThis.OwnerReadFailure && path === ${JSON.stringify(file)}) {
    throw Object.assign(new Error("cannot read accepted records"), { code: "EACCES" });
  }
  return NativeReadFileSync(path, ...rest);
}`;
  const { default: hooks } = await import(`data:text/javascript;base64,${Buffer.from(text).toString("base64")}`);
  const instance = InboxInstance(state, false, hooks);
  try {
    await instance.handlers.get("session_start")({}, state.ctx);
    globalThis.OwnerReadFailure = false;
    assert.equal(await waitUntil(() => Injected(instance, "read recovery") === 1, 3000), true,
      "a failed load must retry the durable records, not initialise an empty log");
  } finally {
    delete globalThis.OwnerReadFailure;
    await instance.handlers.get("session_shutdown")({}, state.ctx);
  }
});

after(async () => {
  for (const instance of inboxInstances) {
    await instance.handlers.get("session_shutdown")({}, instance.ctx);
  }
});

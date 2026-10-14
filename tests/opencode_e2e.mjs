import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { createServer as CreateTCPServer } from "node:net";
import {
  mkdir,
  mkdtemp,
  writeFile,
  readFile,
  chmod,
  unlink,
} from "node:fs/promises";
import { resolve, join } from "node:path";
import { once } from "node:events";

const realConfig = process.env.AMESH_E2E_REAL_CONFIG;
const executable = resolve(process.argv[2] || "target/debug/amesh");
const root = await mkdtemp(resolve("build/opencode-validation/e2e-"));
const config = join(root, "config"),
  workspace = join(root, "workspace"),
  other = join(root, "other");
for (const path of [config, workspace, other])
  await mkdir(path, { recursive: true });
for (const path of [workspace, other])
  execFileSync("git", ["init", "-q", path]);
await writeFile(join(root, "models.json"), "{}");
const env = Object.fromEntries(
  Object.entries(process.env).filter(
    ([key]) => !/^(AMESH_|CODEX_|CLAUDE_|OPENCODE_|XDG_)/.test(key),
  ),
);
Object.assign(env, {
  HOME: root,
  OPENCODE_TEST_HOME: root,
  PWD: workspace,
  OPENCODE_CONFIG_DIR: config,
  OPENCODE_CONFIG_CONTENT: "{}",
  OPENCODE_DB: join(root, "opencode.db"),
  OPENCODE_MODELS_PATH: join(root, "models.json"),
  OPENCODE_DISABLE_MODELS_FETCH: "true",
  OPENCODE_DISABLE_AUTOUPDATE: "true",
  OPENCODE_PASSWORD: "isolated-opencode",
  XDG_CONFIG_HOME: config,
  XDG_CACHE_HOME: join(root, "cache"),
  XDG_DATA_HOME: join(root, "data"),
  XDG_STATE_HOME: join(root, "state"),
  AMESH_STATE: join(root, "amesh.db"),
  AMESH_TOKEN: "isolated-amesh",
  CODEX_HOME: join(root, "codex"),
});
const port = CreateTCPServer();
port.listen(0, "127.0.0.1");
await once(port, "listening");
env.AMESH_BIND = `127.0.0.1:${port.address().port}`;
await new Promise((done) => port.close(done));
const requests = [],
  processes = [],
  events = [];
const eventStop = new AbortController();
let eventTask,
  holdModel = false,
  activeExecutionOffset = 0,
  failNext = 0;
const held = [];
function Start(command, args) {
  const child = spawn(command, args, {
    cwd: workspace,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const record = { child, command, args, stdout: "", stderr: "" };
  child.stdout.on("data", (chunk) => (record.stdout += chunk));
  child.stderr.on("data", (chunk) => (record.stderr += chunk));
  processes.push(record);
  return record;
}
async function Until(callback, label, ms = 15000) {
  const until = Date.now() + ms;
  let last;
  while (Date.now() < until) {
    try {
      const value = await callback();
      if (value) return value;
    } catch (error) {
      last = error;
    }
    await new Promise((done) => setTimeout(done, 40));
  }
  throw new Error(`${label}: ${last?.message || "timeout"}`);
}
async function Hub(path, body) {
  const response = await fetch(`http://${env.AMESH_BIND}${path}`, {
    method: body ? "POST" : "GET",
    headers: {
      Authorization: `Bearer ${env.AMESH_TOKEN}`,
      "Content-Type": "application/json",
    },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(5000),
  });
  assert(response.ok, await response.clone().text());
  return response.json();
}
let url;
async function API(path, body, method) {
  const response = await fetch(url + path, {
    method: method || (body ? "POST" : "GET"),
    headers: {
      Authorization: `Basic ${Buffer.from("opencode:" + env.OPENCODE_PASSWORD).toString("base64")}`,
      "Content-Type": "application/json",
    },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(30000),
  });
  const text = await response.text();
  assert(response.ok, `${path}: ${response.status} ${text}`);
  return text ? JSON.parse(text).data : undefined;
}
const provider = createServer(async (request, response) => {
  let text = "";
  for await (const chunk of request) text += chunk;
  const body = JSON.parse(text);
  requests.push(body);
  if (failNext && body.tools?.length) {
    failNext--;
    response.writeHead(503, { "content-type": "application/json" });
    response.end(JSON.stringify({ error: { message: "controlled retry" } }));
    return;
  }
  if (holdModel && body.tools?.length)
    await new Promise((done) => held.push(done));
  let index = body.messages.findLastIndex((message) => message.role === "user");
  const latest = JSON.stringify(body.messages[index]);
  const hasResult = body.messages
    .slice(index + 1)
    .some((message) => message.role === "tool");
  const correlation = latest.match(/correlation_id=\\?"([^"\\]+)\\?"/);
  const action =
    typeof body.messages[index]?.content === "string"
      ? body.messages[index].content.match(/^AMESH_ACTION=(.*)$/s)
      : undefined;
  const tool = action
    ? JSON.parse(action[1])
    : correlation
      ? {
          name: "amesh_ack",
          arguments: {
            correlation_id: correlation[1],
            message: "fixture acknowledgment",
            failed: latest.includes("FAIL_JOB"),
          },
        }
      : { name: "amesh_whoami", arguments: {} };
  const chunks =
    hasResult || !body.tools?.length
      ? [
          {
            choices: [
              {
                delta: { role: "assistant", content: "completed" },
                finish_reason: null,
              },
            ],
          },
          { choices: [{ delta: {}, finish_reason: "stop" }] },
        ]
      : [
          {
            choices: [
              {
                delta: {
                  role: "assistant",
                  tool_calls: [
                    {
                      index: 0,
                      id: `call_${requests.length}`,
                      type: "function",
                      function: {
                        name: tool.name,
                        arguments: JSON.stringify(tool.arguments),
                      },
                    },
                  ],
                },
                finish_reason: null,
              },
            ],
          },
          { choices: [{ delta: {}, finish_reason: "tool_calls" }] },
        ];
  response.writeHead(200, { "content-type": "text/event-stream" });
  response.end(
    chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join("") +
      "data: [DONE]\n\n",
  );
});
provider.listen(0, "127.0.0.1");
await once(provider, "listening");
await writeFile(
  join(config, "opencode.json"),
  JSON.stringify({
    update: "disable",
    model: "test/test-model",
    providers: {
      test: {
        name: "Test",
        package: "aisdk:@ai-sdk/openai-compatible",
        settings: {
          apiKey: "isolated",
          baseURL: `http://127.0.0.1:${provider.address().port}/v1`,
        },
        models: {
          "test-model": {
            name: "Test Model",
            capabilities: { tools: true, input: ["text"], output: ["text"] },
            cost: { input: 0, output: 0 },
            limit: { context: 100000, output: 1000 },
          },
        },
      },
    },
  }),
);
if (realConfig) {
  await chmod(join(config, "opencode.json"), 0o600);
  await writeFile(join(config, "opencode.json"), await readFile(realConfig));
}
try {
  let hubProcess = Start(executable, ["serve"]);
  await Until(() => Hub("/health"), "hub start");
  execFileSync(executable, ["setup", "opencode"], { cwd: workspace, env });
  const installed = join(config, "plugins/amesh/server.js");
  let pluginSource = await readFile(installed, "utf8");
  const probes = [
    [
      "async setup(ctx) {",
      `async setup(ctx) { const probeID=Math.random(); const Probe=(kind)=>ProbeWrite(${JSON.stringify(join(root, "lifecycle.jsonl"))},JSON.stringify({kind,probeID,location:ctx.location,time:Date.now()})+"\\n"); Probe("setup");`,
    ],
    [
      "cleaning ??= (async () => {",
      'cleaning ??= (async () => { Probe("cleanup");',
    ],
  ];
  for (const [anchor, replacement] of probes) {
    assert(pluginSource.includes(anchor), "lifecycle probe anchor: " + anchor);
    pluginSource = pluginSource.replace(anchor, replacement);
  }
  const marker = "/* amesh-plugin:opencode:v1 */";
  assert(pluginSource.startsWith(marker));
  pluginSource = pluginSource.replace(
    marker,
    marker + '\nimport {appendFileSync as ProbeWrite} from "node:fs";',
  );
  await writeFile(installed, pluginSource);
  let opencode = Start("opencode", [
    "serve",
    "--hostname",
    "127.0.0.1",
    "--port",
    "0",
  ]);
  url = await Until(
    () => opencode.stdout.match(/server listening on (http:\/\/\S+)/)?.[1],
    "opencode start",
  );
  const stream = await fetch(url + "/api/event", {
    headers: {
      Authorization: `Basic ${Buffer.from("opencode:" + env.OPENCODE_PASSWORD).toString("base64")}`,
    },
    signal: eventStop.signal,
  });
  assert(stream.ok);
  eventTask = (async () => {
    let buffered = "";
    const decoder = new TextDecoder();
    try {
      for await (const chunk of stream.body) {
        buffered += decoder.decode(chunk, { stream: true });
        let end;
        while ((end = buffered.indexOf("\n\n")) >= 0) {
          const block = buffered.slice(0, end);
          buffered = buffered.slice(end + 2);
          for (const line of block.split("\n"))
            if (line.startsWith("data: "))
              events.push(JSON.parse(line.slice(6)));
        }
      }
    } catch (error) {
      if (!eventStop.signal.aborted) throw error;
    }
  })();
  const create = (directory, parentID) =>
    API("/api/session", {
      location: { directory },
      parentID,
      permissions: [{ action: "*", resource: "*", effect: "allow" }],
    });
  const a = await create(workspace),
    b = await create(workspace),
    c = await create(other);
  assert.equal(
    (await Hub("/peers")).length,
    0,
    "session creation must not enroll",
  );
  const rejectedJoin = await fetch(url + "/api/rpc/amesh/join", {
    method: "POST",
    headers: {
      Authorization: `Basic ${Buffer.from("opencode:" + env.OPENCODE_PASSWORD).toString("base64")}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ input: { sessionID: c.id, state: "idle" } }),
  });
  assert.equal(rejectedJoin.status, 400);
  assert.equal((await rejectedJoin.json()).type, "location");
  const prompt = async (session) => {
    await API(`/api/session/${session.id}/prompt`, {
      text: "identify using amesh_whoami",
    });
    await API(`/api/experimental/session/${session.id}/wait`, {}, "POST");
  };
  await Promise.all([prompt(a), prompt(b), prompt(c)]);
  const peers = await Hub("/peers");
  assert.equal(peers.length, 3);
  const peerA = peers.find((peer) => peer.session_id === `opencode:${a.id}`),
    peerB = peers.find((peer) => peer.session_id === `opencode:${b.id}`),
    peerC = peers.find((peer) => peer.session_id === `opencode:${c.id}`);
  assert(peerA && peerB && peerC);
  assert.notEqual(peerA.peer_id, peerB.peer_id);
  assert.equal(peerA.circle, peerB.circle);
  assert.notEqual(peerA.circle, peerC.circle);
  if (!realConfig)
    assert(
      requests.some((body) =>
        body.tools?.some((tool) => tool.function.name === "amesh_ask"),
      ),
    );
  if (!realConfig)
    assert(
      requests.some((body) =>
        JSON.stringify(body.messages).includes("ACK responsibility"),
      ),
    );
  const child = await create(workspace, a.id);
  await prompt(child);
  assert.equal((await Hub("/peers")).length, 3, "child shares root");
  if (!realConfig) {
    async function Call(session, name, args) {
      const before = requests.length;
      await API(`/api/session/${session.id}/prompt`, {
        text: "AMESH_ACTION=" + JSON.stringify({ name, arguments: args }),
      });
      await API(`/api/experimental/session/${session.id}/wait`, {}, "POST");
      const used = requests.slice(before).filter((body) => body.tools?.length);
      assert(
        used.some((body) =>
          JSON.stringify(body.messages).includes('"role":"tool"'),
        ),
        "tool returned through real runtime",
      );
    }
    await Promise.all([
      Call(a, "amesh_ask", {
        peer_name: peerB.peer_id,
        query: "concurrent-a-to-b",
      }),
      Call(b, "amesh_ask", {
        peer_name: peerA.peer_id,
        query: "concurrent-b-to-a",
      }),
    ]);
    await Until(async () => {
      const snapshot = await Hub("/snapshot");
      return ["concurrent-a-to-b", "concurrent-b-to-a"].every((text) =>
        snapshot.asks.some(
          (ask) =>
            ask.text === text && !ask.open && ask.closed_by === "recipient",
        ),
      );
    }, "concurrent bidirectional acknowledgments");
    const concurrent = (await Hub("/snapshot")).asks;
    for (const [text, from, to, recipient] of [
      ["concurrent-a-to-b", peerA, peerB, b],
      ["concurrent-b-to-a", peerB, peerA, a],
    ]) {
      const ask = concurrent.find((ask) => ask.text === text);
      assert.equal(ask.from_peer, from.peer_id);
      assert.equal(ask.to_peer_id, to.peer_id);
      assert(
        (await API(`/api/session/${recipient.id}/context`)).some(
          (message) =>
            message.type === "synthetic" &&
            message.text?.includes(ask.correlation_id),
        ),
      );
    }
    assert(
      !(await API(`/api/session/${child.id}/context`)).some(
        (message) =>
          message.type === "synthetic" && message.metadata?.ameshEventID,
      ),
      "root inbound is not delivered to child",
    );
    for (const failed of [false, true]) {
      const title = failed
        ? "integration-failed-job"
        : "integration-success-job";
      await Call(a, "amesh_job_create", {
        title,
        prompt: failed ? "FAIL_JOB" : "complete job",
        assigned_peer: peerB.peer_id,
      });
      await Until(
        async () =>
          (await Hub("/snapshot")).jobs.some(
            (job) =>
              job.title === title && job.state === (failed ? "failed" : "done"),
          ),
        title,
      );
    }
    await Call(a, "amesh_schedule_create", {
      to_peer: peerB.peer_id,
      text: "scheduled-integration-message",
      in_seconds: 1,
      kind: "notify",
    });
    await Until(
      async () =>
        (await API(`/api/session/${b.id}/context`)).some((message) =>
          message.text?.includes("scheduled-integration-message"),
        ),
      "scheduled message delivery",
    );
    const count = (await Hub("/snapshot")).asks.length;
    await Call(a, "amesh_ask", {
      peer_name: peerC.peer_id,
      query: "cross-circle forbidden",
    });
    assert.equal(
      (await Hub("/snapshot")).asks.length,
      count,
      "cross-circle creation rejected",
    );
    assert(
      requests.some((body) =>
        JSON.stringify(body.messages).includes("cross-circle requires"),
      ),
    );
    await Promise.all(
      [a, b].map((session) =>
        API(`/api/experimental/session/${session.id}/wait`, {}, "POST"),
      ),
    );
  }
  if (!realConfig) {
    const permissionChild = await API("/api/session", {
      location: { directory: workspace },
      parentID: a.id,
      permissions: [{ action: "*", resource: "*", effect: "ask" }],
    });
    await API(`/api/session/${permissionChild.id}/prompt`, {
      text:
        "AMESH_ACTION=" +
        JSON.stringify({
          name: "shell",
          arguments: { command: "printf permission-probe" },
        }),
    });
    const pending = await Until(
      async () =>
        (await API(`/api/session/${permissionChild.id}/permission`))[0],
      "child shell approval",
    );
    await Until(
      async () =>
        (await Hub("/snapshot")).roster.find(
          (peer) => peer.peer_id === peerA.peer_id,
        )?.activity?.state === "wait",
      "child approval marks root wait",
    );
    await API(
      `/api/session/${permissionChild.id}/permission/${pending.id}/reply`,
      { decision: "once" },
    );
    await API(
      `/api/experimental/session/${permissionChild.id}/wait`,
      {},
      "POST",
    );
    assert(
      (await API(`/api/session/${permissionChild.id}/context`)).some(
        (message) => JSON.stringify(message).includes("permission-probe"),
      ),
    );
    await Until(
      async () =>
        (await Hub("/snapshot")).roster.find(
          (peer) => peer.peer_id === peerA.peer_id,
        )?.activity?.state === "work",
      "approval restores root work",
    );
    const retryStart = events.length;
    failNext = 1;
    await prompt(a);
    assert(
      events
        .slice(retryStart)
        .some(
          (event) =>
            event.type === "session.retry.scheduled" &&
            event.data.sessionID === a.id,
        ),
      "real retry event",
    );
    const interruptStart = events.length;
    holdModel = true;
    await API(`/api/session/${a.id}/prompt`, { text: "hold until interrupt" });
    await Until(() => held.length, "interrupt model held");
    await API(`/api/session/${a.id}/interrupt?resume=false`, {}, "POST");
    holdModel = false;
    for (const release of held.splice(0)) release();
    await Until(
      () =>
        events
          .slice(interruptStart)
          .some(
            (event) =>
              event.type === "session.execution.interrupted" &&
              event.data.sessionID === a.id,
          ),
      "real interrupt event",
    );
    await Until(
      async () =>
        (await Hub("/snapshot")).roster.find(
          (peer) => peer.peer_id === peerA.peer_id,
        )?.activity?.state === "idle",
      "interrupt marks root idle",
    );
    await API(`/api/session/${a.id}/compact`, {});
    await API(`/api/experimental/session/${a.id}/wait`, {}, "POST");
    const compactStart = requests.length;
    await prompt(a);
    assert(
      requests
        .slice(compactStart)
        .filter((body) => body.tools?.length)
        .every((body) =>
          JSON.stringify(body.messages).includes("ACK responsibility"),
        ),
      "primer survives compaction",
    );
  }
  if (!realConfig) {
    const history = await API(`/api/session/${a.id}/context`);
    const boundary = history.find((message) => message.type === "user");
    assert(boundary);
    await API(`/api/session/${a.id}/revert/stage`, {
      messageID: boundary.id,
      files: false,
    });
    assert((await API(`/api/session/${a.id}`)).revert);
    await Hub("/notify", {
      from_peer: peerB.peer_id,
      to_peer: peerA.peer_id,
      message: "deferred-revert",
    });
    await new Promise((done) => setTimeout(done, 1200));
    assert(
      !(await API(`/api/session/${a.id}/inbox`)).some((item) =>
        JSON.stringify(item).includes("deferred-revert"),
      ),
    );
    assert(
      (await Hub("/snapshot")).roster.find(
        (peer) => peer.peer_id === peerA.peer_id,
      ).queued > 0,
    );
    await API(`/api/session/${a.id}/revert`, undefined, "DELETE");
    await Until(
      async () =>
        (await API(`/api/session/${a.id}/context`)).some((message) =>
          message.text?.includes("deferred-revert"),
        ),
      "revert release",
    );
    await API(`/api/experimental/session/${a.id}/wait`, {}, "POST");
    const requestStart = requests.length;
    holdModel = true;
    await API(`/api/session/${a.id}/prompt`, {
      text: "hold current work at provider boundary",
    });
    await Until(() => held.length, "model held");
    await Hub("/notify", {
      from_peer: peerB.peer_id,
      to_peer: peerA.peer_id,
      message: "busy-steer-notify",
    });
    await Until(
      async () =>
        (await API(`/api/session/${a.id}/inbox`)).some(
          (item) =>
            item.delivery === "steer" &&
            JSON.stringify(item).includes("busy-steer-notify"),
        ),
      "busy notification waits as steer",
    );
    for (let n = 0; n < 3; n++)
      await Hub("/broadcast", {
        from_peer: peerB.peer_id,
        message: `queued-broadcast-${n}`,
      });
    await Until(
      async () =>
        (await API(`/api/session/${a.id}/inbox`)).filter((item) =>
          JSON.stringify(item).includes("queued-broadcast-"),
        ).length === 3,
      "three queued broadcasts",
    );
    holdModel = false;
    for (const release of held.splice(0)) release();
    await Until(
      async () =>
        (await API(`/api/session/${a.id}/context`)).filter(
          (message) =>
            message.type === "synthetic" &&
            message.text?.includes("queued-broadcast-"),
        ).length === 3,
      "all broadcasts promoted",
    );
    await API(`/api/experimental/session/${a.id}/wait`, {}, "POST");
    const batches = requests
      .slice(requestStart)
      .filter((body) => body.tools?.length)
      .map((body) => {
        const messages = JSON.stringify(body.messages);
        return [0, 1, 2].filter((n) =>
          messages.includes(`queued-broadcast-${n}`),
        );
      });
    assert.deepEqual(
      batches.filter(
        (batch, index) =>
          index === 0 ||
          JSON.stringify(batch) !== JSON.stringify(batches[index - 1]),
      ),
      [[], [0], [0, 1], [0, 1, 2]],
      "broadcasts enter successive model input boundaries in order",
    );
  }
  if (realConfig) {
    for (const [from, to, marker] of [
      [a, peerB, "real-forward"],
      [b, peerA, "real-reverse"],
    ]) {
      await API(`/api/session/${from.id}/prompt`, {
        text: `Use amesh_ask to ask peer ${to.peer_id}: Reply using amesh_ack with the exact message ${marker}. Then finish without waiting. Use the amesh tools directly.`,
      });
      await API(`/api/experimental/session/${from.id}/wait`, {}, "POST");
      await Until(
        async () => {
          const snapshot = await Hub("/snapshot");
          return snapshot.asks.some(
            (ask) => !ask.open && ask.reply?.includes(marker),
          );
        },
        `real model ${marker}`,
        60000,
      );
    }
    await writeFile(
      join(root, "real-business-snapshot.json"),
      JSON.stringify(await Hub("/snapshot"), null, 2),
    );
  } else {
    const ask = await Hub("/ask", {
      from_peer: peerB.peer_id,
      to_peer: peerA.peer_id,
      query: "reply using amesh_ack",
    });
    await Until(async () => {
      const snapshot = await Hub(`/snapshot?ask=${ask.correlation_id}`);
      return JSON.stringify(snapshot).includes("fixture acknowledgment");
    }, "synthetic wakes real OpenCode and ACK reaches hub");
  }
  await Promise.all(
    [a, b, c, child].map((session) =>
      API(`/api/experimental/session/${session.id}/wait`, {}, "POST"),
    ),
  );
  if (!realConfig) {
    const pluginPath = join(config, "plugins/amesh/server.js");
    await writeFile(
      pluginPath,
      (await readFile(pluginPath, "utf8")) + "\n/* reload probe */\n",
    );
    await Until(
      async () => !(await Hub("/snapshot")).roster.some((peer) => peer.push),
      "file change cleans up all plugin roots",
    );
    await prompt(a);
    activeExecutionOffset = events.length;
    holdModel = true;
    await API(`/api/session/${a.id}/prompt`, {
      text: "hold for location reload",
    });
    await Until(() => held.length, "active reload model hold");
  }
  await API("/api/location/reload", {}, "POST");
  holdModel = false;
  for (const release of held.splice(0)) release();
  if (!realConfig) {
    await API(`/api/experimental/session/${a.id}/wait`, {}, "POST");
    await Until(
      () =>
        events
          .slice(activeExecutionOffset)
          .some(
            (event) =>
              [
                "session.execution.succeeded",
                "session.execution.failed",
                "session.execution.interrupted",
              ].includes(event.type) && event.data.sessionID === a.id,
          ),
      "active reload execution settles",
    );
    const snapshot = await Hub("/snapshot");
    await writeFile(
      join(root, "active-reload-snapshot.json"),
      JSON.stringify(snapshot, null, 2),
    );
    assert(
      snapshot.roster.every((peer) => !peer.push),
      "active reload closes every old root",
    );
    await API("/api/location/reload", {}, "POST");
  }
  await Until(async () => {
    const snapshot = await Hub("/snapshot");
    await writeFile(
      join(root, "reload-snapshot.json"),
      JSON.stringify(snapshot, null, 2),
    );
    return !JSON.stringify(snapshot).includes('"push":true');
  }, "reload cleanup closes peers");
  const count = requests.length;
  await prompt(a);
  if (!realConfig) assert(requests.length > count);
  if (!realConfig) {
    const subpath = join(workspace, "nested");
    await mkdir(subpath);
    const worktree = join(root, "linked");
    execFileSync("git", [
      "-C",
      workspace,
      "worktree",
      "add",
      "--orphan",
      "-b",
      "linked",
      "--quiet",
      worktree,
    ]);
    const nested = await create(subpath),
      linked = await create(worktree),
      fork = await API(`/api/session/${a.id}/fork`, {});
    assert(!fork.parentID);
    await Promise.all([prompt(nested), prompt(linked), prompt(fork)]);
    const identity = await Hub("/peers");
    for (const session of [nested, linked, fork]) {
      const peer = identity.find(
        (value) => value.session_id === `opencode:${session.id}`,
      );
      assert(peer);
      assert.equal(peer.circle, peerA.circle);
      assert.notEqual(peer.peer_id, peerA.peer_id);
    }
    await writeFile(
      join(root, "placement.json"),
      JSON.stringify({ nested, linked, fork, peers: identity }, null, 2),
    );
    const forkPeer = identity.find(
      (value) => value.session_id === `opencode:${fork.id}`,
    );
    await API(`/api/session/${fork.id}`, undefined, "DELETE");
    await Until(
      async () =>
        !(await Hub("/snapshot")).roster.find(
          (peer) => peer.peer_id === forkPeer.peer_id,
        )?.push,
      "deleted session disconnects",
    );
    const connected = (await Hub("/snapshot")).roster
      .filter((peer) => peer.push)
      .map((peer) => peer.peer_id)
      .sort();
    hubProcess.child.kill("SIGTERM");
    await once(hubProcess.child, "exit");
    hubProcess = Start(executable, ["serve"]);
    await Until(() => Hub("/health"), "hub restart");
    await Until(async () => {
      const roster = (await Hub("/snapshot")).roster;
      return connected.every((id) =>
        roster.some(
          (peer) =>
            peer.peer_id === id && peer.push && peer.activity?.state === "idle",
        ),
      );
    }, "hub restart restores sockets and idle activity");
    eventStop.abort();
    await eventTask;
    opencode.child.kill("SIGTERM");
    await once(opencode.child, "exit");
    await Until(
      async () => (await Hub("/snapshot")).roster.every((peer) => !peer.push),
      "service shutdown cleanup",
    );
    opencode = Start("opencode", [
      "serve",
      "--hostname",
      "127.0.0.1",
      "--port",
      "0",
    ]);
    url = await Until(
      () => opencode.stdout.match(/server listening on (http:\/\/\S+)/)?.[1],
      "opencode restart",
    );
    await API("/api/session");
    assert(
      (await Hub("/snapshot")).roster.every((peer) => !peer.push),
      "history remains offline after service restart",
    );
    const restored = await API(`/api/session/${a.id}`);
    await API(`/api/session/${a.id}/view`, { idle: restored.time.idle });
    const viewed = await API(`/api/session/${a.id}`);
    assert(
      viewed.time.viewed >= viewed.time.idle,
      "restore an already-read session",
    );
    const requestsBeforeOpen = requests.length;
    const tui = Start("python3", [
      resolve("tests/support/opencode_tui_pty.py"),
      "opencode",
      "--server",
      url,
      "--session",
      a.id,
      workspace,
    ]);
    await Until(
      async () =>
        (await Hub("/snapshot")).roster.some(
          (peer) => peer.peer_id === peerA.peer_id && peer.push,
        ),
      "TUI restores root without sending a prompt",
      20000,
    );
    assert.equal(
      requests.length,
      requestsBeforeOpen,
      "opening a root must not request a model",
    );
    const resumed = (await Hub("/snapshot")).roster.filter((peer) => peer.push);
    assert.deepEqual(
      resumed.map((peer) => peer.peer_id),
      [peerA.peer_id],
    );
    const lifecycleBefore = (
      await readFile(join(root, "lifecycle.jsonl"), "utf8")
    )
      .split("\n")
      .filter(Boolean)
      .map(JSON.parse);
    const setupsBefore = lifecycleBefore.filter(
      (event) => event.kind === "setup",
    ).length;
    await API("/api/location/reload", {}, "POST");
    await Until(
      async () => {
        const lifecycle = (
          await readFile(join(root, "lifecycle.jsonl"), "utf8")
        )
          .split("\n")
          .filter(Boolean)
          .map(JSON.parse);
        return (
          lifecycle.filter((event) => event.kind === "setup").length >
            setupsBefore &&
          (await Hub("/snapshot")).roster.some(
            (peer) => peer.peer_id === peerA.peer_id && peer.push,
          )
        );
      },
      "foreground TUI rejoins after reload",
      20000,
    );
    assert.equal(
      requests.length,
      requestsBeforeOpen,
      "reload rejoin must not request a model",
    );
    const reinstallBefore = (
      await readFile(join(root, "lifecycle.jsonl"), "utf8")
    )
      .split("\n")
      .filter(Boolean)
      .map(JSON.parse);
    const existingSetups = reinstallBefore
      .filter((event) => event.kind === "setup")
      .map((event) => event.probeID);
    execFileSync(executable, ["setup", "opencode"], { cwd: workspace, env });
    await Until(
      async () => {
        const lifecycle = (
          await readFile(join(root, "lifecycle.jsonl"), "utf8")
        )
          .split("\n")
          .filter(Boolean)
          .map(JSON.parse);
        return (
          existingSetups.every((id) =>
            lifecycle.some(
              (event) => event.kind === "cleanup" && event.probeID === id,
            ),
          ) &&
          (await Hub("/snapshot")).roster.some(
            (peer) => peer.peer_id === peerA.peer_id && peer.push,
          )
        );
      },
      "foreground TUI rejoins after setup",
      45000,
    );
    assert.equal(
      requests.length,
      requestsBeforeOpen,
      "setup rejoin must not request a model",
    );
    await writeFile(
      join(root, "tui-open.json"),
      JSON.stringify(
        {
          sessionID: a.id,
          viewed: viewed.time.viewed,
          idle: viewed.time.idle,
          requestsBeforeOpen,
          requestsAfterOpen: requests.length,
          onlinePeers: resumed.map((peer) => peer.peer_id),
          reloadRejoined: true,
          setupRejoined: true,
        },
        null,
        2,
      ),
    );
  }
} finally {
  holdModel = false;
  for (const release of held.splice(0)) release();
  eventStop.abort();
  await eventTask;
  await writeFile(join(root, "events.json"), JSON.stringify(events, null, 2));
  for (const record of processes.reverse()) {
    record.child.kill("SIGTERM");
    const timer = setTimeout(() => record.child.kill("SIGKILL"), 3000);
    if (record.child.exitCode === null && record.child.signalCode === null)
      await once(record.child, "exit");
    clearTimeout(timer);
    await writeFile(
      join(
        root,
        record.command === "opencode"
          ? "opencode.log"
          : record.command === "python3"
            ? "tui.log"
            : "hub.log",
      ),
      record.stdout + "\n" + record.stderr,
    );
  }
  await writeFile(
    join(root, "requests.json"),
    JSON.stringify(requests, null, 2),
  );
  provider.closeAllConnections();
  await new Promise((done) => provider.close(done));
  if (realConfig) await unlink(join(config, "opencode.json"));
}

const lifecycle = (await readFile(join(root, "lifecycle.jsonl"), "utf8"))
  .trim()
  .split("\n")
  .map(JSON.parse);
const setups = lifecycle.filter((entry) => entry.kind === "setup");
assert(setups.length > 0, "lifecycle setup probe ran");
for (const setup of setups) {
  const cleanup = lifecycle.find(
    (entry) => entry.kind === "cleanup" && entry.probeID === setup.probeID,
  );
  assert(cleanup, "cleanup paired with setup " + setup.probeID);
  const shutdown = events.find(
    (event) =>
      event.type === "location.shutdown" &&
      event.location?.directory === setup.location.directory &&
      event.location?.workspaceID === setup.location.workspaceID &&
      event.created >= setup.time &&
      event.created <= cleanup.time,
  );
  if (shutdown)
    assert(
      cleanup.time - shutdown.created < 2000,
      "shutdown cleanup starts within 2s",
    );
}

await writeFile(
  join(root, "lifecycle-summary.json"),
  JSON.stringify(
    {
      setups: setups.length,
      cleanups: lifecycle.filter((entry) => entry.kind === "cleanup").length,
    },
    null,
    2,
  ),
);

console.log(
  realConfig ? "REAL MODEL BUSINESS PASS" : "CONTROLLED SSE INTERFACE PASS",
);
console.log(
  "PASS real plugin: two roots, two locations, child identity, direct tools, primer, incoming ask/ACK, reload/rejoin",
  root,
);

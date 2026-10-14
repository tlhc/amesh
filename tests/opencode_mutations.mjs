import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { once } from "node:events";
import { resolve, join } from "node:path";
const executable = resolve(process.argv[2] || "target/debug/amesh");
const root = await mkdtemp(resolve("build/opencode-validation/mutations-"));
const source = await readFile("src/cli/opencode_hook.js", "utf8");
const promptBlock = source.slice(
  source.indexOf('ctx.session.hook("prompt"'),
  source.indexOf('ctx.session.hook("context"'),
);
const cases = [
  [
    "direct-tools",
    "options: { codemode: false }",
    "options: { codemode: true }",
    "catalog is directly visible",
  ],
  [
    "root-identity",
    "if (!info.parentID) return info;",
    'if (!info.parentID) return await ctx.session.get({sessionID:"ses_a"});',
    "two roots remain distinct",
  ],
  [
    "location",
    "(await realpath(info.location.directory)) !== location",
    "false",
    "foreign location rejected",
  ],
  [
    "early-recv",
    "await ctx.session.synthetic({",
    'socket.send(JSON.stringify({type:"recv",id:item.event.id})); await ctx.session.synthetic({',
    "recv waits for durable admission",
  ],
  [
    "unstable-id",
    ".update(eventID)",
    ".update(eventID + Math.random())",
    "successful admission survives lost recv",
  ],
  [
    "revert",
    "if (info.revert) {",
    "if (false && info.revert) {",
    "revert defers durable admission",
  ],
  ["env", "delete env[key];", "void env[key];", "two roots remain distinct"],
  [
    "reconnect-state",
    "if (root.last)",
    "if (false && root.last)",
    "reconnect reports last idle state",
  ],
  [
    "generation",
    "root.generation === generation",
    "true",
    "old generation control frame is ignored",
  ],
  [
    "cleanup",
    "registration.dispose()",
    "Promise.resolve()",
    "cleanup removes hook registrations",
  ],
  [
    "frame-count",
    "root.queue.length >= 128",
    "root.queue.length > 128",
    "queue capacity includes the active admission",
  ],
  [
    "byte-budget",
    "root.bytes + bytes > 32 * 1024 * 1024",
    "root.bytes + bytes > 64 * 1024 * 1024",
    "aggregate byte budget closes connection",
  ],
  [
    "frame-size",
    "bytes > 16 * 1024 * 1024",
    "bytes > 32 * 1024 * 1024",
    "oversized frame terminates",
  ],
  [
    "target-alias",
    "root.queue.push({ event, bytes });",
    'if (event.to_peer && event.to_peer !== root.peerID) throw new Error("target"); root.queue.push({ event, bytes });',
    "transferred backlog keeps original recipient spelling",
  ],
  [
    "prompt-isolation",
    promptBlock,
    promptBlock.replace("Log(error);", "throw error;"),
    "stopped and missing sessions do not reject user prompts",
  ],

  [
    "persistent-backoff",
    "if (!root.failing) root.attempts = 0;",
    "root.attempts = 0;",
    "admission failure retains notification until replay",
  ],
  [
    "location-shutdown",
    'event.type === "location.shutdown"',
    'event.type === "disabled.shutdown"',
    "location shutdown cleans up only the matching instance",
  ],
  [
    "display-sender",
    "${sender}",
    "${item.event.from_peer}",
    "display sender rejects terminal control characters",
  ],
  [
    "tui-description",
    "description: `amesh ${item.event.type} from @${sender}`",
    'description: ""',
    "admission failure retains notification until replay",
  ],
];
for (const [name, from] of cases)
  assert(source.includes(from), name + " mutation anchor");
const results = [];
const selected = new Set(process.argv.slice(3));
for (const name of selected)
  assert(
    cases.some(([candidate]) => candidate === name),
    "unknown mutation " + name,
  );
for (const [name, from, to, expected] of cases.filter(
  ([name]) => !selected.size || selected.has(name),
)) {
  assert(source.includes(from), name + " mutation anchor");
  const dir = join(root, name);
  await mkdir(dir);
  const config = join(dir, "config");
  const port = createServer();
  port.listen(0, "127.0.0.1");
  await once(port, "listening");
  const bind = `127.0.0.1:${port.address().port}`;
  await new Promise((done) => port.close(done));
  const env = {
    ...process.env,
    HOME: dir,
    CODEX_HOME: join(dir, "codex"),
    AMESH_BIND: bind,
    AMESH_TOKEN: "mutation-token",
    AMESH_STATE: join(dir, "state.db"),
    OPENCODE_CONFIG_DIR: config,
  };
  for (const key of [
    "AMESH_PEER_ID",
    "AMESH_CIRCLE",
    "CODEX_THREAD_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
  ])
    delete env[key];
  const hub = spawn(executable, ["serve"], { cwd: dir, env, stdio: "ignore" });
  try {
    const deadline = Date.now() + 5000;
    while (true) {
      try {
        const res = await fetch(`http://${bind}/health`);
        if (res.ok) break;
      } catch {}
      assert(Date.now() < deadline, "hub ready");
      await new Promise((done) => setTimeout(done, 20));
    }
    const setup = spawnSync(executable, ["setup", "opencode"], {
      cwd: dir,
      env,
      encoding: "utf8",
    });
    assert.equal(setup.status, 0, setup.stderr);
    const plugin = join(config, "plugins/amesh/server.js");
    const baked = await readFile(plugin, "utf8");
    assert(baked.includes(from));
    await writeFile(plugin, baked.replace(from, to));
    const run = spawnSync(
      process.execPath,
      [resolve("tests/opencode_hook.mjs"), plugin, executable, dir],
      {
        cwd: dir,
        env: {
          ...env,
          AMESH_CIRCLE: "inherited-wrong-circle",
          AMESH_PEER_ID: "inherited-wrong-peer",
        },
        encoding: "utf8",
        timeout: 60000,
        maxBuffer: 4 * 1024 * 1024,
      },
    );
    const output = run.stdout + "\n" + run.stderr;
    await writeFile(join(dir, "test.log"), output);
    const failed = [
      ...new Set(
        [...output.matchAll(/✖ ([^\n]+?) \([\d.]+ms\)/g)].map(
          (match) => match[1],
        ),
      ),
    ];
    const passed = [
      ...new Set(
        [...output.matchAll(/✔ ([^\n]+?) \([\d.]+ms\)/g)].map(
          (match) => match[1],
        ),
      ),
    ];
    assert.notEqual(run.status, 0, name + " must fail");
    assert(
      failed.some((test) => test.includes(expected)),
      name + " did not fail its claimed subtest: " + failed.join(", "),
    );
    results.push({
      name,
      expected,
      failed,
      passed,
      exit: run.status,
      signal: run.signal,
    });
    console.log(
      name +
        ": " +
        passed.length +
        " pass, " +
        failed.filter((test) => !test.startsWith("OpenCode plugin contracts"))
          .length +
        " failing subtests; target: " +
        expected,
    );
  } finally {
    hub.kill("SIGTERM");
    if (hub.exitCode === null) await once(hub, "exit");
  }
}
await writeFile(join(root, "results.json"), JSON.stringify(results, null, 2));
console.log("PASS mutation targets", root);

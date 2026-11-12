import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { StringDecoder } from "node:string_decoder";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { once } from "node:events";
import { availableParallelism } from "node:os";
import { resolve, join } from "node:path";
/* arguments: amesh binary, a scratch root outside the repository, the hook harness and
the hook source. All four are required, so nothing is resolved against the working
directory and nothing is written into the repository */
const [executableArg, workRoot, hookArg, sourceArg] = process.argv.slice(2);
if (!executableArg || !workRoot || !hookArg || !sourceArg)
  throw new Error(
    "usage: opencode_mutations.mjs <amesh> <scratch-root> <hook-harness.mjs> <hook-source.js> [mutation...]",
  );
const executable = resolve(executableArg);
function Run(file, args, { timeout, ...options }) {
  return new Promise((done) => {
    const child = spawn(file, args, { ...options, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    const timer = timeout ? setTimeout(() => child.kill("SIGKILL"), timeout) : null;
    child.on("close", (status, signal) => {
      if (timer) clearTimeout(timer);
      done({ status, signal, stdout, stderr });
    });
  });
}
/* the harness prints one line per subtest as soon as it finishes, so the sweep stops at
the target's own verdict instead of waiting for the subtests behind it */
function RunHarness(args, options, target) {
  return new Promise((done) => {
    const child = spawn(process.execPath, args, {
      ...options,
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
    });
    const out = new StringDecoder("utf8");
    const err = new StringDecoder("utf8");
    let stdout = "";
    let stderr = "";
    let pending = "";
    /* null until the target reports; true when it failed, false when it passed */
    let verdict = null;
    const stop = () => {
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch {}
    };
    const timer = options.timeout ? setTimeout(stop, options.timeout) : null;
    const Line = (line) => {
      stdout += line + "\n";
      if (verdict !== null) return;
      const match = /^\s*([\u2716\u2714]) (.+?) \([\d.]+ms\)\s*$/.exec(line);
      if (!match || !match[2].includes(target)) return;
      verdict = match[1] === "\u2716";
      stop();
    };
    child.stdout.on("data", (chunk) => {
      pending += out.write(chunk);
      for (let at = pending.indexOf("\n"); at >= 0; ) {
        Line(pending.slice(0, at));
        pending = pending.slice(at + 1);
        at = pending.indexOf("\n");
      }
    });
    child.stderr.on("data", (chunk) => {
      stderr += err.write(chunk);
    });
    child.on("close", (status, signal) => {
      if (timer) clearTimeout(timer);
      stop();
      if (pending) Line(pending);
      done({ status, signal, stdout, stderr: stderr + err.end(), verdict });
    });
  });
}
const hookScript = resolve(hookArg);
const sourcePath = resolve(sourceArg);
const root = await mkdtemp(join(workRoot, "mutations-"));
const rootPath = root + "/";
const source = await readFile(sourcePath, "utf8");
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
const selected = new Set(process.argv.slice(6));
for (const name of selected)
  assert(
    cases.some(([candidate]) => candidate === name),
    "unknown mutation " + name,
  );
/* cases are independent: each has its own home, config and hub port, so a small pool
keeps the sweep inside a normal test budget instead of running the suite 19 times in a
row. The lanes follow the CPUs the box reports, capped at the case count. */
const queue = cases.filter(([name]) => !selected.size || selected.has(name));
const lanes = Math.min(queue.length, availableParallelism());
/* when the CLI cannot reach our hub it starts one of its own and detaches it, and that
hub outlives the lane it came from; every process of this sweep carries the scratch root
in its argv or environment, so the sweep can end without leaving one behind */
async function ReapStrays() {
  let clean = 0;
  for (let pass = 0; pass < 16 && clean < 4; pass++) {
    await new Promise((done) => setTimeout(done, 200));
    let listing;
    try {
      listing = execFileSync("ps", ["axeww"], {
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
      });
    } catch (error) {
      /* proving the sweep left nothing behind is part of what this test checks, so a
      listing that cannot be read is a failure and not a silent pass */
      throw new Error("ps axeww failed, strays cannot be ruled out: " + error.message);
    }
    let killed = 0;
    for (const line of listing.split("\n")) {
      if (!line.includes(rootPath)) continue;
      if (!line.includes("amesh ") && !line.includes("opencode_hook.mjs")) continue;
      const pid = Number.parseInt(line, 10);
      if (!pid || pid === process.pid) continue;
      try {
        process.kill(pid, "SIGKILL");
        killed++;
      } catch {}
    }
    clean = killed ? 0 : clean + 1;
  }
}
async function RunCase([name, from, to, expected]) {
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
    const deadline = performance.now() + 5000;
    while (true) {
      try {
        const res = await fetch(`http://${bind}/health`);
        if (res.ok) break;
      } catch {}
      assert(performance.now() < deadline, "hub ready");
      await new Promise((done) => setTimeout(done, 20));
    }
    const setup = await Run(executable, ["setup", "opencode"], { cwd: dir, env });
    assert.equal(setup.status, 0, setup.stderr);
    const plugin = join(config, "plugins/amesh/server.js");
    const baked = await readFile(plugin, "utf8");
    assert(baked.includes(from));
    await writeFile(plugin, baked.replace(from, to));
    /* the reporter is pinned because a piped stdout defaults to TAP before Node 24,
    and the assertions below read the spec report */
    const run = await RunHarness(
      ["--test-reporter=spec", hookScript, plugin, executable, dir],
      {
        cwd: dir,
        env: {
          ...env,
          AMESH_CIRCLE: "inherited-wrong-circle",
          AMESH_PEER_ID: "inherited-wrong-peer",
        },
        timeout: 60000,
      },
      expected,
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
    if (run.verdict === false)
      assert.fail(name + " did not fail its claimed subtest: " + expected);
    if (run.verdict === null) {
      /* the target never reported, so fall back to the whole-output rules */
      assert.notEqual(run.status, 0, name + " must fail");
      assert(
        failed.some((test) => test.includes(expected)),
        name + " did not fail its claimed subtest: " + failed.join(", "),
      );
    }
    results.push({
      name,
      expected,
      verdict: run.verdict,
      failed,
      passed,
      exit: run.status,
      signal: run.signal,
    });
    console.log(
      name +
        ": " +
        (run.verdict === true ? "caught the mutation" : "verdict " + run.verdict) +
        "; target: " +
        expected,
    );
  } finally {
    hub.kill("SIGTERM");
    if (hub.exitCode === null) await once(hub, "exit");
  }
}
/* every lane owns a hub and a harness process, so a lane that throws must not abandon the
others: wait for every lane to finish its own cleanup, then report the first failure */
const settled = await Promise.allSettled(
  Array.from({ length: lanes }, async () => {
    while (queue.length) await RunCase(queue.shift());
  }),
);
try {
  await writeFile(join(root, "results.json"), JSON.stringify(results, null, 2));
} finally {
  await ReapStrays();
}
const failure = settled.find((lane) => lane.status === "rejected");
if (failure) throw failure.reason;
console.log("PASS mutation targets", root);

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import net from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const self = fileURLToPath(import.meta.url);
const root = join(dirname(self), "..");
const rawSrc = readFileSync(join(root, "src/cli/pi_hook.js"), "utf8");
const srcHash = createHash("sha256").update(rawSrc).digest("hex");
const caseName = process.env.AMESH_ENSURE_CASE || "";

function freePort() {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.listen(0, "127.0.0.1", () => {
      const port = server.address().port;
      server.close(() => resolve(port));
    });
  });
}

async function loadHook(executable) {
  const src = rawSrc
    .replaceAll("__AMESH_EXECUTABLE__", JSON.stringify(executable))
    .replaceAll("__AMESH_PEER_ID__", "null");
  const { default: AmeshHooks } = await import(
    `data:text/javascript;base64,${Buffer.from(src).toString("base64")}`
  );
  return AmeshHooks;
}

function mockPi(handlers, calls) {
  return {
    on(event, handler) {
      handlers.set(event, handler);
    },
    sendMessage() {
      calls.push("sendMessage");
    },
    sendUserMessage() {
      calls.push("sendUserMessage");
    },
    registerTool() {
      calls.push("registerTool");
    },
  };
}

const ctx = {
  cwd: tmpdir(),
  sessionManager: { getSessionId: () => "ensure-error-session" },
};

async function runA() {
  delete process.env.AMESH_SKIP_WS;
  const home = mkdtempSync(join(tmpdir(), "amesh-ensure-A-"));
  process.env.HOME = home;
  process.env.AMESH_STATE = join(home, "state.db");
  process.env.AMESH_BIND = `127.0.0.1:${await freePort()}`;
  const handlers = new Map();
  const calls = [];
  (await loadHook("/nonexistent/amesh-ensure-probe"))(mockPi(handlers, calls));
  await handlers.get("session_start")({}, ctx);
  await new Promise((resolve) => setTimeout(resolve, 3000));
  assert.equal(existsSync(join(home, "serve.log")), true, "Ensure must open serve.log before spawn");
  console.log("ensure-case-A ok");
}

async function runB() {
  delete process.env.AMESH_SKIP_WS;
  const home = mkdtempSync(join(tmpdir(), "amesh-ensure-B-"));
  process.env.HOME = home;
  process.env.AMESH_STATE = join(home, "state.db");
  process.env.AMESH_BIND = `127.0.0.1:${await freePort()}`;
  let attempts = 0;
  globalThis.WebSocket = class {
    constructor() {
      attempts += 1;
      throw new Error("connect-fail");
    }
  };
  const logs = [];
  const orig = console.error;
  console.error = (...args) => {
    logs.push(args.join(" "));
    orig.apply(console, args);
  };
  const handlers = new Map();
  const calls = [];
  (await loadHook("/nonexistent/amesh-ensure-probe"))(mockPi(handlers, calls));
  await handlers.get("session_start")({}, ctx);
  await new Promise((resolve) => setTimeout(resolve, 3000));
  console.error = orig;
  assert.ok(attempts > 0, `WebSocket constructor must run, attempts=${attempts}`);
  assert.match(logs.join("\n"), /connect-fail/, `Connect catch must log, got ${logs.join(" | ")}`);
  console.log("ensure-case-B ok");
}

async function runTry() {
  delete process.env.AMESH_SKIP_WS;
  const home = mkdtempSync(join(tmpdir(), "amesh-ensure-try-"));
  process.env.HOME = home;
  process.env.AMESH_STATE = join(home, "state.db");
  process.env.AMESH_BIND = "127.0.0.1:8378 x";
  const logs = [];
  const orig = console.error;
  console.error = (...args) => {
    logs.push(args.join(" "));
    orig.apply(console, args);
  };
  const handlers = new Map();
  const calls = [];
  (await loadHook("/nonexistent/amesh-ensure-probe"))(mockPi(handlers, calls));
  await handlers.get("session_start")({}, ctx);
  await new Promise((resolve) => setTimeout(resolve, 200));
  console.error = orig;
  assert.match(
    logs.join("\n"),
    /spawn \/nonexistent\/amesh-ensure-probe ENOENT/,
    `Invoke must still run after Ensure reject, got ${logs.join(" | ")}`,
  );
  console.log("ensure-case-try ok");
}

if (caseName === "A") {
  await runA();
} else if (caseName === "B") {
  await runB();
} else if (caseName === "try") {
  await runTry();
} else {
  const stampDir = join(tmpdir(), "amesh-ensure-probe");
  mkdirSync(stampDir, { recursive: true });
  const stamp = join(stampDir, `from-src-${srcHash.slice(0, 12)}.js`);
  writeFileSync(
    stamp,
    rawSrc
      .replaceAll("__AMESH_EXECUTABLE__", JSON.stringify("/nonexistent/amesh-ensure-probe"))
      .replaceAll("__AMESH_PEER_ID__", "null"),
  );
  console.log(`input from-src sha256 ${srcHash} file ${stamp}`);
  for (const name of ["A", "B", "try"]) {
    const env = { ...process.env, AMESH_ENSURE_CASE: name };
    delete env.AMESH_SKIP_WS;
    const result = spawnSync(process.execPath, [self], {
      env,
      encoding: "utf8",
      timeout: 15000,
    });
    assert.equal(result.status, 0, `${name} stdout=${result.stdout} stderr=${result.stderr}`);
    assert.match(result.stdout, new RegExp(`ensure-case-${name} ok`));
    console.log(result.stdout.trim());
  }
  console.log("passed ensure error handling");
}

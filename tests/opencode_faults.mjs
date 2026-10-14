import assert from "node:assert/strict";
import { readFile, writeFile, mkdir, chmod } from "node:fs/promises";
import { resolve, join } from "node:path";
import { test } from "node:test";

const [pluginPath, directory] = process.argv.slice(2);
const original = await readFile(pluginPath, "utf8");
class Socket extends EventTarget {
  readyState = 0;
  constructor() {
    super();
    queueMicrotask(() => {
      this.readyState = 1;
      this.dispatchEvent(new Event("open"));
    });
  }
  send(text) {
    if (JSON.parse(text).type === "connect")
      queueMicrotask(() =>
        this.dispatchEvent(
          new MessageEvent("message", {
            data: JSON.stringify({
              type: "connected",
              peer_id: "fault-peer",
              session_id: "opencode:ses_fault",
            }),
          }),
        ),
      );
  }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.dispatchEvent(new Event("close"));
  }
}
globalThis.WebSocket = Socket;
async function Until(predicate) {
  const deadline = Date.now() + 3000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((done) => setTimeout(done, 20));
  }
  assert.fail("fixture did not reach subprocess boundary");
}
async function Fixture(name, mode, dispose = () => {}) {
  const path = resolve(directory, name);
  await mkdir(path);
  const control = join(path, "mode");
  await writeFile(control, mode);
  const started = join(path, "started");
  const committed = join(path, "committed");
  const executable = join(path, "amesh-fixture.cjs");
  await writeFile(
    executable,
    `#!${process.execPath}
const fs=require("node:fs");
const mode=fs.readFileSync(${JSON.stringify(control)},"utf8");
let input="";process.stdin.on("data",chunk=>input+=chunk);process.stdin.on("end",()=>{
 if(process.argv[2]==="hook"){
  if(process.argv[3]==="session"){
   if(mode==="empty")return;
   if(mode==="malformed"){process.stdout.write("{");return;}
   console.log(JSON.stringify({peer_id:"fault-peer",circle:"fixture",context:"fixture primer"}));
  }
  return;
 }
 fs.writeFileSync(${JSON.stringify(started)},String(process.pid));
 if(mode==="hang"){setTimeout(()=>{},120000);return;}
 if(mode==="lost"){fs.appendFileSync(${JSON.stringify(committed)},"created\\n");return;}
 console.log(JSON.stringify({jsonrpc:"2.0",id:1,result:{content:[{type:"text",text:"fault-peer"}]}}));
});
`,
  );
  await chmod(executable, 0o755);
  const source = original.replace(
    /const executable = [\s\S]*?;\n/,
    `const executable = ${JSON.stringify(executable)};\n`,
  );
  const plugin = (
    await import(
      `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
    )
  ).default;
  const tools = new Map();
  const cleanup = await plugin.setup({
    rpc: {
      async register() {
        return { dispose() {} };
      },
    },
    location: { directory: path },
    tool: {
      async transform(callback) {
        callback({ add: (tool) => tools.set(tool.name, tool) });
        return { dispose };
      },
    },
    session: {
      async get() {
        return { id: "ses_fault", location: { directory: path } };
      },
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
    started,
    committed,
    mode: (next) => writeFile(control, next),
    invoke: (name) =>
      tools
        .get(name)
        .execute(
          {},
          { sessionID: "ses_fault", signal: new AbortController().signal },
        ),
  };
}
for (const mode of ["empty", "malformed"]) {
  test(
    `registration ${mode} output fails and recovers`,
    { concurrency: true },
    async () => {
      const fixture = await Fixture(mode, mode);
      try {
        await assert.rejects(
          fixture.invoke("amesh_whoami"),
          /registration returned empty or invalid JSON/,
        );
        await fixture.mode("ok");
        assert.equal(
          (await fixture.invoke("amesh_whoami")).content[0].text,
          "fault-peer",
        );
      } finally {
        await fixture.cleanup();
      }
    },
  );
}
test(
  "committed creation with lost response is not retried",
  { concurrency: true },
  async () => {
    const fixture = await Fixture("lost", "lost");
    try {
      await assert.rejects(
        fixture.invoke("amesh_job_create"),
        /outcome may be unknown/,
      );
      await new Promise((done) => setTimeout(done, 100));
      assert.equal(await readFile(fixture.committed, "utf8"), "created\n");
    } finally {
      await fixture.cleanup();
    }
  },
);
test(
  "cleanup kills an in-flight tool subprocess",
  { concurrency: true },
  async () => {
    const fixture = await Fixture("cleanup", "hang");
    const operation = assert.rejects(
      fixture.invoke("amesh_wait"),
      /abort|unknown/,
    );
    try {
      await Until(async () => {
        try {
          return await readFile(fixture.started);
        } catch {
          return false;
        }
      });
      const pid = Number(await readFile(fixture.started, "utf8"));
      await fixture.cleanup();
      await operation;
      assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
    } finally {
      await fixture.cleanup();
    }
  },
);
for (const [name, timeout] of [
  ["amesh_whoami", 15000],
  ["amesh_wait", 65000],
]) {
  test(
    `${name} timeout kills the subprocess after ${timeout}ms`,
    { concurrency: true },
    async () => {
      const fixture = await Fixture(name, "hang");
      try {
        const before = Date.now();
        await assert.rejects(
          fixture.invoke(name),
          new RegExp(`timeout ${timeout}ms.*outcome may be unknown`),
        );
        const elapsed = Date.now() - before;
        assert(elapsed >= timeout && elapsed < timeout + 5000);
        const pid = Number(await readFile(fixture.started, "utf8"));
        assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
      } finally {
        await fixture.cleanup();
      }
    },
  );
}

test(
  "cleanup bounds a stalled registration disposer",
  { concurrency: true },
  async () => {
    const fixture = await Fixture(
      "stalled-disposer",
      "ok",
      () => new Promise(() => {}),
    );
    const before = Date.now();
    let timer;
    try {
      await Promise.race([
        fixture.cleanup(),
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(new Error("cleanup exceeded 5s")),
            5000,
          );
        }),
      ]);
      assert(Date.now() - before < 5000);
    } finally {
      clearTimeout(timer);
    }
  },
);

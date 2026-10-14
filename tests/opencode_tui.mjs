import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
const source = await readFile(process.argv[2], "utf8");
const plugin = (
  await import(
    `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
  )
).default;
const Flush = () => new Promise((resolve) => setImmediate(resolve));
function Fixture(route = { type: "session", sessionID: "root-a" }) {
  const originalInterval = globalThis.setInterval;
  const originalClear = globalThis.clearInterval;
  let tick,
    listener,
    cleared = false,
    unlistened = false;
  const calls = [],
    toasts = [];
  let perform = async () => ({ output: { peerID: "peer" } });
  globalThis.setInterval = (callback) => {
    tick = callback;
    return { unref() {} };
  };
  globalThis.clearInterval = () => {
    cleared = true;
  };
  const sessions = new Map(
    ["root-a", "root-b", "child"].map((id) => [
      id,
      {
        id,
        parentID: id === "child" ? "root-a" : undefined,
        location: { directory: "/" + (id === "root-b" ? "b" : "a") },
        time: { idle: 1, viewed: 1 },
      },
    ]),
  );
  const ctx = {
    ui: {
      router: { current: () => route },
      toast: { show: (value) => toasts.push(value) },
    },
    data: {
      session: {
        root: (id) => (id === "child" ? "root-a" : id),
        get: (id) => sessions.get(id),
        status: () => "idle",
        family: (id) => [id],
        permission: { list: () => [] },
      },
      listen: (callback) => {
        listener = callback;
        return () => {
          unlistened = true;
        };
      },
    },
    client: {
      rpc: {
        call: (input, options) => {
          calls.push({ input, options });
          return perform(input, options);
        },
      },
    },
  };
  const cleanup = plugin.setup(ctx);
  return {
    ctx,
    calls,
    toasts,
    route: (next) => {
      route = next;
    },
    perform: (next) => {
      perform = next;
    },
    event: (event) => listener({ details: event }),
    tick: async () => {
      tick();
      await Flush();
    },
    cleanup,
    closed: () => cleared && unlistened,
    dispose() {
      cleanup();
      globalThis.setInterval = originalInterval;
      globalThis.clearInterval = originalClear;
    },
  };
}
test("home does not create or enroll historical sessions", async () => {
  const f = Fixture({ type: "home" });
  try {
    await f.tick();
    assert.equal(f.calls.length, 0);
  } finally {
    f.dispose();
  }
});
test("opening an already-read root immediately joins its own location", async () => {
  const f = Fixture();
  try {
    await Flush();
    assert.equal(f.calls.length, 1);
    assert.deepEqual(f.calls[0].input, {
      rpcID: "amesh",
      method: "join",
      input: { sessionID: "root-a", state: "idle" },
      location: { directory: "/a" },
    });
    await f.tick();
    assert.equal(f.calls.length, 1);
    f.route({ type: "session", sessionID: "child" });
    await f.tick();
    assert.equal(f.calls.length, 1);
    f.route({ type: "session", sessionID: "root-b" });
    await f.tick();
    assert.equal(f.calls.length, 2);
    assert.equal(f.calls[1].input.location.directory, "/b");
  } finally {
    f.dispose();
  }
});
test("shutdown and reconnection rejoin only the foreground root", async () => {
  const f = Fixture();
  try {
    await Flush();
    f.event({
      type: "location.shutdown",
      location: { directory: "/unrelated" },
    });
    await f.tick();
    assert.equal(f.calls.length, 1);
    f.event({ type: "location.shutdown", location: { directory: "/a" } });
    await f.tick();
    assert.equal(f.calls.length, 2);
    f.event({ type: "server.connected" });
    await f.tick();
    assert.equal(f.calls.length, 3);
  } finally {
    f.dispose();
  }
});
test("shutdown during an in-flight join cannot suppress rejoining", async () => {
  const f = Fixture({ type: "home" });
  let done;
  try {
    f.perform(
      () =>
        new Promise((resolve) => {
          done = resolve;
        }),
    );
    f.route({ type: "session", sessionID: "root-a" });
    await f.tick();
    f.event({ type: "location.shutdown", location: { directory: "/a" } });
    done({ output: {} });
    await Flush();
    f.perform(async () => ({ output: {} }));
    await f.tick();
    assert.equal(f.calls.length, 2);
  } finally {
    f.dispose();
  }
});
test("RPC failure backs off and cleanup cancels the request and observer", async () => {
  const f = Fixture({ type: "home" });
  try {
    f.perform(async () => {
      throw new Error("unavailable");
    });
    f.route({ type: "session", sessionID: "root-a" });
    await f.tick();
    await f.tick();
    assert.equal(f.calls.length, 1);
    assert.equal(f.toasts.length, 1);
    f.cleanup();
    assert(f.closed());
    assert(f.calls[0].options.signal.aborted);
    await f.tick();
    assert.equal(f.calls.length, 1);
  } finally {
    f.dispose();
  }
});

test("opening reports running and child approval states", async () => {
  const f = Fixture({ type: "home" });
  try {
    f.ctx.data.session.status = () => "running";
    f.route({ type: "session", sessionID: "root-a" });
    await f.tick();
    assert.equal(f.calls[0].input.input.state, "work");
    f.ctx.data.session.family = () => ["child"];
    f.ctx.data.session.permission.list = (id) =>
      id === "child" ? [{ id: "approval" }] : [];
    f.route({ type: "session", sessionID: "root-b" });
    await f.tick();
    assert.equal(f.calls[1].input.input.state, "wait");
  } finally {
    f.dispose();
  }
});

test("terminal join failure shows reload once and stops retrying", async () => {
  const f = Fixture({ type: "home" });
  try {
    f.perform(async () => {
      throw {
        type: "stopped",
        message: "amesh root is stopped; run opencode reload",
      };
    });
    f.route({ type: "session", sessionID: "root-a" });
    await f.tick();
    assert.match(f.toasts[0].message, /opencode reload/);
    await new Promise((resolve) => setTimeout(resolve, 1100));
    await f.tick();
    assert.equal(f.calls.length, 1);
    f.event({ type: "location.shutdown", location: { directory: "/a" } });
    await f.tick();
    assert.equal(f.calls.length, 2);
  } finally {
    f.dispose();
  }
});

test("foreground join refreshes after server-only plugin replacement", async () => {
  const originalNow = Date.now;
  let now = originalNow();
  Date.now = () => now;
  const f = Fixture();
  try {
    await Flush();
    now += 29999;
    await f.tick();
    assert.equal(f.calls.length, 1);
    now += 1;
    await f.tick();
    assert.equal(f.calls.length, 2);
    assert.equal(f.calls[1].input.input.sessionID, "root-a");
  } finally {
    f.dispose();
    Date.now = originalNow;
  }
});

test("terminal failure after a transient error still shows its action", async () => {
  const f = Fixture({ type: "home" });
  try {
    f.perform(async () => {
      throw new Error("temporary");
    });
    f.route({ type: "session", sessionID: "root-a" });
    await f.tick();
    f.perform(async () => {
      throw { type: "stopped", message: "run opencode reload" };
    });
    await new Promise((resolve) => setTimeout(resolve, 1100));
    await f.tick();
    assert.equal(f.toasts.length, 2);
    assert.match(f.toasts[1].message, /opencode reload/);
  } finally {
    f.dispose();
  }
});

/* amesh-plugin:opencode:v1 */
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { realpath } from "node:fs/promises";
import { resolve } from "node:path";
import { setTimeout as Delay } from "node:timers/promises";

const executable = __AMESH_EXECUTABLE__;
const configuredBind = __AMESH_BIND__;
const catalog = __AMESH_CATALOG__;

function Escape(value, attribute = false) {
  const text = String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;");
  return attribute
    ? text.replaceAll('"', "&quot;").replaceAll(">", "&gt;")
    : text;
}

function Message(event) {
  const fields = ["from_peer", "to_peer", "correlation_id", "circle", "job_id"]
    .filter((key) => event[key] != null)
    .map((key) => `${key}="${Escape(event[key], true)}"`)
    .join(" ");
  return `<peer-message type="${Escape(event.type, true)}" ${fields}>\n${Escape(event.message ?? event.text ?? event.query ?? "")}\n</peer-message>`;
}

function StableID(rootID, eventID) {
  if (rootID.includes("\0") || eventID.includes("\0"))
    throw new Error("invalid message identity");
  return (
    "msg_amesh_" +
    createHash("sha256")
      .update(rootID)
      .update("\0")
      .update(eventID)
      .digest("hex")
  );
}

export default {
  id: "amesh",
  async setup(ctx) {
    const stop = new AbortController();
    const roots = new Map();
    const children = new Set();
    const registrations = [];
    const env = { ...process.env };
    for (const key of [
      "AMESH_PEER_ID",
      "AMESH_CIRCLE",
      "AMESH_BACKEND",
      "CODEX_THREAD_ID",
    ])
      delete env[key];
    env.AMESH_BACKEND = "opencode";
    env.AMESH_BIND = env.AMESH_BIND || configuredBind;
    if (env.AMESH_STATE) env.AMESH_STATE = resolve(env.AMESH_STATE);
    const location = await realpath(ctx.location.directory);
    const Log = (error) => {
      if (stop.signal.aborted) return;
      let message = String(
        error?.message || error?._tag || error?.name || error,
      );
      if (env.AMESH_TOKEN)
        message = message.replaceAll(env.AMESH_TOKEN, "[redacted]");
      console.error(`[amesh] ${message}`);
    };
    const Valid = (root, generation) =>
      !stop.signal.aborted &&
      !root.stop.signal.aborted &&
      root.generation === generation;

    function Exec(root, args, input, timeout, signal) {
      return new Promise((done, reject) => {
        const combined = AbortSignal.any([
          stop.signal,
          root.stop.signal,
          ...(signal ? [signal] : []),
        ]);
        if (combined.aborted) {
          reject(
            new Error(
              "amesh operation aborted; committed outcome may be unknown",
            ),
          );
          return;
        }
        const child = execFile(
          executable,
          args,
          {
            cwd: root.cwd,
            env,
            timeout,
            signal: combined,
            maxBuffer: 16 * 1024 * 1024,
          },
          (error, stdout, stderr) => {
            if (error) {
              reject(
                new Error(
                  `${String(stderr || error.message).trim()}${error.killed ? ` (timeout ${timeout}ms)` : ""}; committed outcome may be unknown`,
                ),
              );
              return;
            }
            done(String(stdout).trim());
          },
        );
        children.add(child);
        child.once("exit", () => children.delete(child));
        child.once("error", () => {
          if (!child.pid) children.delete(child);
        });
        child.stdin.on("error", () => {});
        child.stdin.end(JSON.stringify(input) + "\n");
      });
    }

    async function Resolve(sessionID) {
      const seen = new Set();
      let info;
      while (true) {
        if (typeof sessionID !== "string" || !sessionID || seen.has(sessionID))
          throw new Error("invalid session parent chain");
        seen.add(sessionID);
        info = await ctx.session.get({ sessionID });
        if (
          (await realpath(info.location.directory)) !== location ||
          info.location.workspaceID !== ctx.location.workspaceID
        ) {
          throw Object.assign(
            new Error("session location does not match plugin location"),
            { code: "location" },
          );
        }
        if (!info.parentID) return info;
        sessionID = info.parentID;
      }
    }

    function Root(info) {
      let root = roots.get(info.id);
      if (!root) {
        root = {
          id: info.id,
          cwd: location,
          stop: new AbortController(),
          generation: 0,
          queue: [],
          bytes: 0,
          permissions: new Set(),
          activity: Promise.resolve(),
          attempts: 0,
        };
        roots.set(info.id, root);
      }
      if (root.stop.signal.aborted)
        throw Object.assign(
          new Error(
            "amesh root is stopped; run opencode reload to participate again",
          ),
          { code: "stopped" },
        );
      return root;
    }

    function Terminate(root, reason) {
      if (root.stop.signal.aborted) return;
      root.stop.abort();
      root.generation++;
      clearTimeout(root.retry);
      clearInterval(root.beat);
      root.socket?.close();
      root.socket = undefined;
      root.queue = [];
      root.bytes = 0;
      Log(
        new Error(
          `${root.id}: ${reason}; run opencode reload to participate again`,
        ),
      );
    }

    function Activity(root, state, reason, check = false) {
      if (!check) root.last = { state, reason };
      root.activity = root.activity
        .catch(() => {})
        .then(async () => {
          if (stop.signal.aborted || root.stop.signal.aborted || !root.peerID)
            return;
          await Exec(
            root,
            ["hook", state, "--backend=opencode"],
            { session_id: `opencode:${root.id}`, reason, check },
            12000,
          );
        })
        .catch(Log);
      return root.activity;
    }

    function Retry(root) {
      if (stop.signal.aborted || root.stop.signal.aborted || root.retry) return;
      const delay = Math.min(30000, 1000 * 2 ** Math.min(root.attempts++, 5));
      root.retry = setTimeout(() => {
        root.retry = undefined;
        Join(root, false).catch(Log);
      }, delay);
      root.retry.unref?.();
    }

    async function Drain(root, generation, socket) {
      if (root.draining === generation) return;
      root.draining = generation;
      try {
        while (
          Valid(root, generation) &&
          socket.readyState === 1 &&
          root.queue.length
        ) {
          const item = root.queue[0];
          let retries = 0;
          while (Valid(root, generation)) {
            try {
              const info = await ctx.session.get({ sessionID: root.id });
              if (info.revert) {
                await Delay(1000, undefined, { signal: root.stop.signal });
                continue;
              }
              if (!Valid(root, generation)) break;
              const sender =
                typeof item.event.from_peer === "string" &&
                /^[A-Za-z0-9._-]{1,128}$/.test(item.event.from_peer)
                  ? item.event.from_peer
                  : "anonymous";
              await ctx.session.synthetic({
                sessionID: root.id,
                id: StableID(root.id, item.event.id),
                text: Message(item.event),
                description: `amesh ${item.event.type} from @${sender}`,
                metadata: { ameshEventID: item.event.id },
                delivery: item.event.type === "broadcast" ? "queue" : "steer",
                resume: true,
              });
              if (Valid(root, generation) && socket.readyState === 1) {
                socket.send(
                  JSON.stringify({ type: "recv", id: item.event.id }),
                );
                root.failing = false;
                root.attempts = 0;
              }
              break;
            } catch (error) {
              if (!Valid(root, generation)) break;
              if (
                error._tag === "Session.NotFoundError" ||
                error.name === "Session.NotFoundError"
              ) {
                Terminate(root, "session no longer exists");
                break;
              }
              Log(
                new Error(
                  `${root.id}/${item.event.id}: ${error.message || error._tag || error.name || error}`,
                ),
              );
              if (retries++ >= 1) {
                const conflict =
                  error._tag === "Session.SyntheticConflictError" ||
                  error.name === "Session.SyntheticConflictError";
                if (!conflict) {
                  root.failing = true;
                  socket.close();
                }
                break;
              }
              await Delay(1000, undefined, { signal: root.stop.signal });
            }
          }
          if (Valid(root, generation)) {
            root.queue.shift();
            root.bytes -= item.bytes;
          }
        }
      } catch (error) {
        if (Valid(root, generation)) Log(error);
      } finally {
        if (root.draining === generation) root.draining = undefined;
      }
    }

    function Connect(root) {
      if (stop.signal.aborted || root.stop.signal.aborted) return;
      const generation = ++root.generation;
      root.queue = [];
      root.bytes = 0;
      const socket = new WebSocket(`ws://${env.AMESH_BIND}/ws`);
      root.socket = socket;
      let connected = false;
      const handshake = setTimeout(() => socket.close(), 12000);
      handshake.unref?.();
      socket.addEventListener("open", () => {
        if (!Valid(root, generation)) {
          socket.close();
          return;
        }
        socket.send(
          JSON.stringify({
            type: "connect",
            peer_id: root.peerID,
            recv: true,
            auth_token: env.AMESH_TOKEN,
          }),
        );
      });
      socket.addEventListener("message", (message) => {
        if (!Valid(root, generation)) return;
        try {
          const text = String(message.data);
          const bytes = Buffer.byteLength(text);
          if (bytes > 16 * 1024 * 1024) {
            Terminate(
              root,
              "inbound frame exceeds 16 MiB; manual intervention required",
            );
            return;
          }
          const event = JSON.parse(text);
          if (event.type === "displaced" || event.type === "replaced") {
            Terminate(root, event.type);
            return;
          }
          if (event.type === "connected") {
            if (
              event.session_id !== `opencode:${root.id}` ||
              event.peer_id !== root.peerID
            ) {
              Terminate(root, "connected identity mismatch");
              return;
            }
            connected = true;
            clearTimeout(handshake);
            if (!root.failing) root.attempts = 0;
            if (root.last)
              void Activity(root, root.last.state, root.last.reason, true);
            root.beat = setInterval(() => {
              if (Valid(root, generation) && socket.readyState === 1)
                socket.send(JSON.stringify({ type: "ping" }));
            }, 10000);
            root.beat.unref?.();
            return;
          }
          if (event.type === "pong") return;
          if (
            !connected ||
            !["ask", "notify", "ack", "broadcast"].includes(event.type) ||
            typeof event.id !== "string" ||
            !event.id ||
            event.id.includes("\0")
          )
            throw new Error("invalid inbound frame");
          if (
            root.queue.length >= 128 ||
            root.bytes + bytes > 32 * 1024 * 1024
          ) {
            socket.close();
            return;
          }
          root.queue.push({ event, bytes });
          root.bytes += bytes;
          void Drain(root, generation, socket);
        } catch (error) {
          Log(error);
        }
      });
      socket.addEventListener("error", () => {
        if (Valid(root, generation))
          Log(new Error(`${root.id}: websocket connection error`));
      });
      socket.addEventListener("close", () => {
        clearTimeout(handshake);
        if (!Valid(root, generation)) return;
        root.generation++;
        clearInterval(root.beat);
        root.socket = undefined;
        root.queue = [];
        root.bytes = 0;
        Retry(root);
      });
    }

    async function Join(root, initial) {
      if (root.stop.signal.aborted || stop.signal.aborted)
        throw Object.assign(
          new Error("amesh root is stopped; run opencode reload"),
          { code: "stopped" },
        );
      if (root.join) return root.join;
      if (root.socket && root.socket.readyState < 2) return;
      root.join = (async () => {
        const output = await Exec(
          root,
          ["hook", "session", "--backend=opencode"],
          { session_id: `opencode:${root.id}`, cwd: root.cwd },
          12000,
        );
        let registered;
        try {
          registered = JSON.parse(output);
        } catch {
          throw new Error("amesh registration returned empty or invalid JSON");
        }
        if (
          !/^[A-Za-z0-9._-]{1,128}$/.test(registered.peer_id ?? "") ||
          typeof registered.circle !== "string" ||
          !registered.circle ||
          typeof registered.context !== "string"
        )
          throw new Error("amesh registration returned invalid identity");
        if (root.stop.signal.aborted || stop.signal.aborted) return;
        root.peerID = registered.peer_id;
        root.primer = registered.context;
        if (initial) await Activity(root, "work", "local use");
        Connect(root);
      })();
      try {
        await root.join;
      } catch (error) {
        Retry(root);
        throw error;
      } finally {
        root.join = undefined;
      }
    }

    async function Event(event) {
      if (event.type === "location.shutdown") {
        if (
          event.location?.directory === ctx.location.directory &&
          event.location?.workspaceID === ctx.location.workspaceID
        )
          void Cleanup();
        return;
      }
      const sessionID = event.data?.sessionID;
      if (!sessionID) return;
      if (event.type === "session.deleted" || event.type === "session.moved") {
        const root = roots.get(sessionID);
        if (root) Terminate(root, event.type);
        return;
      }
      if (
        !event.type.startsWith("session.execution.") &&
        ![
          "permission.asked",
          "permission.replied",
          "session.retry.scheduled",
        ].includes(event.type)
      )
        return;
      let session;
      try {
        session = await ctx.session.get({ sessionID });
      } catch (error) {
        if (
          error._tag === "Session.NotFoundError" ||
          error.name === "Session.NotFoundError"
        )
          return;
        throw error;
      }
      let directory;
      try {
        directory = await realpath(session.location.directory);
      } catch {
        return;
      }
      if (
        directory !== location ||
        session.location.workspaceID !== ctx.location.workspaceID
      )
        return;
      const info = await Resolve(sessionID);
      const root = roots.get(info.id);
      if (!root || root.stop.signal.aborted) return;
      if (event.type === "permission.asked") {
        root.permissions.add(event.data.id);
        void Activity(root, "wait", event.data.action);
      } else if (event.type === "permission.replied") {
        root.permissions.delete(event.data.requestID);
        if (!root.permissions.size)
          void Activity(root, "work", "permission resolved");
      } else if (info.id === sessionID) {
        if (
          event.type === "session.execution.started" ||
          event.type === "session.retry.scheduled"
        ) {
          if (!root.permissions.size) void Activity(root, "work", event.type);
        } else {
          root.permissions.clear();
          void Activity(root, "idle", event.data.reason ?? event.type);
        }
      }
    }

    const events = (async () => {
      try {
        for await (const event of ctx.event.subscribe({
          signal: stop.signal,
        })) {
          try {
            await Event(event);
          } catch (error) {
            Log(error);
          }
        }
      } catch (error) {
        Log(error);
      }
    })();
    let cleaning;
    function Cleanup() {
      return (cleaning ??= (async () => {
        const exited = [...children].map(
          (child) =>
            new Promise((done) => {
              if (
                !child.pid ||
                child.exitCode !== null ||
                child.signalCode !== null
              ) {
                done();
                return;
              }
              const deadline = setTimeout(() => child.kill("SIGKILL"), 2000);
              child.once("exit", () => {
                clearTimeout(deadline);
                done();
              });
            }),
        );
        stop.abort();
        for (const root of roots.values()) Terminate(root, "plugin cleanup");
        for (const child of children) child.kill();
        await Promise.race([
          Promise.allSettled([
            ...exited,
            ...registrations.map((registration) => registration.dispose()),
          ]),
          Delay(4000, undefined, { ref: false }),
        ]);
        await Promise.race([events, Delay(2000, undefined, { ref: false })]);
      })());
    }
    try {
      registrations.push(
        await ctx.rpc.register(
          {
            id: "amesh",
            methods: {
              join: {
                input: {
                  type: "object",
                  properties: {
                    sessionID: { type: "string", minLength: 1 },
                    state: { enum: ["idle", "work", "wait"] },
                  },
                  required: ["sessionID", "state"],
                  additionalProperties: false,
                },
                errors: {
                  stopped: { type: "object" },
                  location: { type: "object" },
                },
                output: {
                  type: "object",
                  properties: { peerID: { type: "string" } },
                  required: ["peerID"],
                  additionalProperties: false,
                },
              },
            },
            events: {},
          },
          {
            join: async ({ sessionID, state }, context) => {
              try {
                const root = Root(await Resolve(sessionID));
                if (!root.last) root.last = { state, reason: "session opened" };
                const fresh = !root.socket || root.socket.readyState >= 2;
                await Join(root, false);
                if (fresh)
                  await Activity(root, root.last.state, root.last.reason, true);
                return { peerID: root.peerID };
              } catch (error) {
                if (error?.code === "stopped" || error?.code === "location")
                  return context.error(error.code, error.message, {});
                throw error;
              }
            },
          },
        ),
      );
      registrations.push(
        await ctx.tool.transform((editor) => {
          for (const definition of catalog)
            editor.add({
              name: definition.name,
              description: definition.description,
              input: definition.inputSchema,
              options: { codemode: false },
              async execute(input, context) {
                const root = Root(await Resolve(context.sessionID));
                await Join(root, !root.peerID);
                const output = await Exec(
                  root,
                  ["mcp", "--peer-id", root.peerID],
                  {
                    jsonrpc: "2.0",
                    id: 1,
                    method: "tools/call",
                    params: { name: definition.name, arguments: input },
                  },
                  definition.name === "amesh_wait" ? 65000 : 15000,
                  context.signal,
                );
                let reply;
                try {
                  reply = JSON.parse(output.split("\n").at(-1));
                  if (!reply || typeof reply !== "object")
                    throw new Error("invalid reply");
                } catch {
                  throw new Error(
                    "amesh tool returned invalid JSON; committed outcome may be unknown",
                  );
                }
                if (reply.error || reply.result?.isError)
                  throw new Error(
                    reply.error?.message ??
                      JSON.stringify(reply.result.content),
                  );
                if (!Array.isArray(reply.result?.content))
                  throw new Error(
                    "amesh tool returned invalid content; committed outcome may be unknown",
                  );
                return { content: reply.result.content };
              },
            });
        }),
      );
      registrations.push(
        await ctx.session.hook("prompt", async (input) => {
          try {
            const info = await ctx.session.get({ sessionID: input.sessionID });
            if (info.parentID) return;
            const root = Root(await Resolve(input.sessionID));
            void Join(root, true)
              .then(() => Activity(root, "work", "prompt"))
              .catch(Log);
          } catch (error) {
            Log(error);
          }
        }),
      );
      registrations.push(
        await ctx.session.hook("context", async (input) => {
          try {
            const info = await Resolve(input.sessionID);
            const root = roots.get(info.id);
            if (!root || root.stop.signal.aborted) return;
            if (root.join)
              await Promise.race([
                root.join,
                Delay(2000, undefined, { signal: stop.signal }),
              ]);
            if (root.primer)
              input.system.push({
                type: "text",
                text:
                  root.primer +
                  `\nYou act as root session ${root.id}. Replies enter that root; child agents share its identity and ACK responsibility.`,
              });
          } catch (error) {
            Log(error);
          }
        }),
      );
      return Cleanup;
    } catch (error) {
      await Cleanup();
      throw error;
    }
  },
};

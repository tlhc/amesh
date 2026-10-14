import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { once } from "node:events";

const root = await mkdtemp(resolve("build/opencode-validation/probe-"));
const config = join(root, "config");
const workspace = join(root, "workspace");
await mkdir(join(config, "plugins"), { recursive: true });
await mkdir(workspace);
execFileSync("git", ["init", "-q", workspace]);
const log = join(root, "plugin.jsonl");
const requests = [];
const server = createServer(async (request, response) => {
  let text = "";
  for await (const chunk of request) text += chunk;
  const body = JSON.parse(text);
  requests.push(body);
  const called = body.messages.some((message) => message.role === "tool");
  const chunks = called
    ? [
        {
          choices: [
            {
              delta: { role: "assistant", content: "probe complete" },
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
                    id: "call_probe",
                    type: "function",
                    function: { name: "amesh_probe", arguments: "{}" },
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
server.listen(0, "127.0.0.1");
await once(server, "listening");
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
          apiKey: "isolated-fixture",
          baseURL: `http://127.0.0.1:${server.address().port}/v1`,
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
await writeFile(
  join(config, "plugins/probe.js"),
  `
import { appendFileSync } from "node:fs";
const Log = (value) => appendFileSync(${JSON.stringify(log)}, JSON.stringify(value) + "\\n");
export default { id: "amesh-probe", async setup(ctx) {
  Log({type:"setup", location:ctx.location});
  const stop = new AbortController();
  const events = (async () => { try { for await (const event of ctx.event.subscribe({signal:stop.signal})) Log({type:"event",event}); } catch(error) { if(!stop.signal.aborted) Log({type:"error",error:String(error)}); } })();
  await ctx.tool.transform((editor) => editor.add({name:"amesh_probe",description:"Probe the adapter",input:{type:"object",properties:{}},options:{codemode:false},async execute(input,context) {
    const info = await ctx.session.get({sessionID:context.sessionID});
    Log({type:"tool", info, signal:context.signal instanceof AbortSignal});
    const message = {sessionID:context.sessionID,id:"msg_amesh_probe",text:"probe durable admission",delivery:"queue",resume:false};
    const first = await ctx.session.synthetic(message);
    const second = await ctx.session.synthetic(message);
    Log({type:"synthetic",first,second});
    for (const [kind, candidate] of [["missing",{...message,sessionID:"ses_missing"}],["conflict",{...message,id:context.messageID}]]) {
      try { await ctx.session.synthetic(candidate); Log({type:"unexpected-success",kind}); }
      catch(error) { Log({type:"rejection",kind,tag:error._tag,name:error.name,message:error.message,keys:Object.getOwnPropertyNames(error),string:String(error)}); }
    }
    return {content:"probe success"};
  }}));
  await ctx.session.hook("prompt", (input) => Log({type:"prompt",sessionID:input.sessionID}));
  await ctx.session.hook("context", (input) => { Log({type:"context",sessionID:input.sessionID,system:input.system}); input.system.push({type:"text",text:"AMESH_PROBE_PRIMER"}); });
  return async () => { stop.abort(); await events; Log({type:"cleanup"}); };
}};
`,
);
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
  OPENCODE_MODELS_PATH: join(root, "models.json"),
  OPENCODE_DB: join(root, "opencode.db"),
  OPENCODE_DISABLE_MODELS_FETCH: "true",
  OPENCODE_DISABLE_AUTOUPDATE: "true",
  XDG_CONFIG_HOME: config,
  XDG_CACHE_HOME: join(root, "cache"),
  XDG_DATA_HOME: join(root, "data"),
  XDG_STATE_HOME: join(root, "state"),
});
await writeFile(join(root, "models.json"), "{}");
const child = spawn(
  "opencode",
  [
    "run",
    "--standalone",
    "--auto",
    "--format",
    "json",
    "Run amesh_probe then finish.",
  ],
  { cwd: workspace, env, stdio: ["ignore", "pipe", "pipe"] },
);
let stdout = "",
  stderr = "";
child.stdout.on("data", (chunk) => (stdout += chunk));
child.stderr.on("data", (chunk) => (stderr += chunk));
const timer = setTimeout(() => child.kill("SIGTERM"), 60000);
try {
  const [code] = await once(child, "exit");
  await writeFile(join(root, "output.log"), stdout + "\n" + stderr);
  await writeFile(
    join(root, "requests.json"),
    JSON.stringify(requests, null, 2),
  );
  assert.equal(code, 0, stderr);
  const events = (await readFile(log, "utf8"))
    .trim()
    .split("\n")
    .map(JSON.parse);
  for (const type of [
    "setup",
    "prompt",
    "context",
    "tool",
    "synthetic",
    "cleanup",
  ])
    assert(
      events.some((event) => event.type === type),
      type,
    );
  assert(
    requests.some((body) =>
      body.tools?.some((tool) => tool.function.name === "amesh_probe"),
    ),
    "direct tool visibility",
  );
  assert(
    requests.some((body) =>
      JSON.stringify(body.messages).includes("AMESH_PROBE_PRIMER"),
    ),
    "context primer",
  );
  const synthetic = events.find((event) => event.type === "synthetic");
  assert.deepEqual(synthetic.first, synthetic.second, "same ID replay");
  assert(
    events.some((event) => event.event?.type === "session.execution.started"),
    "execution event",
  );
  console.log(
    "PASS real OpenCode plugin loading, tool execution, hooks, events, synthetic replay, cleanup:",
    root,
  );
} finally {
  clearTimeout(timer);
  child.kill("SIGTERM");
  server.closeAllConnections();
  await new Promise((done) => server.close(done));
}

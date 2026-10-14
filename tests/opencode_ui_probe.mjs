import assert from "node:assert/strict";
import { resolve } from "node:path";
import { mock } from "bun:test";

const upstream = resolve(process.argv[2]);
const tui = resolve(upstream, "packages/tui");
const solid = await import(resolve(tui, "node_modules/solid-js/dist/solid.js"));
mock.module(Bun.resolveSync("solid-js", tui + "/src"), () => solid);
const store = await import(
  resolve(tui, "node_modules/solid-js/store/dist/store.js")
);
mock.module(Bun.resolveSync("solid-js/store", tui + "/src"), () => store);
const [boundary, setBoundary] = solid.createSignal();
const messages = [
  { id: "msg_000001", type: "user", text: "before", time: { created: 1 } },
  {
    id: "msg_amesh_hash",
    type: "synthetic",
    text: "collaboration",
    description: "amesh notify from @peer",
    time: { created: 2 },
  },
  { id: "msg_000003", type: "user", text: "boundary", time: { created: 3 } },
];
const data = {
  on: () => () => {},
  session: {
    get: () => ({ revert: boundary() ? { messageID: boundary() } : undefined }),
    permission: { list: () => [] },
    input: { list: () => [], has: () => false },
    pending: { list: () => [], sync: async () => {} },
    message: {
      list: () => messages,
      sync: async () => {},
      more: () => false,
      get: (_, id) => messages.find((message) => message.id === id),
    },
  },
};
mock.module(resolve(tui, "src/config/index.tsx"), () => ({
  useConfig: () => ({ data: {} }),
}));
mock.module(resolve(tui, "src/context/data.tsx"), () => ({
  useData: () => data,
}));
mock.module(resolve(tui, "src/context/client.tsx"), () => ({
  useClient: () => ({ connection: { status: () => "connected" } }),
}));
const { createSessionRows, reduceSessionRows } = await import(
  resolve(tui, "src/routes/session/rows.ts")
);
assert.equal(reduceSessionRows(messages).length, 3);
assert.equal(
  reduceSessionRows(
    messages.map((message) => ({ ...message, description: undefined })),
  ).length,
  2,
  "synthetic without description is hidden",
);
let dispose;
const rows = solid.createRoot((stop) => {
  dispose = stop;
  return createSessionRows(() => "ses_probe");
});
const tick = () => new Promise((done) => setTimeout(done, 0));
try {
  await tick();
  assert.deepEqual(
    rows.map((row) => row.messageID),
    messages.map((message) => message.id),
  );
  setBoundary("msg_000003");
  await tick();
  assert.deepEqual(
    rows.map((row) => row.messageID),
    ["msg_000001"],
    "known hash-ID revert preview limitation",
  );
  setBoundary(undefined);
  await tick();
  assert.deepEqual(
    rows.map((row) => row.messageID),
    messages.map((message) => message.id),
  );
  console.log(
    "PASS actual TUI row projection: description visibility, history order, hash-ID revert limitation, clear recovery",
  );
} finally {
  dispose();
}

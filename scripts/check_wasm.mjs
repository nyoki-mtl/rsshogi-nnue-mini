// node scripts/check_wasm.mjs <engine.wasm> [model.rsnn]
// Requires a real model argument or an engine built with embedded-rsnn.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { Worker } from "node:worker_threads";
import { MiniEngine } from "../bindings/wasm/mini.js";

const [wasmPath, modelPath] = process.argv.slice(2);
if (!wasmPath) throw new Error("usage: node scripts/check_wasm.mjs <engine.wasm> [model.rsnn]");
const wasm = await readFile(wasmPath);
const model = modelPath ? await readFile(modelPath) : undefined;
const engine = await MiniEngine.create(wasm);
const ok = value => { assert.equal(value.ok, true, JSON.stringify(value)); return value; };
const load = engine => ok(model ? engine.loadModel(model) : engine.request({ command: "load_embedded" }));
try {
  load(engine);
  ok(engine.request({ command: "configure", hash_mb: 4, threads: 1 }));
  assert.equal(engine.request({ command: "configure", hash_mb: 4, threads: 2 }).ok, false);
  ok(engine.request({ command: "set_position", sfen: "startpos", moves: [] }));
  assert(Number.isFinite(ok(engine.request({ command: "evaluate" })).score));
  const result = ok(engine.request({ command: "search", depth: 1 })).result;
  assert.equal(result.depth, 1);
  assert.match(result.bestmove, /^[1-9][a-i][1-9][a-i]\+?$/);
  const nodes = ok(engine.request({ command: "search", depth: 64, nodes: 1 })).result;
  assert(nodes.nodes <= 1);
  const started = performance.now();
  const timed = ok(engine.request({ command: "search", depth: 64, movetime_ms: 10 })).result;
  assert(timed.depth < 64);
  assert(performance.now() - started < 3000, "time bound did not stop search");
  engine.stop();
  ok(engine.request({ command: "new_game" }));
} finally { engine.dispose(); }
assert.throws(() => engine.request({ command: "evaluate" }), /disposed/);
engine.dispose();

// Custom embeddings must fail bounded searches safely even with a broken clock.
for (const badTime of [Infinity, NaN]) {
  const { instance } = await WebAssembly.instantiate(wasm, { env: { mini_now_ms: () => badTime } });
  const invalidClock = new MiniEngine(instance.exports);
  try {
    load(invalidClock);
    ok(invalidClock.request({ command: "configure", hash_mb: 4, threads: 1 }));
    const result = ok(invalidClock.request({ command: "search", depth: 64, nodes: 1000, movetime_ms: 10 })).result;
    assert.equal(result.depth, 0, "invalid clock did not expire the search budget");
  } finally { invalidClock.dispose(); }
}

// Adapt the browser Worker transport only; the actual worker module is unchanged.
const workerUrl = new URL("../bindings/wasm/worker.js", import.meta.url).href;
const bootstrap = `import { parentPort } from 'node:worker_threads';
globalThis.self = globalThis;
globalThis.postMessage = value => parentPort.postMessage(value);
await import(${JSON.stringify(workerUrl)});
parentPort.on('message', data => self.onmessage({data}));`;
const worker = new Worker(new URL(`data:text/javascript,${encodeURIComponent(bootstrap)}`));
const messages = [];
const waiters = new Set();
worker.on("message", message => {
  messages.push(message);
  for (const waiter of waiters) waiter();
});
const wait = predicate => new Promise((resolve, reject) => {
  const timeout = setTimeout(() => { waiters.delete(check); reject(new Error(`worker timeout; messages: ${JSON.stringify(messages)}`)); }, 15000);
  const check = () => {
    const found = messages.find(predicate);
    if (found) { clearTimeout(timeout); waiters.delete(check); resolve(found); }
  };
  waiters.add(check);
  check();
});
const send = (command, id, fields = {}) => worker.postMessage({ command, id, ...fields });
try {
  send("init", "init", { wasm, model, hash_mb: 4 });
  ok(await wait(message => message.id === "init"));
  send("position", "position", { sfen: "startpos" });
  ok(await wait(message => message.id === "position"));
  send("go", "unbounded", { depth: 20 });
  const unbounded = await wait(message => message.id === "unbounded");
  assert.equal(unbounded.ok, false);
  assert.match(unbounded.error, /requires movetime_ms or nodes/);
  assert(!messages.some(message => message.id === "unbounded" && message.event));
  send("go", "cancel", { depth: 64, movetime_ms: 3000 });
  ok(await wait(message => message.id === "cancel" && message.event === "info"));
  send("stop", "stop");
  const cancelled = ok(await wait(message => message.id === "cancel" && message.event === "bestmove"));
  assert(cancelled.result.bestmove);
  ok(await wait(message => message.id === "stop"));
  send("stop", "stop-again");
  ok(await wait(message => message.id === "stop-again"));
  assert.equal(messages.filter(message => message.id === "cancel" && message.event === "bestmove").length, 1);
  send("go", "old", { depth: 64, movetime_ms: 3000 });
  send("position", "replacement", { sfen: "startpos", moves: ["7g7f"] });
  send("go", "new", { depth: 1, movetime_ms: 1000 });
  ok(await wait(message => message.id === "new" && message.event === "bestmove"));
  const boundary = messages.findIndex(message => message.id === "replacement");
  assert(!messages.slice(boundary + 1).some(message => message.id === "old"));
  send("go", "budget", { depth: 64, nodes: 5, movetime_ms: 200 });
  const budget = ok(await wait(message => message.id === "budget" && message.event === "bestmove"));
  assert(budget.result.nodes <= 5);
  send("go", "time", { depth: 64, movetime_ms: 45 });
  const time = ok(await wait(message => message.id === "time" && message.event === "bestmove"));
  assert(time.result.elapsed_ms < 3000);
  send("new_game", "new_game");
  ok(await wait(message => message.id === "new_game"));
  send("go", "disposed-go", { depth: 64, movetime_ms: 3000 });
  send("dispose", "dispose");
  ok(await wait(message => message.id === "dispose"));
  send("go", "after-dispose", { depth: 1 });
  assert.equal((await wait(message => message.id === "after-dispose")).ok, false);
  const disposedAt = messages.findIndex(message => message.id === "dispose");
  assert(!messages.slice(disposedAt + 1).some(message => message.id === "disposed-go"));
} finally { await worker.terminate(); }
console.log("Wasm smoke passed: real model, evaluation, finite depth/node/time bounds, worker stop/newgo/dispose.");

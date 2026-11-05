// Usage: node bindings/wasm/check.mjs [engine.wasm] [model.rsnn]
// Runs JS binding checks unconditionally; when engine.wasm is supplied, also verifies
// Wasm instantiation/lifecycle and (if embedded or model.rsnn is given) search + Worker.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { Worker } from "node:worker_threads";
import { MiniEngine } from "./mini.js";

// 1. Model-free HTTP fetch error handling (before compiling response body).
{
  const original = globalThis.fetch;
  let bodyRead = false;
  globalThis.fetch = async () => ({
    ok: false,
    status: 404,
    statusText: "Not Found",
    arrayBuffer() {
      bodyRead = true;
      throw new Error("body must not be compiled");
    },
  });
  try {
    await assert.rejects(
      MiniEngine.create("https://example.invalid/engine.wasm"),
      /HTTP 404 Not Found/,
    );
    assert.equal(bodyRead, false);
  } finally {
    globalThis.fetch = original;
  }
}

const [wasmPath, modelPath] = process.argv.slice(2);
if (!wasmPath) {
  console.log("Wasm binding fetch check passed.");
  process.exit(0);
}

const wasm = await readFile(wasmPath);
const model = modelPath ? await readFile(modelPath) : undefined;
const ok = value => {
  assert.equal(value.ok, true, JSON.stringify(value));
  return value;
};

// 2. Direct Wasm module lifecycle and validation checks.
const engine = await MiniEngine.create(wasm);
let hasModel = false;
try {
  assert.equal(engine.request({ command: "configure", hash_mb: 0, threads: 1 }).ok, false);
  assert.equal(engine.request({ command: "configure", hash_mb: 4, threads: 2 }).ok, false);
  ok(engine.request({ command: "configure", hash_mb: 4, threads: 1 }));
  assert.equal(
    engine.request({ command: "set_position", sfen: "startpos", moves: ["7g7e"] }).ok,
    false,
  );
  ok(engine.request({ command: "set_position", sfen: "startpos", moves: [] }));
  assert.equal(engine.loadModel(new Uint8Array([1, 2, 3])).ok, false);
  const loaded = model ? engine.loadModel(model) : engine.request({ command: "load_embedded" });
  if (model) ok(loaded);
  hasModel = loaded.ok === true;
  if (hasModel) {
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
  } else {
    assert.equal(engine.request({ command: "evaluate" }).ok, false);
  }
  engine.stop();
  ok(engine.request({ command: "new_game" }));
} finally {
  engine.dispose();
}
assert.throws(() => engine.request({ command: "evaluate" }), /disposed/);
engine.dispose();

if (hasModel) {
  const load = instanceEngine =>
    ok(model ? instanceEngine.loadModel(model) : instanceEngine.request({ command: "load_embedded" }));
  for (const badTime of [Infinity, NaN]) {
    const { instance } = await WebAssembly.instantiate(wasm, { env: { mini_now_ms: () => badTime } });
    const invalidClock = new MiniEngine(instance.exports);
    try {
      load(invalidClock);
      ok(invalidClock.request({ command: "configure", hash_mb: 4, threads: 1 }));
      const result = ok(
        invalidClock.request({ command: "search", depth: 64, nodes: 1000, movetime_ms: 10 }),
      ).result;
      assert.equal(result.depth, 0, "invalid clock did not expire the search budget");
    } finally {
      invalidClock.dispose();
    }
  }
}

// 3. Worker transport checks (unmodified worker.js running on node:worker_threads).
const workerUrl = new URL("./worker.js", import.meta.url).href;
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
const wait = predicate =>
  new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      waiters.delete(check);
      reject(new Error(`worker timeout; messages: ${JSON.stringify(messages)}`));
    }, 15000);
    const check = () => {
      const found = messages.find(predicate);
      if (found) {
        clearTimeout(timeout);
        waiters.delete(check);
        resolve(found);
      }
    };
    waiters.add(check);
    check();
  });
const send = (command, id, fields = {}) => worker.postMessage({ command, id, ...fields });
try {
  if (!hasModel) {
    send("init", "init-no-model", { wasm, hash_mb: 4 });
    assert.equal((await wait(message => message.id === "init-no-model")).ok, false);
    send("position", "pos-uninitialized", { sfen: "startpos" });
    assert.equal((await wait(message => message.id === "pos-uninitialized")).ok, false);
    send("dispose", "dispose");
    ok(await wait(message => message.id === "dispose"));
  } else {
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
  }
} finally {
  await worker.terminate();
}
console.log(
  hasModel
    ? "Wasm smoke passed: real model, evaluation, bounded search, worker stop/newgo/dispose."
    : "Wasm smoke passed: model-free lifecycle, bounds validation, and worker error/dispose paths.",
);

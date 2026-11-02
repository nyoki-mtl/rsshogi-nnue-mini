import { MiniEngine } from "./mini.js";

// Each slice is a separate search retaining TT/history, with a 35 ms deadline.
// This is cooperative cancellation, not native fixed-time search equivalence.
// Messages: {command, id?, ...}; replies echo id and command. Search progress is
// {event:"info"}; exactly one {event:"bestmove"} ends a current go/stop pair.
let engine = null;
let generation = 0;
let active = null;
let initialization = 0;
const sliceMs = 35;

const reply = (message, data) => postMessage({ id: message.id, command: message.command, ...data });
const requireEngine = () => {
  if (!engine) throw new Error("engine is not initialized");
  return engine;
};
const unwrap = response => {
  if (!response.ok) throw new Error(response.error);
  return response;
};
const invalidate = () => {
  generation++;
  active = null;
};
const finish = job => {
  if (active !== job || job.generation !== generation) return;
  active = null;
  reply(job.message, { ok: true, event: "bestmove", result: { ...job.latest, elapsed_ms: Math.round(performance.now() - job.started) } });
};

function slice(job) {
  if (active !== job || job.generation !== generation) return;
  if (job.latest && ((job.time !== undefined && performance.now() - job.started >= job.time)
      || (job.nodes !== undefined && job.usedNodes >= job.nodes))) {
    finish(job);
    return;
  }
  try {
    const remainingMs = job.time === undefined ? sliceMs : Math.max(1, Math.ceil(job.time - (performance.now() - job.started)));
    const request = { command: "search", depth: job.depth, movetime_ms: Math.min(sliceMs, remainingMs) };
    if (job.nodes !== undefined) request.nodes = Math.max(1, job.nodes - job.usedNodes);
    const { result } = unwrap(requireEngine().request(request));
    job.usedNodes += result.nodes;
    // A depth-zero fallback is usable for stop, but cannot replace a completed PV.
    if (!job.latest || result.depth >= job.latest.depth) job.latest = result;
    job.latest = { ...job.latest, nodes: job.usedNodes, elapsed_ms: Math.round(performance.now() - job.started) };
    reply(job.message, { ok: true, event: "info", result: job.latest });
    if (result.depth >= job.target || ["resign", "win"].includes(result.bestmove)
        || (job.nodes !== undefined && job.usedNodes >= job.nodes)
        || (job.time !== undefined && performance.now() - job.started >= job.time)) {
      finish(job);
      return;
    }
    if (result.depth >= job.depth) job.depth++;
    setTimeout(() => slice(job), 0);
  } catch (error) {
    if (active === job) {
      active = null;
      reply(job.message, { ok: false, error: String(error.message ?? error) });
    }
  }
}

self.onmessage = async ({ data: message }) => {
  try {
    switch (message.command) {
      case "init": {
        invalidate();
        const token = ++initialization;
        engine?.dispose();
        engine = null;
        const created = await MiniEngine.create(message.url ?? message.wasm);
        if (token !== initialization) { created.dispose(); return; }
        try {
          const loaded = unwrap(message.model ? created.loadModel(message.model) : created.request({ command: "load_embedded" }));
          unwrap(created.request({ command: "configure", hash_mb: message.hash_mb ?? 16, threads: 1 }));
          engine = created;
          reply(message, loaded);
        } catch (error) { created.dispose(); throw error; }
        break;
      }
      case "position":
        invalidate();
        reply(message, requireEngine().request({ command: "set_position", sfen: message.sfen ?? "startpos", moves: message.moves ?? [] }));
        break;
      case "new_game":
        invalidate();
        reply(message, requireEngine().request({ command: "new_game" }));
        break;
      case "go": {
        requireEngine();
        const target = message.depth ?? 64;
        if (!Number.isInteger(target) || target < 1 || target > 64) throw new Error("depth must be an integer in 1..64");
        for (const field of ["nodes", "movetime_ms"]) {
          if (message[field] !== undefined && (!Number.isSafeInteger(message[field]) || message[field] < 1)) throw new Error(`${field} must be a positive safe integer`);
        }
        if (message.nodes === undefined && message.movetime_ms === undefined) {
          throw new Error("worker go requires movetime_ms or nodes; depth alone cannot bound cooperative slices");
        }
        invalidate();
        const job = { message, generation, target, depth: 1, nodes: message.nodes, time: message.movetime_ms, usedNodes: 0, started: performance.now(), latest: null };
        active = job;
        // Run one bounded slice before yielding, so an immediate stop has a move.
        slice(job);
        break;
      }
      case "stop":
        if (active) finish(active);
        reply(message, { ok: true });
        break;
      case "dispose":
        invalidate();
        initialization++;
        engine?.dispose();
        engine = null;
        reply(message, { ok: true });
        break;
      default:
        throw new Error("unknown worker command");
    }
  } catch (error) {
    reply(message, { ok: false, error: String(error.message ?? error) });
  }
};

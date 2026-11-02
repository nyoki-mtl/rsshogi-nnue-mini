// Usage: node scripts/check_wasm_browser.mjs <embedded-sdk-dir> [playwright module or index.mjs]
// Playwright is a verification tool, not a runtime dependency of the distributed SDK.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { resolve, extname } from "node:path";
import { pathToFileURL } from "node:url";

const directory = resolve(process.argv[2] ?? "dist/wasm-sdk");
const moduleName = process.argv[3] ?? "playwright";
const { chromium } = await import(moduleName.includes(":") || moduleName.startsWith(".") ? pathToFileURL(resolve(moduleName)).href : moduleName);
const files = new Set(["mini.js", "worker.js", "rsshogi_nnue_mini.wasm"]);
const server = createServer(async (request, response) => {
  const name = new URL(request.url, "http://localhost").pathname.slice(1);
  if (!name) { response.writeHead(200, { "Content-Type": "text/html" }).end("<!doctype html><title>Mini SDK smoke</title>"); return; }
  if (!files.has(name)) { response.writeHead(404).end(); return; }
  try {
    response.writeHead(200, { "Content-Type": extname(name) === ".wasm" ? "application/wasm" : "text/javascript" });
    response.end(await readFile(resolve(directory, name)));
  } catch { response.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
let browser;
try {
  browser = await chromium.launch({ headless: true,
    ...(process.env.MINI_BROWSER_CHANNEL ? { channel: process.env.MINI_BROWSER_CHANNEL } : {}) });
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  const result = await page.evaluate(async () => {
    const { MiniEngine } = await import("/mini.js");
    const direct = await MiniEngine.create("/rsshogi_nnue_mini.wasm");
    const loaded = direct.request({ command: "load_embedded" });
    if (!loaded.ok) throw new Error(loaded.error);
    const score = direct.request({ command: "evaluate" });
    const search = direct.request({ command: "search", depth: 2 });
    direct.dispose();
    if (!score.ok || !search.ok) throw new Error("direct browser search failed");
    const worker = new Worker("/worker.js", { type: "module" });
    const messages = [];
    worker.onmessage = event => messages.push(event.data);
    const wait = async predicate => {
      const until = performance.now() + 15000;
      while (performance.now() < until) {
        const found = messages.find(predicate);
        if (found) return found;
        await new Promise(resolve => setTimeout(resolve, 5));
      }
      throw new Error("worker response timeout: " + JSON.stringify(messages));
    };
    try {
      worker.postMessage({ command: "init", id: "init", url: "/rsshogi_nnue_mini.wasm" });
      const initialized = await wait(message => message.id === "init");
      if (!initialized.ok || initialized.digest !== loaded.digest) throw new Error("worker model mismatch");
      let ticks = 0;
      const timer = setInterval(() => ticks++, 5);
      worker.postMessage({ command: "go", id: "cancel", depth: 64, movetime_ms: 3000 });
      await wait(message => message.id === "cancel" && message.event === "info");
      await new Promise(resolve => setTimeout(resolve, 80));
      worker.postMessage({ command: "stop", id: "stop" });
      const stopped = await wait(message => message.id === "cancel" && message.event === "bestmove");
      await wait(message => message.id === "stop");
      clearInterval(timer);
      await new Promise(resolve => setTimeout(resolve, 100));
      if (messages.filter(message => message.id === "cancel" && message.event === "bestmove").length !== 1) throw new Error("duplicate bestmove");
      if (!stopped.result.bestmove || ticks < 2) throw new Error("worker/UI responsiveness");
      worker.postMessage({ command: "go", id: "stale", depth: 64, movetime_ms: 3000 });
      await wait(message => message.id === "stale" && message.event === "info");
      worker.postMessage({ command: "position", id: "position", sfen: "startpos", moves: ["7g7f", "3c3d"] });
      await wait(message => message.id === "position");
      const boundary = messages.length;
      worker.postMessage({ command: "go", id: "next", depth: 2, movetime_ms: 1000 });
      const next = await wait(message => message.id === "next" && message.event === "bestmove");
      if (messages.slice(boundary).some(message => message.id === "stale")) throw new Error("stale search output");
      if (!next.result.bestmove) throw new Error("next position search failed");
      worker.postMessage({ command: "dispose", id: "dispose" });
      await wait(message => message.id === "dispose");
      return { digest: loaded.digest, score: score.score, fixed_depth: search.result,
               ui_ticks: ticks, exactly_once_stop: true, stale_suppressed: true, disposed: true };
    } finally { worker.terminate(); }
  });
  assert.equal(result.fixed_depth.depth, 2);
  assert.equal(result.fixed_depth.pv[0], result.fixed_depth.bestmove);
  console.log(JSON.stringify(result));
} finally {
  await browser?.close();
  await new Promise(resolve => server.close(resolve));
}

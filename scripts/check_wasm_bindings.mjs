// Model-free regression checks; run with node --test.
import assert from "node:assert/strict";
import test from "node:test";
import { MiniEngine } from "../bindings/wasm/mini.js";

test("HTTP failure is reported before compiling a response body", async () => {
  const original = globalThis.fetch;
  let bodyRead = false;
  globalThis.fetch = async () => ({
    ok: false, status: 404, statusText: "Not Found",
    arrayBuffer() { bodyRead = true; throw new Error("body must not be compiled"); },
  });
  try {
    await assert.rejects(MiniEngine.create("https://example.invalid/engine.wasm"), /HTTP 404 Not Found/);
    assert.equal(bodyRead, false);
  } finally { globalThis.fetch = original; }
});

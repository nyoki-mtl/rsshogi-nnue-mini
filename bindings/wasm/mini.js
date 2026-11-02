// ES module for browsers and Node. Every call is synchronous; search requires a
// finite depth (1..64). Use worker.js when the browser must remain responsive.
export class MiniEngine {
  static async create(source) {
    let bytes = source;
    if (!(source instanceof WebAssembly.Module || source instanceof ArrayBuffer || ArrayBuffer.isView(source))) {
      const response = await fetch(source);
      if (!response.ok) throw new Error(`Wasm fetch failed: HTTP ${response.status} ${response.statusText}`);
      bytes = await response.arrayBuffer();
    }
    const loaded = await WebAssembly.instantiate(bytes, {
      env: { mini_now_ms: () => performance.now() },
    });
    return new MiniEngine((loaded.instance ?? loaded).exports);
  }

  constructor(exports) {
    this.exports = exports;
    this.handle = exports.mini_engine_new() >>> 0;
    if (!this.handle) throw new Error("engine allocation failed");
  }

  assertAlive() {
    if (!this.handle) throw new Error("engine has been disposed");
  }

  readResponse(pointer) {
    // WebAssembly exposes i32 as signed JS numbers; linear-memory addresses are u32.
    pointer >>>= 0;
    if (!pointer) throw new Error("engine returned a null response");
    try {
      const memory = new Uint8Array(this.exports.memory.buffer);
      const end = memory.indexOf(0, pointer);
      if (end < 0) throw new Error("engine returned an unterminated response");
      return JSON.parse(new TextDecoder().decode(memory.subarray(pointer, end)));
    } finally {
      this.exports.mini_string_free(pointer);
    }
  }

  withBytes(bytes, call) {
    this.assertAlive();
    const pointer = this.exports.mini_alloc(bytes.length) >>> 0;
    if (!pointer) throw new Error("input allocation failed");
    try {
      // Allocations and engine calls can grow memory; never cache a memory view.
      new Uint8Array(this.exports.memory.buffer, pointer, bytes.length).set(bytes);
      return this.readResponse(call(pointer));
    } finally {
      this.exports.mini_dealloc(pointer, bytes.length);
    }
  }

  request(request) {
    const bytes = new TextEncoder().encode(JSON.stringify(request) + "\0");
    return this.withBytes(bytes, pointer => this.exports.mini_engine_request(this.handle, pointer));
  }

  loadModel(bytes) {
    const input = bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : bytes;
    if (!(input instanceof Uint8Array) || input.length === 0) throw new Error("model must be nonempty bytes");
    return this.withBytes(input, pointer => this.exports.mini_engine_load_model(this.handle, pointer, input.length));
  }

  stop() {
    this.assertAlive();
    this.exports.mini_engine_stop(this.handle);
  }

  dispose() {
    if (this.handle) {
      this.exports.mini_engine_free(this.handle);
      this.handle = 0;
    }
  }
}

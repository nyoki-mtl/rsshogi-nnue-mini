"""Exercise the packaged C ABI; real-model checks require --model, never fetch it."""

import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import re
import sys
import threading
import time


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


class NativeEngine:
    def __init__(self, library):
        self.library = ctypes.CDLL(str(library.resolve()))
        signatures = {
            "mini_engine_new": ([], ctypes.c_void_p),
            "mini_engine_free": ([ctypes.c_void_p], None),
            "mini_engine_stop": ([ctypes.c_void_p], None),
            "mini_engine_request": ([ctypes.c_void_p, ctypes.c_char_p], ctypes.c_void_p),
            "mini_engine_load_model": ([ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t], ctypes.c_void_p),
            "mini_string_free": ([ctypes.c_void_p], None),
        }
        for name, (arguments, result) in signatures.items():
            function = getattr(self.library, name)
            function.argtypes = arguments
            function.restype = result
        self.handle = self.library.mini_engine_new()
        require(self.handle, "engine allocation failed")

    def decode(self, pointer):
        require(pointer, "null JSON response")
        try:
            return json.loads(ctypes.string_at(pointer).decode("utf-8"))
        finally:
            self.library.mini_string_free(pointer)

    def request(self, command=None, **fields):
        return self.raw(json.dumps({"command": command, **fields}).encode())

    def raw(self, data):
        return self.decode(self.library.mini_engine_request(self.handle, data))

    def load(self, data):
        buffer = ctypes.create_string_buffer(data)
        return self.decode(self.library.mini_engine_load_model(self.handle, buffer, len(data)))

    def stop(self):
        self.library.mini_engine_stop(self.handle)

    def close(self):
        if self.handle:
            self.library.mini_engine_free(self.handle)
            self.handle = None


def ok(response):
    require(response.get("ok") is True, response)
    return response


def error(response):
    require(response.get("ok") is False and isinstance(response.get("error"), str), response)


def check(library, model):
    engine = NativeEngine(library)
    worker = None
    checks = []
    try:
        error(engine.raw(b"{"))
        error(engine.raw(None))
        error(engine.request("unknown"))
        error(engine.request("evaluate"))
        error(engine.load(b"not a model"))
        error(engine.request("configure", hash_mb=0))
        error(engine.request("configure", threads=17))
        error(engine.request("search", depth=0))
        error(engine.request("search", depth=65))
        error(engine.request("search", nodes=0))
        error(engine.request("set_position", sfen="startpos", moves=["7g7e"]))
        ok(engine.request("configure", hash_mb=4, threads=1))
        ok(engine.request("set_position", sfen="startpos", moves=[]))
        ok(engine.request("new_game"))
        engine.stop()
        checks.append("lifecycle and JSON error handling")
        if model is None:
            return {"checks": checks, "model_checks": "not run (no --model)"}

        data = model.read_bytes()
        loaded = ok(engine.load(data))
        require(isinstance(loaded.get("digest"), str), loaded)
        # Consume the pending idle stop once a model makes search admissible.
        stopped = ok(engine.request("search", depth=64, movetime_ms=2000))["result"]
        require(stopped["depth"] == 0, stopped)
        checks.append("stop before search entry")
        score = ok(engine.request("evaluate"))["score"]
        require(isinstance(score, int), score)
        error(engine.load(b"invalid replacement"))
        require(ok(engine.request("evaluate"))["score"] == score, "invalid model load changed evaluation")
        result = ok(engine.request("search", depth=2))["result"]
        require(result["depth"] == 2, result)
        require(re.fullmatch(r"[1-9][a-i][1-9][a-i]\+?", result["bestmove"]), result)
        require(result["pv"] and result["pv"][0] == result["bestmove"], result)
        require(ok(engine.request("evaluate"))["score"] == score, "search changed position")
        checks.append("real model load/evaluation/depth-2 search and preserved state")
        nodes = ok(engine.request("search", depth=64, nodes=1))["result"]
        require(nodes["nodes"] <= 1, nodes)
        started = time.monotonic()
        timed = ok(engine.request("search", depth=64, movetime_ms=10))["result"]
        require(time.monotonic() - started < 3 and timed["depth"] < 64, timed)
        checks.append("node and time limits")
        ok(engine.request("configure", threads=2))
        outcome = {}
        admitted = threading.Event()

        def search():
            admitted.set()
            try:
                outcome["response"] = engine.request("search", depth=64, movetime_ms=2000)
            except BaseException as failure:
                outcome["error"] = repr(failure)

        worker = threading.Thread(target=search, daemon=True)
        worker.start()
        require(admitted.wait(1), "search thread did not start")
        time.sleep(0.05)
        started = time.monotonic()
        engine.stop()
        stop_seconds = time.monotonic() - started
        worker.join(timeout=5)
        require(not worker.is_alive(), "search did not finish after stop")
        require(stop_seconds < 0.5, f"stop blocked for {stop_seconds:.3f}s")
        require(time.monotonic() - started < 1, "cancellation did not finish within 1s")
        require("response" in outcome, outcome)
        require(ok(outcome["response"])["result"]["depth"] < 64, outcome)
        require(ok(engine.request("search", depth=1))["result"]["depth"] == 1, "stop was not consumed")
        checks.append("two-thread search, concurrent stop, and next-search recovery")
        return {"checks": checks, "model_sha256": hashlib.sha256(data).hexdigest(), "score": score}
    finally:
        # On a broken cancellation implementation, do not free a live handle.
        if worker is None or not worker.is_alive():
            engine.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", type=Path, required=True, help="library file or containing directory")
    parser.add_argument("--model", type=Path, help="optional local .rsnn package")
    arguments = parser.parse_args()
    library = arguments.library
    if library.is_dir():
        name = "rsshogi_nnue_mini.dll" if sys.platform == "win32" else (
            "librsshogi_nnue_mini.dylib" if sys.platform == "darwin" else "librsshogi_nnue_mini.so"
        )
        library /= name
    print(json.dumps(check(library, arguments.model), indent=2))


if __name__ == "__main__":
    main()

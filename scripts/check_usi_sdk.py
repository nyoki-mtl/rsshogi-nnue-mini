"""Compare the C ABI and USI entry points at fixed depth, without timing/NPS claims."""
import argparse
import json
import queue
import subprocess
import threading
from pathlib import Path

from check_native import NativeEngine, ok, require


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--library', type=Path, required=True)
    parser.add_argument('--usi', type=Path, required=True)
    parser.add_argument('--model', type=Path, required=True)
    args = parser.parse_args()
    engine = NativeEngine(args.library)
    process = subprocess.Popen([str(args.usi.resolve())], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, bufsize=1)
    output = queue.Queue()
    def reader():
        for line in process.stdout:
            output.put(line.strip())
    threading.Thread(target=reader, daemon=True).start()
    def send(text):
        process.stdin.write(text + '\n')
        process.stdin.flush()
    def until(prefix):
        lines = []
        while True:
            line = output.get(timeout=30)
            require('NNUE load error' not in line, line)
            lines.append(line)
            if line.startswith(prefix):
                return lines
    signatures = []
    try:
        digest = ok(engine.load(args.model.read_bytes()))['digest']
        ok(engine.request('configure', hash_mb=16, threads=1))
        send('usi'); until('usiok')
        send('setoption name Threads value 1')
        send('setoption name USI_Hash value 16')
        send(f'setoption name EvalPackage value {args.model.resolve()}')
        send('isready')
        require(any(digest in line for line in until('readyok')), 'model digest mismatch')
        for moves in ([], ['7g7f', '3c3d'], ['2g2f', '8c8d', '2f2e', '8d8e']):
            ok(engine.request('new_game'))
            ok(engine.request('set_position', sfen='startpos', moves=moves))
            native = ok(engine.request('search', depth=3))['result']
            send('usinewgame')
            send('position startpos' + (' moves ' + ' '.join(moves) if moves else ''))
            send('go depth 3')
            lines = until('bestmove ')
            info = next(line.split() for line in reversed(lines) if line.startswith('info depth '))
            require(info[info.index('score') + 1] == 'cp', 'fixture unexpectedly has a mate score')
            signature = dict(depth=int(info[info.index('depth') + 1]),
                             score=int(info[info.index('score') + 2]),
                             nodes=int(info[info.index('nodes') + 1]),
                             pv=info[info.index('pv') + 1:], bestmove=lines[-1].split()[1])
            require({key: native[key] for key in signature} == signature, (native, signature))
            signatures.append(dict(moves=moves, **signature))
        send('quit')
        require(process.wait(timeout=10) == 0, 'USI quit failed')
        print(json.dumps(dict(model_sha256=digest, matched_positions=len(signatures), signatures=signatures)))
    finally:
        if process.poll() is None:
            process.kill(); process.wait()
        engine.close()


if __name__ == '__main__':
    main()

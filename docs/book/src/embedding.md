# アプリへの組み込み

標準入出力を使うUSI実行ファイルに加え、ホストアプリから同期的に呼べるRust APIとC ABIを提供する。
SwiftとJavaScriptのラッパーも同じC ABIを使う。
探索定数と評価器はUSI版と共通で、アプリ側が実行先とオブジェクトの寿命を管理する。

| 用途 | 呼び出し口 | ビルド手順 |
| --- | --- | --- |
| Rustアプリ | `rsshogi_nnue_mini::api::Engine` | 通常のRust library |
| Windowsアプリ | DLL、import library、C header | `scripts/build_sdk.ps1` |
| iOS実機、iOS Simulator、Apple SiliconのMac | XCFramework、C header、Swiftラッパー | macOS上で`bash scripts/build_apple.sh` |
| ブラウザ | Wasm、JavaScript、Web Worker | `scripts/build_wasm.ps1` |

Windows x86_64 SDKはUSI版と同じくAVX2対応CPUを必要とし、実行時のCPU判定によるscalarへの切り替えは行わない。
ARM64ではNEON kernelを選ぶ。
dot-product命令は実行環境に応じて選択し、対応していないCPUにも一律で強制しない。
Wasmの評価kernelはscalarで、内部の探索スレッド数は1に固定する。

WindowsとWasmは学習済みモデルを使った実行確認を行っている。
Apple向けは2026年11月2日の候補をM4のMacでビルドし、macOSとiOS SimulatorからSwift 6経由の実モデル評価、探索、停止と終了を確認した。
配布用static libraryの組み込みモデルdigestと3局面の固定深さ探索もWindows版に一致した。
iOS実機とIntel Macは未検証であり、棋力や速度の同等性を示す確認ではない。
公開物の対応環境は、そのリリースで実行確認した範囲を参照する。
sourceを変更した公開候補は、改めて対象環境でビルドと実行を確認する。

## モデルを含めてビルドする

モデルを組み込まないビルドでは、生成直後のengineに評価器はない。
モデルのbytesを`load_model`へ渡してから評価と探索を行う。
`embedded-rsnn`付きの配布物では`load_embedded`を呼び出せる。
組み込み時のSHA-256とpackage内の評価仕様、tensor形状、数値範囲を検証する。
読み込みに失敗した場合は、直前に読み込んでいたモデルを維持する。

Windows SDKの例を示す。

```powershell
$env:RSSHOGI_EMBEDDED_RSNN_PATH = (Resolve-Path 'C:/models/model.rsnn').Path
$env:RSSHOGI_EMBEDDED_RSNN_SHA256 = (Get-FileHash $env:RSSHOGI_EMBEDDED_RSNN_PATH).Hash.ToLowerInvariant()
pwsh -NoProfile -File scripts/build_sdk.ps1
```

Wasmはモデルのpathを直接指定する。

```powershell
pwsh -NoProfile -File scripts/build_wasm.ps1 -ModelPath 'C:/models/model.rsnn'
```

Apple向けはmacOS、Xcodeと次のRust targetを用意する。

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin
export RSSHOGI_EMBEDDED_RSNN_PATH=/path/to/model.rsnn
export RSSHOGI_EMBEDDED_RSNN_SHA256=$(shasum -a 256 "$RSSHOGI_EMBEDDED_RSNN_PATH" | awk '{print $1}')
bash scripts/build_apple.sh
```

実機、Simulator、macOSは別sliceとしてXCFrameworkにまとめる。
`MiniEngine.swift`をアプリへ追加し、`MiniEngine.xcframework`をリンクする。
スクリプトはSwift 6の型検査とmacOS上の呼び出し確認も行う。
`MINI_APPLE_INTEL=1`を指定するとIntelのmacOSとSimulatorも追加する。
対応するRust targetは別途必要になる。

このprojectの配布SDKにはモデルをバイナリへ組み込み、単独の`.rsnn`は同梱しない。
[v0.3.0 Release](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/v0.3.0)でWindows AVX2 USI、Windows SDK、Apple SDK、Wasm SDKの4種類のZIPを配布する。
公開sourceの参照先はReleaseのtagである。
manifestの`source_revision`はビルド元checkoutの記録で、公開snapshotのcommitとは異なる場合がある。
`public_source_tag`を公開sourceとの対応に使う。

## C ABIの呼び出し

headerは`include/rsshogi_nnue_mini.h`にある。
JSON requestを渡す関数は、成功時に`{"ok":true,...}`、通常の入力エラー時に`{"ok":false,"error":"..."}`を返す。

```c
#include "rsshogi_nnue_mini.h"

MiniEngine *engine = mini_engine_new();
/* 実際のアプリではNULLと各JSONのokを確認する。 */
char *loaded = mini_engine_request(engine, "{\"command\":\"load_embedded\"}");
mini_string_free(loaded);
char *result = mini_engine_request(engine, "{\"command\":\"search\",\"depth\":6,\"movetime_ms\":500}");
/* resultを読み取ってから解放する。 */
mini_string_free(result);
mini_engine_free(engine);
```

主なrequestは次のとおり。

| command | 主な引数 | 応答 |
| --- | --- | --- |
| `load_embedded` | なし | `digest` |
| `set_position` | `sfen`、省略可能な`moves`配列 | `ok` |
| `configure` | `hash_mb`、`threads` | `ok` |
| `evaluate` | なし | 手番側から見た`score` |
| `search` | `depth`、`nodes`、`movetime_ms` | `result` |
| `new_game` | なし | `ok` |

`sfen`は`startpos`または盤面、手番、持ち駒、手数の4フィールド。
`moves`を渡すと着手履歴も再生する。
不正な局面や指し手は拒否し、それまでの局面を変更しない。

探索深さは1〜64、Hashは1〜256MiB、nativeの探索スレッド数は1〜16。
`nodes`と`movetime_ms`は指定する場合に正の値を使う。
`search`では少なくとも深さ、node数、持ち時間のいずれかを指定する。
応答には`bestmove`、`ponder`、`score`、`depth`、`nodes`、`elapsed_ms`、`hashfull`、`pv`を含む。
`score`は探索内部の整数値で、詰みを示す領域も含む。

通常のrequestはhandleごとに直列化される。
nativeでは`mini_engine_stop`を別threadから呼べる。
探索の入口の直前に停止された場合も取りこぼさないよう、待機中のstopは次の探索で一度だけ消費する。
返された文字列は必ず`mini_string_free`で解放する。
`mini_engine_free`はstopを含む全呼び出しが戻った後にだけ実行し、以後handleを使わない。
Rust APIでは同じ役割を`Engine`と複製可能な`StopHandle`が持つ。

## Swift

次の処理は同期的に実行されるため、アプリ側のbackground queueなどで呼び出す。

```swift
let engine = try MiniEngine()
defer { engine.close() }
_ = try engine.loadEmbedded()
try engine.configure(hashMB: 16, threads: 1)
try engine.setPosition(moves: ["7g7f", "3c3d"])
let result = try engine.search(depth: 6, movetimeMS: 500)
print(result.bestmove)
```

ラッパーはrequestを直列化し、`stop()`は別queueから呼び出せる。
`close()`は新たな呼び出しを拒否し、進行中の処理を止めてからhandleを解放する。
内部のhandleと利用数をlockで保護するため、Swift 6のqueue間でも共有できる。
`request(_:)`へ渡す呼び出し側の可変オブジェクトを、別queueで同時に変更しない。

## ブラウザ

生成したSDK directoryをHTTP serverから配信し、ES moduleのWorkerを作る。

```javascript
const engine = new Worker("./worker.js", { type: "module" });
engine.onmessage = ({ data }) => {
  if (data.event === "bestmove") console.log(data.result.bestmove);
};
const ready = new Promise((resolve, reject) => {
  const onReady = ({ data }) => {
    if (data.id !== "init") return;
    engine.removeEventListener("message", onReady);
    if (data.ok) resolve();
    else reject(new Error(data.error));
  };
  engine.addEventListener("message", onReady);
});
engine.postMessage({ command: "init", id: "init", url: "./rsshogi_nnue_mini.wasm" });
await ready;
engine.postMessage({ command: "position", id: "position", sfen: "startpos", moves: [] });
engine.postMessage({ command: "go", id: "search", depth: 6, movetime_ms: 500 });
// 停止操作に対応する関数の例。
function stopSearch() {
  engine.postMessage({ command: "stop", id: "stop" });
}
```

各応答はrequestの`id`と`command`を返す。
モデルなしのWasmに独自モデルを読み込む場合は、`init`の`model`に`Uint8Array`を渡す。
進捗は`event: "info"`、完了は`event: "bestmove"`で、通常のgoとstopに対してbestmoveを一度だけ返す。
局面変更、新しいgo、破棄によって無効になった探索の結果は返さない。
`new_game`は次局の初期化、`dispose`はengineの解放に使う。

Workerは35ms以内を目安とする探索を繰り返し、合間に停止要求を受け取る。
Workerの`go`には正の`movetime_ms`か`nodes`を必ず指定し、`depth`だけの要求は受け付けない。
同期APIの`search`は`depth`だけでも使える。
TTと履歴は引き継ぐが、各sliceは別の探索であり、nativeの連続した固定時間探索と同じ手順になるとは限らない。
`nodes`はslice全体の合計、持ち時間はJavaScriptの待ち時間も含めた予算である。
深いiterationがsliceに収まらない場合は、その深さへの到達を保証しない。
Wasm SIMD、共有メモリと複数探索threadを必要としない構成である。

`mini.js`の`MiniEngine.create()`も直接使えるが、その`request()`は同期処理である。
UIを止めない用途にはWorkerを使う。

## 実行確認

Windowsでは`check_native.py`でC ABIを、`check_usi_sdk.py`でUSIとSDKの固定深さの結果を照合できる。
Wasmは`check_wasm.mjs`でNode上の処理を確認する。
`check_wasm_browser.mjs`はPlaywrightが利用できる検証環境で、実際のブラウザのmodule Worker、停止、古い結果の抑止を確認する。
Playwrightは配布SDKの実行依存ではない。

公開前には対象OSでモデル読み込み、探索、停止、解放を確認する。
cross-targetのClippyやコンパイル成功だけを、Apple実機での動作確認や棋力の証拠とは扱わない。

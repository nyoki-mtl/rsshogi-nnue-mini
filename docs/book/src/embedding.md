# アプリへの組み込み

`rsshogi-nnue-mini` は、標準入出力で動く USI エンジンとしてだけでなく、アプリケーションやブラウザのプロセス内から直接呼び出せる組み込みライブラリとしても設計されています。

組み込み経路では USI 用の入出力制御（`UsiSession` や `UsiSearchBridge`）を通さず、呼び出し側が用意したワーカースレッドの上で探索・評価コア（`SearchCoordinator` と `Evaluator`）を直接実行します。

## 組み込みインターフェース一覧

用途やターゲット環境に応じて4つのレイヤーを用意しています。

| 対象 | インターフェース | 実装場所 | 特徴 |
| --- | --- | --- | --- |
| Rust アプリ | `rsshogi_nnue_mini::api::Engine` | `src/api.rs` | 型付きの同期メソッド。別スレッドからの停止用に `StopHandle` を分離 |
| C / 他言語 FFI | `mini_engine_*`（C ABI） | `src/ffi.rs`<br>`bindings/apple/include/rsshogi_nnue_mini.h` | `--features c-api` で有効化。不透明ハンドルと JSON 文字列でやり取り |
| iOS / macOS（Swift） | `MiniEngine` クラス / `MiniEngine.xcframework` | `bindings/apple/MiniEngine.swift` | C ABI を包む Swift 6 `Sendable` 対応ラッパー。安全な `stop()` / `close()` を提供 |
| ブラウザ（JavaScript） | Web Worker（`worker.js`） / `MiniEngine`（`mini.js`） | `bindings/wasm/` | 35ms スライスの協調実行で UI や `stop` をブロックしない Web Worker SDK |

### 共通の仕様と制限

どのインターフェースから呼び出した場合も、エンジン内部の契約は共通です。

- **モデルの読み込みが必須**：生成直後のエンジンは評価関数を持っていません。評価（`evaluate`）や探索（`search`）を呼ぶ前に、外部の `.rsnn` バイト列を渡す `load_model` か、`--features embedded-rsnn` ビルドの内蔵モデルを読む `load_embedded` を実行してください。
- **エラー時の状態保全**：不正なモデルデータ・非合法な SFEN や指し手・範囲外の設定値を渡した場合、エラーが返りますが、それまでに読み込まれていたモデルや局面は壊れずにそのまま残ります。
- **設定値の範囲**：
  - `hash_mb`（置換表サイズ）：`1..=256` MiB（既定値 `16`）
  - `threads`（探索スレッド数）：ネイティブ環境では `1..=16`、WebAssembly では `1` のみ（既定値 `1`）
  - `depth`（最大深さ）：`1..=64`、`nodes` と `movetime_ms` は指定する場合 `1` 以上の正の整数
- **対局ルールと評価値**：入玉宣言は 27点法（`CSARule27`）、探索パラメータは既定値（`SearchParams::default()`）で動作します。返される `score` は手番側から見た内部スコアで、通常評価は `-31,753..=31,753`、詰みスコアは `±32,000` 基準（`32,000 - ply` など）で表されます。

---

## 1. Rust から直接組み込む（`src/api.rs`）

Rust のホストアプリケーションからは、`rsshogi_nnue_mini::api::Engine` をそのまま利用できます。`Engine` のメソッドは同期実行で `&mut self` を取るため、探索は UI スレッドではなくバックグラウンドスレッドで呼び出し、中断したい場合はあらかじめ複製しておいた `StopHandle` を使います。

```rust
use std::thread;
use std::time::Duration;
use rsshogi_nnue_mini::api::{Engine, SearchOptions};

fn main() -> Result<(), String> {
    let mut engine = Engine::new();

    // ファイルから .rsnn を読み込む（embedded-rsnn 有効時は engine.load_embedded()? も可）
    let model_bytes = std::fs::read("eval/model.rsnn").map_err(|e| e.to_string())?;
    let digest = engine.load_model(&model_bytes)?;
    println!("loaded model sha256: {digest}");

    // 置換表 32 MiB・2 スレッドに設定し、初手 ▲７六歩 の局面をセット
    engine.configure(Some(32), Some(2))?;
    engine.set_position("startpos", &["7g7f".to_owned()])?;
    println!("static eval: {}", engine.evaluate()?);

    // 別スレッドから探索を止められるよう StopHandle を複製しておく
    let stop_handle = engine.stop_handle();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        stop_handle.stop();
    });

    // 呼び出しスレッド上で同期探索を実行
    let output = engine.search(SearchOptions {
        depth: 20,
        nodes: None,
        movetime_ms: Some(1_000),
    })?;
    println!("bestmove={} score={} depth={}", output.bestmove, output.score, output.depth);
    Ok(())
}
```

`StopHandle::stop()` はロックを取らずにアトミック変数だけを更新します。もしホスト側のタイミング都合で `engine.search(...)` に入る直前に `stop()` が呼ばれていた場合でも、停止要求は消えずに次の `search()` 1回分を即座に打ち切り（最小1ノードのフォールバック手を返し）、`search()` から抜けた時点で自動的にリセットされます。

---

## 2. C ABI と JSON プロトコル（`src/ffi.rs`）

`--features c-api` を有効にしてビルドすると、`bindings/apple/include/rsshogi_nnue_mini.h` に対応する C ABI 関数が公開されます。Rust 内部で万一パニックが起きた場合も FFI 境界の `catch_unwind` で捕捉し、エラー JSON として返すため、C 側へ巻き戻しが漏れることはありません。

### 公開関数とメモリ管理の規則

- `mini_engine_new()` / `mini_engine_free(handle)`：エンジンハンドルの生成と破棄。`mini_engine_free` を呼ぶ際は、`mini_engine_stop` を含むすべての呼び出しが完了している必要があります。
- `mini_engine_load_model(handle, bytes, len)`：`1..=67,108,864` バイト（最大 64 MiB）の `.rsnn` バッファを読み込みます。バッファは関数呼び出し中だけ借用されます。
- `mini_engine_request(handle, json_cstr)`：NUL 終端 UTF-8 の JSON コマンドを実行し、結果の JSON 文字列ポインタを返します。
- `mini_engine_stop(handle)`：実行中の探索へ停止フラグを立てます。この関数だけは、別スレッドで `mini_engine_request`（探索）が実行されている最中でも安全に同時呼び出しできます。
- `mini_string_free(ptr)`：`mini_engine_load_model` と `mini_engine_request` が返した文字列を解放します（呼び出し側で必ず1回解放してください）。
- `mini_alloc(len)` / `mini_dealloc(ptr, len)`：主に WebAssembly でホスト側から線形メモリへモデルバイト列や文字列を渡すための転送バッファ確保・解放関数です。

### JSON リクエストの形式

`mini_engine_request` に渡す JSON は `"command"` タグで種類を指定します（未知のフィールドがあるとエラーになります）。

```json
{"command": "load_embedded"}
{"command": "configure", "hash_mb": 32, "threads": 2}
{"command": "new_game"}
{"command": "set_position", "sfen": "startpos", "moves": ["7g7f", "3c3d"]}
{"command": "evaluate"}
{"command": "search", "depth": 12, "movetime_ms": 500}
```

> **注意**：C ABI の `search` では、呼び出しスレッドの無制限ブロックを防ぐため、`depth`・`nodes`・`movetime_ms` のうち**少なくとも1つ**を指定する必要があります。

応答は成功時に `{"ok": true, ...}`、失敗時に `{"ok": false, "error": "..."}` の形で返ります。`search` 成功時の `"result"` オブジェクトには `bestmove`、`ponder`、`score`、`depth`、`nodes`、`elapsed_ms`、`hashfull`、`pv` が入ります。

---

## 3. Swift / iOS・macOS への組み込み（`bindings/apple/`）

Release の `apple-sdk.zip` には、静的ライブラリを束ねた `MiniEngine.xcframework` と、薄いラッパークラス `MiniEngine.swift` が同梱されています。手元の Mac（Xcode と Rust ツールチェーンが必要）で `.rsnn` モデルから再ビルドする場合は次のスクリプトを実行します。

```sh
# arm64（macOS + iOS Simulator + iOS デバイス向けスライス）をビルド
./scripts/build_apple.sh /path/to/model.rsnn

# Intel Mac（x86_64）向けスライスも含める場合
MINI_APPLE_INTEL=1 ./scripts/build_apple.sh /path/to/model.rsnn
```

Xcode プロジェクトに `MiniEngine.xcframework` と `MiniEngine.swift` を追加すると、次のように呼び出せます。

```swift
let engine = try MiniEngine()
_ = try engine.loadEmbedded() // 外部ファイルの場合は try engine.loadModel(data)
try engine.configure(hashMB: 32, threads: 2)
try engine.setPosition(sfen: "startpos", moves: ["7g7f"])

// 探索は同期処理のため、UI スレッドではなくバックグラウンドで実行する
let searchTask = Task.detached {
    try engine.search(depth: 16, movetimeMS: 500)
}

// 別スレッドからの中断はロック待ちなしで即座に通る
// engine.stop()

let result = try await searchTask.value
print("bestmove: \(result.bestmove), score: \(result.score)")

// 明示的に解放する場合（実行中の処理があれば停止・完了を待ってから解放される）
engine.close()
```

`MiniEngine.swift` では、通常のリクエスト直列化に `NSLock` を使いつつ、C ハンドルの生存確認と参照カウントには独立した `NSCondition` を使っています。そのため探索が `NSLock` を握っている最中でも `stop()` は待たされずに実行でき、`close()` を呼んだ際も「新規呼び出しの受付停止 → 停止要求 → 実行中呼び出しの完了待ち → `mini_engine_free`」の順で安全に破棄されます。

> **動作確認の範囲**：Apple SDK は Apple Silicon Mac（macOS arm64）および iOS Simulator（arm64）でテストを実施しています。iOS 実機での署名・動作および Intel Mac 実機での動作は未検証です。

---

## 4. WebAssembly / ブラウザへの組み込み（`bindings/wasm/`）

Release の `wasm-sdk.zip` には、モデル内蔵の `.wasm` バイナリ、線形メモリと JSON 変換を担当する `mini.js`、そしてブラウザのメインスレッドを止めずに探索を回す Web Worker スクリプト `worker.js` が入っています。手元でビルドする場合は `wasm32-unknown-unknown` ターゲットを追加してスクリプトを実行します。

```powershell
rustup target add wasm32-unknown-unknown
pwsh -NoProfile -File scripts/build_wasm.ps1 -ModelPath "C:\path\to\model.rsnn"
```

### Web Worker からの利用例

ブラウザアプリからは `worker.js` を Module Worker として起動し、`postMessage` でコマンドを送るのが標準的な使い方です。

```javascript
const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });

worker.onmessage = ({ data }) => {
  if (!data.ok) {
    console.error("engine error:", data.error);
    return;
  }
  if (data.event === "info") {
    console.log(`depth=${data.result.depth} score=${data.result.score} pv=${data.result.pv.join(" ")}`);
  } else if (data.event === "bestmove") {
    console.log("bestmove:", data.result.bestmove);
  }
};

// 1. 初期化（内蔵モデルの代わりに外部 Uint8Array を使う場合は model プロパティを渡す）
worker.postMessage({ id: 1, command: "init", url: "./rsshogi_nnue_mini.wasm", hash_mb: 16 });

// 2. 局面設定と探索開始（Worker の go には movetime_ms または nodes が必須）
worker.postMessage({ id: 2, command: "position", sfen: "startpos", moves: ["7g7f"] });
worker.postMessage({ id: 3, command: "go", depth: 20, movetime_ms: 1000 });

// 途中で打ち切る場合: worker.postMessage({ id: 4, command: "stop" });
```

### 35ms スライスによる協調探索の仕組み

シングルスレッドの WebAssembly には OS スレッドがないため、1回の長い `search` をそのまま走らせると、探索が終わるまで Worker のメッセージキューが詰まり `stop` を受け取れなくなってしまいます。

そこで `worker.js` では、目標の `movetime_ms` や `nodes` に達するまで、**1回あたり最大 `35ms` の短い探索スライス**を `setTimeout(..., 0)` を挟みながら段階的に深さを上げて繰り返します。

- 各スライスが終わるたびに `event: "info"` を返し、目標到達または `stop` 受信時に `event: "bestmove"` をちょうど1回だけ返します。
- スライスを有限時間で区切る必要があるため、Worker の `go` では **`movetime_ms` または `nodes` の指定が必須**です（`depth` 単独指定はエラーになります。なお、`mini.js` の `MiniEngine` クラスを直接同期呼び出しする場合は `depth` 単独指定も可能です）。
- 各スライスの間で置換表と学習履歴はそのまま引き継がれますが、反復深化を細切れに再開する協調動作であるため、ネイティブ環境で1回の連続探索を行った場合と探索木やノード数が完全に一致するわけではありません。
- 探索中に新しい `position`・`new_game`・`go`・`dispose` が届いた場合は内部の `generation` カウンタが進み、古いスライスの結果が遅れて `bestmove` として送出されるのを防ぎます。

手元で SDK のスモークテストを実行する場合は、Wasm ビルド後に `node tests/wasm_smoke.mjs dist/wasm/rsshogi_nnue_mini.wasm` を、macOS 上の Apple SDK ビルド後には `bindings/apple` ディレクトリで `swift test` を利用できます。

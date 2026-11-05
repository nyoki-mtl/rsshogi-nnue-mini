# セットアップとビルド

すぐに将棋 GUI で使いたい場合は Release の配布バイナリを、コードを改造したり実験用機能を有効化したい場合はソースからのビルドを利用します。

## 配布バイナリを使う

[GitHub Releases（v0.4.0）](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/v0.4.0) で環境別の ZIP アーカイブを配布しています（実際のファイル名には `rsshogi-nnue-mini-v0.4.0-` の接頭辞が付きます）。

| 用途 | アーカイブ | 中身と備考 |
| --- | --- | --- |
| Windows の将棋 GUI | `windows-avx2-usi.zip` | AVX2 対応 x86_64 CPU 向けの USI 実行ファイル |
| Apple Silicon Mac の将棋 GUI | `apple-sdk.zip` | 同梱の `rsshogi-nnue-mini-macos-arm64` を USI エンジンとして登録 |
| iOS / macOS アプリへの組み込み | `apple-sdk.zip` | `MiniEngine.xcframework` と `MiniEngine.swift`（[アプリへの組み込み](embedding.md)） |
| ブラウザへの組み込み | `wasm-sdk.zip` | `.wasm` 本体と `mini.js` / `worker.js`（[アプリへの組み込み](embedding.md)） |

配布バイナリには学習済みの NNUE 評価関数があらかじめ内蔵されているため、別途モデルファイルを配置する必要はありません。ZIP を展開し、将棋 GUI（将棋所、ShogiGUI、ShogiHome など）に USI エンジンとして実行ファイルを登録してください。

macOS で展開後に実行権限が外れている場合は、ターミナルで実行権限を付与します。

```sh
chmod +x rsshogi-nnue-mini-macos-arm64
```

## ソースからビルドする

ソースからのビルドには **Rust 1.95 以降**が必要です。スクリプトや `justfile` を使う場合は PowerShell 7（`pwsh`）も用意してください。

### 1. 通常のビルド（外部モデル読み込み）

リポジトリには `.rsnn` モデルファイルを同梱していないため、通常の Cargo ビルドではモデルを内蔵しない実行ファイルが生成されます。

```powershell
# 通常のリリースビルド（scalar カーネル）
cargo build --locked --release

# x86_64 で AVX2 SIMD カーネルを有効化する場合
$env:RUSTFLAGS = "-C target-feature=+avx2"
cargo build --locked --release
```

この方法でビルドしたバイナリを対局で使うには、`aonly1024_i8_progress8_v1` 仕様の検証済み `.rsnn` パッケージを用意し、既定パスの `eval/model.rsnn` に置くか、USI オプション `EvalPackage` でパスを指定します。

> **CPU ごとの SIMD 指定について**
> - **x86_64**：コンパイル時の `target-feature` で経路が決まります。`-C target-feature=+avx2` を付けると AVX2 カーネル、付けないと汎用 scalar カーネルになります（実行時の自動切り替えは行いません）。
> - **ARM64（Apple Silicon / iOS など）**：標準で NEON カーネルが使われ、全結合層の dot-product 命令（`sdot`）は対応 CPU を実行時に自動判定して利用します。

### 2. モデル内蔵バイナリのビルド（Windows AVX2）

手元に `aonly1024_i8_progress8_v1` 仕様の `.rsnn` パッケージがある場合は、付属スクリプトで評価関数をバイナリに埋め込んだ単一実行ファイルを作成できます。

```powershell
pwsh -NoProfile -File scripts/build_embedded_avx2.ps1 -ModelPath "C:\path\to\model.rsnn"
```

ビルドが成功すると `dist/rsshogi-nnue-mini-embedded-avx2.exe` と、ハッシュ値などを記録した JSON メタデータが出力されます。この実行ファイルは既定で内蔵モデル（`EvalPackage = @default`）を読むため、単体で将棋 GUI に登録できます。

Apple 向け・WebAssembly 向けの SDK ビルド手順は [アプリへの組み込み](embedding.md) を参照してください。

### 3. Cargo プロファイルと feature フラグ

目的に応じて Cargo の feature やプロファイルを切り替えられます。

| 指定 | 役割 |
| --- | --- |
| `--features embedded-rsnn` | 環境変数 `RSSHOGI_EMBEDDED_RSNN_PATH` と `RSSHOGI_EMBEDDED_RSNN_SHA256` で指定したモデルをバイナリへ埋め込む |
| `--features c-api` | C ABI 関数（`mini_engine_*`）を公開する。Apple / Wasm SDK で使用 |
| `--features tuning` | `Clear Hash` や51個の探索パラメータ調整用 USI オプション、`usi_tunables` コマンドを有効化する |
| `--features stats` | 探索終了時などにノード内訳や枝刈り発火数の集計を標準エラー出力へ出す |
| `--profile production` | `release`（thin LTO）に代わり fat LTO と `codegen-units = 1` でビルドする任意プロファイル |

なお、`cargo clean` で `target/` を削除した後に再ビルドする際は、必要な `RUSTFLAGS` や `--features` を改めて指定してください（`dist/` にコピー済みのファイルは自動では更新されません）。

## ターミナルでの動作確認

USI エンジンは標準入力からテキストを1行ずつ受け取り、標準出力へ応答を返します。配布バイナリ（またはモデルを用意したビルド済みバイナリ）をターミナルで起動し、次のコマンドを順に入力すると最小限の動作を確かめられます。

```text
usi
isready
usinewgame
position startpos moves 7g7f
go depth 4
```

1. `usi` に対してエンジン名・オプション一覧・`usiok` が返ります。
2. `isready` で評価関数の読み込みと検証が走り、成功すると `info string loaded A-only .rsnn: ...` に続いて `readyok` が返ります。
3. `position` で初手 `▲７六歩`（`7g7f`）を指した局面をセットし、`go depth 4` を送ると深さ1〜4までの読み筋（`info depth ...`）と最終的な指し手（`bestmove ...`）が出力されます。
4. 確認が終わったら `quit` を入力して終了します。

各コマンドや設定項目の詳細は、次の [USIコマンドとオプション](usi.md) で説明します。

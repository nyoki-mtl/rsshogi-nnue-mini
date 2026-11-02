# セットアップ

[v0.3.0のRelease](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/v0.3.0)から、使う環境に合うZIPを取得します。

| 用途 | ZIP |
| --- | --- |
| Windowsの将棋GUI | `windows-avx2-usi.zip`（AVX2対応CPUが必要） |
| Apple Silicon Macの将棋GUI | `apple-sdk.zip`内の`rsshogi-nnue-mini-macos-arm64` |
| アプリやブラウザへの組み込み | `windows-sdk.zip`、`apple-sdk.zip`、`wasm-sdk.zip`。詳しくは[組み込み手順](embedding.md) |

実際のZIP名には`rsshogi-nnue-mini-v0.3.0-`が付きます。
将棋GUIには展開したUSI実行ファイルを登録します。
Macで実行権限が失われた場合は`chmod +x rsshogi-nnue-mini-macos-arm64`を実行してください。
評価関数は内蔵済みで、別のモデルファイルは不要です。

## ソースからのビルド

以下はWindowsのAVX2対応CPUでソースからビルドする手順です。Rust 1.95以降、PowerShell 7、MSVCツールチェーンを用意してください。

1. ソースコードを取得し、PowerShell 7でリポジトリのルートへ移動します。
2. `aonly1024_i8_progress8_v1`仕様の検証済みpackageを用意します。モデル単体は配布していません。旧2026.09.30 packageは読み込めません。
3. 次を実行します。`-ModelPath`には用意したファイルのパスを指定してください。

   ```powershell
   pwsh -NoProfile -File scripts/build_embedded_avx2.ps1 -ModelPath "C:\path\to\aonly1024-i8-progress8.rsnn"
   ```

4. `dist/rsshogi-nnue-mini-embedded-avx2.exe`を将棋GUIにUSIエンジンとして登録します。

この実行ファイルには評価関数が含まれています。対局時に`.rsnn`を置く必要はありません。

## 動作確認

ターミナルから実行ファイルを起動し、次を一行ずつ入力します。`isready`に対して`readyok`が返るのを待ってから次へ進んでください。

```text
usi
isready
usinewgame
position startpos
go depth 3
```

`bestmove`が返ったら、`quit`で終了します。設定を変える場合は[USIオプション](usi.md)を参照してください。

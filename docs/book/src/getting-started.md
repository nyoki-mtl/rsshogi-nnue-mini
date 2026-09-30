# セットアップ

以下はWindowsのAVX2対応CPUで使う手順です。Rust 1.95以降、PowerShell 7、MSVCツールチェーンを用意してください。

1. ソースコードを取得し、PowerShell 7でリポジトリのルートへ移動します。
2. [評価関数 2026.09.30](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/eval-2026.09.30)から`rsshogi-nnue-mini-eval-20260930.rsnn`をダウンロードします。
3. 次を実行します。`-ModelPath`にはダウンロードしたファイルのパスを指定してください。

   ```powershell
   pwsh -NoProfile -File scripts/build_embedded_avx2.ps1 -ModelPath "C:\path\to\rsshogi-nnue-mini-eval-20260930.rsnn"
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

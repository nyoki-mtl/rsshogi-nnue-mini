# rsshogi-nnue-mini

[![Documentation](https://img.shields.io/badge/docs-GitHub%20Pages-0969da?logo=github)](https://nyoki-mtl.github.io/rsshogi-nnue-mini/)

[rsshogi](https://github.com/nyoki-mtl/rsshogi)、[rsshogi-usi](https://github.com/nyoki-mtl/rsshogi-usi)、[rsshogi-csa](https://github.com/nyoki-mtl/rsshogi-csa)、[ShogiArena](https://github.com/nyoki-mtl/ShogiArena)を組み合わせ、エンジンの実装から対局・調整までを試せる小さなUSIエンジンです。
反復深化、PVS、qsearch、置換表、主要な枝刈り、Lazy SMPと、512幅のNNUE評価を実装しています。

## セットアップ

WindowsのAVX2対応CPUで使う手順です。Rust 1.95以降、PowerShell 7、MSVCツールチェーンが必要です。

1. [評価関数 2026.09.30](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/eval-2026.09.30)から`rsshogi-nnue-mini-eval-20260930.rsnn`をダウンロードします。
2. リポジトリのルートで次を実行します。`-ModelPath`にはダウンロードしたファイルのパスを指定してください。

   ```powershell
   pwsh -NoProfile -File scripts/build_embedded_avx2.ps1 -ModelPath "C:\path\to\rsshogi-nnue-mini-eval-20260930.rsnn"
   ```

3. 作成された`dist/rsshogi-nnue-mini-embedded-avx2.exe`を将棋GUIにUSIエンジンとして登録します。

評価関数は実行ファイルに内蔵されるため、対局時に`.rsnn`を置く必要はありません。

詳しくは[セットアップガイド](docs/book/src/getting-started.md)と[USIオプション](docs/book/src/usi.md)を参照してください。

## 対局記録

2026年10月1日に確認した[FloodgateレートはR3447](https://wdoor.c.u-tokyo.ac.jp/shogi/x/2026/player/rss-mini-84e-20260929+72a7fa5.html)です。

## ライセンス

ソースコードはMITライセンスです。

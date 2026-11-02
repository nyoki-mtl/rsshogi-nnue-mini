# rsshogi-nnue-mini

[![Documentation](https://img.shields.io/badge/docs-GitHub%20Pages-0969da?logo=github)](https://nyoki-mtl.github.io/rsshogi-nnue-mini/)

[rsshogi](https://github.com/nyoki-mtl/rsshogi)、[rsshogi-usi](https://github.com/nyoki-mtl/rsshogi-usi)、[rsshogi-csa](https://github.com/nyoki-mtl/rsshogi-csa)、[ShogiArena](https://github.com/nyoki-mtl/ShogiArena)を組み合わせ、エンジンの実装から対局・調整までを試せる小さなUSIエンジンです。
反復深化、PVS、qsearch、置換表、主要な枝刈り、Lazy SMPと、NNUE評価を実装しています。

評価関数が約3MBと軽量でMITライセンスのため、WASMやアプリ埋め込みにも適した設計になっています。

[ダウンロード（v0.3.0）](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/v0.3.0) / [使い方](docs/book/src/getting-started.md) / [アプリへの組み込み](docs/book/src/embedding.md)
配布バイナリは評価関数を内蔵しており、別のモデルファイルは不要です。

## ライセンス

MIT

# rsshogi-nnue-mini

[![Documentation](https://img.shields.io/badge/docs-GitHub%20Pages-0969da?logo=github)](https://nyoki-mtl.github.io/rsshogi-nnue-mini/)

[rsshogi](https://github.com/nyoki-mtl/rsshogi)、[rsshogi-usi](https://github.com/nyoki-mtl/rsshogi-usi)、[rsshogi-csa](https://github.com/nyoki-mtl/rsshogi-csa)、[ShogiArena](https://github.com/nyoki-mtl/ShogiArena)を組み合わせて、現代的な将棋AI開発の一巡を読める形にする小さなUSIエンジンです。

反復深化、PVS、qsearch、置換表、主要な枝刈り、Lazy SMPと、standard `HalfKP256x32x32`のNNUE評価を実装しています。

## 動かす

Rust 1.95以降を使います。

```powershell
cargo build --release
./target/release/rsshogi-nnue-mini.exe
```

起動後に`usi`、`isready`、`position startpos`、`go depth 3`を送ると探索できます。
評価関数は実行時の作業ディレクトリにある`eval/nn.bin`を`isready`で読み込みます。

## ドキュメント

- [動かしてみる](https://nyoki-mtl.github.io/rsshogi-nnue-mini/getting-started.html)
- [評価関数（NNUE）](https://nyoki-mtl.github.io/rsshogi-nnue-mini/nnue.html)
- [探索](https://nyoki-mtl.github.io/rsshogi-nnue-mini/search.html)
- [USIと実行時の構成](https://nyoki-mtl.github.io/rsshogi-nnue-mini/usi.html)
- [運用（ShogiArena、SPSA、SPRT、CSA）](https://nyoki-mtl.github.io/rsshogi-nnue-mini/operations/index.html)

## ライセンス

MIT

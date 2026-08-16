# rsshogi-nnue-mini

[![Documentation](https://img.shields.io/badge/docs-GitHub%20Pages-0969da?logo=github)](https://nyoki-mtl.github.io/rsshogi-nnue-mini/)

[rsshogi](https://github.com/nyoki-mtl/rsshogi)、[rsshogi-usi](https://github.com/nyoki-mtl/rsshogi-usi)、[rsshogi-csa](https://github.com/nyoki-mtl/rsshogi-csa)、[ShogiArena](https://github.com/nyoki-mtl/ShogiArena)を組み合わせて、現代的な将棋AI開発の一巡を読める形にする小さなUSIエンジンです。

現在のコードは、反復深化、PVS、qsearch、TT、主要な枝刈り、Lazy SMP、常駐USI探索workerを実装しています。
standard `HalfKP256x32x32`はstrict loader、実行時に選ぶscalar/AVX2推論、指し手から差分を求めるaccumulatorを備え、公式の水匠5 `nn.bin`と独立したYaneuraOu実行ファイルを使う外部parity testも通しています。

## ビルドと起動

Rust 1.95以降を使います。

```powershell
cargo build --release
./target/release/rsshogi-nnue-mini.exe
```

起動後に`usi`、`isready`、`position startpos`、`go depth 3`を送ると探索できます。
通常ビルドはstandard NNUEを使用し、実行時の作業ディレクトリにある`eval/nn.bin`を`isready`時に読み込みます。
公式配布物から取得してdigestを確認した`nn.bin`を、直接起動なら`eval/nn.bin`、stage後なら`dist/eval/nn.bin`へ置いてください。
読み込みに失敗した場合は`readyok`を返さず、material評価へfallbackしません。
ShogiArenaの設定例を使う前には、固定した配置先へ実行ファイルを複製します。

```powershell
just stage-engine
```

調整用の公開面が必要なときだけ、通常ビルドとは別に`tuning` featureを使います。

```powershell
just release-avx2-tuning
```

このバイナリはSPSA用に探索12個と`FV_SCALE`、`usi_tunables`、`Clear Hash`を公開します。
通常配布用のバイナリは`tuning` featureなしで作成してください。

## 実戦と調整への導線

- [ShogiArenaでsmoke test、SPSA、SPRTを行う](docs/book/src/operations/shogiarena.md)
- [rsshogi-csaを介してCSAサーバへ接続する](docs/book/src/operations/csa.md)
- [standard NNUE対応の境界と評価関数の扱い](docs/book/src/nnue.md)
- [全体のmdBook](docs/book/src/SUMMARY.md)

採用候補の棋力は、調整に使った局面と分けたholdout openingによるSPRTで確認します。
SPSAの原理、設定、実行、監査、結果の読み方は[公開bookのSPSA章](docs/book/src/operations/spsa.md)にまとめています。

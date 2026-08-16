# はじめに

`rsshogi-nnue-mini`は、公開された`rsshogi` ecosystemを組み合わせて作る、Rust製のNNUE将棋エンジンである。
盤面操作や通信処理は既存のライブラリへ任せ、このリポジトリでは評価関数と探索を中心に実装している。
巨大な完成品から機能を切り出すのではなく、各部の役割と接続を追える規模に保っている。

盤面、合法手、SFENは`rsshogi`、USIのcommand modelとparser/formatterは`rsshogi-usi`を利用する。
このリポジトリはengine固有のsession、探索、評価、parameter管理を所有する。
CSA接続は`rsshogi-csa`、対局実験、SPSA、SPRTは`ShogiArena`へ委譲する。

通常ビルドはstandard `HalfKP256x32x32`のNNUE評価を使用し、厳密なloader、実行時に選ぶscalar/AVX2推論、accumulatorの差分更新を実装している。
水匠5の公式networkは独立した実行ファイルと照合済みで、利用者が公式releaseから取得して配置する。

探索は反復深化とalpha-beta法を骨格に、置換表、主要な枝刈り、Lazy SMPまで含む。
学習器は含めず、parameter調整と検定はShogiArenaのSPSAとSPRTへ委譲する。

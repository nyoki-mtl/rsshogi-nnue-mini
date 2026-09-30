# rsshogi-nnue-mini

[![Documentation](https://img.shields.io/badge/docs-GitHub%20Pages-0969da?logo=github)](https://nyoki-mtl.github.io/rsshogi-nnue-mini/)

[rsshogi](https://github.com/nyoki-mtl/rsshogi)と[rsshogi-usi](https://github.com/nyoki-mtl/rsshogi-usi)を使った、Rust製のUSI将棋エンジンです。
評価関数と探索の実装を追えるように構成しています。
CSA接続には[rsshogi-csa](https://github.com/nyoki-mtl/rsshogi-csa)、ローカル対局やパラメータ調整には[ShogiArena](https://github.com/nyoki-mtl/ShogiArena)を組み合わせます。

反復深化、PVS、静止探索、置換表、枝刈り、Lazy SMPと、512幅・ThreatなしのSFNNv15 `.rsnn`評価を実装しています。
評価は着手ごとの特徴差分を反映し、AVX2またはNEON対応環境では専用の整数演算経路を使います。
探索では、必要な段階まで指し手の生成を遅らせる方式も採用しています。

## Floodgateでの対局記録

84エポックのSFNNv15評価を内蔵した版は、2026年9月30日（JST）にFloodgateで24局を指し、17勝7敗でした。
[公式の対局者ページ](https://wdoor.c.u-tokyo.ac.jp/shogi/x/2026/player/rss-mini-84e-20260929+72a7fa5.html)で同日17:24 JSTに確認したレートは**R3444**です。
実行条件と検証範囲は[開発時の測定結果](docs/book/src/verification.md)に記録しています。
このレートは当該構成と対局条件での観測値です。

## 動かす

Rust 1.95以降を使います。
通常版には評価ファイルを同梱していません。
Floodgateで使用した84エポックの`.rsnn`は、[評価ファイルのRelease](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/model-84e-20260930)から取得できます。ソースコードには学習済みモデルも作成ツールも含まれません。モデルがない状態では対局できません。
評価ファイルを`eval/model.rsnn`へ配置するか、USIの`EvalPackage`にファイルのパスを指定してください。
相対パスは実行時の作業ディレクトリを基準に読み込みます。
以下はWindowsで、リポジトリのルートから通常版を実行する例です。

```powershell
cargo build --release
./target/release/rsshogi-nnue-mini.exe
```

起動後に`usi`を送り、`usiok`を待ちます。
続いて`isready`を送り、`readyok`を確認してから、`usinewgame`、`position startpos`、`go depth 3`を順に送ります。
`bestmove`を受け取った後に`quit`で終了します。
評価ファイルのdigest、評価仕様、tensor形状を検証できない場合は`readyok`を返しません。

評価ファイルを実行ファイルへ内蔵するAVX2ビルドは、次のコマンドで`dist/rsshogi-nnue-mini-embedded-avx2.exe`に作れます。
`EvalPackage`の既定値は`@default`で、外部の評価ファイルは不要です。
AVX2対応CPUで実行してください。内蔵したモデルのSHA-256は、ビルド時に生成される`dist/rsshogi-nnue-mini-embedded-avx2.json`で確認できます。
モデルファイルと内蔵版実行ファイルはGitに追加しません。

```powershell
just release-avx2-embedded "C:\path\to\model.rsnn"
```

## ドキュメント

- 初めて動かす場合は[動かしてみる](docs/book/src/getting-started.md)から始めてください。
- 実装を追う場合は[評価関数](docs/book/src/nnue.md)、[探索](docs/book/src/search.md)、[USIと実行時の構成](docs/book/src/usi.md)を参照してください。
- 対局と測定では[運用](docs/book/src/operations/index.md)と[開発時の測定結果](docs/book/src/verification.md)を確認してください。

## ライセンス

MIT

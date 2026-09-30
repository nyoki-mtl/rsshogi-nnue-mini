# はじめに

`rsshogi-nnue-mini`は、評価関数と探索の役割をコードから追えるRust製のUSI将棋エンジンである。
初めて起動する場合は[動かしてみる](getting-started.md)から始め、実装を読む場合は[評価関数](nnue.md)と[探索](search.md)へ進む。
対局や測定を行う場合は[運用](operations/index.md)を参照する。

盤面操作、合法手生成、SFENの解釈には`rsshogi`を使い、USIコマンドの解析と整形には`rsshogi-usi`を使う。
エンジン固有の通信状態、探索、評価、パラメータ管理は、このリポジトリで実装する。
CSA接続は`rsshogi-csa`、対局実験とパラメータ調整は`ShogiArena`を別途起動して行う。

通常ビルドは512幅・Threatなし・PSQTありのSFNNv15 `.rsnn`評価を使用する。
packageのdigest、評価仕様、tensor形状を検証してから整数推論を行う。
通常版の評価ファイルは利用者が別途配置し、内蔵版はビルド時に指定したファイルを実行ファイルに含める。
評価ファイルはGitに含めていない。
対応形式と読み込み時の検証は[評価関数](nnue.md)で説明する。

探索は反復深化とalpha-beta法を骨格に、置換表、主要な枝刈り、Lazy SMPまで含む。
ShogiArenaのSPSAで探索パラメータの候補を作り、別の対局によるSPRTで比較できる。
測定結果は比較条件とともに[開発時の測定結果](verification.md)にまとめた。

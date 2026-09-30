# CSA対局とFloodgate

CSAの通信と対局時計は`rsshogi-csa`に含まれる`csabridge`が担当する。
この章は、手元で中継を確かめてから外部サーバへ接続し、対局記録を確認するまでの手順を示す。

## ローカルでCSA対局を確認する

外部サーバへ接続する前に、`rsshogi-csa`のloopbackテストで、実エンジンを使った通常対局とponder対局を確認する。
mini側で`just stage-engine`を実行し、SHA-256を確認した評価ファイルを`dist/eval/model.rsnn`へ配置してから、隣接する`rsshogi-csa`のルートで次を実行する。
このテストは、実行ファイルがあるディレクトリを作業ディレクトリにしてエンジンを起動する。

```powershell
$env:RSSHOGI_CSA_TEST_ENGINE = `
  (Resolve-Path ../rsshogi-nnue-mini/dist/rsshogi-nnue-mini.exe).Path
cargo test -p rsshogi-csa-bridge --test loopback -- --nocapture
```

このテストはローカルのソケット上で2種類の対局を最後まで進め、`csabridge`とエンジンの通信、終局処理、イベントJSONLを検証する。
評価ファイルを内蔵した実行ファイルを使う場合は、`RSSHOGI_CSA_TEST_ENGINE`にそのパスを指定すれば、`dist/eval/model.rsnn`の配置は不要である。

## 外部CSAサーバ

リリースビルドしたUSI実行ファイルを、別途導入した`csabridge`から起動する。
CSAの評価値コメントを先手視点で統一するため、`rsshogi-csa` 0.1.3以降に含まれる版を使う。
接続前にバージョンを確認する。

```powershell
csabridge --version
```

パスワードは環境変数から渡す。
以下はminiのリポジトリのルートから、`csabridge`の既定の接続先であるFloodgateへ1局だけ接続する例である。
事前に`dist/eval/model.rsnn`を配置し、`<自分のtrip>`を任意の文字列に、`--name`のログイン名を自分の測定対象に合わせた名前に置き換える。

```powershell
$env:RSSHOGI_CSA_PASSWORD = 'floodgate-300-10F,<自分のtrip>'

csabridge --name 'my-mini-measurement' `
  --password-env RSSHOGI_CSA_PASSWORD `
  --engine '.\dist\rsshogi-nnue-mini.exe' `
  --engine-dir '.\dist' `
  --setoption 'USI_Ponder=true' `
  --setoption 'Threads=2' `
  --setoption 'USI_Hash=64' `
  --games 1 `
  --log-dir '.\csa-logs'
```

接続先host、port、アカウント名、パスワード形式は大会またはサーバ管理者の案内に合わせる。
長時間動かす場合は、パスワードをGit管理外のファイルへ置き、`--password-file`で渡すこともできる。
外部対局の確認では、1局の完了、反則と時間切れの有無、各`go`に対する`bestmove`、次局の`usinewgame`、終了時の`bridge_stop`をログで調べる。

## Floodgateでレートを測る

[Floodgateの案内](https://wdoor.c.u-tokyo.ac.jp/shogi/)によると、通常の対局は毎時0分と30分に組まれる。
対局場は`wdoor.c.u-tokyo.ac.jp:4081`で、レートが付くまでの目安は約15局である。
接続条件と集計方法は運用時に公式案内を確認する。

異なる評価関数や探索設定の対局を一つのレートに混ぜないよう、測定対象ごとに新しいログイン名を使う。
最初は`--games 1`で1局を完走し、棋譜、USIログ、event JSONLを点検する。
継続運転では局数と終了時刻の上限を決め、同じ実行ファイル、`.rsnn`評価ファイル、USI設定を維持する。
表示されたレートには対戦相手、先後、局数、運用期間が影響するため、ローカルのSPRTや別アカウントの値と直接比較しない。

## 対局ログを監視する

CSA対局とダッシュボードは別のプロセスで動く。
ShogiArena 1.2.6以降では次のようにイベントJSONLを監視できる。

```powershell
shogiarena dashboard watch `
  --csa-log-dir '.\csa-logs' `
  --out-run-dir '.\csa-dashboard' `
  --port 7777
```

対局時に生成されたイベントJSONL、CSA通信ログ、エンジン通信ログを原本として保存する。
ダッシュボードの表示内容だけで対局後の監査を済ませない。

# rsshogi-csaとFloodgate

## ローカルでCSA対局を確認する

外部サーバへ接続する前に、`rsshogi-csa`のopt-in loopback testでbridge、実エンジン、CSA handshake、通常対局、ponder対局、event JSONLをまとめて確認できる。
mini側で`just stage-engine`を実行してから、隣接する`rsshogi-csa`のrootで次を実行する。

```powershell
$env:RSSHOGI_CSA_TEST_ENGINE = `
  (Resolve-Path ../rsshogi-nnue-mini/dist/rsshogi-nnue-mini.exe).Path
cargo test -p rsshogi-csa-bridge --test loopback -- --nocapture
```

このtestはローカルのsocket上で通常対局とponder対局を最後まで進め、bridgeとエンジンの通信、終局処理、event JSONLを検証する。

## 外部CSAサーバ

release buildしたUSI executableを、別途導入した`rsshogi-csa-bridge`から起動する。
CSAの評価値コメントを先手視点で統一するため、`rsshogi-csa` 0.1.3以降を使う。
接続前にbridgeの版を確認する。

```powershell
csabridge --version
```

passwordは環境変数から渡す。

```powershell
$env:RSSHOGI_CSA_PASSWORD = 'server-specific-password'

csabridge --name 'rsshogi-nnue-mini' `
  --password-env RSSHOGI_CSA_PASSWORD `
  --engine '.\dist\rsshogi-nnue-mini.exe' `
  --engine-dir '.\dist' `
  --setoption 'USI_Ponder=true' `
  --setoption 'Threads=2' `
  --setoption 'USI_Hash=64' `
  --games 1 `
  --log-dir '.\csa-logs'
```

接続先host、port、account、password形式は大会またはserver管理者の案内に合わせる。
外部対局の確認では、1局の完了、illegal moveとtime loss、各`go`に対する`bestmove`、次局の`usinewgame`、終了時の`bridge_stop`をlogで調べる。

CSA対局とdashboardは別processである。
ShogiArena 1.2.6以降では次のようにevent JSONLを監視できる。

```powershell
shogiarena dashboard watch `
  --csa-log-dir '.\csa-logs' `
  --out-run-dir '.\csa-dashboard' `
  --port 7777
```

dashboardはCSA対局とは独立した監視processである。
source event JSONLを原本として保存する。

# 動かしてみる

## 必要なもの

- Rust 1.95以降
- WindowsではPowerShell 7とMSVC toolchain

## build

```powershell
cargo build --release
```

ShogiArenaの例は`dist/`に固定した実行ファイルを参照する。

```powershell
just stage-engine
```

## USI smoke test

```text
usi
isready
usinewgame
position startpos moves 7g7f 3c3d
go depth 3
quit
```

`go`はsessionが所有する常駐coordinatorで探索される。
`stop`はcancelを通知して終端結果を回収し、その探索で最後に確定した合法手を一度だけ`bestmove`として返す。
coordinator thread自体は次の探索で再利用し、session終了時に`Shutdown`してjoinする。

複数workerを確認する場合は、`go`より前に`setoption name Threads value 2`を送る。

## standard NNUE

通常ビルドはstandard NNUEを使用する。
公式配布物から取得してdigestを確認した`nn.bin`を、実行時の作業ディレクトリにある`eval/nn.bin`へ置く。
通常ビルドでは、評価fileとscaleが`eval/nn.bin`と`FV_SCALE = 24`に固定されている。

```text
isready
```

読み込みに失敗すると`info string NNUE load error: ...`を出し、`readyok`を返さない。
この状態の`go`はmaterial評価へ切り替わらず、`bestmove resign`でfail closedする。

SPSA用の調整面が必要な場合は、別のtuning binaryを作成する。

```powershell
just release-avx2-tuning
```

## test

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
```

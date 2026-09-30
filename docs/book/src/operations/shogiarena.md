# ShogiArenaで対局とパラメータ調整を行う

ShogiArenaでUSI対局を確認し、SPSAで候補を作り、未使用の開始局面を使ったSPRTで候補を比較する。
設定例は`examples/shogiarena/`にあり、ShogiArena 1.2.7での実験を基に探索パラメータの名前を現在のエンジンへ合わせている。
ShogiArenaは別途導入し、コマンドはリポジトリのルートから実行する。
使用する版を`shogiarena --version`で記録し、各設定を`--validate-only`で確認してから実行する。
SPSAの更新式、gain schedule、整数丸め、分散削減、artifact監査、holdout判定は[SPSAの設計と結果の読み方](spsa.md)で説明する。

最初にrelease版の実行ファイルを`dist/`へ配置し、実行結果の保存先をエンジンの作業ディレクトリの外に作る。
設定例はエンジンの`working_directory`を`dist`に限定し、SPSAの保存物へリポジトリ全体が混入することを防ぐ。
`dist/eval/model.rsnn`には、利用者が別途作成してSHA-256を確認した512幅・ThreatなしSFNNv15 packageを置く。
`just stage-engine`が配置するのは実行ファイルだけで、評価ファイルはコピーしない。

```powershell
just stage-engine
New-Item -ItemType Directory -Force ..\rsshogi-nnue-mini-runs | Out-Null
```

SPSAまたはSPRTの設定例は、調整用のAVX2ビルドを参照する。
AVX2対応CPUを使い、次のレシピで別名の実行ファイルを配置する。

```powershell
just release-avx2-tuning
```

AVX2を前提にしない環境では、`just release-tuning`で作り、設定の`engine_path`を`dist/rsshogi-nnue-mini-tuning.exe`へ変更する。

## 1. USI対局の通信を確認する

```powershell
shogiarena run tournament ./examples/shogiarena/smoke.yaml --validate-only
shogiarena run tournament ./examples/shogiarena/smoke.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/tournament-smoke
```

同じ実行ファイルを先後の2役で2局だけ対局させ、起動、`usinewgame`、`position`、`go`、`bestmove`、終了を確認する。
ここで確認するのは対局経路であり、棋力差はSPRTで測る。

## 2. SPSAの通信と記録を確認する

`spsa-smoke.yaml`は更新1回、先後ペア1組だけを実行する。
これでmanifestの受け渡し、探索パラメータ7個と`FV_SCALE`のvariant適用、対局、ledgerへの記録、後片付けを確認する。
SPSAでは`dist`全体がengine runtimeとしてarchiveされ、エンジンはその中の`eval/model.rsnn`を使う。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/spsa-protocol-smoke
```

`--validate-only`は設定を静的に検査し、`--dry-run`は実行計画を確認する。
続く先後ペア1組の対局で、エンジンの初期通信からledgerへの記録、後片付けまでを確認する。

## 3. SPSAで候補を作る

```powershell
shogiarena run spsa ./examples/shogiarena/spsa.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/spsa-campaign
```

1局面から先後を入れ替えた2局を1ペアとし、同じノード上限を使う。
`Clear Hash`は、調整用ビルドが公開するvariant間リセット用のUSIオプションで、各variantを適用するときに送られる。
`spsa.yaml`は、`spsa-space.yaml`の`select`で選んだ探索パラメータ7個と`FV_SCALE`の計8個を同時に動かすための初期設定である。
manifestにある探索パラメータ35個と時間管理の定数5個のうち、`select`で選んでいない項目は動かさない。
`instances-spsa.yaml`は、同時実行を8対局、16エンジンプロセスまでに制限する。
実行時間はマシンとネットワークで変わるため、短い確認対局で所要時間を測り、時間枠へ収まる更新回数を決める。

`manifest.json`、`game.db`、`completion_status.json`、`spsa/ledger.sqlite3`、`spsa/accepted-best.json`を含むrun directory全体を保存する。
完了判定には`completion_status.json`とledgerを使う。
dashboardや`state.json`だけではresumeや完了の根拠にならない。

このSPSA例は、調整用ビルドのNNUE評価を固定したままパラメータを調整する。

訓練用定跡は160局面である。
少数の定跡だけを使うと、その序盤に特化したparameterが選ばれやすい。
そこで、訓練用とholdoutの定跡を別々のseedから作り、局面の重複も検査する。
SPSAが出力した候補は`candidate.yaml`へ記録し、次のholdout SPRTで既定値と比較する。

### 専用Linuxホストで実行する

長い実験では、開始前にホスト、ソースのrevision、ツールの版、実行ファイル、NNUE評価を固定する。
CPU model、論理CPU数、NUMA構成、memoryも記録し、他の重いprocessが動いていないことを確認する。
Linux binaryには`.exe` suffixを付けない。ShogiArenaはbinary名もplatform検証に使うため、ELFへ`.exe`を付けると対局開始前に拒否される。

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export RUSTFLAGS='-C target-feature=+avx2'
cargo build --release --features tuning
cp target/release/rsshogi-nnue-mini dist/rsshogi-nnue-mini-tuning-avx2
chmod +x dist/rsshogi-nnue-mini-tuning-avx2

rustc --version
shogiarena --version
sha256sum dist/rsshogi-nnue-mini-tuning-avx2 dist/eval/model.rsnn
```

Linuxでは`engine-tuning-linux.yaml`を使う。
本番条件を決め打ちせず、同じnode limitとpair数で1 updateだけ実行し、`completion_status.json`がcleanであること、ledgerへ1 updateがcommitされたこと、CPU使用率、wall timeを確認する。
`Threads=1`でも1対局はengineを2 process起動する一方、通常は片側だけが探索するため、適切な同時対局数はprocess数だけで決めず実測する。

前回より`pairs_per_update`やnode limitを増やす場合は、一度に両方を変えた事実を記録する。
得られた候補の違いを、どちらの変更によるものか分離できなくなるためである。
本番開始前に、選択したパラメータ、更新数、ペア数、ノード上限、並列数、開始局面、乱数方式、見積り時間を固定する。

このrepositoryで専用8 vCPU Linux host向けに固定した例は`spsa-dedicated-host.yaml`である。
280 updates、8 pairs/update、一手あたり100,000ノード、8対局並列を使う。
現在の設定は`spsa-space.yaml`を参照し、8パラメータを調整する。

```bash
shogiarena run spsa ./examples/shogiarena/spsa-dedicated-host.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa-dedicated-host.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa-dedicated-host.yaml \
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/spsa-dedicated-host
```

SSH切断後も走らせる場合は、標準出力と標準エラーをrun directory外のlogへ保存する。
進捗はlogだけでなくledgerのcommitted updateで確認し、完了は`completion_status.json`の`status: clean`、`termination_reason: completed`、anomaly 0、cleanup cleanをすべて確認する。
途中の`state.json`やdashboard表示だけを完了根拠にしない。

## 4. SPRTの通信と記録を確認する

2局だけの`sprt-smoke.yaml`は、baselineとcandidateの起動、holdout opening、paired result、SPRT stateの記録を確認する。

```powershell
shogiarena run sprt ./examples/shogiarena/sprt-smoke.yaml --validate-only
shogiarena run sprt ./examples/shogiarena/sprt-smoke.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/sprt-protocol-smoke
```

2局の確認対局では、先後ペアの結果とSPRTの状態が最後まで記録されることを確認する。

## 5. holdout SPRTで候補を確認する

`spsa/accepted-best.json`の`wire_value`から、候補として送るUSIオプションを`candidate.yaml`へ転記する。
`baseline.yaml`にも比較基準の値を明示し、学習に使っていない`openings-holdout.sfen`で比較する。
同梱の2ファイルは過去の比較に由来する値を含むため、そのままでは現在の既定値との比較にならない。
未指定のオプションには使用する実行ファイルの既定値が使われるので、比較対象の全パラメータを記録する。

holdout定跡は160局面である。
`flip_policy: pair_both`で先後を入れ替えた2局を1 pairとして扱い、設定例の`max_games`は320局としている。
開始局面が少ないと、決定的なengine同士では同じ対局を繰り返すだけになり、pairの結果が分散せずSPRTが統計として成立しない。
定跡を差し替えるときは、局面数、`max_games`、pentanomialの分散が噛み合っているかを確認する。

```powershell
shogiarena run sprt ./examples/shogiarena/sprt.yaml --validate-only
shogiarena run sprt ./examples/shogiarena/sprt.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/sprt-holdout
```

専用Linuxホストで比較するときは、Linux用の設定例を使い、参照先のbaselineとcandidateも今回の比較条件へ合わせる。

```bash
shogiarena run sprt ./examples/shogiarena/sprt-dedicated-host.yaml --validate-only
shogiarena run sprt ./examples/shogiarena/sprt-dedicated-host.yaml \
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/sprt-dedicated-host
```

SPRTはSPSAとは独立したrunである。
H0/H1のElo幅、alpha/beta、最大局数を実験前に固定し、最大局数までに境界へ達しなければ「未決着」と解釈する。

同じgateで探索の変更も比較できる。SPSA候補ではなく二つのbinaryを比べるときは、
`baseline.yaml`と`candidate.yaml`の代わりに`engine_path`だけを書いたengine configを用意する。
`elo0 = 0`、`elo1 = 5`は近い仮説なので、320局で決着するのは差が大きい場合だけである。
決着しなかったときは、点推定と信頼区間をSPRTのdecisionとは別の主張として読む。

時間制限の実験は、探索速度と探索効率の両方を含む実戦的な比較になる。
ノード制限の実験は、一定の探索量でどの程度良い手を選べるかを比べる。
両方を測ると差の原因を考える材料になるが、並列探索や局面ごとの処理時間も影響するため、原因を完全に分離できるわけではない。

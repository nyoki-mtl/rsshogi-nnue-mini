# ShogiArenaでsmoke test、SPSA、SPRT

ShogiArena 1.2.7で検証した設定例を`examples/shogiarena/`に置いている。
commandはrepository rootから実行する。
SPSAの更新式、gain schedule、整数丸め、分散削減、artifact監査、holdout判定は[SPSAの設計と結果の読み方](spsa.md)で説明する。

最初にrelease binaryを`dist/`へ配置し、run directoryをengineのworking directory外に作る。
ShogiArenaのSPSA archiveへrepository全体を混入させないため、engineの`working_directory`は`dist`に限定している。

```powershell
just stage-engine
New-Item -ItemType Directory -Force ..\rsshogi-nnue-mini-runs | Out-Null
```

SPSAまたはSPRTのparameter候補を使うときは、tuning binaryとそのstage先を別に作成する。

```powershell
just release-avx2-tuning
```

## 1. USI対局経路をsmoke testする

```powershell
shogiarena run tournament ./examples/shogiarena/smoke.yaml --validate-only
shogiarena run tournament ./examples/shogiarena/smoke.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/tournament-smoke
```

同じbinaryを2役で2局だけ対局させ、起動、`usinewgame`、`position`、`go`、`bestmove`、終了を確認する。
ここで確認するのは対局経路であり、棋力差はSPRTで測る。

## 2. SPSA protocolを確認する

1 update、1 pairだけの`spsa-smoke.yaml`で、manifest handshake、探索parameter 12個と`FV_SCALE`のvariant適用、対局、ledger commit、cleanupを確認する。
`dist/eval/nn.bin`には、利用者が別途取得してdigestを確認したstandard HalfKP networkを置く。
SPSAでは`dist`全体がengine runtimeとしてarchiveされるため、run configから`EvalFile`を上書きせず、エンジン既定の`eval/nn.bin`を使う。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/spsa-protocol-smoke
```

`--validate-only`は設定を静的に検査し、`--dry-run`は実行計画を確認する。
続く1 pairのsmoke runで、engineのhandshakeからledger commit、cleanupまでを確認する。

## 3. SPSAで候補を作る

```powershell
shogiarena run spsa ./examples/shogiarena/spsa.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/spsa-campaign
```

1局面を先後反転したpairとして扱い、同じnode limitを使う。
`Clear Hash`はtuning binaryが広告するvariant間リセット用buttonとして各variantの適用時に送られる。
`spsa.yaml`は探索parameter 12個と`FV_SCALE`を同時に動かすcampaign用の初期設定である。
campaign用の`instances-spsa.yaml`は8対局、16 engine processを上限にする。
実行時間はmachineとnetworkで変わるため、所要時間を先にsmokeで測り、固定した時間枠へ収まるupdate数に調整する。

`manifest.json`、`game.db`、`completion_status.json`、`spsa/ledger.sqlite3`、`spsa/accepted-best.json`を含むrun directory全体を保存する。
完了判定には`completion_status.json`とledgerを使う。
dashboardや`state.json`だけではresumeや完了の根拠にならない。

このSPSA例はtuning binaryの固定NNUE経路でparameterを調整する。

訓練用定跡は160局面である。
少数の定跡だけを使うと、その序盤に特化したparameterが選ばれやすい。
そこで、訓練用とholdoutの定跡を別々のseedから作り、局面の重複も検査する。
SPSAが出力した候補は`candidate.yaml`へ記録し、次のholdout SPRTで既定値と比較する。

### 専用Linux hostで実行する

長いcampaignでは、実験前にhost、source revision、tool version、engine binary、NNUEを固定する。
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
sha256sum dist/rsshogi-nnue-mini-tuning-avx2 dist/eval/nn.bin
```

Linuxでは`engine-tuning-linux.yaml`を使う。
本番条件を決め打ちせず、同じnode limitとpair数で1 updateだけ実行し、`completion_status.json`がcleanであること、ledgerへ1 updateがcommitされたこと、CPU使用率、wall timeを確認する。
`Threads=1`でも1対局はengineを2 process起動する一方、通常は片側だけが探索するため、適切な同時対局数はprocess数だけで決めず実測する。

前回より`pairs_per_update`やnode limitを増やす場合は、一度に両方を変えた事実を記録する。
得られた候補の違いを、どちらの変更によるものか分離できなくなるためである。
本番開始前に、13 parameter、update数、pair数、node limit、並列数、開始局面、乱数方式、見積り時間を固定する。

このrepositoryで専用8 vCPU Linux host向けに固定した例は`spsa-dedicated-host.yaml`である。
280 updates、8 pairs/update、100,000 nodes/game、8対局並列を使う。

```bash
shogiarena run spsa ./examples/shogiarena/spsa-dedicated-host.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa-dedicated-host.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa-dedicated-host.yaml \
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/spsa-dedicated-host
```

SSH切断後も走らせる場合は、標準出力と標準エラーをrun directory外のlogへ保存する。
進捗はlogだけでなくledgerのcommitted updateで確認し、完了は`completion_status.json`の`status: clean`、`termination_reason: completed`、anomaly 0、cleanup cleanをすべて確認する。
途中の`state.json`やdashboard表示だけを完了根拠にしない。

実測では280 updates、4,480局を約4時間57分でclean完了した。
ただし、独立holdout SPRTは320局で158勝3分159敗、LLR -0.0462の未決着だった。
SPSAを大規模化しても候補の改善が保証されるわけではないため、候補を既定値へ反映しなかった。

## 4. SPRT protocolを確認する

2局だけの`sprt-smoke.yaml`は、baselineとcandidateの起動、holdout opening、paired result、SPRT stateの記録を確認する。

```powershell
shogiarena run sprt ./examples/shogiarena/sprt-smoke.yaml --validate-only
shogiarena run sprt ./examples/shogiarena/sprt-smoke.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/sprt-protocol-smoke
```

2局のsmoke runでは、paired resultとSPRT stateが最後まで記録されることを確認する。

## 5. holdout SPRTで候補を確認する

`spsa/accepted-best.json`から採用候補を`candidate.yaml`へ転記し、学習に使っていない`openings-holdout.sfen`で比較する。

holdout定跡は160局面である。
`flip_policy: pair_both`で先後を入れ替えた2局を1 pairとして扱うため、上限は320局になる。
開始局面が少ないと、決定的なengine同士では同じ対局を繰り返すだけになり、pairの結果が分散せずSPRTが統計として成立しない。
定跡を差し替えるときは、局面数、`max_games`、pentanomialの分散が噛み合っているかを確認する。

```powershell
shogiarena run sprt ./examples/shogiarena/sprt.yaml --validate-only
shogiarena run sprt ./examples/shogiarena/sprt.yaml `
  --run-dir ../rsshogi-nnue-mini-runs/sprt-holdout
```

専用Linux host campaignの候補を再検証するときは、固定済みの設定を使う。

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

時間制限のrunはNPSと探索効率を合わせて測り、node制限のrunは探索効率を測る。
両方を比較すると、棋力差の由来を切り分けられる。

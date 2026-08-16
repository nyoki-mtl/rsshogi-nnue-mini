# SPSAの設計と結果の読み方

**SPSA**（Simultaneous Perturbation Stochastic Approximation）は、対局結果から複数parameterの更新方向を同時に推定する最適化法である。
このrepositoryでは、探索parameter 12個と`FV_SCALE`をShogiArena 1.2.7で調整する。

SPSAが返すのは、訓練用の対局条件に対するparameter候補である。
「SPSAが完走した」は「候補の棋力が上がった」を意味しない。
候補生成と棋力判定を分けるため、SPSAの後に訓練で使っていない局面によるholdout SPRTを別runとして実行する。

## 一回の更新で何を測るか

parameterを並べたvectorを`theta`とする。
更新`k`では、各parameterについて`+1`または`-1`を等確率で選び、摂動方向`flip`を作る。
ShogiArenaはparameterごとの摂動幅`c_i`を使い、次の二つのvariantを作る。

```text
theta_plus[i]  = theta[i] + c_i * flip[i]
theta_minus[i] = theta[i] - c_i * flip[i]
```

`theta_plus`と`theta_minus`は、13個のparameterを一度に動かした組である。
有限差分法のようにparameterを一個ずつ動かさないため、parameter数が増えても一更新で比較するvariantは二つのままである。

各variantの対局scoreを`score_plus`と`score_minus`とする。
両者の差を摂動方向へ割り当てると、parameter`i`の勾配を次の形で推定できる。

```text
gradient[i] = (score_plus - score_minus) / (2 * c_i * flip[i])
```

一回の推定には、ほかのparameterを同時に動かした影響と対局結果の揺らぎが混ざる。
更新ごとに`flip`を引き直して反復することで、混ざった成分を平均化する。

## ShogiArenaのgain schedule

このrepositoryはclassic SPSAを使い、`alpha = 0.602`、`gamma = 0.101`、`A.mode = ratio`、`A.value = 0.1`を指定する。
更新番号そのものではなく、消化したpair数でscheduleが進む。

一更新のpair数を`m`、1から始まる更新番号を`u`、総更新数を`N`とすると、scheduleが使う通し番号は次のとおりである。

```text
k_pair  = u * m
k_total = N * m
A_abs   = 0.1 * k_total
```

各parameterのmanifestは、最終更新時の摂動幅`c_end`と学習率係数`r_end`を持つ。
ShogiArenaは前半の摂動を大きくし、最終更新で`c_i = c_end`になるように減衰させる。

```text
c_i = c_end[i] * (k_total / k_pair) ^ gamma
```

勝ちを1、引き分けを0.5、負けを0として、一更新で得たplus側のscore合計からminus側のscore合計を引いた値を`step_score`とする。
ShogiArenaはparameterごとの更新係数`r_i`と更新量を次の式で求める。

```text
r_i = r_end[i] * c_end[i]^2 * (A_abs + k_total)^alpha
      / ((A_abs + k_pair)^alpha * c_i^2)

delta_theta[i] = r_i * c_i * step_score * flip[i]
theta[i]       = theta[i] + delta_theta[i]
```

`c_end`はparameterをどれだけ離して比較するかを決め、`r_end`は観測したscore差を次の`theta`へどれだけ反映するかを決める。
グローバルな学習率を一つ置かず、parameterの単位に合わせて両者を指定する。

`c_end`を小さくすると局所的な差を測れる一方、同じ対局noiseを小さい値で割るため勾配の分散が増える。
`c_end`を大きくするとscore差は見えやすくなるが、離れた二点から求めるため局所勾配としての偏りが増える。
範囲とscheduleは、parameterの単位と探索への感度に合わせて決める。

## parameter空間をエンジンに持たせる

tuning binaryは`usi_tunables`へ、parameter名、USI option名、型、既定値、最小値、最大値、`c_end`、`r_end`をJSONで返す。
ShogiArenaの`spsa-space.yaml`は対象IDだけを選び、実際の範囲とscheduleはこのmanifestを正本とする。

この分担には二つの目的がある。
第一に、探索実装と離れた設定fileへ古い既定値や範囲を複製しないためである。
第二に、実行前のhandshakeでbinaryとspaceの不一致を拒否するためである。

correctnessやresource safetyを変える値はSPSAへ公開しない。
詰みscoreの帯、最大qsearch ply、SEEの作業量上限、置換表構造は、勝率だけで自由に動かせるparameterではない。

## 整数parameterと確率的丸め

SPSA内部の`theta`は小数で保持するが、USI spin optionへ送れる値は整数である。
毎回最近接整数へ丸めると、`theta`が丸め境界を越えるまで小さな更新がすべて消える。

`integer_rounding: stochastic`は、小数部分を確率として上下の整数を選ぶ。
例えば`1.25`は25%の確率で2、75%の確率で1になる。
多数の試行における平均は1.25なので、小さな更新を対局へ反映できる。

最終候補は`accepted-best.json`の`wire_value`で固定する。
内部値`value`が`1.8335`でも、エンジンへ送る採用値は`wire_value: 2`である。

## 対局noiseを減らす

この設定は、plus/minus以外の条件を揃えてscore差の分散を減らす。

- `pairing: plus_minus`：一つの更新でplusとminusを必ず比較する
- `crn: true`：plusとminusへ同じ開始局面を割り当てる
- `flip_policy: pair_both`：同じ開始局面を先後反転した2局で一つのpairにする
- `sync_scope: pair`：pair内の条件を同期する
- `node_limit`：両variantへ同じ探索node数を与える
- `Clear Hash`：variantを適用するたびに置換表を消す
- `after_setoption: isready`：option適用後の準備完了を待ってから対局する

同じ開始局面を使うだけでは局面の偏りを解消できない。
訓練用定跡が3局面だった実験では、候補がその序盤へ適応し、160局面のholdoutへ移らなかった。
現在は、異なるseedで生成した訓練160局面とholdout 160局面を使い、SFENの重複が0であることを確認している。

## campaign条件を先に固定する

長いrunを始める前に、次を記録する。

- source revisionとsource archiveのSHA-256
- RustとShogiArenaのversion
- engine binaryとNNUEのSHA-256
- CPU model、論理CPU数、NUMA、memory
- parameter数、更新数、pair数、node limit、並列数
- 訓練用定跡、乱数方式、gain schedule
- 予想所要時間とrun directory

条件を結果の後で変えると、どの変更が候補へ効いたか分からなくなる。
特に`pairs_per_update`とnode limitを同時に増やした場合は、その事実を記録し、個別の効果を主張しない。

## smoke、benchmark、本番の順で実行する

最初に設定と実行計画を検査する。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --dry-run
```

次に、1 update、1 pairのsmokeでUSI handshake、option適用、対局、ledger commit、cleanupを確認する。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml `
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/spsa-smoke
```

本番と同じpair数、node limit、並列数による1 update benchmarkでwall timeとCPU使用率を測る。
1対局はengineを2 process起動するが、通常は片側だけが探索するため、process数だけから適切な並列数は決められない。

条件を固定したら、新しいrun directoryで本番を開始する。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa.yaml `
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/spsa-campaign
```

`--no-resume`は既存runを上書きする指定ではない。
同名directoryが存在する場合は別名を選び、過去のledgerを保存する。

## 完了をledgerで判定する

dashboardは進捗表示であり、完了の正本ではない。
run directoryでは、少なくとも次を保存する。

- `manifest.json`：resolved configとartifact provenance
- `game.db`：対局記録
- `completion_status.json`：runの終端状態
- `spsa/ledger.sqlite3`：updateのtransaction ledger
- `spsa/accepted-best.json`：最後にcommitされた候補
- `spsa/terminal.json`：SPSAの終端状態

採用候補を読む前に、次をすべて確認する。

```text
completion_status.status == "clean"
completion_status.termination_reason == "completed"
last_committed_update == num_updates
pending_update == null
incomplete == 0
failed == 0
cleanup.status == "clean"
ledger上の全update == "COMMITTED"
game数 == num_updates * pairs_per_update * 2
```

`state.json`だけを見て完走と判断しない。
process停止時にpending updateが残っていれば、`accepted-best.json`が存在してもcampaign全体の完了にはならない。

## holdout SPRTを別runにする

SPSAのscoreは、parameter更新に使った訓練局面で観測された値である。
同じ局面を採用判定にも使うと、局面への過適合と一般的な改善を区別できない。

そこで、`accepted-best.json`の`wire_value`をcandidate設定へ転記し、別seedのholdout局面でbaselineと比較する。
SPRTの`elo0`、`elo1`、`alpha`、`beta`、`max_games`は実験前に固定する。

```powershell
shogiarena run sprt ./examples/shogiarena/sprt.yaml --validate-only
shogiarena run sprt ./examples/shogiarena/sprt.yaml `
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/sprt-holdout
```

LLRが上側境界へ達すれば`H1`、下側境界へ達すれば`H0`を採択する。
最大局数に達してどちらにも届かなければ「未決着」である。
未決着を「差がない」と言い換えず、勝敗、LLR、点推定、信頼区間を別々に記録する。

## 2026年8月の実行結果

専用8 vCPU Linux hostでは、13 parameter、280 updates、8 pairs/update、100,000 nodes/game、8対局並列で実行した。
SPSAは4,480局を約4時間57分でclean完了した。

独立holdout SPRTは320局を完了し、candidateから見て158勝3分159敗、LLR -0.0462、点推定約-1.1 Eloだった。
95%信頼区間は約[-39.4, +37.2] Eloで、SPRTは未決着だった。

したがって、この測定は棋力向上を示していない。
その後、開発方針としてcandidateの探索12 parameterのwire値を新しい既定値へ採用した。
`FV_SCALE`候補25は採用せず、水匠5公式profileと同じ24を維持した。
採用後の探索値は「SPSAで棋力向上を確認した値」ではなく、「cleanな候補生成を経て選んだ新しいbaseline」である。

`examples/shogiarena/baseline.yaml`と`candidate.yaml`は、holdoutで実際に比較した二つのvariantを再現する履歴である。
このためcandidate側は`FV_SCALE: 25`を含む。
現在の通常実行の既定値とは用途が異なるため、これらのfileを既定値一覧として使わない。

## 失敗しやすい境界

- Linux ELFへ`.exe`を付けない。ShogiArenaはsuffixもplatform検証に使う
- instance IDはengine specが参照する`local`のままにする
- engineの`working_directory`を`dist`へ限定し、run archiveへrepositoryを混入させない
- NNUEをrepositoryへcommitしない。runtimeへ別配置してdigestを記録する
- dashboardの表示とledgerのtransaction状態を混同しない
- SPSA候補とholdout結果を同じ結論にまとめない
- max games到達のSPRTを「同等」と判定しない

## 参考資料

- James C. Spall, “Multivariate Stochastic Approximation Using a Simultaneous Perturbation Gradient Approximation,” 1992
- [ShogiArena](https://github.com/nyoki-mtl/ShogiArena)
- [ShogiArena documentation](https://nyoki-mtl.github.io/ShogiArena/)

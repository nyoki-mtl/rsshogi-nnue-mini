# SPSAの設計と結果の読み方

**SPSA**（Simultaneous Perturbation Stochastic Approximation）は、対局結果から複数のパラメータの更新方向を同時に推定する最適化法である。
現在の設定例では、探索パラメータ7個と`FV_SCALE`の計8個を選んで調整する。
設定例はShogiArena 1.2.7での実験を基にしているが、パラメータ名は現在のエンジンに合わせている。

SPSAが返すのは、訓練用の対局条件に対するパラメータの候補である。
「SPSAが完走した」は「候補の棋力が上がった」を意味しない。
候補生成と棋力判定を分けるため、SPSAの後に訓練で使っていない局面による確認用のSPRTを、独立した実行記録として行う。

## 一回の更新で何を測るか

パラメータを並べたベクトルを`theta`とする。
更新`k`では、各パラメータについて`+1`または`-1`を等確率で選び、摂動方向`flip`を作る。
ShogiArenaはパラメータごとの摂動幅`c_i`を使い、次の二つの候補を作る。

```text
theta_plus[i]  = theta[i] + c_i * flip[i]
theta_minus[i] = theta[i] - c_i * flip[i]
```

`theta_plus`と`theta_minus`は、選択した8個のパラメータを一度に動かした組である。
有限差分法のようにパラメータを一個ずつ動かさないため、個数が増えても一更新で比較する候補は二つのままである。

各候補の対局スコアを`score_plus`と`score_minus`とする。
両者の差を摂動方向へ割り当てると、パラメータ`i`の勾配を次の形で推定できる。

```text
gradient[i] = (score_plus - score_minus) / (2 * c_i * flip[i])
```

一回の推定には、ほかのパラメータを同時に動かした影響と対局結果の揺らぎが混ざる。
更新ごとに`flip`を引き直して反復することで、混ざった成分を平均化する。

## 更新係数の変化

このリポジトリはclassic SPSAを使い、`alpha = 0.602`、`gamma = 0.101`、`A.mode = ratio`、`A.value = 0.1`を指定する。
更新係数は更新番号そのものではなく、消化した先後ペアの数に従って変わる。

一更新のpair数を`m`、1から始まる更新番号を`u`、総更新数を`N`とすると、scheduleが使う通し番号は次のとおりである。

```text
k_pair  = u * m
k_total = N * m
A_abs   = 0.1 * k_total
```

各パラメータのmanifestは、最終更新時の摂動幅`c_end`と学習率係数`r_end`を持つ。
ShogiArenaは前半の摂動を大きくし、最終更新で`c_i = c_end`になるように減衰させる。

```text
c_i = c_end[i] * (k_total / k_pair) ^ gamma
```

勝ちを1、引き分けを0.5、負けを0として、一更新で得たplus側のscore合計からminus側のscore合計を引いた値を`step_score`とする。
ShogiArenaはパラメータごとの更新係数`r_i`と更新量を次の式で求める。

```text
r_i = r_end[i] * c_end[i]^2 * (A_abs + k_total)^alpha
      / ((A_abs + k_pair)^alpha * c_i^2)

delta_theta[i] = r_i * c_i * step_score * flip[i]
theta[i]       = theta[i] + delta_theta[i]
```

`c_end`はparameterをどれだけ離して比較するかを決め、`r_end`は観測したscore差を次の`theta`へどれだけ反映するかを決める。
学習率を一つに固定せず、パラメータの単位に合わせて両者を指定する。

`c_end`を小さくすると局所的な差を測れる一方、同じ対局noiseを小さい値で割るため勾配の分散が増える。
`c_end`を大きくするとscore差は見えやすくなるが、離れた二点から求めるため局所勾配としての偏りが増える。
範囲と更新係数の変化は、パラメータの単位と探索への感度に合わせて決める。

## パラメータの定義をエンジンに持たせる

調整用ビルドは`usi_tunables`へ、パラメータ名、USIオプション名、型、既定値、最小値、最大値、`c_end`、`r_end`をJSONで返す。
ShogiArenaの`spsa-space.yaml`は対象IDだけを選び、実際の範囲と更新係数はこのmanifestを正本とする。

この分担には二つの目的がある。
第一に、探索実装と離れた設定ファイルへ古い既定値や範囲を複製しないためである。
第二に、実行前の初期通信で実行ファイルと設定の不一致を拒否するためである。

正しさや資源の安全性に関わる値はSPSAへ公開しない。
詰みスコアの帯、静止探索の最大手数、SEEの作業量上限、置換表構造は、勝率だけで自由に動かせるパラメータではない。

## 整数パラメータと確率的丸め

SPSA内部の`theta`は小数で保持するが、USI spin optionへ送れる値は整数である。
毎回最近接整数へ丸めると、`theta`が丸め境界を越えるまで小さな更新がすべて消える。

`integer_rounding: stochastic`は、小数部分を確率として上下の整数を選ぶ。
例えば`1.25`は25%の確率で2、75%の確率で1になる。
多数の試行における平均は1.25なので、小さな更新を対局へ反映できる。

最終候補は`accepted-best.json`の`wire_value`で固定する。
内部値`value`が`1.8335`でも、エンジンへ送る採用値は`wire_value: 2`である。

## 対局結果の揺らぎを減らす

この設定は、plus側とminus側でほかの条件を揃え、スコア差のばらつきを減らす。

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

## 長い実験の条件を先に固定する

長い実験を始める前に、次を記録する。

- ソースのrevisionとソースarchiveのSHA-256
- RustとShogiArenaのバージョン
- エンジン実行ファイルとNNUE評価のSHA-256
- CPUの型番、論理CPU数、NUMA構成、メモリ容量
- パラメータ数、更新数、先後ペア数、ノード上限、並列数
- 訓練用開始局面、乱数方式、更新係数の設定
- 予想所要時間と実行結果の保存先

条件を結果の後で変えると、どの変更が候補へ効いたか分からなくなる。
特に`pairs_per_update`とノード上限を同時に増やした場合は、その事実を記録し、個別の効果を主張しない。

## 小規模な確認から本番へ進む

最初に設定と実行計画を検査する。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml --dry-run
```

次に、更新1回、先後ペア1組の対局でUSIの初期通信、オプション適用、対局、ledgerへの記録、後片付けを確認する。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa-smoke.yaml `
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/spsa-smoke
```

本番と同じ先後ペア数、ノード上限、並列数で更新を1回実行し、所要時間とCPU使用率を測る。
1対局はエンジンを2プロセス起動するが、通常は片側だけが探索するため、プロセス数だけから適切な並列数は決められない。

条件を固定したら、新しいディレクトリで本番の結果を記録する。

```powershell
shogiarena run spsa ./examples/shogiarena/spsa.yaml --validate-only
shogiarena run spsa ./examples/shogiarena/spsa.yaml --dry-run
shogiarena run spsa ./examples/shogiarena/spsa.yaml `
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/spsa-campaign
```

`--no-resume`は、前の実行を再開せず、新しく始める指定である。
既存の実行記録を保護する指定ではないため、新しい実行ディレクトリ名を使い、過去のledgerを保存する。

## 完了はledgerで判定する

ダッシュボードは進捗表示であり、完了の正本ではない。
実行結果のディレクトリでは、少なくとも次を保存する。

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
プロセス停止時に未確定の更新が残っていれば、`accepted-best.json`が存在しても実験全体の完了にはならない。

## 未使用の局面でSPRTを別に行う

SPSAのスコアは、パラメータ更新に使った訓練局面で観測された値である。
同じ局面を採用判定にも使うと、局面への過適合と一般的な改善を区別できない。

そこで、`accepted-best.json`の`wire_value`を候補側の設定へ転記し、別の乱数seedで作った未使用の局面で基準版と比較する。
SPRTの`elo0`、`elo1`、`alpha`、`beta`、`max_games`は実験前に固定する。

```powershell
shogiarena run sprt ./examples/shogiarena/sprt.yaml --validate-only
shogiarena run sprt ./examples/shogiarena/sprt.yaml `
  --no-resume --run-dir ../rsshogi-nnue-mini-runs/sprt-holdout
```

LLRが上側境界へ達すれば`H1`、下側境界へ達すれば`H0`を採択する。
最大局数に達してどちらにも届かなければ「未決着」である。
未決着を「差がない」と言い換えず、勝敗、LLR、点推定、信頼区間を別々に記録する。

## 失敗しやすい境界

- LinuxのELF実行ファイルへ`.exe`を付けない。ShogiArenaは拡張子もプラットフォームの検証に使う
- instance IDはエンジン設定が参照する`local`のままにする
- エンジンの`working_directory`を`dist`へ限定し、実行結果のアーカイブへリポジトリを混入させない
- NNUE評価ファイルをリポジトリへcommitしない。実行環境へ別配置してdigestを記録する
- ダッシュボードの表示とledgerの確定状態を混同しない
- SPSA候補とholdout結果を同じ結論にまとめない
- max games到達のSPRTを「同等」と判定しない

## 参考資料

- James C. Spall, “Multivariate Stochastic Approximation Using a Simultaneous Perturbation Gradient Approximation,” 1992
- [ShogiArena](https://github.com/nyoki-mtl/ShogiArena)
- [ShogiArena documentation](https://nyoki-mtl.github.io/ShogiArena/)

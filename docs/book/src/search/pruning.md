# 枝刈りと深さ調整

[alpha-beta法](alphabeta.md)と[手の並べ替え](ordering.md)は「探索結果を一切変えない」厳密な高速化でした。しかし、それだけでは実戦の持ち時間で十数手〜二十数手先まで読み切ることはできません。

現代の将棋エンジンが大きく強さを伸ばしている理由は、**「ほぼ確実に逆転しない枝」を浅く済ませたり切り落としたりする一方で、「唯一の急所になりそうな手」は逆に深く読む**というメリハリの効いたリソース配分にあります。実装は `src/search/negamax.rs`・`pruning.rs`・`lmr.rs` にあり、調整可能な定数はすべて `src/search/params.rs` の `SearchParams` に集約されています。

---

## 1. 枝刈りを支える2つの判断材料

個別の枝刈り手法に入る前に、多くの判定で共通して参照する2つの指標を押さえておきます。

### 静的評価の補正履歴（`CorrectionHistory`：`pruning.rs`）

NNUE 評価関数にも、特定の持ち駒バランスや歩の形・玉の位置関係で「探索してみると毎回いつもより少し高め（または低め）に出る」という局所的なクセが生じます。

そこで、王手がかかっておらず静かな手が最善になったノードで、探索結果 `best_score` と生の静的評価値 `raw_eval` のズレを次の3つのハッシュ表（各16,384エントリ、ワーカー別）に少しずつ学習させます。

1. **持ち駒ハッシュ（`hand`）**
2. **先後の歩の配置ハッシュ（`pawn`）**
3. **先後の玉の位置ペア（`king_pair`）**

以降のノードでは、NNUE の生スコアにこの3表から引いた補正値（最大 `±45` 点、`correction_apply_max = 45`）を加えた **`corrected_eval`** を基準にして枝刈りの閾値判定を行います。なお、置換表の `static_eval` 欄には二重補正を防ぐため常に補正前の `raw_eval` を保存します。

### 局面の改善フラグ（`improving`）

現在の局面の `corrected_eval` が、**2手前（自分の前回の手番）の静的評価値より上がっているか**を表す真偽値が `improving` です（王手されている局面では常に `false`）。

2手前より形勢が良くなっている（`improving == true`）ときは、自分の構想が順調に進んでいる可能性が高いため無謀な縮小を控え、逆に2手前より悪化している（`improving == false`）ときは、見込みの薄い静かな手をより積極的に枝刈り・縮小します。

---

## 2. 手を生成する前のノード単位の調整・枝刈り

`negamax` に入って置換表と静的評価を確認したあと、候補手ループへ進む前に次の処理を順番に試みます。

### IIR（Internal Iterative Reduction）と振り返り深さ調整

- **IIR**：残り深さ `depth >= 2` なのに置換表に `best_move` が登録されていないノードでは、手の並べ替え精度が低くそのまま深く読むとコストが膨らむため、`depth` を `1` 減らして探索します。
- **振り返り深さ調整（Hindsight Adjustment）**：1手前の親ノードで大きな LMR 縮小（`prior_reduction`）が掛かっていた場合、現在の静的評価と親の静的評価の和（＝1手前後での評価の食い違い）を見て深さを微調整します。縮小量が `3` 以上で相手視点の評価が悪化していなければ見落としを防ぐため `depth + 1` し、逆に縮小量が `2` 以上で評価の和が `+162`（`hindsight_margin`）を超えて予想どおり楽観的ならさらに `depth - 1` します。

### Reverse Futility Pruning（RFP / 静的 null move 枝刈り）

非 PV ノードで王手がなく、残り深さ `depth <= 12` のとき、現在の評価値がマージンを引いてもなお `beta` を大きく上回っている場合——すなわち **`eval - 132 * depth >= beta`** なら、「相手がこれから多少反撃してきても `beta` を割り込むことはない」と判断し、手を1手も生成せずに `eval` を返して打ち切ります（置換表が空だった場合は `Lower` として記録します）。

### Null Move Pruning（パスによる上限確認）

ゼロ幅探索（非 PV）で王手がなく、`eval >= beta + 5` を満たす優勢な局面では、**「もしここで自分が1手パス（null move）して相手に2手連続で指させてあげても、まだ `beta` 以上を保てるか？」**を試します。

- パスした子局面を、通常の1手分よりさらに大きく浅くした深さ `depth - R`（`R = 4 + depth / 3 + min((eval - beta) / 296, 3)`）でゼロ幅探索します。
- それでも `score >= beta` になった場合、普通に指せばなおさら有利なはずなので、その場で beta cutoff します。
- ただし将棋にも「パスすると助かるが、何か指すと手が詰まる（ツークツワング）」局面が稀にあるため、`depth >= 16` の深いノードでは、本当にパス抜きでも `beta` を超えるかを手番そのままで浅く**検証探索（verification search）**して確かめます（その際、同じ `ply` での再帰的な null move だけを一時的に禁止します）。

### ProbCut（浅い探索による早期カット）

ゼロ幅探索で `depth >= 4` のとき、「本来の `beta` よりさらに `+194`（`probcut_margin`）も高い厳しい閾値 `probcut_beta` を、浅い探索であっさり超えるような決定的な駒取りがあるか」を先に調べます。

SEE で見込みのある捕獲手だけを取り出し、まず `qsearch` で軽く確認し、通った手だけ `depth - 3` の浅い `negamax` に掛けます。ここで `probcut_beta` 以上が返れば、全深さで読んでもまず間違いなく `beta` を超えるため、置換表へ `depth - 2` の下限を記録して `score - 194` を返します。

### Singular Extension（突出した最善手の1手延長）と Multi-Cut

`depth >= 5`（かつ `ply < 2 * root_depth`）のノードで、置換表に十分な深さ（`entry.depth + 3 >= depth`）の下限または確定スコアを持つ最善手 `tt_move` がある場合、**「この `tt_move` 以外にまともな代替手があるか」**を検査します。

`tt_move` だけを除外（`excluded_move`）した状態で、半分の深さ `(depth - 1) / 2`・少し下げた窓 `singular_beta = tt_score - 2 * depth` で他の手を探します。

- **どの代替手も `singular_beta` に届かなかった場合**：`tt_move` は代えの効かない唯一の手（王手しのぎの急所や寄せの決め手など）であると分かるため、`tt_move` を読むときだけ残り深さを **`+1` 延長**します。
- **代替手でも `singular_beta` 以上になり、かつ非 PV で `singular_beta >= beta` だった場合**：本命の `tt_move` を除いた2番手以下ですらすでに `beta` を超えているため、`tt_move` を読むまでもなくその場で **`singular_beta` を返して打ち切ります（Multi-Cut）**。

---

## 3. 候補手ループ内の枝刈り（2手目以降）

少なくとも1手は普通に探索して基準を作ったあと（`index > 0`）、かつ現在の最善スコアが詰み負け帯にないときに限り、残りの下位候補手を次の基準でスキップします。

| 手法 | 対象となる手 | 深さ条件 | スキップする条件（既定値） | 狙い |
| --- | --- | --- | --- | --- |
| **捕獲 SEE 枝刈り** | 戦術的な手（`!quiet`） | `depth <= 5` | `SEE < -177 * depth^2` | 浅い残り深さでの大損な駒取りを省く |
| **Futility Pruning** | 静かな非王手 | `depth <= 7` | `eval + 299 * depth <= alpha` | 現時点で大きく負けており静かな手では届かない枝を省く |
| **History Pruning** | 静かな非王手 | `depth <= 3` | `history_score < -2148 * depth` | 過去の探索で失敗し続けている静かな手を省く |
| **Late Move Pruning (LMP)** | 静かな非王手（非 PV） | 全深さ（実質浅層） | 探索済み静かな手数が上限以上<br>盤上：`(4 + depth^2) / (2 - improving)`<br>駒打ち：その半分（最低1手） | 並べ替え下位の静かな手を個数上限で打ち切る（種類が多い駒打ちはさらに厳しく絞る） |
| **静かな手の SEE 枝刈り** | 静かな非王手（`!= tt_move`） | `depth <= 3` | `SEE < -177 * depth^2` | タダ取りされるマスへ駒を動かす・打つ手を省く |

いずれの静かな手の枝刈りも、**王手回避中（`in_check`）や、その手自体が王手になる場合（`gives_check`）には適用しません**。

---

## 4. Late Move Reductions（対数 LMR：`src/search/lmr.rs`）

ループ内枝刈りを生き残った手であっても、並べ替えの後ろのほうに出てくる手（Late Move）が1番目の手を逆転する確率は低くなります。そこで `depth >= 3` では、後ろの手ほど残り深さを削ってゼロ幅探索を行い、**予想に反して `alpha` を超えた手だけを本来の深さで読み直す** **LMR（Late Move Reductions）** を行います。

<svg viewBox="0 0 700 175" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="LMRによる縮小探索と必要時のみの再探索フロー">
  <defs>
    <marker id="lmr-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 1 L 9 5 L 0 9 z" fill="#475569"/>
    </marker>
  </defs>
  <rect x="10" y="10" width="680" height="155" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <rect x="24" y="46" width="175" height="72" rx="7" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="111" y="69" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">1. 縮小ゼロ幅探索</text>
  <text x="111" y="88" text-anchor="middle" font-size="11" fill="#334155">深さ depth - 1 - R</text>
  <text x="111" y="105" text-anchor="middle" font-size="11" fill="#475569">窓 (-alpha-1, -alpha)</text>
  <rect x="250" y="24" width="185" height="44" rx="7" fill="#f0fdf4" stroke="#16a34a" stroke-width="1.5"/>
  <text x="342" y="44" text-anchor="middle" font-size="12" font-weight="bold" fill="#14532d">大半の手：score ≦ alpha</text>
  <text x="342" y="60" text-anchor="middle" font-size="11" fill="#334155">そのまま完了（大幅に節約）</text>
  <rect x="250" y="88" width="205" height="60" rx="7" fill="#fffbeb" stroke="#d97706" stroke-width="1.5"/>
  <text x="352" y="110" text-anchor="middle" font-size="12" font-weight="bold" fill="#78350f">2. score &gt; alpha (有望!)</text>
  <text x="352" y="128" text-anchor="middle" font-size="11" fill="#334155">本来の深さ (±1微調整) の</text>
  <text x="352" y="142" text-anchor="middle" font-size="11" fill="#334155">ゼロ幅で再探索して確認</text>
  <rect x="500" y="88" width="174" height="60" rx="7" fill="#fff1f2" stroke="#e11d48" stroke-width="1.5"/>
  <text x="587" y="110" text-anchor="middle" font-size="12" font-weight="bold" fill="#881337">3. PV ノードでさらに突破</text>
  <text x="587" y="128" text-anchor="middle" font-size="11" fill="#334155">全窓 (-beta, -alpha) で</text>
  <text x="587" y="142" text-anchor="middle" font-size="11" fill="#334155">正確なスコアを確定</text>
  <line x1="199" y1="68" x2="248" y2="48" stroke="#16a34a" stroke-width="1.5" marker-end="url(#lmr-arrow)"/>
  <line x1="199" y1="98" x2="248" y2="115" stroke="#d97706" stroke-width="1.5" marker-end="url(#lmr-arrow)"/>
  <line x1="455" y1="118" x2="498" y2="118" stroke="#e11d4a" stroke-width="1.5" marker-end="url(#lmr-arrow)"/>
</svg>

### `1/1024` 段単位の基礎テーブルと補正項

静かな手（非 PV では `index >= 2`、PV では `index >= 3`）の基礎縮小量は、あらかじめ計算した `64 × 1024` の対数テーブル `ln(depth) * ln(index) * 100 / 190`（内部では `1/1024` 段の固定小数点）から引き、局面の性質に応じて細かく加減算します。

- **縮小を強める要因（より浅く読む）**：`!improving`（`+945`）、beta cutoff が期待される `cut_node`（`+1688`）、置換表の最善手が駒取りだったノード（`+549`）、履歴スコアが負の手
- **縮小を弱める要因（丁寧に読む）**：過去に PV 上にあった局面 `tt_pv`（`-943`）、先行する子ノードで反駁が少なかった場合（`-807`）、履歴スコアが高い手（`history_score / 7199` 段、最大 `±2` 段）、評価補正値 `|correction|` が大きい不安定な局面、PV ノードで窓が広い場合

また、非 PV ノードで後回しにされた損な捕獲（`SEE < 0`）に対しても、基礎値から3段引いた `1..=depth - 2` 段の縮小を掛けます。

### 再探索時の深さ微調整

縮小探索の結果が `alpha` を超えたときは、縮小なしの深さ `research_depth = depth - 1` でゼロ幅の再探索を行いますが、その際にも縮小探索でどれだけ余裕を持って超えたかを見ます。最善スコアを `26 + 2 * research_depth` より大きく上回っていれば勢いがあると見て `research_depth` を `+1` 深くし、逆に `+10` 未満の僅差なら `-1` 浅くします。そして再探索でも `alpha` を超えた静かな手には継続履歴へボーナス（`+806`）を与え、PV ノードであれば最後に全窓 `(-beta, -alpha)` で正確な評価値を確定させます。

# 静止探索

主探索（`negamax`）で残り深さ `depth == 0` に到達したとき、その場でそのまま静的評価関数（NNUE）を呼んで終わりにすると、将棋では致命的な読み抜けが起きます。これを防ぎ、**激しい駒の取り合いが一段落した「静かな局面」まで読みを延長する**のが `src/search/qsearch.rs` の **静止探索（Quiescence Search）** です。

---

## 1. 水平線効果（Horizon Effect）とは

たとえば残り深さ1の局面で、自分の飛車（1000点）で相手の歩（100点）を取る手 `▲２四飛` があったとします。もし次の手番（残り深さ0）で機械的に探索を打ち切って静的評価を返してしまうと、その直後に相手の角で `△２四同角` と飛車を取り返される手が見えません。エンジンは「歩をただで1枚得した！」と勘違いして飛車をタダ捨てしてしまいます。

<svg viewBox="0 0 700 185" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="深さ0で打ち切ると起きる水平線効果と静止探索の役割">
  <defs>
    <marker id="qs-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 1 L 9 5 L 0 9 z" fill="#475569"/>
    </marker>
  </defs>
  <rect x="10" y="10" width="680" height="165" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <rect x="26" y="56" width="150" height="66" rx="7" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="101" y="79" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">depth = 1 (自分)</text>
  <text x="101" y="99" text-anchor="middle" font-size="11" fill="#334155">▲２四飛 (歩を取る)</text>
  <line x1="212" y1="24" x2="212" y2="160" stroke="#ef4444" stroke-width="2" stroke-dasharray="5 4"/>
  <text x="212" y="40" text-anchor="middle" font-size="11" font-weight="bold" fill="#b91c1c">主探索の水平線 (depth = 0)</text>
  <rect x="252" y="56" width="192" height="66" rx="7" fill="#fff1f2" stroke="#e11d48" stroke-width="1.5"/>
  <text x="348" y="77" text-anchor="middle" font-size="12" font-weight="bold" fill="#881337">ここで打ち切ると誤認</text>
  <text x="348" y="96" text-anchor="middle" font-size="11" fill="#334155">「歩得 (+100)」と評価してしまう</text>
  <text x="348" y="112" text-anchor="middle" font-size="10" fill="#be123c">(飛車が当たりになっている現実が見えない)</text>
  <rect x="484" y="42" width="188" height="94" rx="7" fill="#f0fdf4" stroke="#16a34a" stroke-width="1.5"/>
  <text x="578" y="64" text-anchor="middle" font-size="12" font-weight="bold" fill="#14532d">静止探索 (qsearch) で延長</text>
  <text x="578" y="84" text-anchor="middle" font-size="11" fill="#334155">△２四同角 (飛車を取り返す)</text>
  <text x="578" y="103" text-anchor="middle" font-size="11" fill="#15803d" font-weight="bold">→ 実際は大損 (-900) と判明</text>
  <text x="578" y="122" text-anchor="middle" font-size="10" fill="#475569">取り合いが収まるまで読む</text>
  <line x1="176" y1="89" x2="250" y2="89" stroke="#475569" stroke-width="1.5" marker-end="url(#qs-arrow)"/>
  <line x1="444" y1="89" x2="482" y2="89" stroke="#475569" stroke-width="1.5" marker-end="url(#qs-arrow)"/>
</svg>

このように探索深さの境界（「水平線」）のすぐ向こうにある直後の反撃が見えなくなる現象を **水平線効果** と呼びます。これを防ぐため、`negamax` は `depth == 0` になった時点で静的評価を直接返すのではなく、`qsearch_with_key` へバトンを渡します。

---

## 2. `qsearch` ノードの処理フロー

静止探索ではすべての合法手を読むわけではありません（全部読むと主探索と同じく爆発してしまいます）。戦術的に盤面の損得が大きく動く手だけに絞り、次の順序で処理します。

### ステップ1：終局判定と置換表（TT）参照

まず千日手・手数上限（`terminal_score`）および入玉宣言勝ち（`declaration_score`）を判定し、続いて置換表を引きます。

- 静止探索では PV / 非 PV を問わず、置換表に十分な境界（`Exact`、または `score >= beta` の `Lower`、`score <= alpha` の `Upper`）があればその場で値を返します。主探索で深く読まれたエントリほど信頼できるため、深さによる制限は設けていません。
- 打ち切れなかった場合でも、置換表エントリに生の静的評価値（`static_eval`）が保存されていればそれを取り出し、直後の NNUE 推論を省略します。

### ステップ2：Stand Pat（「何もしない選択肢」による下限設定）

**王手がかかっていない局面（`!in_check`）**では、無理に損な駒取りをする義務はありません。静かな手（普通の駒組みなど）を指せば、少なくとも現在の静的評価値くらいの形勢は保てるという仮定を置けます。これを **stand pat** と呼びます。

1. 静的評価値 `raw` を計算（または TT から再利用）し、後述する補正履歴（`corrected_eval`）を加えた値を `stand_pat` とします。
2. **`stand_pat >= beta`** なら、何も取り返さなくてもすでに相手の上限を超えているため、手の生成も1手詰め判定も行わずに即座に cutoff します。
3. そうでなければ **`alpha = alpha.max(stand_pat)`** として下限を引き上げます。これによって、「無理に駒を取ってかえって損をする変化」を選ばずに済みます。

> **王手されているとき（`in_check == true`）**：王手を放置することはルール上できないため、stand pat は使えません。

### ステップ3：1手詰めルーチン（`solve_mate_in_one`）

王手がかかっておらず、かつ stand pat で打ち切れなかった局面では、`rsshogi::mate::solve_mate_in_one` を呼んで即詰み（1手詰め）がないかを高速に検査します。見つかれば `MATE - ply - 1` の確定スコア（`Bound::Exact`）を置換表へ記録して即座に返します。将棋は持ち駒による短手数の即詰みが非常に多いため、末端での読みの鋭さに大きく効きます。

### ステップ4：候補手の生成と深さ上限

局面の状態によって生成する手の種類を切り替えます。

- **王手されていない通常局面**：`rsshogi` の `CapturePlusPro` モードで、**「駒を取る手」と「歩が成る手（と金作り）」だけ**を直接スタック上に生成します（静かな手はそもそも生成しません）。
- **王手されている局面**：王手を外す必要があるため、静かな玉の移動や合駒も含む**すべての合法回避手（`Legal`）**を生成します。合法手が0手なら詰み（`-MATE + ply`）です。

また、王手と駒取りの応酬が延々と続いて静止探索自体が肥大化するのを防ぐため、静止探索に入ってからの段数 `qply` が **`MAX_QPLY = 12`** に達した時点（ただし王手放置にならない `!in_check` のとき）で `alpha` を返して打ち切ります。

---

## 3. 静止探索での枝刈りと置換表への保存

生成した候補手を `MovePicker` で有利な順に並べたあと、明らかに見込みのない手は子ノードへ降りる前に2種類の基準で省きます。ただし、**王手回避中（`in_check`）またはその手が王手になる場合（`gives_check`）は戦術的な変化を見落とさないよう枝刈りを行いません**。

1. **SEE 枝刈り（損な駒取りの除外）**
   そのマスでの駒の取り合いを計算した静的交換評価（[SEE](ordering.md)）が `-1` 未満（`qsearch_see_margin = 1`、つまり歩で飛車を取るような得な手ではなく、角で守られた歩を取るような明確な駒損の手）であればスキップします。
2. **デルタ枝刈り（Delta Pruning）**
   その手で直ちに得られる最大駒得 `gain`（取った駒の価値 ＋ 取った駒を持ち駒として使う価値 ＋ 成りボーナス）に余裕幅 `qsearch_delta_margin = 196` を足しても、なお `stand_pat + gain + 196 < alpha` にしかならない手は、読んでも `alpha` に届く見込みが薄いためスキップします。

最後に探索結果を `depth = 0` で置換表へ保存しますが、ここには大切なルールがあります。**SEE 枝刈りやデルタ枝刈りで1手でも候補手を飛ばした（`pruned_move == true`）まま beta cutoff に至らなかった場合、その結果は置換表へ保存しません**。候補の一部を省略した不完全な探索結果を `Upper` や `Exact` としてキャッシュしてしまうと、後で別の窓から参照された際に誤ったカットを引き起こすためです。

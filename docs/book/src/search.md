# 探索

評価関数は、与えられた局面に点数を付けるだけである。
その点数を指し手の選択へ変えるには、候補手の先を読み、相手の最善の応手まで比べなければならない。
限られた時間で深く読むため、このエンジンは反復深化を骨格に、PVS、置換表、枝刈り、静止探索、Lazy SMPを組み合わせる。

## 相手も最善手を選ぶ

候補手は、その直後の評価値だけでは比べられない。
相手が最も厳しい応手を返したあとの点数で比べる必要がある。

| 自分の候補手 | 相手の応手ごとの評価 | 相手が選ぶ評価 |
| --- | --- | ---: |
| A | +120、−80 | −80 |
| B | +30、+10 | +10 |

自分にとってはAに魅力的な変化があっても、相手は−80になる応手を選ぶ。
したがって、自分は最悪の応手を受けても+10が残るBを選ぶ。
このように、自分は最大値、相手は最小値を選ぶ考え方が**minimax**である。

実装では、手番が変わるたびに点数の符号を反転する**negamax**へ書き換える。
`score(position) = max(-score(child))`と表せるため、最大化と最小化を一つの再帰で扱える。
以降の探索では、点数は常に手番側から見た値である。

## goから bestmoveまで

`go`を受けた探索は、深さを1手ずつ増やしながら同じ局面を読み直す。

<svg viewBox="0 0 740 330" xmlns="http://www.w3.org/2000/svg" style="max-width: 740px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-go" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <g fill="none" stroke="currentColor" stroke-width="1.3">
    <rect x="20" y="20" width="180" height="48" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="280" y="20" width="200" height="48" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="280" y="105" width="200" height="48" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="280" y="190" width="200" height="48" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="560" y="105" width="160" height="48" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="280" y="270" width="200" height="44" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="20" y="105" width="180" height="48" rx="6" fill="currentColor" fill-opacity="0.05"/>
  </g>
  <g stroke="currentColor" stroke-width="1.3" fill="none" marker-end="url(#arrow-go)">
    <line x1="200" y1="44" x2="278" y2="44"/>
    <line x1="380" y1="68" x2="380" y2="103"/>
    <line x1="380" y1="153" x2="380" y2="188"/>
    <line x1="480" y1="129" x2="558" y2="129"/>
    <path d="M 480 214 C 540 214 540 44 484 44" />
    <line x1="380" y1="238" x2="380" y2="268"/>
    <line x1="278" y1="129" x2="202" y2="129"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="110" y="41" text-anchor="middle">局面と評価器のsnapshot</text>
    <text x="110" y="59" text-anchor="middle" font-size="11" opacity="0.75">worker起動、fallback確保</text>
    <text x="380" y="41" text-anchor="middle">反復深化 depth = 1, 2, …</text>
    <text x="380" y="59" text-anchor="middle" font-size="11" opacity="0.75">aspiration窓で開始</text>
    <text x="380" y="126" text-anchor="middle">root探索（PVS）</text>
    <text x="380" y="144" text-anchor="middle" font-size="11" opacity="0.75">negamax → 静止探索</text>
    <text x="380" y="211" text-anchor="middle">iteration完了</text>
    <text x="380" y="229" text-anchor="middle" font-size="11" opacity="0.75">info送信、次のdepthへ</text>
    <text x="640" y="126" text-anchor="middle">共有置換表</text>
    <text x="640" y="144" text-anchor="middle" font-size="11" opacity="0.75">全workerで共有</text>
    <text x="110" y="126" text-anchor="middle">枝刈りとordering</text>
    <text x="110" y="144" text-anchor="middle" font-size="11" opacity="0.75">各nodeで適用</text>
    <text x="380" y="288" text-anchor="middle">制限到達で停止</text>
    <text x="380" y="306" text-anchor="middle" font-size="11" opacity="0.75">bestmoveとponder候補を公開</text>
  </g>
</svg>

**反復深化**は、depth 1、depth 2、depth 3と探索を繰り返す方式である。
浅い探索は捨て仕事ではない。
そこで見つけた最善手と置換表が次の探索の並び順を整え、alpha-beta法の枝刈りを早く効かせる。
何より、時間が尽きた瞬間にその時点の最善手を返せる。

各iterationの中身は章を分けて追う。

- [alpha-beta法とPVS](search/alphabeta.md)：読まなくてよい枝を証明付きで切る探索の骨格
- [手の並べ替えとSEE](search/ordering.md)：有望な手を先に読み、取り合いの損得を探索せずに見積もる方法
- [枝刈りと探索深さの調整](search/pruning.md)：評価や手の順序を手掛かりに、読む手と深さを減らす
- [静止探索](search/qsearch.md)：深さ0に達した局面を、取り合いが落ち着くまで読み延ばす
- [置換表と千日手](search/tt.md)：合流した局面の結果を共有する仕組みと、千日手が壊すその前提
- [時間管理](search/time.md)：持ち時間から予算を作り、二段の締切で使い切る
- [Lazy SMPによる並列探索](search/parallel.md)：複数のワーカーで同じルート局面を読む並列化

## 探索で使う点数

探索が扱う点数は一つの`i32`だが、通常評価と詰みを区別するため、用途ごとに範囲を分けている。

| 帯 | 値 | 意味 |
| --- | ---: | --- |
| 番兵 | ±32,767 | 探索窓の初期値。実際のscoreとしては現れない |
| 詰み | ±(32,000 − 手数) | ルートからの手数を含む終局スコア。絶対値31,754以上が詰み帯で、宣言勝ちや連続王手の勝敗にも使う |
| 優等／劣等局面 | ±31,753 | 同じ盤面と手番の過去局面に比べ、持ち駒の包含関係で有利または不利と判定した場合の探索用スコア |
| 通常評価 | −31,753〜+31,753 | NNUEまたはmaterial評価のclamp後の値。端点は優等／劣等局面と共有する |

詰みscoreは「rootからの手数」を運ぶため、置換表に入れるときは保存局面からの手数へ、取り出すときは現在の手数へ補正する。
この補正を怠ると、同じ局面を違う深さから参照したときに詰みまでの距離が崩れる。
通常評価を31,753でclampすると、詰み帯とのあいだに境界を保てる（[評価関数](nnue.md)を参照）。

深さの上限は、`go depth`で指定できる明示深さが64、静止探索まで含めた構造上の上限がrootから128 plyである。
千日手と手数上限は宣言勝ちより先に判定する。
探索中の通常千日手、連続王手、優等／劣等局面は、その探索枝を打ち切るスコアとして扱う。
優等／劣等局面は実対局の終局規則ではなく、ルートでは探索を続ける。

## depth 1を完了できない場合

nodeや時間の制限が厳しく、depth 1すら完了しない場合がある。
それでも`bestmove`は返す必要があり、公開する評価値もその手と対応していなければならない。

このためにメインワーカーは最初の1ノードを予約し、ルートの最初の子局面を必ず評価して深さ0の代替結果を作る。
予算が残っていれば残りのroot moveも順に評価し、評価済み候補の中の最良手を使う。
返す手と評価値は必ず同じ候補に由来し、「手はAだが点数はBのもの」という組み合わせは作らない。

## 読み筋の公開

iterationごとに、置換表を最善手から最大32手まで辿って読み筋を組み立て、`info ... pv`として公開する。
ponder候補はこの読み筋の2手目である。
置換表は上書きされるので、これは探索が読んだ手順の再構成であって保証ではない。
各手の合法性を確認し、非合法手、終局、長さ上限のいずれかで打ち切る。

途中で打ち切られたiterationでも、そこまでにalphaを更新できた手があれば最善手として採用する。
その探索窓の下側の境界を上回る結果が得られたためである。
前の反復の最善手との、同じ深さでの比較を完了したという意味ではない。
ただし公開する`depth`は完了したiterationのものを保ち、打ち切られた途中経過を`info`として出すことはない。

## 参考資料

- [Chess Programming Wiki: Minimax](https://www.chessprogramming.org/Minimax)
- [Chess Programming Wiki: Negamax](https://www.chessprogramming.org/Negamax)
- [Chess Programming Wiki: Iterative Deepening](https://www.chessprogramming.org/Iterative_Deepening)
- [Claude E. Shannon, “Programming a Computer for Playing Chess”](https://doi.org/10.1080/14786445008521796)

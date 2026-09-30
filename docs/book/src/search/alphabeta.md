# alpha-beta法とPVS

仮に各局面に80手の候補があると、深さ6の全探索は80^6、およそ2.6×10^11局面になる。
すべての手を同じ深さまで読む方法は現実的ではない。
探索を深くするには、最善手の判定に不要な枝を早く見つける必要がある。

## negamax

**negamax**は、「相手にとって良い点数は自分にとって悪い点数」として、最大化と最小化を一つの処理にまとめる方法である。
各ノードは子局面を`-negamax(child, -beta, -alpha)`で評価し、常に手番側から見た点数だけを扱う。
以下の呼び出し表記では、深さなどの引数を省略している。

## alpha-betaの窓

negamaxの各呼び出しは、**探索窓**`(alpha, beta)`を受け取る。
alphaは「これを上回る手を探すための下側の境界」、betaは「この値以上と分かれば探索を打ち切れる上側の境界」である。
祖先から受け継いだ窓や予測で狭めた窓も使うため、alphaが常にこの局面ですでに確保した点数とは限らない。
子の評価がこの窓のどこに落ちたかで、続きを読むかどうかが決まる。

<svg viewBox="0 0 720 190" xmlns="http://www.w3.org/2000/svg" style="max-width: 720px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-ab" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <g stroke="currentColor" stroke-width="1.5" fill="none">
    <line x1="40" y1="90" x2="690" y2="90" marker-end="url(#arrow-ab)"/>
    <line x1="230" y1="78" x2="230" y2="102"/>
    <line x1="500" y1="78" x2="500" y2="102"/>
  </g>
  <rect x="230" y="82" width="270" height="16" fill="currentColor" fill-opacity="0.12" stroke="none"/>
  <g fill="currentColor" font-size="13">
    <text x="230" y="70" text-anchor="middle">alpha</text>
    <text x="500" y="70" text-anchor="middle">beta</text>
    <text x="700" y="112" text-anchor="end" font-size="11" opacity="0.75">score →</text>
    <text x="130" y="130" text-anchor="middle">fail low</text>
    <text x="130" y="148" text-anchor="middle" font-size="11" opacity="0.75">この手は現状を超えない</text>
    <text x="130" y="164" text-anchor="middle" font-size="11" opacity="0.75">次の手へ</text>
    <text x="365" y="130" text-anchor="middle">窓の中</text>
    <text x="365" y="148" text-anchor="middle" font-size="11" opacity="0.75">新しい最善手</text>
    <text x="365" y="164" text-anchor="middle" font-size="11" opacity="0.75">alphaを引き上げる</text>
    <text x="600" y="130" text-anchor="middle">fail high（beta cutoff）</text>
    <text x="600" y="148" text-anchor="middle" font-size="11" opacity="0.75">相手はこの局面を選ばない</text>
    <text x="600" y="164" text-anchor="middle" font-size="11" opacity="0.75">残りの手を読まずに打ち切る</text>
  </g>
</svg>

ある手の点数がbeta以上になると、相手は一つ前の分岐でこの局面を選ばない。
この局面がbetaよりどれだけ良いかは、現在の最善手を決める材料にならない。
残りの手を読まずに「beta以上」という下界だけを返して打ち切れる。
これが**beta cutoff**で、alpha-betaの削減はすべてここから生まれる。

削減の量は手の並び順に強く依存する。
有望な手を先に読むほどalphaとbetaの境界が早く狭まり、後続の枝でcutoffが起きやすくなる。
最悪の順で読めば、cutoffは一度も起きず全探索と同じになる。
理想的な並び順なら訪問node数は全探索の平方根近くまで落ちるため、手の順序は探索速度を大きく左右する（[手の並べ替えとSEE](ordering.md)）。

## PVS：二番目以降の手はまず疑う

並び順が良いという前提を信じるなら、もう一歩進める。
最初の手が最善で、二番目以降の手はどうせalphaを超えない、と仮定してしまうのである。

**Principal Variation Search（PVS）**は、最初の手だけを通常の窓`(alpha, beta)`で読み、二番目以降の手を最小幅の窓`(alpha, alpha+1)`で読む。
この**zero-window探索**は「alphaを超えるか超えないか」だけを判定する探索で、窓が狭いぶんcutoffが頻発し、通常の窓よりずっと安い。

仮定が当たっている限り、zero-window探索は「超えない」を安く確認して終わる。
仮定が外れてalphaを超えたときだけ、その手を通常の窓で読み直して正確な点数を得る。
読み直しは二重の費用だが、並び順が整っていれば発生はまれで、大多数の手が安い確認だけで済む利得のほうが大きい。
zero-window探索の点数がbeta以上なら読み直しもせず、そのままcutoffになる。

## aspiration window：rootの窓も狭める

反復深化には、もう一つ窓を狭める材料がある。
前のiterationのscoreである。
depth 7のscoreがdepth 8で大きく動くことは少ないので、rootの窓を最初から`前回のscore ± aspiration_window`に絞って探索を始める（既定値は83）。

予測が当たれば、窓の外に出る枝が早く刈れてiteration全体が安くなる。
外れてscoreが窓の外に落ちたときは、外れた側だけを「外れた値から幅の2倍」の位置まで広げて、そのiterationを読み直す。
反対側は動かさず、外すたびに幅を倍にするので、何度外しても数回で全幅`(-INF, INF)`に届く。
この読み直しは完了したiterationに対してだけ行い、打ち切られた途中結果には適用しない。

## iterationをまたぐ並べ替え

各iterationの開始時には、前のiterationの最善手をroot movesの先頭へ動かす。
rootの置換表entryに最善手が残っていれば、それも先頭へ引き上げる。
PVSもaspirationも「最初の手がおそらく最善」という前提に費用を賭ける方式なので、この並べ替えが前提の当たり率を直接支えている。

## 参考資料

- [Chess Programming Wiki: Alpha-Beta](https://www.chessprogramming.org/Alpha-Beta)
- [Chess Programming Wiki: Principal Variation Search](https://www.chessprogramming.org/Principal_Variation_Search)
- [Chess Programming Wiki: Aspiration Windows](https://www.chessprogramming.org/Aspiration_Windows)
- [Donald E. Knuth and Ronald W. Moore, “An Analysis of Alpha-Beta Pruning”](https://www.sciencedirect.com/science/article/pii/0004370275900193)

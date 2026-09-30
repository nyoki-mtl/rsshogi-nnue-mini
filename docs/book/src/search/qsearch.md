# 静止探索

depth 0に達した局面をそのまま評価すると、奇妙なことが起きる。
飛車が銀を取った直後の局面なら、評価は銀得を計上する。
次の一手で飛車が取り返されることは、深さの壁の向こうにあって見えない。
取り合いの途中で打ち切ると、一時的な駒得を最終的な利益と誤認することがある。
これも、探索範囲の外にある不利な変化を見落とす**水平線効果**の一例である。

**静止探索**（qsearch）は、depth 0に達した局面を「取り合いが落ち着くまで」だけ読み延ばす。
全部の手ではなく応酬の続きだけを読むので、深さの壁を、評価が信用できる静かな局面まで押し出せる。

<svg viewBox="0 0 740 250" xmlns="http://www.w3.org/2000/svg" style="max-width: 740px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-qsearch" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <g fill="none" stroke="currentColor" stroke-width="1.3">
    <rect x="20" y="42" width="160" height="50" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="290" y="42" width="160" height="50" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="560" y="42" width="160" height="50" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="290" y="170" width="160" height="50" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <path d="M 180 67 L 288 67" marker-end="url(#arrow-qsearch)"/>
    <path d="M 450 67 L 558 67" marker-end="url(#arrow-qsearch)"/>
    <path d="M 370 94 L 370 168" marker-end="url(#arrow-qsearch)"/>
    <line x1="492" y1="26" x2="492" y2="112" stroke-dasharray="5 4"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="100" y="63" text-anchor="middle">取り合いの前</text>
    <text x="100" y="81" text-anchor="middle" font-size="11" opacity="0.75">評価はほぼ互角</text>
    <text x="370" y="63" text-anchor="middle">飛車が銀を取る</text>
    <text x="370" y="81" text-anchor="middle" font-size="11" opacity="0.75">一時的に駒得</text>
    <text x="640" y="63" text-anchor="middle">固定深さの評価</text>
    <text x="640" y="81" text-anchor="middle" font-size="11" opacity="0.75">銀得と誤認</text>
    <text x="492" y="20" text-anchor="middle" font-size="11">深さの壁</text>
    <text x="370" y="191" text-anchor="middle">歩が飛車を取り返す</text>
    <text x="370" y="209" text-anchor="middle" font-size="11" opacity="0.75">qsearchはここまで読む</text>
    <text x="470" y="238" text-anchor="middle" font-size="11" opacity="0.75">応酬後の局面を評価する</text>
  </g>
</svg>

## stand pat：指さない自由

静止探索の各nodeは、まず現局面の静的評価を求める。
この値を**stand pat**と呼び、そのノードの下界の見積もりとして使う。
取り合いを避けて静かな手を指せる、という仮定に基づく。
実際にパスできるわけではなく、どの着手でも形勢が悪化する局面では、この仮定が外れる。
静的評価には、主探索と同じcorrection historyの補正を加える。

stand patがbeta以上なら、その場でcutoffする。
alphaを超えていればalphaを引き上げ、それから捕獲の候補を読む。
王手されている局面は例外で、指さない自由がないためstand patを取らず、王手回避の手をすべて読む。

## 候補は捕獲と歩の成りだけ

王手されていないnodeで読むのは、捕獲と歩の成りだけである。
全合法手を生成してから絞るのではなく、この2種類だけを直接生成してから合法性を確かめる。
捕獲でも歩の成りでもない成り（銀成りなど）は、この時点で候補から落ちる。
歩・角・飛が成れる手は成りだけを生成し、不成は読まない（[段階ごとの生成](ordering.md#段階ごとの生成)を参照）。

王手されていないnodeでは、1手詰めの検出も行う。
詰みがあればその点数を確定値として返し、置換表にも保存する。

読み延ばしにも上限を置き、静止探索の入れ子は12 plyで打ち切ってその時点の値を返す。
王手されているnodeはこの上限では打ち切らず、回避を読み切る。

## 静止探索の中の枝刈り

捕獲と歩の成りに絞った候補も、駒得の見積もりを使ってさらに減らす。

**delta pruning**は、直後の駒得を見積もり、`stand pat + 駒得 + 124 < alpha`となる手を読まない。
駒得には、取った盤上の駒の消失、成り駒を生駒として得る持ち駒、移動駒の成りによる増分をすべて数える。
これは駒得に基づく近似であり、NNUEの評価変動の上限を保証するものではない。

**SEEによる枝刈り**は、[SEE](ordering.md)が−1未満の捕獲を読まない。
取り返される捕獲は応酬を続ける理由にならないからである。

どちらも、王手を掛ける手には適用しない。
王手されているnodeの回避手も削らない。

## 置換表へ保存する範囲

置換表を使うのは、主探索から静止探索へ入る入口（`qply == 0`）に限る。
参照時には深さを制限せず、主探索の深いエントリも、境界の種類に応じて打ち切りやalphaの引き上げに使う。
生の静的評価が保存されていれば、それも再利用する。
深い主探索の値と静止探索だけの値は、常に同じになるとは限らない。
ここでは深い探索の結果を利用する方針を採っている。

保存するエントリは深さ0とするため、同じキーの深い主探索エントリは上書きしない。
入れ子の静止探索は残りの探索手数が入口と異なるため、置換表を参照も保存もしない。

枝刈りしたノードでは、beta cutoffで得た`Lower`だけを保存する。
cutoffしなかった選択的な結果を`Upper`で保存する主探索とは、この条件が異なる。

## 参考資料

- [Chess Programming Wiki: Quiescence Search](https://www.chessprogramming.org/Quiescence_Search)
- [Chess Programming Wiki: Delta Pruning](https://www.chessprogramming.org/Delta_Pruning)
- [Chess Programming Wiki: Static Exchange Evaluation](https://www.chessprogramming.org/Static_Exchange_Evaluation)

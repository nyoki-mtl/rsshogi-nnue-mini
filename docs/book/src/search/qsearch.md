# 静止探索

depth 0に達した局面をそのまま評価すると、奇妙なことが起きる。
飛車が銀を取った直後の局面なら、評価は銀得を計上する。
次の一手で飛車が取り返されることは、深さの壁の向こうにあって見えない。
どこで読みを打ち切っても、打ち切った位置の直前の手だけが利益を計上し、応酬の残りが消える。
この**水平線効果**を放置すると、探索は取り合いの途中の局面を系統的に誤評価する。

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
この値を**stand pat**と呼び、そのnodeの点数の下界として使う。
取り合いを続けるかどうかは手番側の選択であり、どの捕獲も損なら応酬に乗らなければよいからである。

stand patがbeta以上なら、その場でcutoffする。
alphaを超えていればalphaを引き上げ、それから捕獲の候補を読む。
王手されている局面は例外で、指さない自由がないためstand patを取らず、全合法手（王手回避）を読む。

## 候補は捕獲と歩の成りだけ

王手されていないnodeで読むのは、捕獲と歩の成りだけである。
全合法手を生成してから絞るのではなく、この2種類だけを直接生成してから合法性を確かめる。
捕獲でも歩の成りでもない成り（銀成りなど）は、この時点で候補から落ちる。

王手されていないnodeでは、1手詰めの検出も行う。
詰みがあればその点数を確定値として返し、置換表にも保存する。

読み延ばしにも上限を置き、静止探索の入れ子は12 plyで打ち切ってその時点の値を返す。
王手されているnodeはこの上限では打ち切らず、回避を読み切る。

## 静止探索の中の枝刈り

候補が捕獲だけでも、まだ削れる。

**delta pruning**は、この捕獲が最大限うまくいったときの駒得を数え、`stand pat + 駒得 + 126`がalphaに届かない手を読まない。
駒得には、取った盤上の駒の消失、成り駒を生駒として得る持ち駒、移動駒の成りによる増分をすべて数える。
最大限に見積もってなお届かないなら、読んでも届かない。

**SEEによる枝刈り**は、[SEE](ordering.md)が負の捕獲を読まない。
取り返される捕獲は応酬を続ける理由にならないからである。

どちらも、王手を掛ける手には適用しない。
王手されているnodeの回避手も削らない。

## 置換表へ保存する範囲

静止探索も置換表を使うが、main searchと同じentryを混ぜると別の問題を持ち込む。
静止探索の点数は「捕獲だけを読んだ値」であり、全合法手を読んだmain searchの値とは探索の定義域が違う。

そこで、置換表を使うのはmain searchから静止探索へ入る入口のnodeに限り、entryはdepth 0として保存する。
probeもdepth 0のentryだけを対象にするので、main searchの値が静止探索へ、静止探索の値がmain searchへ、意図せず流れることはない。
入れ子の再帰的な静止探索は、残りhorizonが入口と異なるため置換表を使わない。
また、[置換表](tt.md)の同一key置換はdepthの浅いentryを退けるので、浅い静止探索entryが同じ局面の深いmain search entryを上書きすることもない。

枝刈りが発生したnodeでは、beta cutoffで得た下界だけを保存する。
この条件は[枝刈り](pruning.md)のmain search側と同じである。

## 参考資料

- [Chess Programming Wiki: Quiescence Search](https://www.chessprogramming.org/Quiescence_Search)
- [Chess Programming Wiki: Delta Pruning](https://www.chessprogramming.org/Delta_Pruning)
- [Chess Programming Wiki: Static Exchange Evaluation](https://www.chessprogramming.org/Static_Exchange_Evaluation)

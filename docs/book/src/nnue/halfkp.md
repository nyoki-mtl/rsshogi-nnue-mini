# HalfKP特徴量とネットワーク構造

局面をネットワークへ入れるには、まず数値の列にしなければならない。
81マス×駒種を素朴に並べる方法もあるが、standard NNUEはもう一段情報の濃い表現を選んだ。
「自玉がどこにいるか」を、すべての特徴量の前提に織り込むのである。

## HalfKPの考え方

**HalfKP**は「Half King-Piece」の略で、玉と他の駒1枚のペアを1個の特徴量とする表現である。
「自玉が5九にいて、味方の歩が2六にいる」がひとつの次元、「自玉が5八にいて、同じ歩が2六にいる」は別の次元になる。
同じ駒配置でも、玉の位置が違えば全く別の入力として扱われる。
玉の位置は将棋の形勢判断を最も強く条件づける情報なので、これをペアの片割れとして全特徴量に含める。

「Half」は、この表現を先手視点と後手視点で別々に作ることを指す。
それぞれの視点は自玉だけを基準にし、盤は自分から見た向きへ揃える（後手視点では盤を180度回して読む）。
一つの局面から、視点ごとに1本ずつ、計2本の特徴量集合ができる。

各視点の特徴量は、玉以外の駒1枚につき、盤上か持ち駒かを問わず1個立つ。
駒は全部で40枚、うち玉が2枚なので、平手の対局では視点あたりちょうど38個が立つ。
125,388次元のうち38個だけが1で残りが0という、極端に疎な入力である。
この疎さが、後述する差分更新と全結合層の軽さの両方を支えている。

## indexの構成

特徴量のindexは三つの部品の和で決まる。

```text
index = 自玉のマス × 1548 + 駒の種類の基点 + マスまたは枚数
```

玉の位置ごとに1,548個の区画があり、81マス分で `81 × 1548 = 125,388` 次元になる。
区画の内側は、持ち駒の領域（0〜89）と盤上の駒の領域（90〜1,547）に分かれる。

盤上の駒の基点は駒種と敵味方で決まり、そこへ駒のいるマスの番号（0〜80）を足す。

| 盤上の駒 | 味方の基点 | 相手の基点 |
| --- | ---: | ---: |
| 歩 | 90 | 171 |
| 香 | 252 | 333 |
| 桂 | 414 | 495 |
| 銀 | 576 | 657 |
| 金、と、成香、成桂、成銀 | 738 | 819 |
| 角 | 900 | 981 |
| 馬 | 1,062 | 1,143 |
| 飛 | 1,224 | 1,305 |
| 竜 | 1,386 | 1,467 |

金と成小駒が同じ基点を共有している点は覚えておく価値がある。
動きが同じ駒は同じ特徴量として扱われ、ネットワークからは区別できない。

持ち駒の基点には枚数（0始まり）を足す。
同じ歩でも1枚目と2枚目は別の特徴量なので、「歩を何枚持っているか」がそのまま入力に現れる。

| 持ち駒 | 味方の基点 | 相手の基点 |
| --- | ---: | ---: |
| 歩 | 1 | 20 |
| 香 | 39 | 44 |
| 桂 | 49 | 54 |
| 銀 | 59 | 64 |
| 金 | 69 | 74 |
| 角 | 79 | 82 |
| 飛 | 85 | 88 |

offset 0は「駒がない」を表す予約slotである。
駒落ちなどで立つ特徴量が38個に満たないとき、残りをこのslotで埋めて長さを38に揃える。
平手の対局では埋めは発生しない。

## ネットワーク構造

立った特徴量の列は、次の形のネットワークを通って1個の評価値になる。

<svg viewBox="0 0 760 400" xmlns="http://www.w3.org/2000/svg" style="max-width: 760px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-net" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <g fill="none" stroke="currentColor" stroke-width="1.3">
    <rect x="20" y="40" width="150" height="52" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="20" y="150" width="150" height="52" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="230" y="40" width="170" height="52" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="230" y="150" width="170" height="52" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="458" y="95" width="164" height="52" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="230" y="255" width="130" height="46" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="230" y="330" width="130" height="46" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="430" y="255" width="130" height="46" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="430" y="330" width="200" height="46" rx="6" fill="currentColor" fill-opacity="0.05"/>
  </g>
  <g stroke="currentColor" stroke-width="1.3" fill="none" marker-end="url(#arrow-net)">
    <line x1="170" y1="66" x2="228" y2="66"/>
    <line x1="170" y1="176" x2="228" y2="176"/>
    <line x1="400" y1="66" x2="468" y2="108"/>
    <line x1="400" y1="176" x2="468" y2="134"/>
    <line x1="540" y1="147" x2="360" y2="253" />
    <line x1="295" y1="301" x2="295" y2="328"/>
    <line x1="360" y1="353" x2="428" y2="278"/>
    <line x1="495" y1="301" x2="495" y2="328"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="95" y="61" text-anchor="middle">先手視点の特徴量</text>
    <text x="95" y="80" text-anchor="middle" font-size="11" opacity="0.75">125,388次元、38個が1</text>
    <text x="95" y="171" text-anchor="middle">後手視点の特徴量</text>
    <text x="95" y="190" text-anchor="middle" font-size="11" opacity="0.75">125,388次元、38個が1</text>
    <text x="315" y="61" text-anchor="middle">FeatureTransformer</text>
    <text x="315" y="80" text-anchor="middle" font-size="11" opacity="0.75">125,388 → 256（重み共有）</text>
    <text x="315" y="171" text-anchor="middle">FeatureTransformer</text>
    <text x="315" y="190" text-anchor="middle" font-size="11" opacity="0.75">125,388 → 256（重み共有）</text>
    <text x="540" y="116" text-anchor="middle">手番側を先頭に連結</text>
    <text x="540" y="135" text-anchor="middle" font-size="11" opacity="0.75">256×2 = 512、ClippedReLU</text>
    <text x="295" y="274" text-anchor="middle">隠れ層1</text>
    <text x="295" y="292" text-anchor="middle" font-size="11" opacity="0.75">512 → 32、ClippedReLU</text>
    <text x="295" y="349" text-anchor="middle">隠れ層2</text>
    <text x="295" y="367" text-anchor="middle" font-size="11" opacity="0.75">32 → 32、ClippedReLU</text>
    <text x="495" y="274" text-anchor="middle">出力層</text>
    <text x="495" y="292" text-anchor="middle" font-size="11" opacity="0.75">32 → 1</text>
    <text x="530" y="349" text-anchor="middle">÷ FV_SCALE → 評価値</text>
    <text x="530" y="367" text-anchor="middle" font-size="11" opacity="0.75">clamp ±31,753</text>
  </g>
</svg>

**FeatureTransformer**は125,388次元を256次元へ落とす最初の全結合層で、両視点が同じ重みを共有する。
入力が疎なので、この層の計算は「立っている38個の特徴量に対応する重みベクトルを足す」だけで済む。
125,388×256という巨大な重み行列を持ちながら、実際に触るのは38行だけである。

二つの視点の256次元は、手番側を先頭にして512次元へ連結する。
評価値は手番側から見た点数として学習されているので、どちらの視点を先頭にするかが手番の情報を運ぶ。

その後は512→32→32→1と続く小さな全結合層で、各層の間に**ClippedReLU**（0〜127へのclamp）が入る。
最初の1層が巨大で残りが極端に小さいこの形は、「重い部分は差分更新でほぼ無料になる」というNNUEの設計判断そのものである。
どう無料になるかは[accumulatorの差分更新](accumulator.md)で、整数演算の内訳は[量子化推論とAVX2](inference.md)で追う。

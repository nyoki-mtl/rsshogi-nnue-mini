# accumulatorの差分更新

FeatureTransformerの出力、つまり視点ごとの256次元ベクトルを**accumulator**と呼ぶ。
毎回の評価でこれを一から作ると、38個の特徴量それぞれについて256次元の重みを足すことになり、推論時間のほとんどがこの層に消える。
だが探索には、この計算をほぼ省ける事情がある。
探索が評価する局面は、直前に評価した局面から一手しか違わない。

## 一手が動かす特徴量は最大4個

一手が特徴量の集合に与える差分は小さい。

- 移動元から駒が消える（盤上の特徴量を1個除去）
- 移動先に駒が現れる（盤上の特徴量を1個追加。成りなら成り駒として）
- 捕獲なら、取られた駒が盤から消え（除去）、取った側の持ち駒が1枚増える（追加）
- 駒打ちなら、持ち駒が1枚減り（除去）、盤上に駒が現れる（追加）

どう組み合わせても、視点あたり除去2個、追加2個が上限になる。
また、盤上の駒と持ち駒の合計枚数は指し手で変わらないので、38個へ揃える埋めのslotは動かない。
accumulatorの更新は「消えた特徴量の重みを引き、現れた特徴量の重みを足す」だけで済み、38本の重みベクトルを足し直す代わりに最大4本の加減算になる。

具体例として、2三の歩が2二の銀を取って成る手を考える。
除去と追加は両視点で起きるが、基点の敵味方とマスの向きが視点ごとに違う。

| 盤と持ち駒の変化 | 先手視点の差分 | 後手視点の差分 |
| --- | --- | --- |
| 2三の先手歩が消える | 除去：味方の歩@2三 | 除去：相手の歩@8七（反転） |
| 2二に先手のとが現れる | 追加：味方の成駒@2二 | 追加：相手の成駒@8八（反転） |
| 2二の後手銀が消える | 除去：相手の銀@2二 | 除去：味方の銀@8八（反転） |
| 先手の持ち駒に銀が増える | 追加：味方の持ち銀1枚目 | 追加：相手の持ち銀1枚目 |

この差分は局面を見比べて求めるのではなく、指し手そのものから直接構成する。
移動元、移動先、捕獲駒、打った駒は指し手が全部知っているので、盤面を走査する必要がない。

## 玉が動いた側だけfull refresh

この差分更新には、成立しない場合が一つある。
玉が動いたときである。

特徴量のindexは`自玉のマス × 1548`を原点にしているから、玉が動くとその視点の全特徴量が別の区画へ移る。
38個全部が入れ替わるので、差分で追う意味がない。
このとき、玉が動いた側の視点だけを一から計算し直す（full refresh）。

もう一方の視点は影響を受けない。
HalfKPの特徴量は自玉と玉以外の駒のペアなので、相手玉はそもそも特徴量に現れないからである。
相手玉の移動が差分を生むのは、その移動が捕獲を伴うときだけで、その場合も通常の捕獲と同じ差分で処理できる。

<svg viewBox="0 0 720 300" xmlns="http://www.w3.org/2000/svg" style="max-width: 720px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-acc" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <g fill="none" stroke="currentColor" stroke-width="1.3">
    <rect x="20" y="115" width="150" height="60" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="250" y="40" width="200" height="70" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="250" y="180" width="200" height="70" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="540" y="115" width="160" height="60" rx="6" fill="currentColor" fill-opacity="0.05"/>
  </g>
  <g stroke="currentColor" stroke-width="1.3" fill="none" marker-end="url(#arrow-acc)">
    <line x1="170" y1="132" x2="248" y2="82"/>
    <line x1="170" y1="158" x2="248" y2="208"/>
    <line x1="450" y1="75" x2="538" y2="130"/>
    <line x1="450" y1="215" x2="538" y2="160"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="95" y="140" text-anchor="middle">一手を適用</text>
    <text x="95" y="160" text-anchor="middle" font-size="11" opacity="0.75">視点ごとに判定</text>
    <text x="350" y="64" text-anchor="middle">その視点の玉が動いた</text>
    <text x="350" y="83" text-anchor="middle" font-size="11" opacity="0.75">full refresh</text>
    <text x="350" y="99" text-anchor="middle" font-size="11" opacity="0.75">直前の256次元を丸ごと退避</text>
    <text x="350" y="204" text-anchor="middle">それ以外</text>
    <text x="350" y="223" text-anchor="middle" font-size="11" opacity="0.75">差分更新（除去≤2、追加≤2）</text>
    <text x="350" y="239" text-anchor="middle" font-size="11" opacity="0.75">差分そのものを履歴に記録</text>
    <text x="620" y="140" text-anchor="middle">履歴へpush</text>
    <text x="620" y="160" text-anchor="middle" font-size="11" opacity="0.75">undoで逆適用または復元</text>
  </g>
</svg>

## undoは履歴を戻すだけ

探索は手を進めては戻すことを繰り返すので、undoも安くなければならない。
accumulatorは更新のたびに、何をしたかを履歴のstackへ積む。
差分更新なら差分そのもの、full refreshなら退避しておいた直前の256次元ベクトルである。
undoは履歴を1個popし、差分なら逆向きに適用し、refreshなら退避を書き戻す。
探索rootで一度だけfull refreshして初期化すれば、以後の探索木の上り下りはこの差分と履歴だけで追える。

なお、null move（[枝刈り](../search/pruning.md)で使う手番だけの反転）は盤面も持ち駒も変えないので、accumulatorには何もしない。

## 正しさはoracleとの一致で確かめる

差分更新は速いが、除去と追加を1個でも取り違えると評価値が静かに壊れる。
探索は文句を言わずに間違った点数で指し続けるので、この種のバグは対局結果からは見つけにくい。

そこで、毎回一から計算するscalarのfull refreshを**oracle**（正解を与える遅い実装）として残し、テストで突き合わせる。
6局面の全合法手と3手までの入れ子のdo/undoについて、次の三つが一致することを確かめている。

1. 指し手から構成した差分で進めたaccumulator
2. 進めた後の局面からのfull refresh
3. 前後の局面の特徴量集合を多重集合として比較した差分

捕獲、駒打ち、成り、玉の移動、玉での捕獲、undoがこの範囲に入る。
公式networkを使った外部parity testでも、各操作の後に差分とfull refreshの一致を確かめている（[厳密な読み込みと水匠5互換性](loader.md)を参照）。

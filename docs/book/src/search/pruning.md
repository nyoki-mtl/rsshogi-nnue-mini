# 枝刈り

alpha-betaのcutoffは証明である。
beta以上の手が見つかった以上、残りを読まない判断に反例はない。
この章の枝刈りはそうではない。
「この手はどうせalphaを超えない」という見込みに賭けて読む量を減らす、間違えることのある削減である。
だから各技法は、賭けの内容と、賭けてよい条件を対で持つ。

実装している枝刈りは次のとおりで、静止探索の中の2つは[静止探索](qsearch.md)で扱う。

| 技法 | 適用場所 | 主な条件 | 動作 |
| --- | --- | --- | --- |
| null move pruning | 非PVのdepth 4以上 | 王手なし、静的評価がbeta以上 | 手番を渡して浅く探索し、それでもbeta以上ならcutoff |
| reverse futility pruning | 非PVのdepth 2以下 | 静的評価 − 135×depth ≥ beta | 静的評価を返して打ち切る |
| futility pruning | depth 1 | 静的評価 + 183 ≤ alpha | 先頭以外の静かな手を読まない |
| late move reductions（LMR） | depth 3以上の5手目以降 | 静かな手 | 2 ply浅く読み、alphaを超えたら読み直す |
| late move pruning（LMP） | 非PVのdepth 1〜3 | 評価が2 ply前から改善していない | 遅い静かな手を個数で打ち切る |
| delta pruning | 静止探索 | stand pat + 駒得 + 126 < alpha | 読まない |
| SEEによる捕獲の枝刈り | 静止探索 | SEEが負の捕獲 | 読まない |

## null move pruning

一番大胆な賭けから見る。
「手番を相手に渡しても、まだbeta以上の点数が出る」なら、普通に指せばなおさらbetaを超えるはずである。
そこで、パスに相当する**null move**を適用し、通常より2 ply浅いzero-window探索を走らせる。
それでもbeta以上が返れば、本物の手を読まずにcutoffする。

浅い探索一回で深い探索を丸ごと省くので削減は大きいが、この論法には既知の穴がある。
「指したくない手しかない」局面、つまりzugzwangではパスのほうが良く、前提が崩れる。
将棋は持ち駒のおかげでchessよりzugzwangが希だが、それでも適用条件は保守的に絞っている。
depth 4以上の非PVのzero-window nodeで、王手されておらず、静的評価がbeta以上で、betaが被詰み側の詰み帯にない場合だけ試す。
直前がnull moveなら適用せず、パスの応酬になることを防ぐ。

fail highしたときは、best moveを持たない下界（`Lower`）として置換表へ保存し、betaを返す。
探索用のnull moveは盤面と持ち駒を変えず手番だけを反転するので、NNUE accumulatorは更新しない。

## reverse futility pruning

浅いnodeでは逆向きの賭けもできる。
静的評価がbetaを深さに応じたmargin（135×depth）以上超えているなら、残りの数plyで形勢が落ちてbetaを下回ることはまずない。
depth 2以下の非PV nodeで、王手されていなければ、手を読まずに静的評価を返す。

## futility pruning

depth 1のnodeで静的評価がalphaより183以上低いなら、静かな手を1手指したくらいではalphaに届かない。
このnodeでは先頭の手だけを通常どおり読み、残りの静かな手のうち王手を掛けないものを省く。
捕獲と成りは駒得で一気にalphaを超え得るので省かない。

## late move reductions

読まない、まで賭けたくない手には、浅く読むという中間の選択肢がある。
**LMR**は、depth 3以上のnodeで5手目以降の静かな手を2 ply浅いzero-windowで読む。
並べ替えの後ろにいる静かな手が最善である見込みは低い、という賭けである。

読み違えたときの保険が本体で、浅い探索がalphaを超えたら通常の深さで読み直す。
王手されている局面と王手を掛ける手には適用しない。

## late move pruning

depth 1〜3の非PV nodeでは、並べ替えの遅い位置にある静かな手を、途中から個数で打ち切る。
depth 1では8個、depth 2では12個、depth 3では19個の対象手を通常どおり読み、それ以降の対象手を読まない。

対象になるのは、静かな手のうち駒打ちでも王手でもないものだけである。
さらに、同じ手番の2 ply前と現在の静的評価が両方得られ、現在が改善していない場合に限る。
形勢が上向いているnodeでは、遅い手の中に浮上する手が残っている見込みが相対的に高いからである。
null moveの配下ではこの静的評価の履歴をリセットし、パスをまたいだ改善判定をしない。

## 置換表へ保存できる値

置換表へ保存する点数には、同じ局面を再探索したときにも成り立つ根拠が要る。
読まずに省いた手や、浅いまま確定した手があるnodeの点数は、そのnodeの厳密な値ではない。
そこで、futility、LMP、読み直しなしのLMRが発生したnodeでは、beta cutoffで得た下界だけを保存する。
どれかの手がbeta以上だったという下界は、他の手を読んだかどうかに依存しないからである。

## 枝刈りを支える安全条件

枝刈りは、静的評価と手の並び順を信用できる範囲に絞って適用する。

- reverse futilityとLMPは非PV nodeに限る。PVは`bestmove`として公開する読み筋になる
- reverse futilityとfutilityは、王手されていない局面に限る。王手中は静的評価より応手の合法性が先に立つ
- futility、LMR、delta pruningは、王手を掛けない手に限る。王手には静的な駒得に現れない強制力がある

枝刈りの定数は小さな固定値で、tuning binaryでは12個の探索parameterとしてSPSAの対象になる（[運用](../operations/shogiarena.md)を参照）。

## 参考資料

- [Chess Programming Wiki: Pruning](https://www.chessprogramming.org/Pruning)
- [Chess Programming Wiki: Null Move Pruning](https://www.chessprogramming.org/Null_Move_Pruning)
- [Chess Programming Wiki: Futility Pruning](https://www.chessprogramming.org/Futility_Pruning)
- [Chess Programming Wiki: Late Move Reductions](https://www.chessprogramming.org/Late_Move_Reductions)

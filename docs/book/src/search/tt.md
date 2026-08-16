# 置換表と千日手

探索木は木ではない。
手順前後で同じ局面へ合流する経路が無数にあり、素朴な探索は合流のたびに同じ部分木を読み直す。
**置換表**（transposition table、TT）は局面をkeyにした共有のcacheで、一度読んだ結果を合流してきた別の経路へ再利用させる。

entryには探索の深さ、点数、点数の性質（厳密値か、下界か、上界か）、最善手を保存する。
probeで同じkeyのentryが見つかり、保存された深さが今必要な深さ以上なら、性質に応じて点数をそのまま使う。
深さが足りなくても最善手は使えるので、並べ替えの先頭に置く（[手の並べ替えとSEE](ordering.md)を参照）。
詰みの点数は「rootからの手数」を含むため、保存時に局面基準へ、取得時に現在の手数基準へ補正する。

## 構造と置換

表の実体は固定長の配列で、大きさは`USI_Hash`（既定16 MiB、1〜1024）で決まる。
各slotは同じindexに衝突した2局面を保持する**cluster**である。

書き込みの優先順位は次のとおりで、新しい探索を始めるたびに世代番号を進める。

1. 同じkeyのentryがあれば、深さが同等以上のときだけ中身を置き換える。浅いときは世代だけ現在に引き直す
2. 空きslotがあれば使う
3. どちらも埋まっていれば、旧世代のentry、それがなければ現世代の浅いentryを犠牲に選ぶ。現世代のentryは、同等以上の深さの新entryでだけ置き換える

深い探索の結果は再現に費用がかかっているので、浅い結果より生き残らせる。
一方、何世代も前のentryは深くても参照される見込みが薄いので、先に手放す。
`usinewgame`は全entryを消去し、`USI_Hash`の変更は表を作り直す。
tuning binaryだけが広告する`Clear Hash`も全entryを消去する。

## 千日手がkeyを壊す

ここまでは教科書どおりのTTである。
将棋固有の問題は、千日手がこの仕組みの前提を壊すところにある。

TTの前提は「同じ局面なら同じ点数」である。
しかし千日手の判定は局面だけでは決まらない。
同じ盤面と持ち駒でも、そこへ至る経路で同一局面を何回数えたか、連続王手が続いているかで、引き分けにも反則負けにもなる。
局面のhashだけをkeyにすると、経路Aで「千日手、0点」と保存したentryを、千日手ではない経路Bが信じてしまう。

このエンジンのkeyは、局面のhashに、千日手判定の結果を左右する状態だけを混ぜた指紋を重ねる。

<svg viewBox="0 0 740 200" xmlns="http://www.w3.org/2000/svg" style="max-width: 740px; width: 100%; height: auto; font-family: sans-serif;">
  <g fill="none" stroke="currentColor" stroke-width="1.3">
    <rect x="20" y="30" width="200" height="52" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="300" y="16" width="420" height="80" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="312" y="40" width="120" height="44" rx="4" fill="currentColor" fill-opacity="0.07"/>
    <rect x="440" y="40" width="130" height="44" rx="4" fill="currentColor" fill-opacity="0.07"/>
    <rect x="578" y="40" width="130" height="44" rx="4" fill="currentColor" fill-opacity="0.07"/>
    <rect x="270" y="140" width="200" height="44" rx="6" fill="currentColor" fill-opacity="0.12"/>
  </g>
  <g stroke="currentColor" stroke-width="1.3" fill="none">
    <line x1="120" y1="82" x2="120" y2="162"/>
    <line x1="120" y1="162" x2="268" y2="162"/>
    <line x1="510" y1="96" x2="510" y2="120"/>
    <line x1="510" y1="120" x2="370" y2="120"/>
    <line x1="370" y1="120" x2="370" y2="138"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="120" y="52" text-anchor="middle">局面のhash</text>
    <text x="120" y="70" text-anchor="middle" font-size="11" opacity="0.75">盤面、持ち駒、手番</text>
    <text x="510" y="34" text-anchor="middle" font-size="12">千日手判定を左右する状態の指紋</text>
    <text x="372" y="60" text-anchor="middle" font-size="12">遡れる幅</text>
    <text x="372" y="76" text-anchor="middle" font-size="10" opacity="0.75">min(ply, null後, 16)</text>
    <text x="505" y="60" text-anchor="middle" font-size="12">同一局面の回数と</text>
    <text x="505" y="76" text-anchor="middle" font-size="12">判定の種別</text>
    <text x="643" y="60" text-anchor="middle" font-size="12">連続王手の長さ</text>
    <text x="643" y="76" text-anchor="middle" font-size="10" opacity="0.75">両者分</text>
    <text x="345" y="167" text-anchor="middle" font-size="13">XOR → TT key</text>
    <text x="480" y="167" font-size="11" opacity="0.75">（手数上限の使用時は手数も混ぜる）</text>
  </g>
</svg>

`rsshogi`の探索用千日手判定は`min(探索ply, 直前のnull moveからのply, 16)`手までしか遡らない。
そこで、この遡れる幅、すでに数えた同一局面の回数と判定の種別、連続王手の長さをkeyへ入れる。
`MaxMovesToDraw`を使う対局では、手数上限と現在の手数も混ぜる。

一方で、その窓の中身、つまりどの経路でこの局面へ来たかはkeyへ入れない。
経路を入れると手順前後で合流した同一局面が別entryになり、TTがtranspositionを共有できなくなって存在意義を失うからである。
代わりに、祖先の集合が違う二つの経路が同じentryを共有し得るという**graph history interaction**（GHI）を受け入れる。
これは現局面だけをkeyにする一般的なengineと同じ割り切りで、遡れる幅をkeyに残すぶんだけ、混入し得る面は一般的な設計より狭い。
rootにすでに成立している千日手は、TTを見る前に判定する。

## 探索をまたいだ再利用の限界

このkey設計には副作用がある。
keyが「rootからの距離」に依存するため、前回の`go`と今回の`go`で同じ局面が共有されるのは、rootから同じ距離に現れた場合と、遡れる幅が16で飽和する深いnodeに限られる。
一手指すとほとんどの局面はrootからの距離がずれるので、対局を進めながらの持ち越しはあまり効かない。
持ち越した表が最もよく効くのは、同じrootを読み直す場合と、`stop`して再開する場合である。

## 保存する値の条件

TTへ保存できるのは、同じ定義域の探索が読み直したときにも同じ意味を持つ値である。
[枝刈り](pruning.md)ではbeta cutoffで得た下界に保存を絞り、[静止探索](qsearch.md)では入口のentryをdepth 0としてmain searchの値と区別する。

## 参考資料

- [Chess Programming Wiki: Transposition Table](https://www.chessprogramming.org/Transposition_Table)
- [Chess Programming Wiki: Zobrist Hashing](https://www.chessprogramming.org/Zobrist_Hashing)
- [Chess Programming Wiki: Graph History Interaction](https://www.chessprogramming.org/Graph_History_Interaction)

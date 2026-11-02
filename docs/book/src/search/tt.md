# 置換表と千日手

探索木は木ではない。
手順前後で同じ局面へ合流する経路が無数にあり、素朴な探索は合流のたびに同じ部分木を読み直す。
**置換表**（transposition table、TT）は局面をkeyにした共有のcacheで、一度読んだ結果を合流してきた別の経路へ再利用させる。
子局面を作った直後、x86_64では対応するclusterをCPUへ先読みするよう伝える。
これはメモリ待ちを隠すためのhintで、entryの内容や探索の判断は変えない。

エントリには探索の深さ、点数、点数の性質（`Exact`、`Lower`、`Upper`）、最善手と、求めてあれば生の静的評価を保存する。
probeで同じkeyのentryが見つかり、保存された深さが今必要な深さ以上なら、PV node以外では性質に応じて点数をそのまま使う。
PV nodeで打ち切らないのは、打ち切るとPVが途中で切れ、以前の窓で得た値がPV上へそのまま戻ってくるためである。
深さが足りなくても最善手は使えるので、並べ替えの先頭に置く（[手の並べ替えとSEE](ordering.md)を参照）。
詰みの点数は「rootからの手数」を含むため、保存時に局面基準へ、取得時に現在の手数基準へ補正する。

## 構造と置換

表の実体は固定長の配列で、大きさは`USI_Hash`（既定16 MiB、1〜1024）で決まる。
同じインデックスに対応する4個のスロットをまとめて、**cluster**と呼ぶ。
clusterは64バイトで、cache lineの境界に揃えてあるので、一回のprobeが読むメモリは1本のcache lineに収まる。
各スロットは2個の`AtomicU64`からなる16バイトの領域で、ロックを使わず読み書きする。
キーの32ビット検証子と指し手を、評価値などのデータとXORで組み合わせて保存し、読み出した検証子が一致しないエントリは捨てる。
検証子の衝突や同時書き込みによる混在を完全に排除する方式ではない。
データには評価値、静的評価、深さ、境界の種類、世代のほかに、そのnodeがPV上で読まれたかを示すPVビットを持つ。主探索はこのビットでLMRの縮小を控える。

書き込みの優先順位は次のとおりで、新しい探索を始めるたびに世代番号を進める。

1. 同じkeyのentryがあれば、新しい結果が`Exact`か、深さの差が2以内のときに中身を置き換える。新しい結果に最善手が無ければ、保存済みの手を残す。大きく浅いときは世代だけ現在に引き直す
2. 空きslotがあれば使う
3. どちらも埋まっていれば、「深さ − 2 × 世代の古さ」が最も小さいentryを追い出し、新しい結果を必ず保存する

深い探索の結果は再現に費用がかかっているので、浅い結果より生き残らせる。
一方、何世代も前のentryは深くても参照される見込みが薄いので、世代が進むほど浅いものとして扱う。
新しい結果を捨てないのは、同じ局面を読み直したときに最善手の情報が失われるほうが高くつくためである。
`usinewgame`は全entryを消去し、`USI_Hash`の変更は表を作り直す。

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
    <text x="372" y="60" text-anchor="middle" font-size="12">同一局面の回数</text>
    <text x="372" y="76" text-anchor="middle" font-size="10" opacity="0.75">履歴で数えた回数</text>
    <text x="505" y="60" text-anchor="middle" font-size="12">判定の種別</text>
    <text x="505" y="76" text-anchor="middle" font-size="10" opacity="0.75">引き分け・勝ち・負け・優劣</text>
    <text x="643" y="60" text-anchor="middle" font-size="12">連続王手の長さ</text>
    <text x="643" y="76" text-anchor="middle" font-size="10" opacity="0.75">両者分</text>
    <text x="345" y="167" text-anchor="middle" font-size="13">XOR → TT key</text>
    <text x="480" y="167" font-size="11" opacity="0.75">（手数上限の使用時は手数も混ぜる）</text>
  </g>
</svg>

`rsshogi`の千日手判定は、直前のnull move（無ければ対局開始）まで履歴を遡り、その局面で同一局面の回数と判定の種別を決める。
keyへ入れるのは、この回数と種別、連続王手の長さである。
探索plyは入れない。rootが2手進んだ次の`go`でも、同じ局面は同じentryを引ける。
`MaxMovesToDraw`を使う対局では、手数上限と現在の手数も混ぜる。

一方で、その窓の中身、つまりどの経路でこの局面へ来たかはkeyへ入れない。
経路を入れると手順前後で合流した同一局面が別entryになり、TTがtranspositionを共有できなくなって存在意義を失うからである。
代わりに、祖先の集合が違う二つの経路が同じentryを共有し得るという**graph history interaction**（GHI）を受け入れる。
履歴の要約を加えても経路全体を区別できるわけではなく、GHIを完全に防ぐものではない。
rootにすでに成立している千日手は、TTを見る前に判定する。

## 保存する値の条件

主探索は、beta cutoffした結果を`Lower`、窓内でalphaを上げた結果を`Exact`、alphaを上げられなかった結果を`Upper`として保存する。
この境界の種類は実際に行った探索の窓に対する分類であり、枝刈りや縮小を省いた全幅minimax値への厳密な保証ではない。
alphaを上げられなかったノードは、読んだ手の最善値をそのまま返して保存する（fail-soft）。
置換表に無い局面がreverse futility pruningで打ち切られたときも、返した値をその深さの`Lower`として保存する（[reverse futilityとfutility](pruning.md#reverse-futilityとfutility)を参照）。

[静止探索](qsearch.md)は全てのノードで深さを問わずエントリを参照し、保存時には深さ0を付ける。
枝刈り後にcutoffしなかった結果は保存しない。
生の静的評価は両探索で再利用し、correction historyの補正を利用時に加える。

## 使用率の観測

USIの`info`には、置換表の現世代の使用率を**hashfull**として千分率で出す。
先頭の最大1000 clusterを読み、現世代に属する使用slotの割合を`0..=1000`で示す。
全容量を走査する正確な使用率ではなく、並行更新中の概算である。
旧世代のentryが残っていても、現世代の使用率には数えない。
世代は6bitで循環するため、一周以上残ったentryを古さで区別することはできない。

反復完了時に測定し、最終報告では全補助ワーカーのジョブ完了後に測り直す。
この観測でentryの世代や内容を書き換えることはない。
TTのhit数とcutoff数は、既存の`stats` featureで別に確認できる。

## 参考資料

- [Chess Programming Wiki: Transposition Table](https://www.chessprogramming.org/Transposition_Table)
- [Chess Programming Wiki: Zobrist Hashing](https://www.chessprogramming.org/Zobrist_Hashing)
- [Chess Programming Wiki: Graph History Interaction](https://www.chessprogramming.org/Graph_History_Interaction)

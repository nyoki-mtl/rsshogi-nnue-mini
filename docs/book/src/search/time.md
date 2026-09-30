# 時間管理

時間切れは反則負けである。
そのため、一手に使う時間から安全余裕を引き、探索中も締切を確認する。
停止確認は協調的に行うので、OSの待ち時間や一回の評価にかかる時間まで含めた厳密な応答期限を保証するものではない。

## 予算の作り方

`go`の時間指定から、この一手に使う**予算**を作る。

```text
配分 = 残り時間 ÷ 配分手数 + 増秒 + 秒読み
上限適用後 = min(配分, 残り時間 + 秒読み)
安全余裕 = min(MoveOverhead, 上限適用後 ÷ 2)
予算 = max(上限適用後 − 安全余裕, 1 ms)
```

配分手数の既定値は30で、`movestogo`が指定されていればその値を使う（最低1）。
残り時間をこの手数で割り、着手ごとに戻る増秒と秒読みを上乗せする。
ただし、この一手で物理的に使えるのは残り時間と秒読みの和までで、配分がそれを超えるなら上限で頭打ちにする。
増秒を上限に数えないのは、増秒が着手を終えてから加算されるためである。
`btime 0 binc 2000`を「2秒使える」と読むと、実際には残高0で時間切れ負けになる。

**安全余裕**（`MoveOverhead`、既定500 ms）は、GUIや中継との往復遅延のために予算から引く。
引く量は予算の半分を超えないので、短い秒読みでも思考時間が半分は残る。
`movetime`が指定されたときは配分を行わず、その値から安全余裕を引いた全額を予算にする。
手番側の残り時間が省略され、秒読みか増秒だけがある場合は、その和から安全余裕を引く。
残り時間の省略と、明示的な残り時間0は区別する。
`go infinite`では時間予算を設けない。

## 二段の締切と伸縮

予算から締切を二つ作る。

<svg viewBox="0 0 740 190" xmlns="http://www.w3.org/2000/svg" style="max-width: 740px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-tm" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <rect x="60" y="52" width="150" height="18" fill="currentColor" fill-opacity="0.12" stroke="none"/>
  <rect x="210" y="52" width="170" height="18" fill="currentColor" fill-opacity="0.07" stroke="none"/>
  <rect x="380" y="52" width="300" height="18" fill="currentColor" fill-opacity="0.03" stroke="none"/>
  <g stroke="currentColor" stroke-width="1.5" fill="none">
    <line x1="40" y1="61" x2="710" y2="61" marker-end="url(#arrow-tm)"/>
    <line x1="60" y1="45" x2="60" y2="77"/>
    <line x1="210" y1="45" x2="210" y2="77"/>
    <line x1="680" y1="45" x2="680" y2="77"/>
  </g>
  <g stroke="currentColor" stroke-width="1.2" fill="none" stroke-dasharray="4 3">
    <line x1="120" y1="84" x2="120" y2="98"/>
    <line x1="380" y1="84" x2="380" y2="98"/>
    <line x1="120" y1="91" x2="380" y2="91"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="60" y="34" text-anchor="middle">go</text>
    <text x="210" y="34" text-anchor="middle">soft（予算の60%）</text>
    <text x="680" y="34" text-anchor="middle">hard（予算の最大3倍）</text>
    <text x="250" y="118" text-anchor="middle" font-size="12">softは局面に応じて0.4〜2.5倍に伸縮する</text>
    <text x="370" y="148" text-anchor="middle" font-size="12">最善手が揺れる・評価値が下がる → 伸ばす　同じ最善手が続く → 縮める</text>
    <text x="370" y="176" text-anchor="middle" font-size="11" opacity="0.75">movetimeと純秒読みでは soft = hard = 予算（伸縮も延長もしない）</text>
  </g>
</svg>

**hard deadline**は各ノードの入口などで確認し、到達を検知したら探索を止める。
持ち時間があるときは、予算の3倍までhardを延ばす。
ただし、残り時間の1/4と秒読みの和を超えず、この一手で物理的に使える量（残り時間と秒読みの和）からも安全余裕を引く。
延長で持ち時間を使い込み、後半の手が即指しになるのを防ぐためである。

**soft deadline**は予算の60%の位置を基準にし、これを過ぎたら新しいiterationを始めない。
softを設ける理由は、反復深化の費用構造にある。
深い反復ほど多くの探索が必要になり、残り予算が少ない時点で始めた反復は完了する前にhardへ達しやすい。

softの位置は、iterationを終えるたびに結果を見て伸縮する。

- 最善手が前のiterationから変わると伸ばす。変化の回数はiterationごとに半分へ減衰させて数えるので、最近の揺れほど強く効く。
- 評価値が過去のiterationの平均より下がると伸ばす。悪くなりつつある局面で浅い結論のまま指すと、取り返しがつかないことが多い。
- 同じ最善手が続くほど縮める。次のiterationで結論が変わりにくいので、節約した時間を揺れている局面へ回せる。
- 合法手が1つしかなければ、最初のiterationを終えた時点で指す。

伸縮率は0.4倍から2.5倍に収め、hardを越えることはない。

例外が二つある。
`movetime`はその時間を使えという指示であり、手番側に持ち時間のない純秒読みは残しても次の手へ回せない。
どちらもsoftとhardを予算と同じ位置に置き、伸縮も延長もしない。
ただし、深さやノード数の制限などにより、その前に探索を終える場合はある。

softを確認するのは結果を生成するメインワーカーだけで、補助ワーカーは停止まで置換表を埋め続ける（[Lazy SMP](parallel.md)を参照）。

## ponderと締切の引き直し

`go ponder`は相手の手番で走らせる探索なので、`go`の時刻で締切を固定してはならない。
そうすると、相手が長考しただけで自分の思考時間が失われる。

締切は「予算」と「起点」を分けて持ち、ponder中は到達判定そのものを保留する。
`ponderhit`が来たら、その時刻を新しい起点として同じ予算を計り直す。
締切は探索workerとsessionが同じ実体を共有するatomicなobjectで、起点の差し替えに探索の停止を要しない。
探索中ならそのまま継続し、すでに完了していれば保留していた結果をその場で公開する。

## 深さ1を完了できない場合

hard deadlineは探索を途中で止めるので、depth 1すら完了しないことがある。
その場合でも、[探索](../search.md)で述べたdepth-0のfallback（予約した1 nodeで評価済みのroot child）が必ず残っており、`bestmove`は評価値と対応の取れた合法手になる。
この仕組みは、通常の停止処理で返す手と評価値を確保するためのものである。
応答を送るまでの遅延を含めた時間切れの有無は、実際の対局環境で確認する。

## 参考資料

- [Chess Programming Wiki: Time Management](https://www.chessprogramming.org/Time_Management)
- [Chess Programming Wiki: Iterative Deepening](https://www.chessprogramming.org/Iterative_Deepening)

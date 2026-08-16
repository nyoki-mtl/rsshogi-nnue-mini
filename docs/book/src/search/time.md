# 時間管理

時間切れは反則負けである。
探索のバグは悪手で済むことが多いが、時間管理のバグは盤上がどれだけ優勢でも対局を落とす。
だからこの層の設計は、強さより先に「絶対に超えない」を置き、その制約の中で使い残しを減らす。

## 予算の作り方

`go`の時間指定から、この一手に使う**予算**を作る。

```text
予算 = min(残り時間 ÷ 30 + 増秒 + 秒読み,  残り時間 + 秒読み) − 安全余裕
```

第一項が配分、第二項が上限である。
残り30手で使い切る配分を基本に、着手ごとに戻る増秒と秒読みを上乗せする。
ただし、この一手で物理的に使えるのは残り時間と秒読みの和までで、配分がそれを超えるなら上限で頭打ちにする。
増秒を上限に数えないのは、増秒が着手を終えてから加算されるためである。
`btime 0 binc 2000`を「2秒使える」と読むと、実際には残高0で時間切れ負けになる。

**安全余裕**（`MoveOverhead`、既定500 ms）は、GUIや中継との往復遅延のために予算から引く。
引く量は予算の半分を超えないので、短い秒読みでも思考時間が半分は残る。
`movetime`が指定されたときは配分を行わず、その値から安全余裕を引いた全額を予算にする。

## 二段の締切

予算は一つでも、締切は二つ置く。

<svg viewBox="0 0 740 170" xmlns="http://www.w3.org/2000/svg" style="max-width: 740px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-tm" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <rect x="60" y="52" width="372" height="18" fill="currentColor" fill-opacity="0.12" stroke="none"/>
  <rect x="432" y="52" width="248" height="18" fill="currentColor" fill-opacity="0.05" stroke="none"/>
  <g stroke="currentColor" stroke-width="1.5" fill="none">
    <line x1="40" y1="61" x2="710" y2="61" marker-end="url(#arrow-tm)"/>
    <line x1="60" y1="45" x2="60" y2="77"/>
    <line x1="432" y1="45" x2="432" y2="77"/>
    <line x1="680" y1="45" x2="680" y2="77"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="60" y="34" text-anchor="middle">go</text>
    <text x="432" y="34" text-anchor="middle">soft（予算の60%）</text>
    <text x="680" y="34" text-anchor="middle">hard（予算）</text>
    <text x="246" y="100" text-anchor="middle" font-size="12">新しいiterationを始めてよい</text>
    <text x="556" y="100" text-anchor="middle" font-size="12">進行中のiterationだけ続ける</text>
    <text x="680" y="128" text-anchor="end" font-size="12">全nodeで確認し、即座に停止</text>
    <text x="432" y="150" text-anchor="middle" font-size="11" opacity="0.75">movetimeと純秒読みでは soft = hard（残しても次の手へ回せない）</text>
  </g>
</svg>

**hard deadline**は予算そのものの位置にあり、探索の全nodeで確認して到達したら探索を止める。
**soft deadline**は予算の60%の位置にあり、これを過ぎたら新しいiterationを始めない。

softを設ける理由は、反復深化の費用構造にある。
iterationの費用は深さとともに数倍ずつ増えるので、残り予算が僅かな時点で始めたiterationは、まず完了せずhard deadlineで捨てられる。
その時間は`bestmove`の質にほとんど寄与しない。
60%の位置で新規開始を止めれば、最後のiterationがちょうど残りを使い切る形に近づく。

例外が二つある。
`movetime`はその時間を使えという指示であり、手番側に持ち時間のない純秒読みは残しても次の手へ回せない。
どちらもsoftをhardと同じ位置に置き、使い切る。

softを確認するのは結果を公開するmain workerだけで、helperは最後まで置換表を埋め続ける（[Lazy SMP](parallel.md)を参照）。

## ponderと締切の引き直し

`go ponder`は相手の手番で走らせる探索なので、`go`の時刻で締切を固定してはならない。
そうすると、相手が長考しただけで自分の思考時間が失われる。

締切は「予算」と「起点」を分けて持ち、ponder中は到達判定そのものを保留する。
`ponderhit`が来たら、その時刻を新しい起点として同じ予算を計り直す。
締切は探索workerとsessionが同じ実体を共有するatomicなobjectで、起点の差し替えに探索の停止を要しない。
探索中ならそのまま継続し、すでに完了していれば保留していた結果をその場で公開する。

## 締切に間に合わなくても手は返る

hard deadlineは探索を途中で止めるので、depth 1すら完了しないことがある。
その場合でも、[探索](../search.md)で述べたdepth-0のfallback（予約した1 nodeで評価済みのroot child）が必ず残っており、`bestmove`は評価値と対応の取れた合法手になる。
時間管理の失敗モードは「浅い手を指す」までで、「手を返せない」には到達しない。

## 参考資料

- [Chess Programming Wiki: Time Management](https://www.chessprogramming.org/Time_Management)
- [Chess Programming Wiki: Iterative Deepening](https://www.chessprogramming.org/Iterative_Deepening)

# Lazy SMPによる並列探索

alpha-betaの並列化は難しい。
cutoffは「先に読んだ手の結果」に依存するので、木を機械的に分割して配ると、片方の結果を待たずに読んだ枝が無駄になる。
木の分割を精密に管理する並列化もあるが、実装の複雑さは探索本体に匹敵する。

**Lazy SMP**は、複数のワーカーに同じルート局面を探索させ、置換表を通じて探索結果を共有する方式である。
あるワーカーが保存した結果を別のワーカーが参照し、深さと境界の条件が合えば再探索を省ける。
探索木を明示的に分割しないため、ワーカー間で重複して読む部分も残る。

<svg viewBox="0 0 720 260" xmlns="http://www.w3.org/2000/svg" style="max-width: 720px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-smp" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <g fill="none" stroke="currentColor" stroke-width="1.3">
    <rect x="20" y="30" width="190" height="64" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="20" y="120" width="190" height="52" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="20" y="190" width="190" height="52" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="360" y="105" width="170" height="70" rx="6" fill="currentColor" fill-opacity="0.12"/>
    <rect x="580" y="30" width="120" height="64" rx="6" fill="currentColor" fill-opacity="0.05"/>
  </g>
  <g stroke="currentColor" stroke-width="1.3" fill="none">
    <line x1="210" y1="62" x2="358" y2="125" marker-end="url(#arrow-smp)"/>
    <line x1="210" y1="146" x2="358" y2="140" marker-end="url(#arrow-smp)"/>
    <line x1="210" y1="216" x2="358" y2="155" marker-end="url(#arrow-smp)"/>
    <line x1="210" y1="52" x2="578" y2="52" marker-end="url(#arrow-smp)"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="115" y="52" text-anchor="middle">メインワーカー</text>
    <text x="115" y="70" text-anchor="middle" font-size="11" opacity="0.75">infoと最終結果を生成</text>
    <text x="115" y="85" text-anchor="middle" font-size="11" opacity="0.75">soft deadlineを判定</text>
    <text x="115" y="141" text-anchor="middle">補助ワーカー 1</text>
    <text x="115" y="159" text-anchor="middle" font-size="11" opacity="0.75">ルートの手順を1つずらす</text>
    <text x="115" y="211" text-anchor="middle">補助ワーカー 2</text>
    <text x="115" y="229" text-anchor="middle" font-size="11" opacity="0.75">ルートの手順を2つずらす</text>
    <text x="445" y="133" text-anchor="middle">共有置換表</text>
    <text x="445" y="151" text-anchor="middle" font-size="11" opacity="0.75">ノード数と停止フラグも共有</text>
    <text x="445" y="166" text-anchor="middle" font-size="11" opacity="0.75">局面と評価器は各ワーカーが保持</text>
    <text x="640" y="56" text-anchor="middle">USIセッション</text>
    <text x="640" y="74" text-anchor="middle" font-size="11" opacity="0.75">stdoutを所有</text>
  </g>
</svg>

## 役割の分離

`Threads`（既定1、1〜16）のうち1本が**メインワーカー**、残りが**補助ワーカー**である。
`info`と最終結果を生成するのはメインワーカーだけで、USIへの出力はセッションが行う。
補助ワーカーの成果は共有置換表への書き込みとして現れる。
soft deadlineで次の反復を始めるかどうかを判断するのもメインワーカーである。
補助ワーカーは停止の合図まで置換表を埋め続ける。

各ワーカーは局面、NNUE accumulator、history、killerを個別に持つ。
共有するのは置換表、ノードカウンタ、停止フラグ、ponder状態と締切である。
補助ワーカーの起動を知らせるフラグも共有するが、局面と評価の更新は各ワーカー内で完結する。

補助ワーカーがメインワーカーと同じ順で読むと、同じ枝を同時に読む重複が増える。
そこで補助ワーカーはルートの指し手の開始位置をワーカー番号だけ回転させ、読む順をずらす。
補助ワーカーはメインワーカーが深さ1の探索を終えてから起動する。

共有のnode counterは、上限があるときはatomicなadmission（空きがあるときだけ加算に成功する更新）で数え、複数workerが同時に加算しても上限を越えない。

## 終了とpanicの回収

メインワーカーの探索が終わると、全補助ワーカーを停止して合流を待ち、完了結果を一度だけ送る。
ワーカーがpanicした場合も、補助ワーカーを停止して合流を待ってから、異常を管理側へ伝える。
前の探索のワーカーを残したまま次の探索を始めると、共有置換表への書き込みが続いてしまう。

ワーカー間で同じ枝を読むこともあるため、スレッド数に比例して探索が速くなるとは限らない。

## 参考資料

- [Chess Programming Wiki: Lazy SMP](https://www.chessprogramming.org/Lazy_SMP)
- [Chess Programming Wiki: Shared Hash Table](https://www.chessprogramming.org/Shared_Hash_Table)
- [Chess Programming Wiki: Parallel Search](https://www.chessprogramming.org/Parallel_Search)

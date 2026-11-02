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
補助ワーカーのスレッドは常駐し、メインワーカーが深さ1の探索を終えてから探索を始める。

さらに補助ワーカーは、深さ3以上の反復のうち、ワーカー番号ごとにずれた3回に1回を読み飛ばす。
全ワーカーが同じ深さを同じ順に読むと、共有置換表を同じ局面で埋め合うだけになる。
読み飛ばした補助ワーカーはメインワーカーより先の深さを読み、その結果を置換表へ早く置く。
メインワーカーは読み飛ばさない。

共有のnode counterは、上限があるときはatomicなadmission（空きがあるときだけ加算に成功する更新）で数え、複数workerが同時に加算しても上限を越えない。
rootの各候補手に費やした探索量は、共有counterとは別に、そのworkerだけのnode数で数える。
この探索量で次の反復の手順序を決めるため、ほかのworkerが同じ時間に読んだ量は混ぜない。
時間で止める探索では、各ワーカーが256 nodeずつまとめて共有の数へ加算し、締切もそのまとめの最初でだけ確かめる。
全ワーカーが毎nodeで同じカウンタへ加算し時計を読むと、カウンタのcache lineの取り合いと時計の読み出しが無視できない費用になるためで、停止は最大256 nodeだけ遅れる。

## 終了とpanicの回収

メインワーカーの探索が終わると、全補助ワーカーへ停止を知らせ、ジョブの完了を待ってから結果を一度だけ送る。
補助ワーカーは待機に戻り、自分の履歴と探索用バッファを次の`go`でも使う。
`usinewgame`と、`tuning`付きビルドだけの`Clear Hash`は置換表と履歴の値を消し、`USI_Hash`の変更は共有置換表を差し替える。
いずれも補助ワーカーのスレッドを作り直さない。

`Threads`の変更後は、次の探索の前に旧補助ワーカーへ終了を通知し、joinしてから新しい構成を作る。
`EvalPackage`の変更時は旧ワーカーをその場で破棄し、評価関数の古いsnapshotを解放する。
次の探索で新しい構成を作る。
ワーカーがpanicした場合も、全ジョブの完了と全補助ワーカーのjoinを待ってから、異常をセッションへ伝える。
途中まで更新された探索状態は破棄し、次の`go`で新しいワーカーを作る。
前の探索が置換表へ書き込む状態を残したまま、次の探索へ進めない。

ワーカー間で同じ枝を読むこともあるため、スレッド数に比例して探索が速くなるとは限らない。

## 参考資料

- [Chess Programming Wiki: Lazy SMP](https://www.chessprogramming.org/Lazy_SMP)
- [Chess Programming Wiki: Shared Hash Table](https://www.chessprogramming.org/Shared_Hash_Table)
- [Chess Programming Wiki: Parallel Search](https://www.chessprogramming.org/Parallel_Search)

# 実行時の構成

探索中にも、GUIから`stop`、`ponderhit`、新しい`position`が届く。
探索threadがUSIへ直接出力すると、停止したはずの探索から古い`bestmove`が遅れて届き、次の局面の応答と混ざり得る。
この競合を避けるため、通信と探索を別の所有者へ分けている。

<svg viewBox="0 0 760 410" xmlns="http://www.w3.org/2000/svg" style="max-width: 760px; width: 100%; height: auto; font-family: sans-serif;">
  <defs>
    <marker id="arrow-runtime" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>
  <g fill="none" stroke="currentColor" stroke-width="1.3">
    <rect x="20" y="34" width="120" height="54" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="190" y="34" width="170" height="54" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="410" y="34" width="160" height="54" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="620" y="34" width="120" height="54" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="190" y="150" width="170" height="66" rx="6" fill="currentColor" fill-opacity="0.09"/>
    <rect x="70" y="285" width="190" height="82" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="300" y="285" width="190" height="82" rx="6" fill="currentColor" fill-opacity="0.05"/>
    <rect x="540" y="285" width="170" height="82" rx="6" fill="currentColor" fill-opacity="0.12"/>
  </g>
  <g stroke="currentColor" stroke-width="1.3" fill="none" marker-end="url(#arrow-runtime)">
    <line x1="140" y1="61" x2="188" y2="61"/>
    <line x1="360" y1="61" x2="408" y2="61"/>
    <line x1="570" y1="61" x2="618" y2="61"/>
    <path d="M 490 88 L 490 118 L 275 118 L 275 148"/>
    <line x1="245" y1="216" x2="175" y2="283"/>
    <line x1="275" y1="216" x2="395" y2="283"/>
    <line x1="360" y1="183" x2="623" y2="283"/>
    <path d="M 165 367 L 165 396 L 610 396 L 610 369"/>
    <path d="M 395 367 L 395 384 L 640 384 L 640 369"/>
  </g>
  <g fill="currentColor" font-size="13">
    <text x="80" y="56" text-anchor="middle">GUI</text>
    <text x="80" y="75" text-anchor="middle" font-size="11" opacity="0.75">USI command</text>
    <text x="275" y="56" text-anchor="middle">parser / formatter</text>
    <text x="275" y="75" text-anchor="middle" font-size="11" opacity="0.75">rsshogi-usi</text>
    <text x="490" y="56" text-anchor="middle">USI session</text>
    <text x="490" y="75" text-anchor="middle" font-size="11" opacity="0.75">stdoutとcommand順序を所有</text>
    <text x="680" y="56" text-anchor="middle">stdout</text>
    <text x="680" y="75" text-anchor="middle" font-size="11" opacity="0.75">info / bestmove</text>
    <text x="275" y="174" text-anchor="middle">search coordinator</text>
    <text x="275" y="193" text-anchor="middle" font-size="11" opacity="0.75">job開始、cancel、join</text>
    <text x="275" y="208" text-anchor="middle" font-size="11" opacity="0.75">探索をまたいで常駐</text>
    <text x="165" y="309" text-anchor="middle">main worker</text>
    <text x="165" y="328" text-anchor="middle" font-size="11" opacity="0.75">infoと最終結果を生成</text>
    <text x="165" y="346" text-anchor="middle" font-size="11" opacity="0.75">局面、評価器、history</text>
    <text x="395" y="309" text-anchor="middle">helper workers</text>
    <text x="395" y="328" text-anchor="middle" font-size="11" opacity="0.75">探索結果をTTへ蓄積</text>
    <text x="395" y="346" text-anchor="middle" font-size="11" opacity="0.75">局面、評価器、history</text>
    <text x="625" y="309" text-anchor="middle">共有状態</text>
    <text x="625" y="328" text-anchor="middle" font-size="11" opacity="0.75">TT、node count</text>
    <text x="625" y="346" text-anchor="middle" font-size="11" opacity="0.75">cancel</text>
  </g>
</svg>

## sessionが出力を直列化する

**USI session**だけがstdoutを所有し、parserから受け取ったcommandと探索結果を順番に処理する。
探索workerは`info`や完了結果をchannelへ送り、sessionがUSIの文字列へ整形して出力する。

`position`や`usinewgame`で局面が切り替わると、sessionは進行中の探索をcancelして完了を回収する。
探索jobには世代があるため、古いjobの結果は新しい局面へ流れない。

## coordinatorが探索の寿命を管理する

**search coordinator**はsessionと同じ期間だけ常駐し、`go`ごとにworkerへ探索jobを渡す。
main workerが`info`と最終結果を作り、helper workerは共有置換表を通じて探索を助ける。
探索が終わるとhelperを停止してjoinし、全workerの終了を確定してから結果をsessionへ返す。

各workerは局面、NNUE accumulator、history、killerを個別に持つ。
共有するのは置換表、node count、cancelであり、探索中に頻繁に書き換わる状態はworker内へ閉じ込める。

## 局面と評価状態を同じ手数に保つ

探索workerが指し手を進めると、局面の直後にNNUE accumulatorも進める。
指し手を戻した直後にはaccumulatorの履歴も戻す。
この順序により、探索木の局面と評価値が常に同じ手数を指す。

workerの分担は[Lazy SMPによる並列探索](search/parallel.md)、停止とponderの時間契約は[時間管理](search/time.md)で詳しく扱う。

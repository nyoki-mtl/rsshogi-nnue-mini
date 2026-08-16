# Lazy SMPによる並列探索

alpha-betaの並列化は難しい。
cutoffは「先に読んだ手の結果」に依存するので、木を機械的に分割して配ると、片方の結果を待たずに読んだ枝が無駄になる。
木の分割を精密に管理する並列化もあるが、実装の複雑さは探索本体に匹敵する。

**Lazy SMP**はこの問題を、調整をほぼ放棄することで回避する。
全workerに同じrootを最初から読ませ、共有するのは置換表だけにする。
あるworkerが読み終えた部分木の結果は置換表を経由して他のworkerへ届き、後から同じ局面に来たworkerはprobe一発で素通りする。
workerどうしの合意も分割も要らない。
「怠惰」の名のとおり設計は素朴だが、置換表が実質的な作業分配器として機能する。

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
    <text x="115" y="52" text-anchor="middle">main worker</text>
    <text x="115" y="70" text-anchor="middle" font-size="11" opacity="0.75">infoと最終結果を公開</text>
    <text x="115" y="85" text-anchor="middle" font-size="11" opacity="0.75">soft deadlineを判断</text>
    <text x="115" y="141" text-anchor="middle">helper worker 1</text>
    <text x="115" y="159" text-anchor="middle" font-size="11" opacity="0.75">root順を1つ回転</text>
    <text x="115" y="211" text-anchor="middle">helper worker 2</text>
    <text x="115" y="229" text-anchor="middle" font-size="11" opacity="0.75">root順を2つ回転</text>
    <text x="445" y="133" text-anchor="middle">共有置換表</text>
    <text x="445" y="151" text-anchor="middle" font-size="11" opacity="0.75">node counterとcancelも共有</text>
    <text x="445" y="166" text-anchor="middle" font-size="11" opacity="0.75">局面と評価器はworker局所</text>
    <text x="640" y="56" text-anchor="middle">USI session</text>
    <text x="640" y="74" text-anchor="middle" font-size="11" opacity="0.75">stdoutを所有</text>
  </g>
</svg>

## 役割の分離

`Threads`（既定1、1〜16）のうち1本が**main worker**、残りが**helper**である。
USIへの`info`と最終結果を公開するのはmainだけで、helperの成果は共有置換表への書き込みとしてだけ現れる。
soft deadlineでiterationの新規開始をやめる判断もmainだけが行い、helperは停止の合図まで置換表を埋め続ける。

各workerは局面、NNUE accumulator、history、killerを個別に持ち、共有するのは置換表、node counter、cancelの3つに限る。
共有状態を減らすほど、並列化のbugが入り込む面が狭くなる。

helperがmainと完全に同じ順で読むと、同じ枝を同時に読む重複が最大になる。
そこでhelperはroot moveの開始位置をworker番号だけ回転させ、読む順をずらす。
helperの開始はmainがdepth 1を完了した後で、最初のiterationの`bestmove`保証をhelperの書き込みが乱さないようにしている。

共有のnode counterは、上限があるときはatomicなadmission（空きがあるときだけ加算に成功する更新）で数え、複数workerが同時に加算しても上限を越えない。

## 終了とpanicの回収

mainの探索が終わると、全helperへcancelを送り、joinしてから完了結果を一度だけ送る。
mainまたはhelperがpanicした場合も、先に全helperを停止・joinしてからcoordinator境界へ伝播させる。
workerを残したまま次の探索を始めると、前の探索のthreadが共有置換表を書き続ける。

## 効果を測る

Lazy SMPではworker間の重複が残るため、thread数を増やした比率だけ探索が速くなるわけではない。
効果は`Threads = 1`をbaseline、増やした設定をcandidateとして、同じ持ち時間とholdout定跡でSPRT比較する。
この比較には探索量の増加と並列化のoverheadがともに反映される。

## 参考資料

- [Chess Programming Wiki: Lazy SMP](https://www.chessprogramming.org/Lazy_SMP)
- [Chess Programming Wiki: Shared Hash Table](https://www.chessprogramming.org/Shared_Hash_Table)
- [Chess Programming Wiki: Parallel Search](https://www.chessprogramming.org/Parallel_Search)

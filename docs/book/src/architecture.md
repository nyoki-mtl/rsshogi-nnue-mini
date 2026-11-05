# 全体構成とコードマップ

この章では、`rsshogi-nnue-mini` のソースコードがどのように分割され、実行時にどのスレッドがどのデータを所有しているかを俯瞰します。以降の章で個別のアルゴリズムを読む際の地図として参照してください。

## 1. モジュールとディレクトリの構成

エンジン本体は単一の Rust クレートで構成されており、外部から見えるモジュールは組み込み用の `api` と USI 実行用の `usi` の2つだけに絞っています。

| パス | 主な型・関数 | 役割と対応する章 |
| --- | --- | --- |
| `src/main.rs`, `src/lib.rs` | `main`, `api`, `usi` | USI 実行ファイルのエントリポイントと公開モジュール宣言 |
| `src/position.rs`<br>`src/position/search_ext.rs` | `replay`, `MoveFacts`,<br>`SearchPositionExt` | SFEN と指し手列の厳密な検証、駒取り・王手判定、手数を進めない探索用パス（null move） |
| `src/eval.rs`<br>`src/eval/nnue/` | `Evaluator`, `AOnlyNetwork`,<br>`AccumulatorStack` | 駒割り・NNUE 評価器、A-only 特徴量、遅延差分更新スタック、`.rsnn` 検証、SIMD カーネル（[NNUE評価関数](nnue.md)） |
| `src/search.rs`<br>`src/search/root.rs`<br>`src/search/negamax.rs` | `Searcher`, `SearchResult`,<br>`negamax` | 反復深化ドライバ、アスピレーション窓、ルート探索、PVS 再帰（[探索の全体像](search.md)、[alpha-beta法とPVS](search/alphabeta.md)） |
| `src/search/qsearch.rs` | `quiescence` | 駒取り・歩成り・王手回避に絞った静止探索と1手詰め判定（[静止探索](search/qsearch.md)） |
| `src/search/tt.rs` | `TranspositionTable` | 64バイト境界の lock-free 置換表と XOR 整合性検証（[置換表と千日手](search/tt.md)） |
| `src/search/ordering.rs`<br>`src/search/history.rs`<br>`src/search/see.rs` | `MovePicker`, `ContinuationHistory`,<br>`static_exchange_ge` | 段階的な手の生成、キラー手・脅威別メイン履歴・継続履歴、静的交換評価 SEE（[手の並べ替えとSEE](search/ordering.md)） |
| `src/search/pruning.rs`<br>`src/search/lmr.rs`<br>`src/search/params.rs` | `CorrectionHistory`, `LmrTable`,<br>`SearchParams` | 静的評価の補正履歴、RFP / Null Move / ProbCut / Singular Extension、LMR テーブルと調整パラメータ（[枝刈りと深さ調整](search/pruning.md)） |
| `src/search/time.rs`<br>`src/usi/go_time.rs` | `SearchDeadline`, `IterationTiming`,<br>`budget_from_go` | 持ち時間からの予算算出、soft / hard の二段階締切、`ponderhit` 起点更新（[時間管理と先読み](search/time.md)） |
| `src/search/coordinator.rs`<br>`src/search/context.rs` | `SearchCoordinator`, `SearchControl`,<br>`SearchContext` | メインワーカーと常駐補助ワーカーによる Lazy SMP 並列実行、ノード数バッチ管理（[Lazy SMPによる並列探索](search/parallel.md)） |
| `src/usi/` | `UsiSession`, `UsiSearchBridge` | 標準入出力の制御、コマンド直列化、古い探索結果の抑止（[USIコマンドとオプション](usi.md)） |
| `src/api.rs`, `src/ffi.rs`<br>`bindings/` | `Engine`, `StopHandle`,<br>`MiniEngine`, `worker.js` | Rust 同期 API、C ABI、Swift / WebAssembly ラッパー（[アプリへの組み込み](embedding.md)） |

---

## 2. スレッド構成とデータの所有権

並行バグ（「探索を止めた後に古い `bestmove` が出てしまう」「複数スレッドで盤面や評価器を壊してしまう」など）を防ぐため、このエンジンでは**入出力の制御**と**探索ワーカーの状態**を明確に切り離しています。

<svg viewBox="0 0 760 285" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="USIモードと組み込みモードのスレッド・所有権構成図">
  <defs>
    <marker id="arch-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 1 L 9 5 L 0 9 z" fill="#475569"/>
    </marker>
  </defs>
  <rect x="10" y="10" width="740" height="265" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <rect x="24" y="26" width="215" height="145" rx="8" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="131" y="47" text-anchor="middle" font-size="13" font-weight="bold" fill="#1e3a8a">USI 実行経路 (src/usi/)</text>
  <rect x="38" y="58" width="187" height="36" rx="5" fill="#ffffff" stroke="#93c5fd"/>
  <text x="131" y="80" text-anchor="middle" font-size="12" fill="#1e293b">stdin 読取スレッド (input.rs)</text>
  <rect x="38" y="112" width="187" height="46" rx="5" fill="#ffffff" stroke="#3b82f6"/>
  <text x="131" y="132" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">UsiSession (メインスレッド)</text>
  <text x="131" y="149" text-anchor="middle" font-size="11" fill="#475569">stdout 独占・状態遷移・停止回収</text>
  <line x1="131" y1="94" x2="131" y2="110" stroke="#475569" stroke-width="1.5" marker-end="url(#arch-arrow)"/>
  <rect x="24" y="185" width="215" height="74" rx="8" fill="#f0fdf4" stroke="#16a34a" stroke-width="1.5"/>
  <text x="131" y="206" text-anchor="middle" font-size="13" font-weight="bold" fill="#14532d">組み込み経路 (src/api.rs)</text>
  <text x="131" y="227" text-anchor="middle" font-size="12" fill="#1e293b">api::Engine (呼び出しスレッド)</text>
  <text x="131" y="245" text-anchor="middle" font-size="11" fill="#475569">StopHandle で別スレッド停止</text>
  <rect x="278" y="26" width="225" height="233" rx="8" fill="#fffbeb" stroke="#d97706" stroke-width="1.5"/>
  <text x="390" y="47" text-anchor="middle" font-size="13" font-weight="bold" fill="#78350f">探索コーディネータ (coordinator.rs)</text>
  <rect x="294" y="62" width="193" height="68" rx="5" fill="#ffffff" stroke="#f59e0b"/>
  <text x="390" y="83" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">MainWorker (#0)</text>
  <text x="390" y="101" text-anchor="middle" font-size="11" fill="#475569">USI時は UsiSearchBridge スレッド</text>
  <text x="390" y="117" text-anchor="middle" font-size="11" fill="#475569">時間管理・info 通知・最終手決定</text>
  <rect x="294" y="146" width="193" height="96" rx="5" fill="#ffffff" stroke="#fbbf24"/>
  <text x="390" y="168" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">PermanentHelper (#1..N-1)</text>
  <text x="390" y="187" text-anchor="middle" font-size="11" fill="#475569">各自が独立所有するもの：</text>
  <text x="390" y="204" text-anchor="middle" font-size="11" fill="#475569">Position 複製 / AccumulatorStack</text>
  <text x="390" y="220" text-anchor="middle" font-size="11" fill="#475569">History 各種 / 局所ノード数</text>
  <rect x="540" y="26" width="196" height="233" rx="8" fill="#f3e8ff" stroke="#9333ea" stroke-width="1.5"/>
  <text x="638" y="47" text-anchor="middle" font-size="13" font-weight="bold" fill="#581c87">ワーカー間の共有資源</text>
  <rect x="554" y="66" width="168" height="46" rx="5" fill="#ffffff" stroke="#c084fc"/>
  <text x="638" y="86" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">Arc&lt;TranspositionTable&gt;</text>
  <text x="638" y="103" text-anchor="middle" font-size="11" fill="#475569">Lock-free 置換表</text>
  <rect x="554" y="126" width="168" height="46" rx="5" fill="#ffffff" stroke="#c084fc"/>
  <text x="638" y="146" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">Arc&lt;AOnlyNetwork&gt;</text>
  <text x="638" y="163" text-anchor="middle" font-size="11" fill="#475569">読み取り専用の NNUE 重み</text>
  <rect x="554" y="186" width="168" height="56" rx="5" fill="#ffffff" stroke="#c084fc"/>
  <text x="638" y="206" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">SearchControl (Atomics)</text>
  <text x="638" y="223" text-anchor="middle" font-size="11" fill="#475569">stop / pondering / 合計ノード数</text>
  <line x1="225" y1="135" x2="292" y2="96" stroke="#475569" stroke-width="1.5" marker-end="url(#arch-arrow)"/>
  <line x1="239" y1="220" x2="292" y2="115" stroke="#475569" stroke-width="1.5" marker-end="url(#arch-arrow)"/>
  <line x1="487" y1="96" x2="552" y2="96" stroke="#475569" stroke-width="1.5" marker-end="url(#arch-arrow)"/>
  <line x1="487" y1="190" x2="552" y2="150" stroke="#475569" stroke-width="1.5" marker-end="url(#arch-arrow)"/>
</svg>

この設計の要点は次の3つです。

1. **標準出力の単一所有と古い結果の抑止**
   USI モードでは、`stdin` を読む専用スレッドと探索を回す `UsiSearchBridge` スレッドを分けつつ、標準出力（`stdout`）への書き込みはメインスレッドの `UsiSession` だけが行います。探索中に次の `position` や `usinewgame` が届いた場合、`UsiSession` は停止フラグを立ててからブリッジ側の `SearchEvent::Done` を最後まで受け取って破棄したうえで次のコマンドへ進むため、古い局面の `info` や `bestmove` が後から混ざることはありません。
2. **ワーカーごとの完全独立な可変状態**
   各探索ワーカーは探索開始時に `clone_for_search()` でルート局面を複製し、自分専用の盤面（`Position`）・NNUE 差分スタック（`AccumulatorStack`）・探索スタック・各種学習履歴（`History`）を持ちます。ワーカー間で共有するのは、読み取り専用の NNUE ネットワーク、lock-free 構造の置換表、および停止やノード数を管理するアトミック変数だけです。
3. **常駐ワーカープールの再利用**
   `Threads` が2以上のとき、`SearchCoordinator` は `Threads - 1` 本の補助スレッド（`PermanentHelper`）を保持し、探索のたびにスレッドを作り直さず条件変数で起床・待機させます。これにより秒読みや短時間対局でもスレッド生成のオーバーヘッドがかかりません。

---

## 3. 盤面と評価器のロックステップ更新

探索木を1手進めて戻す際、盤面（`Position`）と評価器（`Evaluator`）は常に次の順序で対になって動きます。

```text
1. position.apply_move32(mv)          // 盤面を1手進める
2. evaluator.advance(&position, mv)   // NNUE スタックに「変化した特徴の差分」だけを記録
3. 子ノードを探索（必要なノードでだけ evaluator.evaluate(&position) が呼ばれる）
4. position.undo_move32(mv)           // 盤面を1手戻す
5. evaluator.undo()                   // NNUE スタックを1段ポップする
```

`evaluator.advance()` の時点ではまだ重いベクトル演算を行わず、実際にそのノードや子孫ノードで静的評価が必要になった瞬間にまとめて差分を適用します（遅延差分更新）。枝刈りや置換表ヒットで評価値を計算せずに引き返したノードでは、NNUE の更新コスト自体がかかりません。

また、Null Move Pruning で「自分の手番をパスする」ときは、`src/position/search_ext.rs` の `do_search_null_move` を使います。この操作では盤上の駒が一切動かないため、NNUE スタックの `advance` / `undo` は呼ばず、評価時に先手視点と後手視点の参照順序を入れ替えるだけで手番反転に対応します。あわせて対局手数（`game_ply`）も変化させないことで、`MaxMovesToDraw` の判定手数がずれないようにしています。

---

## 4. コマンドや設定変更をまたぐ状態の寿命

同じ対局の次の `go` で再利用してよい情報と、ルールやモデルが変わったために消去すべき情報は、操作ごとに次のように管理されています。

| 操作 | 置換表（TT） | ワーカーの探索履歴 | NNUE 重み（`Arc`） | 補助ワーカースレッド |
| --- | --- | --- | --- | --- |
| 通常の `go` | 世代（generation）を1進めて保持 | 前回の学習値を保持 | 保持 | 待機中のまま再利用 |
| `usinewgame` / `Clear Hash` | 全スロットをゼロクリア | メイン履歴・継続履歴・補正履歴を全消去 | 保持 | 維持したまま履歴消去 |
| `USI_Hash` の変更 | 指定サイズで再確保・クリア | 保持 | 保持 | 維持したまま新 TT を参照 |
| `EnteringKingRule` / `MaxMovesToDraw` の変更 | 全スロットをゼロクリア | 全消去 | 保持 | 維持したまま履歴消去 |
| `Threads` の変更 | 全スロットをゼロクリア | 次回 `go` 時に新規作成 | 保持 | 旧プールを終了し次回再構築 |
| `EvalPackage`（モデル）の変更 | 全スロットをゼロクリア | 次回 `go` 時に新規作成 | 新モデルへ差し替え | 旧プールを即座に終了・解放 |

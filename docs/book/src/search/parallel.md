# Lazy SMPによる並列探索

マルチコア CPU を活かして探索を高速化する手法として、かつては1つのノードの候補手を複数スレッドへ細かく分配してロックで同期する方式（YBW など）が多く使われていました。しかし、同期オーバーヘッドや実装の複雑さが大きくなるという課題があります。

現代の NNUE エンジンで標準となっているのが **Lazy SMP（Lazy Symmetric Multiprocessing）** です。各スレッドはロックで協調するのではなく、**唯一の共有メモリである置換表（TT）を介して緩やかに知識を共有しながら、それぞれが独立にルート局面から反復深化を回します**。実装は `src/search/coordinator.rs`・`context.rs`・`src/search.rs` にあります。

---

## 1. メインワーカーと常駐補助ワーカーの役割分担

USI オプション `Threads`（または組み込み API の `threads`）に `N`（`1..=16`）を指定すると、1本の **MainWorker（`worker_id = 0`）** と、`N - 1` 本の **PermanentHelper（`worker_id = 1..N-1`）** が動きます。

<svg viewBox="0 0 700 195" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="Lazy SMPにおけるMainWorkerとPermanentHelperの共有・独立構造">
  <defs>
    <marker id="smp-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 1 L 9 5 L 0 9 z" fill="#475569"/>
    </marker>
  </defs>
  <rect x="10" y="10" width="680" height="175" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <rect x="26" y="26" width="215" height="66" rx="7" fill="#eff6ff" stroke="#2563eb" stroke-width="1.5"/>
  <text x="133" y="48" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">MainWorker (#0)</text>
  <text x="133" y="66" text-anchor="middle" font-size="11" fill="#334155">depth 1, 2, 3... を順当に探索</text>
  <text x="133" y="81" text-anchor="middle" font-size="10" fill="#475569">時間管理・info 出力・bestmove 決定</text>
  <rect x="26" y="104" width="215" height="66" rx="7" fill="#fffbeb" stroke="#d97706" stroke-width="1.5"/>
  <text x="133" y="126" text-anchor="middle" font-size="12" font-weight="bold" fill="#78350f">PermanentHelper (#1..N-1)</text>
  <text x="133" y="144" text-anchor="middle" font-size="11" fill="#334155">候補順ローテーション＋深さスキップ</text>
  <text x="133" y="159" text-anchor="middle" font-size="10" fill="#475569">先行して置換表を温めることに専念</text>
  <rect x="305" y="42" width="180" height="112" rx="8" fill="#faf5ff" stroke="#9333ea" stroke-width="1.5"/>
  <text x="395" y="68" text-anchor="middle" font-size="12" font-weight="bold" fill="#581c87">共有 TranspositionTable</text>
  <text x="395" y="90" text-anchor="middle" font-size="11" fill="#334155">Lock-free 64B クラスタ</text>
  <text x="395" y="110" text-anchor="middle" font-size="11" fill="#475569">あるワーカーが読んだ枝の</text>
  <text x="395" y="126" text-anchor="middle" font-size="11" fill="#475569">境界値・最善手が即座に</text>
  <text x="395" y="142" text-anchor="middle" font-size="11" fill="#475569">他ワーカーの枝刈りを助ける</text>
  <rect x="525" y="42" width="146" height="112" rx="8" fill="#f0fdf4" stroke="#16a34a" stroke-width="1.5"/>
  <text x="598" y="68" text-anchor="middle" font-size="12" font-weight="bold" fill="#14532d">探索完了時の結果</text>
  <text x="598" y="92" text-anchor="middle" font-size="11" fill="#334155">MainWorker の最終結果</text>
  <text x="598" y="110" text-anchor="middle" font-size="11" fill="#334155">＋ 全ワーカーの</text>
  <text x="598" y="128" text-anchor="middle" font-size="11" fill="#334155">合計訪問ノード数</text>
  <line x1="241" y1="59" x2="303" y2="82" stroke="#475569" stroke-width="1.5" marker-end="url(#smp-arrow)"/>
  <line x1="303" y1="72" x2="241" y2="50" stroke="#475569" stroke-width="1.5" marker-end="url(#smp-arrow)"/>
  <line x1="241" y1="137" x2="303" y2="114" stroke="#475569" stroke-width="1.5" marker-end="url(#smp-arrow)"/>
  <line x1="303" y1="124" x2="241" y2="146" stroke="#475569" stroke-width="1.5" marker-end="url(#smp-arrow)"/>
  <line x1="485" y1="98" x2="523" y2="98" stroke="#16a34a" stroke-width="1.5" marker-end="url(#smp-arrow)"/>
</svg>

どの状態を共有し、どの状態をワーカーごとに分離しているかを整理すると次のようになります。

| 状態 | 所有形態 | 理由 |
| --- | --- | --- |
| 置換表（`TranspositionTable`） | 全ワーカーで共有（`Arc`） | 探索済みの評価境界・最善手・静的評価値をロックなしで融通し合うため |
| NNUE 重み（`AOnlyNetwork`） | 全ワーカーで共有（`Arc`） | 読み取り専用の不変データであり、メモリ重複を防ぐため |
| 停止フラグ・合計ノード数 | 全ワーカーで共有（`Atomics`） | 一斉停止と `go nodes` 上限管理・USI `nodes` 報告のため |
| 盤面（`Position`）と NNUE 差分スタック | **ワーカーごとに独立** | `clone_for_search()` で複製し、競合なしで高速に進退させるため |
| キラー手・メイン履歴・継続履歴・補正履歴 | **ワーカーごとに独立** | ロック競合を避けつつ、ワーカーごとに異なる手の順序を生み出すため |
| 時間管理（Soft 締切）・`info` 送出・最終 `bestmove` | **MainWorker のみ** | 出力順序と意思決定の主体を1つに固定し、動作を安定させるため |

---

## 2. 同じ枝ばかり読まないための3つの工夫

もし全ワーカーがまったく同じ順序・同じ深さで探索を始めたら、全員が同じ局面を重複して計算するばかりで並列化の効果が薄れてしまいます。そこで意図的に探索の足並みをずらす仕組みを入れています。

1. **`depth = 1` 完了後の補助ワーカー解禁ゲート（`helpers_start`）**
   補助ワーカーは最初から一斉に走り出すのではなく、まず MainWorker が `depth = 1` を完了して置換表に最低限のルート指針を書き込んだ瞬間にゲートが開いて探索を始めます。
2. **ルート候補手の初期ローテーション**
   補助ワーカー `worker_id` は、ルート候補手リストを `worker_id % 候補数` だけ回転させた並び順から開始します。これにより探索序盤で別々のルート候補手が並列に掘り下げられます。
3. **周期的な深さスキップ（`helper_skips_depth`）**
   補助ワーカーは `depth >= 3` において **`(depth + worker_id) % 3 == 0` にあたる反復深さをスキップ**し、1つ先の深さへジャンプします。MainWorker が現在の深さを固めている間に、一部の補助ワーカーが一足先に深い枝の情報を置換表へ書き込むことで、お互いの枝刈りが噛み合いやすくなります。

MainWorker が時間上限や目標深さに達して終了すると、共有の停止フラグを立てて全補助ワーカーを速やかに帰還させ、MainWorker が確定させた最善手と全ワーカーの合計ノード数を最終結果として返します。

---

## 3. ノード数のバッチ集計と正確な上限管理（`src/search/context.rs`）

1秒間に数百万回呼ばれる `enter_node` のたびに共有の `AtomicU64` を更新すると、CPU コア間のキャッシュコヒーレンシ通信が激しいボトルネックになります。そのためノード数管理は制限の有無で2通りに切り替えています。

- **通常の探索（`max_nodes` 指定なし）**
  各ワーカーは手元の変数にノード数を貯め、**`NODE_BATCH = 256` ノードごとに1回だけ**共有カウンタへ `fetch_add` します（探索終了時に端数をまとめてフラッシュします）。これによりアトミック通信の頻度を 1/256 に抑えています。
- **ノード数制限つき探索（`go nodes <N>`）**
  残り予算がまだ十分に大きい間は `256` ノード単位で一括予約し、残り予算が少なくなったら1ノード単位の `fetch_update` に切り替えることで、マルチスレッド環境でも指定された `max_nodes` を1ノードたりとも超過しない厳密な上限管理を行います。

なお、[alpha-beta法とPVS](alphabeta.md)で触れた「ルート候補手の労力順ソート」には、他スレッドの動きに左右されないそのワーカー自身の `local_nodes` を使います。

---

## 4. ワーカープールの寿命とパニック隔離

`SearchCoordinator` が抱える補助ワーカースレッドは、通常の `go` や `usinewgame`、`USI_Hash` のサイズ変更では破棄されず、条件変数（`Condvar`）で待機したまま次の探索へ引き継がれます。スレッドプールを作り直すのは `Threads` 数が変更されたときと、`EvalPackage` でモデルが差し替えられたときだけです。

また、万一いずれかの補助ワーカーやメインワーカー内部でパニックが発生した場合でも、各スレッド境界の `catch_unwind` がそれを受け止め、共有の停止フラグを立てて残りのワーカーを安全に合流させたうえでプールを破棄・次回再構築するため、デッドロックやゾンビスレッドが残らない設計になっています。

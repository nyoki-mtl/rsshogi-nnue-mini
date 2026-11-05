# 時間管理と先読み

対局時計がある実戦では、「残り時間を使い切って時間切れ負けになること」も「時間を余らせすぎて浅い読みで悪手を指すこと」も避けなければなりません。また、評価値が急に落ち込んだ危険な局面では普段より時間を使ってでも最善の受けを探す必要があります。

持ち時間パラメータからの予算計算は `src/usi/go_time.rs`、探索中の締切判定と動的な伸縮は `src/search/time.rs` が担当しています。

---

## 1. 持ち時間からの基準予算の算出（`budget_from_go`）

USI の `go` コマンドで手番側の持ち時間（`btime` / `wtime`）、フィッシャー増秒（`binc` / `winc`）、秒読み（`byoyomi`）、残り手数目安（`movestogo`、省略時は `30` 手と仮定）が渡されると、まず1手あたりの基準予算 `budget` を計算します。

```text
raw_budget = 残り時間 / movestogo(30) + 増秒(inc) + 秒読み(byoyomi)
```

1. **実残時間による上限クリップ**：フィッシャー増秒は手を指し終えた後に加算されるため、手元の時計を超える時間を使わないよう `残り時間 + 秒読み` を上限として頭打ちにします。
2. **通信安全余裕（`MoveOverhead`）の差し引き**：GUI とのプロセス間通信や OS スケジューリングの遅れで時間切れになるのを防ぐため、`MoveOverhead`（既定 `500 ms`、ただし予算が極端に短いときは `budget / 2` まで）を差し引き、最低 `1 ms` を保証します。

---

## 2. Soft 締切と Hard 締切の二段階制御

算出した `budget` に対し、通常の持ち時間対局では性質の異なる2つの締切を設定します（`movetime` 固定時間探索や、持ち時間0で毎手一定の `byoyomi` だけで指す状態では、延長の余地がないため `soft = hard = budget` になります）。

<svg viewBox="0 0 700 150" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="Soft締切とHard締切の二段階タイムライン">
  <rect x="10" y="10" width="680" height="130" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <line x1="45" y1="74" x2="655" y2="74" stroke="#94a3b8" stroke-width="4" stroke-linecap="round"/>
  <rect x="45" y="68" width="235" height="12" rx="6" fill="#3b82f6"/>
  <rect x="280" y="68" width="265" height="12" rx="0" fill="#f59e0b"/>
  <rect x="545" y="68" width="110" height="12" rx="6" fill="#ef4444"/>
  <circle cx="45" cy="74" r="6" fill="#1e3a8a"/>
  <text x="45" y="48" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">探索開始 (0 ms)</text>
  <line x1="280" y1="32" x2="280" y2="112" stroke="#d97706" stroke-width="2" stroke-dasharray="4 3"/>
  <text x="280" y="27" text-anchor="middle" font-size="12" font-weight="bold" fill="#b45309">Soft 締切 (基準 budget × 60% × 係数)</text>
  <text x="280" y="126" text-anchor="middle" font-size="11" fill="#475569">反復深化の区切りで確認：超えていれば次の深さへ入らない</text>
  <line x1="545" y1="32" x2="545" y2="112" stroke="#dc2626" stroke-width="2"/>
  <text x="545" y="27" text-anchor="middle" font-size="12" font-weight="bold" fill="#b91c1c">Hard 締切 (最大 budget × 300%)</text>
  <text x="545" y="126" text-anchor="middle" font-size="11" fill="#be123c">探索途中でも強制中断</text>
</svg>

- **Soft 締切（基準値：`budget` の `60%`、`time_soft_permille = 600`）**
  反復深化で1つの深さ（`depth`）が完了した瞬間にだけチェックする目安ラインです。すでに次の深さを丸ごと読み切るほどの余裕はない場合に、キリよく今の深さで探索を終えて持ち時間を温存します。
- **Hard 締切（上限：`budget` の `300%`、かつ `残り時間 / 4 + 秒読み` から `MoveOverhead` を引いた値以下）**
  探索木の真っ最中（`SearchContext::enter_node`）でチェックする絶対上限です。ある深さの途中で想定外に探索木が膨らんだ場合でも、この時刻を過ぎたら直ちに探索を打ち切って時間切れを防ぎます。

---

## 3. 局面の揺れに応じた Soft 締切の伸縮（`IterationTiming`）

すべての局面で一律に `budget` の60%で切り上げるのではなく、反復深化が進むごとの「最善手の安定度」と「評価値の推移」から倍率 `scale`（`0.4`〜`2.5` 倍）を計算し、Soft 締切を動的に伸縮させます。

- **時間を短く切り上げるケース**
  - **合法手が1手しかない局面**：そもそも迷う余地がないため `scale = 0.0` とし、`depth = 1` が完了した時点で即座に指し手を返します。
  - **同じ最善手が何反復も連続して変わらない局面**：読みが安定していると判断し、連続安定回数1回につき `-0.07` ずつ倍率を下げて（下限 `0.4` 倍）持ち時間を節約します。
- **時間を延ばして粘るケース**
  - **深さが進むたびに最善手が入れ替わる局面**：減衰つきの変化カウンタ `changes` に応じて倍率を `+0.6 × changes` 引き上げ、見落としが起きやすい途中で打ち切るのを避けます。
  - **前回の深さより評価値が大きく下落した局面**：300点（`time_falling_span = 300`）下がるごとに倍率を `+1.0` 引き上げ、切り返しや受けの手がないかを Hard 締切の範囲内で深く探します。

---

## 4. 先読み（`ponder` / `ponderhit`）と WebAssembly の時計

### `go ponder` から `ponderhit` への切り替え

相手の思考時間中に予想応手の先を読む **`go ponder`** では、最初は `pondering = true` フラグが立っています。この間はたとえ Soft / Hard 締切の時刻を過ぎても時間切れによる停止を行わず、`stop` または `ponderhit` が届くまで探索を続けます。

相手が予想どおりの手を指して GUI から **`ponderhit`** が届くと、エンジンは探索木や置換表を止めずにそのまま走り続けながら、次の2つをアトミックに更新します。

1. `SearchDeadline::restart()` で探索開始時刻（経過時間0の起点）を「`ponderhit` を受け取った瞬間」へ引き直す
2. `pondering` フラグを `false` に下ろし、そこからの通常時間管理を有効化する

これにより、相手が考えている間に稼いだ探索深さを無駄にせず、自分の持ち時間だけを正確に使い切ることができます。

### WebAssembly 環境での時刻取得

`wasm32-unknown-unknown` ターゲットでは標準ライブラリの `std::time::Instant::now()` がパニックするため、`src/search/time.rs` ではホスト側の JavaScript が提供する `env.mini_now_ms`（`performance.now()`）をインポートして単調増加ミリ秒時計として利用しています。

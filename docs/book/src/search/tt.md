# 置換表と千日手

将棋では、`▲７六歩 △３四歩 ▲２六歩` と進んでも、`▲２六歩 △３四歩 ▲７六歩` と進んでも、まったく同じ盤面・持ち駒・手番に合流します。探索木は見た目上は木構造ですが、局面のつながりとしては合流を含む有向グラフです。

一度調べた局面の探索結果をハッシュ表に記録しておき、別手順から同じ局面に達したときや次の反復深化で再利用する仕組みが、`src/search/tt.rs` の **置換表（Transposition Table: TT）** です。

---

## 1. 64バイト境界クラスタと Lock-free 検証

並列探索（Lazy SMP）では複数のワーカースレッドが同時に置換表を読み書きします。アクセスのたびに Mutex でロックを取るとたちまちボトルネックになるため、`TranspositionTable` はロックを使わない **XOR 検証方式（lockless hashing）** を採っています。

### クラスタとスロットのメモリ配置

表全体は CPU のキャッシュライン長に合わせた **64バイトの `Cluster`（`#[repr(align(64))]`）** の配列です。1クラスタの中に16バイトの `Slot` が4つ並んでおり、1回のメモリアクセスで4候補をまとめて検査できます（x86_64 では子局面へ降りる直前に `_mm_prefetch` でキャッシュへ先読みします）。

各 `Slot` は2つの `AtomicU64`（`guard` と `data`）で構成されています。

- **`data`（64ビット）**：`score (i16)`・`static_eval (i16)`・`depth (u8)`・`bound (2bit)`・`generation (6bit)`・`pv (1bit)` を1ワードに詰めたもの
- **`keymove`（64ビット）**：上位32ビットに局面キーの検証値 `key_verifier`、下位32ビットに `best_move`（`Move32`）を詰めたもの
- **保存される `guard`**：`keymove` をそのまま書くのではなく、**`guard = keymove ^ data`** として XOR を取って格納します

読み出し（`probe`）のときは `guard` と `data` を `Relaxed` で読み、`keymove = guard ^ data` を復元して上位32ビットが探している局面の `key_verifier` と一致するかを確かめます。もし別スレッドの書き込みと競合して片方だけが新しい値になっていた場合、XOR で復元した検証ビットが壊れて不一致になるため、壊れたデータ（torn read）を誤って採用するのを高確率で防げます。

---

## 2. 境界の種類（Bound）と詰みスコアの正規化

alpha-beta 探索では多くのノードが途中で打ち切られるため、置換表に入るスコアが常に「真の評価値」とは限りません。そこで3種類の `Bound` を区別して保存します。

| `Bound` | 記録される場面 | 再利用（`tt_cutoff`）できる条件 |
| --- | --- | --- |
| `Exact` | 窓 `(alpha, beta)` の内側で最後まで読み切った真の値 | `score` をそのまま返せる |
| `Lower` | `score >= beta` で beta cutoff した（本当はもっと高いかもしれない下限値） | 現在の窓でも `score >= beta` なら打ち切れる（未満でも `alpha` の引き上げに使える） |
| `Upper` | どの手も `alpha` を超えなかった（本当はもっと低いかもしれない上限値） | 現在の窓でも `score <= alpha` なら打ち切れる |

主探索（`negamax`）では、記録されている `entry.depth` が現在の残り深さ `depth` 以上であり、かつ**非 PV ノード（`!pv_node`）**であるときに限り `tt_cutoff` で探索を打ち切ります。PV ノードで安易に置換表カットを許すと、読み筋（PV）が途中で途切れたり、後述する経路依存の問題が表面化しやすくなったりするためです（PV ノードでも置換表の `best_move` や `static_eval` 自体は最大限活用します）。

### 詰みスコアの `ply` 正規化（`score_to_tt` / `score_from_tt`）

詰み帯のスコア（絶対値 `>= 31,754`）は「ルートから何手目の詰みか（`MATE - ply`）」で表されています。しかし、別手順でルートから5手目に合流した局面と、7手目に合流した局面とでは `ply` が異なります。

そのため置換表へ書き込むときは **`score_to_tt`** で「この局面からあと何手で詰むか」という相対距離へ変換し（勝ちなら `+ ply`、負けなら `- ply`）、読み出すときに **`score_from_tt`** で現在の `ply` を差し引きしてルート基準のスコアへ戻します。

---

## 3. 3段階の置き換え戦略と `hashfull`

同じクラスタ（4スロット）が埋まったときにどのエントリを残すかは、次の優先順で決まります。

1. **同一局面のスロットがすでにある場合**
   新しい結果が `Bound::Exact` であるか、`depth + 2 >= stored_depth` を満たすなら内容を上書きします（このとき新しい結果に `best_move` が無ければ、既存の `best_move` を消さずに引き継ぎます）。浅い結果で上書きしない場合も、世代（`generation`）だけは最新に更新して追い出されにくくします。
2. **空きスロットがある場合**：最初の空きスロットへそのまま書き込みます。
3. **4スロットすべてが別局面で埋まっている場合**
   各スロットの「価値」を `stored_depth - 2 * age`（`age` は現在の探索世代との差）で計算し、最も古く浅いスロットを選んで置き換えます。

なお USI の `info hashfull` は、表全体を走査するコストを避けるため、先頭最大1,000クラスタ（4,000スロット）だけをサンプル調査し、「現在と同じ `generation` を持つ使用中スロットの割合」を千分率（`0..=1000`）で返しています。

---

## 4. 将棋特有の課題：千日手と置換表キー（`tt_key`）

チェスに比べ、持ち駒を再利用できる将棋では同一局面への合流や千日手が頻繁に起こります。ここで素朴に「盤面＋持ち駒＋手番」だけの Zobrist ハッシュを置換表のキーにすると、深刻な誤判定が生じます。

<svg viewBox="0 0 700 185" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="同じ盤面でも経路によって千日手状態が異なる例">
  <rect x="10" y="10" width="680" height="165" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <rect x="28" y="32" width="250" height="50" rx="7" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="153" y="53" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">経路1：普通の駒組みから到達</text>
  <text x="153" y="71" text-anchor="middle" font-size="11" fill="#334155">出現1回目 / 連続王手 0回</text>
  <rect x="28" y="102" width="250" height="50" rx="7" fill="#fff1f2" stroke="#e11d48" stroke-width="1.5"/>
  <text x="153" y="123" text-anchor="middle" font-size="12" font-weight="bold" fill="#881337">経路2：王手を繰り返して到達</text>
  <text x="153" y="141" text-anchor="middle" font-size="11" fill="#334155">出現3回目 / 連続王手 3回 (次は反則負け)</text>
  <rect x="330" y="54" width="145" height="76" rx="7" fill="#ffffff" stroke="#64748b" stroke-width="1.5"/>
  <text x="402" y="82" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">盤面・持駒・手番は</text>
  <text x="402" y="102" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">まったく同一</text>
  <rect x="515" y="42" width="155" height="100" rx="7" fill="#f0fdf4" stroke="#16a34a" stroke-width="1.5"/>
  <text x="592" y="66" text-anchor="middle" font-size="12" font-weight="bold" fill="#14532d">tt_key の解決策</text>
  <text x="592" y="86" text-anchor="middle" font-size="11" fill="#334155">Zobrist キーに</text>
  <text x="592" y="103" text-anchor="middle" font-size="11" fill="#334155">反復回数・種別・連続王手</text>
  <text x="592" y="120" text-anchor="middle" font-size="11" fill="#334155">を混ぜて別エントリ化</text>
  <line x1="278" y1="57" x2="328" y2="80" stroke="#3b82f6" stroke-width="1.5"/>
  <line x1="278" y1="127" x2="328" y2="104" stroke="#e11d48" stroke-width="1.5"/>
  <line x1="475" y1="92" x2="513" y2="92" stroke="#16a34a" stroke-width="1.5"/>
</svg>

そこで `src/search/context.rs` の `tt_key` では、`rsshogi` が持つ盤面ハッシュ（`position.key().low_u64()`）に対して、経路上の状態を FNV-1a ハッシュで折り畳んで XOR しています。

- その局面の過去の反復出現回数（`repetition_times`）と反復種別（通常・連続王手・優等・劣等）
- 先手・後手それぞれの連続王手回数（`continuous_checks`）
- `MaxMovesToDraw` が有効な（`> 0`）対局に限り、設定上限値と現在の対局手数（`game_ply`）

一方、ルートからの探索段数 `ply` はキーに含めません。`ply` を入れないことで、前の `go` や浅い深さで蓄えたエントリを次の探索でもそのままヒットさせることができます。

> **GHI（Graph History Interaction）とのトレードオフ**：`tt_key` は「その局面に到達した時点の反復状態」を区別しますが、「その局面の先でどの祖先局面を踏むと千日手になるか」という未来の履歴までは完全には表現できません。PV ノードで置換表による即座の打ち切り（`tt_cutoff`）を禁止しているのは、本筋の読みにおいてこの経路依存の誤差が直接最善手を歪めるのを抑える役割も兼ねています。

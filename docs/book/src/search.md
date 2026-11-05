# 探索の全体像

NNUE 評価関数が返すのは、あくまで「その瞬間の盤面だけを静的に見た評価値」です。一見すると駒得で優勢に見える局面でも、次の1手で王手飛車がかかったり、即詰みに討ち取られたりすることがあります。

そこで将棋エンジンは、現在の局面（ルート）から交互に指し手を進めた**ゲーム木**を展開し、お互いが最善を尽くしたときに帰着する評価値と指し手（最善応手列＝**PV: Principal Variation**）を求めます。この仕組みを担うのが `src/search.rs` と `src/search/` 配下のモジュール群です。

---

## 1. minimax から negamax へ

二人ゼロサムゲームの基本原理は「自分は評価値を最大化し、相手は評価値を最小化する」という **minimax 法**です。ただし、先手番用の最大化関数と後手番用の最小化関数を別々に書くとコードが二重になってしまいます。

このエンジンの評価関数は常に**「いま手番を持っている側から見た有利さ」**を返すため、「相手にとって `+V` 点の局面は、自分にとって `-V` 点の局面である」と言い換えられます。子ノードから返ってきたスコアの符号を反転（`-score`）するだけで、どちらの手番もすべて「自分のスコアを最大化する処理」に統一できます。これが **negamax 形式**です。

---

## 2. 反復深化（Iterative Deepening）の流れ

対局では持ち時間が限られており、最初から「深さ20まで読む」と決めて探索を始めると、途中で時間切れになったときに1手も結果が残らない恐れがあります。

そこで `src/search.rs` のドライバは、深さ1から始めて `depth = 1, 2, 3, ...`（最大 `MAX_SEARCH_DEPTH = 64`）と1段ずつ探索を深くしていく **反復深化** を行います。

<svg viewBox="0 0 740 165" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="反復深化と情報引き継ぎの流れ">
  <defs>
    <marker id="id-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 1 L 9 5 L 0 9 z" fill="#475569"/>
    </marker>
  </defs>
  <rect x="10" y="10" width="720" height="145" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <rect x="24" y="36" width="130" height="62" rx="7" fill="#ffffff" stroke="#64748b" stroke-width="1.5"/>
  <text x="89" y="59" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e293b">事前準備 (root.rs)</text>
  <text x="89" y="78" text-anchor="middle" font-size="11" fill="#475569">宣言勝ち判定・1ノード</text>
  <text x="89" y="92" text-anchor="middle" font-size="10" fill="#64748b">フォールバック手の確保</text>
  <rect x="180" y="36" width="118" height="62" rx="7" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="239" y="59" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">depth = 1</text>
  <text x="239" y="78" text-anchor="middle" font-size="11" fill="#334155">全窓探索 (-INF..INF)</text>
  <text x="239" y="92" text-anchor="middle" font-size="10" fill="#475569">完了後に補助ワーカー解禁</text>
  <rect x="324" y="36" width="126" height="62" rx="7" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="387" y="59" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">depth = 2</text>
  <text x="387" y="78" text-anchor="middle" font-size="11" fill="#334155">Aspiration 窓で探索</text>
  <text x="387" y="92" text-anchor="middle" font-size="10" fill="#475569">info 出力・soft 締切判定</text>
  <rect x="476" y="36" width="112" height="62" rx="7" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="532" y="59" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">depth = 3 .. d</text>
  <text x="532" y="78" text-anchor="middle" font-size="11" fill="#334155">置換表・履歴・ノード数</text>
  <text x="532" y="92" text-anchor="middle" font-size="10" fill="#475569">で順序を磨きながら深化</text>
  <rect x="614" y="36" width="102" height="62" rx="7" fill="#f0fdf4" stroke="#16a34a" stroke-width="1.5"/>
  <text x="665" y="59" text-anchor="middle" font-size="12" font-weight="bold" fill="#14532d">探索完了</text>
  <text x="665" y="78" text-anchor="middle" font-size="11" fill="#334155">最終 info と</text>
  <text x="665" y="92" text-anchor="middle" font-size="11" fill="#334155">bestmove 出力</text>
  <line x1="154" y1="67" x2="178" y2="67" stroke="#475569" stroke-width="1.5" marker-end="url(#id-arrow)"/>
  <line x1="298" y1="67" x2="322" y2="67" stroke="#475569" stroke-width="1.5" marker-end="url(#id-arrow)"/>
  <line x1="450" y1="67" x2="474" y2="67" stroke="#475569" stroke-width="1.5" marker-end="url(#id-arrow)"/>
  <line x1="588" y1="67" x2="612" y2="67" stroke="#475569" stroke-width="1.5" marker-end="url(#id-arrow)"/>
  <text x="380" y="132" text-anchor="middle" font-size="11" fill="#475569">浅い深さで得た「最善手・置換表・履歴スコア・前回の評価値」が次の深さの枝刈り効率を劇的に高める</text>
</svg>

「浅い深さを何度もやり直すのは無駄ではないか」と思われがちですが、alpha-beta 探索では**最初に最善手を調べられるかどうか**で探索木の大きさが桁違いに変わります。浅い反復で置換表や学習履歴が温まるおかげで、いきなり深い探索を行うよりもトータルでははるかに速くなります。

また、実装上の安全性と強さのために次の工夫を入れています。

- **1ノード予約によるフォールバック手の確保（`fallback_root_choice`）**
  `nodes 1` や開始直後の `stop` でも合法手を返せるよう、本探索の前に最小1ノードを使ってフォールバック手（置換表の手または合法手の先頭）を確保します。その際、反復深化本体に残りのノード予算（`max_nodes - 1`）を渡し、1手目を読む前に予算を使い切らないようにしています。
- **途中中断した深さの部分採用（`src/search/root.rs`）**
  ある深さの探索途中で hard 締切や `stop` がかかった場合でも、すでに1手目を読み終えて元の `alpha` を超える改善手が見つかっていたなら、その新しい最善手を採用します（ただし USI の `info depth` には、最後まで完了した深さ `completed_depth` を報告します）。
- **PV（読み筋）の再構成（`collect_pv`）**
  各反復の完了時には、確定したルート最善手から出発して置換表の `best_move` を合法性と局面重複を確認しながら辿り、最大 `MAX_PV_LENGTH = 32` 手までの読み筋を組み立てます。

---

## 3. スコア帯の設計と終局判定（`src/search/score.rs`）

通常の評価値・千日手・入玉宣言・詰みスコアが互いに衝突しないよう、整数スコアの範囲を明確に区切っています（探索木の最大手数は `MAX_SEARCH_PLY = 128` です）。

| 定数・区分 | 値・範囲 | 意味 |
| --- | --- | --- |
| `INF` | `32,767` | alpha-beta 窓の初期上限（`-INF`〜`+INF`） |
| `MATE` | `32,000` | 詰みおよび入玉宣言勝ちの基準点。ルートから `ply` 手での勝ちは `32,000 - ply`、負けは `-32,000 + ply` |
| `MATE_TT_THRESHOLD` | `31,754` | これ以上の絶対値を持つスコアを「詰み帯」として扱い、置換表への保存時に `ply` 補正を行う |
| `REPETITION_SUPERIOR` | `31,753` | 優等局面（同じ盤面で持ち駒が増えている状態）のスコア。劣等局面は `-31,753`。詰みではないため `ply` 補正の対象外 |
| 通常の評価値 | `-31,753`〜`+31,753` | NNUE（または駒割り評価）が返すスコア帯（`±MAX_NNUE_EVAL`） |
| 引き分け | `0` | 通常の千日手、および `MaxMovesToDraw` 手数上限を超えた局面 |

詰みスコアに `- ply` を付けているのは、**「どうせ勝つなら最短手数の詰みを選び、どうせ負けるなら最長手数まで粘る」**という性質を通常の大小比較だけで自然に実現するためです。

また、終局判定の優先順位には将棋特有の注意点があります。

- **千日手・手数上限を入玉宣言より先に判定する**：連続王手の千日手（反則負け）や手数引き分けが成立している局面で誤って宣言勝ちを返さないよう、まず `terminal_score`（千日手・`MaxMovesToDraw`）を調べ、その後に `declaration_score`（入玉宣言勝ち）を判定します。
- **ルートでの優等・劣等局面は打ち切らない**：探索木の途中（`ply > 0`）で優等・劣等局面に突入した場合は `±31,753` を返して枝を閉じますが、GUI から渡された開始局面（`ply == 0`）自体がすでに劣等局面だった場合は対局が継続しているため、`root_terminal_score` では終局扱いにせず通常どおり最善手を探索します。

---

## 4. 探索パートの読み進め方

ここから先の7つの章は、前の章の仕組みを前提にして順番に積み上がる構成になっています。

1. **[alpha-beta法とPVS](search/alphabeta.md)**：不要な枝を切り落とす `(alpha, beta)` 窓の基本、Principal Variation Search（ゼロ幅探索）、ルートの Aspiration Window と労力順ソートを解説します。
2. **[静止探索](search/qsearch.md)**：残り深さ0で駒取りの途中に探索を打ち切る危険（水平線効果）と、それを防ぐ `qsearch`・1手詰め判定・SEE / デルタ枝刈りを解説します。
3. **[置換表と千日手](search/tt.md)**：合流局面のメモ化、64バイト lock-free クラスタの構造、そして将棋で厄介な「手順依存の千日手と置換表の相性問題」への対策を解説します。
4. **[手の並べ替えとSEE](search/ordering.md)**：段階的に手を生成する `MovePicker`、キラー手・脅威別メイン履歴・継続履歴、駒の取り合い損得を見積もる静的交換評価（SEE）を解説します。
5. **[枝刈りと深さ調整](search/pruning.md)**：評価値の補正履歴（Correction History）、ノード単位の枝刈り・延長（RFP、Null Move、ProbCut、Singular Extension）、ループ内枝刈り、対数 LMR を体系的に整理します。
6. **[時間管理と先読み](search/time.md)**：持ち時間や秒読みから soft / hard の二段階締切を決める方法、評価値の揺れに応じた時間延長、`ponder` / `ponderhit` の制御を解説します。
7. **[Lazy SMPによる並列探索](search/parallel.md)**：置換表を共有して複数スレッドで同じルート局面を探索する Lazy SMP の仕組みと、ノード数のバッチ管理を解説します。

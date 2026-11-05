# 参考資料とライセンス

## `rsshogi` エコシステムの関連リポジトリ

`rsshogi-nnue-mini` は、盤面処理やプロトコル解析・対局運営を役割ごとに分離した `rsshogi` エコシステムの一部として開発されています。Bitboard による盤面表現や合法手生成の仕組みを詳しく知りたい場合は `rsshogi` 本体もあわせて参照してください。

- **[rsshogi](https://github.com/nyoki-mtl/rsshogi)**：盤面表現（Bitboard）、合法手・疑似合法手生成、SFEN 入出力、千日手・入玉宣言判定、1手詰めソルバ
- **[rsshogi-usi](https://github.com/nyoki-mtl/rsshogi-usi)**：USI プロトコルのコマンド・応答の型表現、パーサ、フォーマッタ
- **[rsshogi-csa](https://github.com/nyoki-mtl/rsshogi-csa)**：USI エンジンを CSA プロトコルの対局サーバーへ中継するブリッジツール
- **[ShogiArena](https://github.com/nyoki-mtl/ShogiArena)**：エンジン間の自動対局、SPRT による有意差検定、SPSA による探索パラメータ調整、Web ダッシュボード

---

## アルゴリズムの参考文献・技術資料

本書で解説した探索・評価アルゴリズムの背景や理論をさらに掘り下げたいときに役立つ文献・資料です。

### 基礎論文

- Claude E. Shannon, [“Programming a Computer for Playing Chess”](https://doi.org/10.1080/14786445008521796), *Philosophical Magazine*, Ser.7, Vol. 41, No. 314 (1950) — 評価関数と minimax 探索、静止探索の必要性を定式化した古典論文
- Donald E. Knuth and Ronald W. Moore, [“An Analysis of Alpha-Beta Pruning”](https://www.sciencedirect.com/science/article/pii/0004370275900193), *Artificial Intelligence*, Vol. 6, Issue 4 (1975) — alpha-beta 法の数学的性質と手の並び順が探索木サイズに与える影響の解析

### トピック別の解説（Chess Programming Wiki）

将棋とチェスでは持ち駒や千日手ルールに違いがあるものの、探索エンジンの基本構造の多くは共通しています。[Chess Programming Wiki](https://www.chessprogramming.org/Main_Page) では以下の項目が本書の各章に対応しています。

- [Principal Variation Search](https://www.chessprogramming.org/Principal_Variation_Search) / [Aspiration Windows](https://www.chessprogramming.org/Aspiration_Windows)（[alpha-beta法とPVS](search/alphabeta.md)）
- [Quiescence Search](https://www.chessprogramming.org/Quiescence_Search) / [Horizon Effect](https://www.chessprogramming.org/Horizon_Effect) / [Delta Pruning](https://www.chessprogramming.org/Delta_Pruning)（[静止探索](search/qsearch.md)）
- [Transposition Table](https://www.chessprogramming.org/Transposition_Table) / [Graph History Interaction](https://www.chessprogramming.org/Graph_History_Interaction)（[置換表と千日手](search/tt.md)）
- [Move Ordering](https://www.chessprogramming.org/Move_Ordering) / [Killer Heuristic](https://www.chessprogramming.org/Killer_Heuristic) / [History Heuristic](https://www.chessprogramming.org/History_Heuristic) / [Static Exchange Evaluation](https://www.chessprogramming.org/Static_Exchange_Evaluation)（[手の並べ替えとSEE](search/ordering.md)）
- [Null Move Pruning](https://www.chessprogramming.org/Null_Move_Pruning) / [Reverse Futility Pruning](https://www.chessprogramming.org/Reverse_Futility_Pruning) / [ProbCut](https://www.chessprogramming.org/ProbCut) / [Singular Extensions](https://www.chessprogramming.org/Singular_Extensions) / [Late Move Reductions](https://www.chessprogramming.org/Late_Move_Reductions)（[枝刈りと深さ調整](search/pruning.md)）
- [Lazy SMP](https://www.chessprogramming.org/Lazy_SMP)（[Lazy SMPによる並列探索](search/parallel.md)）
- [NNUE](https://www.chessprogramming.org/NNUE)（[NNUE評価関数](nnue.md)）

---

## ライセンスと配布物の方針

### ソースコードと依存クレート

`rsshogi-nnue-mini` のソースコード、ドキュメント、および SDK ラッパーは **[MIT License](https://github.com/nyoki-mtl/rsshogi-nnue-mini/blob/main/LICENSE)** のもとで公開されています。直接利用しているクレートは以下のとおりです。

| クレート | 用途 | ライセンス |
| --- | --- | --- |
| `rsshogi` (`1.7.2`) | 盤面・合法手・千日手・入玉・1手詰め | MIT |
| `rsshogi-usi` (`0.2.0`) | USI コマンド解析・整形 | MIT |
| `serde` / `serde_json` | 組み込み JSON API・`.rsnn` マニフェスト・`usi_tunables` | MIT OR Apache-2.0 |
| `serde_json_canonicalizer` | `.rsnn` 評価仕様 JSON の正規化ハッシュ検証 | MIT OR Apache-2.0 |
| `sha2` | `.rsnn` パッケージおよび内蔵モデルの SHA-256 検証 | MIT OR Apache-2.0 |

### 評価関数ファイル（`.rsnn`）の扱い

ソースリポジトリ自体には学習済みの `.rsnn` バイナリを含めず、コードとモデルデータの管理を分離しています（固定の進行度係数テーブル `src/eval/nnue/kp-progress.bin` のみソースツリーに含まれます）。すぐに動かせる評価関数内蔵バイナリや SDK が必要な場合は、[GitHub Releases](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases) の配布アーカイブを利用してください。

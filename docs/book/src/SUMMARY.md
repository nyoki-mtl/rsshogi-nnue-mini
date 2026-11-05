# Summary

[はじめに](introduction.md)

# 導入と組み込み

- [セットアップとビルド](getting-started.md)
- [USIコマンドとオプション](usi.md)
- [アプリへの組み込み](embedding.md)

# エンジンの仕組み

- [全体構成とコードマップ](architecture.md)
- [NNUE評価関数](nnue.md)
- [探索の全体像](search.md)
  - [alpha-beta法とPVS](search/alphabeta.md)
  - [静止探索](search/qsearch.md)
  - [置換表と千日手](search/tt.md)
  - [手の並べ替えとSEE](search/ordering.md)
  - [枝刈りと深さ調整](search/pruning.md)
  - [時間管理と先読み](search/time.md)
  - [Lazy SMPによる並列探索](search/parallel.md)

---

[参考資料とライセンス](references.md)

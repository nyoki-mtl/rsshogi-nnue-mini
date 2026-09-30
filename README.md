# rsshogi-nnue-mini

Rust製のUSI将棋エンジンです。将棋GUIに登録して対局できます。現在はソースコードからビルドして使います。

## 動かす

Rust 1.95以降が必要です。WindowsではMSVCツールチェーンを使います。

1. [評価ファイル 2026.09.30](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/eval-2026.09.30)から`.rsnn`をダウンロードし、リポジトリ内の`eval/model.rsnn`に名前を変えて置きます。
2. リポジトリのルートでビルドします。

   ```powershell
   cargo build --release --locked
   ```

3. `target/release/rsshogi-nnue-mini.exe`を将棋GUIに登録します。Linuxでは末尾の`.exe`を外してください。

エンジンは起動時の作業ディレクトリを基準に`eval/model.rsnn`を探します。GUIから読み込めない場合は、USIオプション`EvalPackage`に評価ファイルの絶対パスを指定してください。

詳しい手順は[動かしてみる](docs/book/src/getting-started.md)、主な設定は[USIオプション](docs/book/src/usi.md)を参照してください。

## エンジンについて

複数スレッドでの探索と先読みに対応しています。評価ファイルはソースコードには含めず、Releaseから配布しています。

[Floodgateでの対局記録](https://wdoor.c.u-tokyo.ac.jp/shogi/x/2026/player/rss-mini-84e-20260929+72a7fa5.html)では、2026年9月30日の確認時点でR3444でした。この値は当時の構成と対局条件での結果です。

## ライセンス

ソースコードはMITライセンスです。

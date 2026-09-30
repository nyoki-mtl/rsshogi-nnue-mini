# rsshogi-nnue-mini

Rust製のUSI将棋エンジンです。将棋GUIに登録して対局できます。

[rsshogi](https://github.com/nyoki-mtl/rsshogi)で盤面と指し手を扱い、[rsshogi-usi](https://github.com/nyoki-mtl/rsshogi-usi)でUSIコマンドを解析・整形します。CSA接続には[rsshogi-csa](https://github.com/nyoki-mtl/rsshogi-csa)、ローカル対局やパラメータ調整には[ShogiArena](https://github.com/nyoki-mtl/ShogiArena)を組み合わせます。

## 動かす

Rust 1.95以降が必要です。WindowsではMSVCツールチェーンを使います。実行ファイルはソースコードからビルドします。

[評価ファイル 2026.09.30](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/eval-2026.09.30)から`.rsnn`をダウンロードします。通常版と、評価ファイルを内蔵するAVX2版の二通りがあります。

### 通常版

ダウンロードしたファイルをリポジトリ内の`eval/model.rsnn`に名前を変えて置き、ルートでビルドします。

```powershell
cargo build --release --locked
```

`target/release/rsshogi-nnue-mini.exe`を将棋GUIに登録します。Linuxでは末尾の`.exe`を外してください。評価ファイルは起動時の作業ディレクトリを基準に探すため、GUIから読み込めない場合はUSIオプション`EvalPackage`に絶対パスを指定します。

### 評価ファイル内蔵版（Windows、AVX2）

WindowsとAVX2対応CPUでは、PowerShell 7から次を実行すると、評価ファイルを内蔵した`dist/rsshogi-nnue-mini-embedded-avx2.exe`を作れます。この版は起動時に外部の`.rsnn`を必要としません。Floodgateで使ったのも内蔵版です。

```powershell
pwsh -NoProfile -File scripts/build_embedded_avx2.ps1 -ModelPath "C:\path\to\rsshogi-nnue-mini-eval-20260930.rsnn"
```

詳しい手順は[動かしてみる](docs/book/src/getting-started.md)、主な設定は[USIオプション](docs/book/src/usi.md)を参照してください。

## エンジンについて

複数スレッドでの探索と先読みに対応しています。評価ファイルはGitには含めず、Releaseから配布しています。

[Floodgateでの対局記録](https://wdoor.c.u-tokyo.ac.jp/shogi/x/2026/player/rss-mini-84e-20260929+72a7fa5.html)では、2026年9月30日の確認時点でR3444でした。この値は当時の構成と対局条件での結果です。

## ライセンス

ソースコードはMITライセンスです。

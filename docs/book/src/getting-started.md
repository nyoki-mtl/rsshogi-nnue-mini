# 動かしてみる

## 必要なもの

- Rust 1.95以降
- WindowsではMSVCツールチェーン
- 対応する512幅・Threatなし・PSQTありSFNNv15の`.rsnn`評価ファイル（このリポジトリには同梱していない）
- `just`のレシピを使う場合は、`just`とPowerShell 7

以下はWindowsのPowerShellで、リポジトリのルートから実行する例である。

## 評価ファイルを準備する

Floodgateで使用した84エポックの`.rsnn`は、配布条件を確認したうえで、リリース時にGitHub Releasesの別assetとして配布する予定である。
リリース前は対応する`.rsnn`を別途用意し、リポジトリ内の`eval/model.rsnn`へ置く。
公開リポジトリには学習済みモデルも作成ツールも含まれない。
別の場所に置く場合は、`isready`の前に`setoption name EvalPackage value <path>`を送る。
期待するファイルのSHA-256を別途記録し、配置後に照合する。

```powershell
Get-FileHash -Algorithm SHA256 ./eval/model.rsnn
```

この章の直接実行では、作業ディレクトリはリポジトリのルートである。
通常ビルドと調整用ビルドは、ともにその下の`eval/model.rsnn`を既定で読む。
実行ファイルの場所を基準に探すわけではないため、GUIから起動する場合も作業ディレクトリを確認する。
リリースから取得した評価ファイルを使う場合も、リリースに記載されたSHA-256と照合する。

## ビルドして起動する

```powershell
cargo build --release
./target/release/rsshogi-nnue-mini.exe
```

Linuxでは、実行ファイル名の末尾の`.exe`を外す。

## USIで探索を確認する

起動したエンジンへ、次の順に入力する。
まず`usi`を送り、オプション一覧に続く`usiok`を待つ。

```text
usi
```

次に`isready`を送り、評価ファイルの読み込み完了を示す`readyok`を待つ。

```text
isready
```

`readyok`を確認してから、対局の初期化、局面の設定、探索開始を送る。

```text
usinewgame
position startpos moves 7g7f 3c3d
go depth 3
```

`info`に続いて`bestmove`が返れば、この探索は完了している。
途中で止める場合は`stop`を送り、その応答の`bestmove`を待つ。
終了するときは、最後に`quit`を送る。
`go`の直後に`quit`まで一括入力すると、探索結果を受け取る前に終了することがある。

複数ワーカーで動かす場合は、探索開始前に`setoption name Threads value 2`を送る。
オプションの詳細は[USI](usi.md)、停止処理の構成は[実行時の構成](architecture.md)を参照。

## 評価ファイルを読み込めない場合

読み込みに失敗すると、`info string NNUE load error: ...`を出し、`readyok`を返さない。
この状態で`go`を送ると`bestmove resign`を返す。
エラーを確認してファイルを配置し直し、再度`isready`を送る。

## 対局ツール用の実行ファイルを用意する

配布・性能測定用の実行ファイルは`dist/`を使う。`target/release/`はCargoのビルド中間出力である。
手元の`.rsnn`を内蔵したAVX2版は、次のレシピで`dist/rsshogi-nnue-mini-embedded-avx2.exe`に作る。

```powershell
just release-avx2-embedded "C:\path\to\model.rsnn"
```

モデルのSHA-256は`dist/rsshogi-nnue-mini-embedded-avx2.json`に記録され、起動時にも内蔵バイト列と照合される。
AVX2対応CPUでは`EvalPackage`を設定せずに使える。
モデルをGitへ追加したり、`dist/eval/`へ別途コピーしたりする必要はない。

外部ファイルを読む通常版は、次のレシピで`dist/rsshogi-nnue-mini.exe`へ配置できる。

```powershell
just stage-engine
```

通常版がコピーするのは実行ファイルだけである。
対局ツール向けの設定例では、直接実行時と違って作業ディレクトリを`dist/`にする。
その設定で通常版を使う場合は、`dist/eval/model.rsnn`を別途配置する。
SPSAでパラメータを調整する場合は、[ShogiArenaの手順](operations/shogiarena.md)に従って調整用ビルドを用意する。

## テストと文書のビルド

```powershell
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`--workspace`は、`.rsnn`の読み取りと検証を担う`crates/rsnn-package`のtestも含める。
通常の`cargo test`では、実`.rsnn`を必要とする検証は実行しない。fixtureを用いた検証方法は[評価関数](nnue.md)を参照。
文書は`mdbook`を導入してから、次のコマンドでビルドできる。

```powershell
mdbook build docs/book
```

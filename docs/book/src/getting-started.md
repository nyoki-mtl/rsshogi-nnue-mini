# 動かしてみる

## 準備

Rust 1.95以降を用意します。WindowsではMSVCツールチェーンが必要です。

[評価ファイル 2026.09.30](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/eval-2026.09.30)から`rsshogi-nnue-mini-eval-20260930.rsnn`をダウンロードし、リポジトリ内の`eval/model.rsnn`に名前を変えて置きます。`eval`フォルダーがなければ作成してください。

## ビルド

リポジトリのルートで実行します。

```powershell
cargo build --release --locked
```

Windowsの実行ファイルは`target/release/rsshogi-nnue-mini.exe`です。Linuxでは末尾の`.exe`を外します。

## 将棋GUIに登録する

ビルドした実行ファイルをUSIエンジンとして登録します。エンジンが評価ファイルを見つけられるよう、作業ディレクトリをリポジトリのルートに設定してください。GUIで作業ディレクトリを指定できない場合は、エンジンの設定画面で`EvalPackage`に`model.rsnn`の絶対パスを指定します。

起動時に`readyok`が返らない場合は、評価ファイルの場所と`EvalPackage`の値を確認してください。エンジンは読み込みに失敗した理由を`info string NNUE load error: ...`として出力します。

## コマンドで動作を確認する

実行ファイルを起動して、次を一行ずつ入力します。`isready`への`readyok`を待ってから次へ進んでください。

```text
usi
isready
usinewgame
position startpos
go depth 3
```

`bestmove`が返れば探索は完了です。最後に`quit`を入力します。

評価ファイルの配置とハッシュの確認方法は[評価ファイル](nnue.md)、スレッド数などの設定は[USIオプション](usi.md)を参照してください。

# 評価ファイル

エンジンを動かすには`.rsnn`評価ファイルが必要です。[評価ファイル 2026.09.30](https://github.com/nyoki-mtl/rsshogi-nnue-mini/releases/tag/eval-2026.09.30)からダウンロードできます。

通常版の配置先は、エンジンを起動する作業ディレクトリ内の`eval/model.rsnn`です。ダウンロードしたファイルをこの名前に変更して置いてください。別の場所に置く場合は、USIオプション`EvalPackage`に絶対パスを指定します。

[内蔵版のビルド](getting-started.md#ビルド)では、指定した評価ファイルが実行ファイルに含まれます。内蔵版では起動時の外部ファイルは不要です。

ダウンロードしたファイルのSHA-256は次の値です。

```text
a4a61c91f85a1ee1eb67cb7c6483c66fdb6ed7c2832d3184901e2faf89dfb398
```

Windowsでは次のコマンドで照合できます。

```powershell
Get-FileHash -Algorithm SHA256 ./eval/model.rsnn
```

読み込みに失敗するとエンジンは`readyok`を返しません。まずファイルの配置とパスを確認してください。

# USIオプション

将棋GUIのエンジン設定画面、または`isready`より前の`setoption`コマンドで指定します。

| オプション | 既定値 | 用途 |
| --- | --- | --- |
| `EvalPackage` | `eval/model.rsnn` | 評価ファイルのパス。別の場所に置いた場合は絶対パスを指定 |
| `Threads` | 1 | 探索に使うスレッド数（1〜16） |
| `USI_Hash` | 16 | 置換表に使うメモリ（1〜1024 MiB） |
| `USI_Ponder` | false | GUIが先読みを使う場合に有効化 |
| `MoveOverhead` | 500 | 通信などに備えて一手の持ち時間から差し引く余裕（ミリ秒） |
| `EnteringKingRule` | `CSARule27` | 入玉宣言の規則 |
| `MaxMovesToDraw` | 0 | 指定手数を超えた局面を引き分けとする。0は無効 |

たとえば評価ファイルを別の場所に置く場合は、次のように設定します。

```text
setoption name EvalPackage value C:\shogi\model.rsnn
isready
```

全オプションと選択できる値は、エンジンへ`usi`を送ると表示されます。評価ファイルが読み込めない場合は[評価ファイル](nnue.md)を参照してください。

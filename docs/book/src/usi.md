# USIオプション

将棋GUIのエンジン設定画面、または`isready`より前の`setoption`コマンドで指定します。

| オプション | 既定値 | 用途 |
| --- | --- | --- |
| `Threads` | 1 | 探索に使うスレッド数（1〜16） |
| `USI_Hash` | 16 | 置換表に使うメモリ（1〜1024 MiB） |
| `USI_Ponder` | false | GUIが先読みを使う場合に有効化 |
| `MoveOverhead` | 500 | 通信などに備えて一手の持ち時間から差し引く余裕（ミリ秒） |
| `EnteringKingRule` | `CSARule27` | 入玉宣言の規則 |
| `MaxMovesToDraw` | 0 | 指定手数を超えた局面を引き分けとする。0は無効 |
| `EvalPackage` | 内蔵版は`@default`、それ以外は`eval/model.rsnn` | 読み込む`.rsnn`のパス。`@default`は内蔵モデルを選ぶ |

全オプションと選択できる値は、エンジンへ`usi`を送ると表示されます。
`@default`を使えるのは`embedded-rsnn`付きのモデル内蔵版だけです。
`Clear Hash`と探索パラメータの調整用オプションは、`tuning`付きのビルドだけに表示されます。
`Clear Hash`は置換表と探索履歴を消去します。通常版では`usinewgame`で同じ初期化を行います。
NNUEの出力スケールは16で固定されており、`FV_SCALE`オプションはありません。

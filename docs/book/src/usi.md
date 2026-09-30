# USI

**USI**は、GUIと将棋エンジンが標準入出力を通じてコマンドと応答をやり取りするプロトコルである。
初回起動の例は[動かしてみる](getting-started.md)を参照。

## 対応コマンド

| コマンド | 本エンジンの動作 |
| --- | --- |
| `usi` | エンジン情報、オプション一覧、`usiok`を返す |
| `isready` | 評価ファイルを読み込み、成功時に`readyok`を返す |
| `setoption` | オプションを設定する |
| `usinewgame` | 進行中の探索結果を破棄し、置換表と探索履歴を初期化する |
| `position` | 初期局面またはSFENから指し手を再生する |
| `go` | 現在の局面で探索を開始する |
| `stop` | 探索を止め、最終結果を一度だけ返す |
| `ponderhit` | 先読みの結果を利用する探索へ移る |
| `gameover` | 進行中の探索を止め、結果を破棄する |
| `quit` | 探索を止め、ワーカーの終了を待って終了する |
| `go mate` | `checkmate notimplemented`を返す |

`go depth`の指定値は1〜64に収める。
時間やノード数の指定がなく、`infinite`でも`ponder`でもない`go`は、深さ4を上限とする。
`go nodes 0`は、返す手と評価値を確保するため1ノードとして扱う。
`searchmoves`を指定するとルートの候補手を制限できる。
`position`で受理するSFENは、物理的な駒数と探索用の手数の範囲に収まり、相手玉を取れる合法手を含まない局面に限る。
手数は65407以下とし、探索用に128 plyを確保する。

## 先読みと停止

`USI_Ponder`は、GUIや中継が先読みを利用するかどうかを示すオプションである。
探索モードそのものは`go ponder`で決まる。
`go ponder`では、`ponderhit`または`stop`を受け取るまで`bestmove`を返さない。
深さやノード数の上限で先に探索を終えた場合は、結果を保留する。

ponder中は時間切れによる停止を保留し、`ponderhit`を受けた時点から、`go`で決めた時間予算を計り直す。
進行中の探索を止める必要はない。
ShogiHomeが送る時刻引数付きの`ponderhit`も同じ扱いで、引数から時間予算を再計算するわけではない。
`go infinite`の結果も、`stop`を受け取るまで保留する。

`bestmove`の直前には、返す手に対応する深さ、評価値、ノード数、読み筋を最終`info`として出力する。
読み筋の2手目が合法と確認できた場合は、`bestmove`にponder候補を付ける。
詰み評価は`score mate N`、通常評価は`score cp N`で出力する。
ルートで勝敗が決まっている場合は、距離0の符号を表すため、勝ちは`score mate +`、負けは`score mate -`とする。

評価値は手番側の視点である。
棋譜や対局ツールへ転記した値は、そのツールの保存形式に依存する。
詰みと通常評価の種別を確認するときは、USIの通信記録を参照する。

## 実行時オプション

| 名前 | 既定値 | 設定範囲と意味 |
| --- | --- | --- |
| `USI_Hash` | 16 | 共有置換表のサイズ。1〜1024 MiB |
| `Threads` | 1 | メインを含む探索ワーカー数。1〜16 |
| `MoveOverhead` | 500 | 一手の時間予算から引く安全余裕。0〜5000 ms |
| `USI_Ponder` | false | GUIや中継による先読み利用の指定 |
| `EvalPackage` | 通常版: `eval/model.rsnn`、内蔵版: `@default` | 読み込む512幅・ThreatなしSFNNv15 `.rsnn`のパス。内蔵版の`@default`は実行ファイル内のモデル |
| `EnteringKingRule` | `CSARule27` | 下表の入玉規則 |
| `MaxMovesToDraw` | 0 | SFENの手数がこの値を超えた局面を引き分けとする。0で無効、最大65407 |

| `EnteringKingRule`の値 | 意味 |
| --- | --- |
| `NoEnteringKing` | 入玉宣言勝ちを使わない |
| `CSARule24` / `CSARule27` | 24点法 / 27点法 |
| `CSARule24H` / `CSARule27H` | それぞれの駒落ち用規則 |

通常ビルドと調整用ビルドは、ともに`.rsnn`と既定の`eval/model.rsnn`を使う。
通常ビルドは`FV_SCALE=21`に固定する。
調整用ビルド（`--features tuning`）は探索パラメータ35個、時間管理の定数5個、`FV_SCALE`、`Clear Hash`を追加でオプション一覧に出す。
駒の価値を表す8個のパラメータは、どちらのビルドでもUSIからは変更できない。

`USI_Hash`、`Threads`、入玉規則、手数上限の変更は進行中の探索を止め、古い結果を出さずに適用する。
`MoveOverhead`は次の`go`から適用するため、設定時に進行中の探索は止めない。
`Clear Hash`は探索を止めて、置換表と探索履歴を消去する。
`usinewgame`も同じ初期化を行う。
時間の配分と締切の詳細は[時間管理](search/time.md)を参照。

## 調整対象の一覧を取得する

調整用ビルドだけが、独自コマンド`usi_tunables`に応答する。
応答は`info string shogiarena_tunables_json `に続く`shogiarena.usi_tunables.v1`形式のJSONと、終端の`usi_tunablesok`である。
JSONには探索パラメータ35個、時間管理の定数5個、`FV_SCALE`の計41個について、既定値、範囲、更新スケジュールを含める。
通常ビルドはこのコマンドに応答しない。

各パラメータは`go`開始時に確定し、その探索中は同じ値を使う。
SPSAで動かす項目は、設定ファイルの`select`で選ぶ。
実行方法は[ShogiArenaの手順](operations/shogiarena.md)を参照。

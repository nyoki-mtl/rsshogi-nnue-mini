# USIコマンドとオプション

将棋 GUI とエンジンは **USI（Universal Shogi Interface）** と呼ばれる行単位のテキストプロトコルでやり取りします。このエンジンでは `rsshogi-usi` クレートでコマンドの構文解析と整形を行い、`src/usi/session.rs` の `UsiSession` が状態遷移と出力順序を管理しています。

## 対局開始から終了までの流れ

典型的な USI セッションは次の順序で進みます。

```text
GUI → エンジン                     エンジン → GUI
-------------------------------------------------------------------------
usi                           ──►  id name / id author / option ... / usiok
setoption name Threads value 4
setoption name USI_Hash value 64
isready                       ──►  (評価関数を読み込み) readyok
usinewgame
position startpos moves 7g7f
go btime 60000 wtime 60000 ... ──► info depth 1 score cp ... pv ...
                                   info depth 2 score cp ... pv ...
                                   bestmove 3c3d ponder 2g2f
quit
```

- **`setoption` は `isready` の前に送る**のが基本です。特に `EvalPackage` を変更した後は、次の `isready` で新しい `.rsnn` パッケージが読み込まれます。
- 探索中に新しい `position`、`usinewgame`、`gameover`、`quit` が届いた場合、エンジンは進行中の探索を内部で停止・回収してから次の処理へ移ります。このとき中断した古い探索の `bestmove` は出力しません。
- `stop` が届いた場合だけは、その時点で確保できている最善手を `bestmove` として必ず1回返します。

## USIオプション一覧

エンジンへ `usi` を送ると表示される標準オプションは次の7項目です。

| オプション | 型 | 既定値 | 範囲・選択肢 | 役割 |
| --- | --- | --- | --- | --- |
| `USI_Ponder` | check | `false` | `true` / `false` | GUI が先読み（ponder）を使う意思を通知する標準項目（実際の先読み動作は `go ponder` で開始） |
| `EvalPackage` | string | 内蔵版：`@default`<br>通常版：`eval/model.rsnn` | ファイルパスまたは `@default` | 読み込む `.rsnn` 評価関数の指定。`@default` は `embedded-rsnn` ビルドの内蔵モデルを表す |
| `EnteringKingRule` | combo | `CSARule27` | `NoEnteringKing` / `CSARule24` / `CSARule24H` / `CSARule27` / `CSARule27H` | 入玉宣言勝ちの判定ルール（やねうら王互換の選択肢名） |
| `MaxMovesToDraw` | spin | `0` | `0`〜`65407` | 指定手数を超えた局面を引き分け（0点）として扱う。`0` は手数上限なし |
| `USI_Hash` | spin | `16` | `1`〜`1024` | 置換表（TT）に確保するメモリ容量（MiB） |
| `Threads` | spin | `1` | `1`〜`16` | 探索ワーカーのスレッド数（1本がメイン、残りが Lazy SMP の補助ワーカー） |
| `MoveOverhead` | spin | `500` | `0`〜`5000` | 通信や GUI 処理の遅延に備え、1手の思考予算から差し引く安全余裕（ミリ秒） |

> **オプション変更時の内部状態の扱い**
> - `USI_Hash` を変更すると置換表を指定サイズで作り直し、待機中ワーカーの参照も新しい表へ付け替えます。
> - `EnteringKingRule` や `MaxMovesToDraw` を変更すると、前提ルールが変わるため置換表と探索履歴を消去します。
> - `Threads` を変更すると、次の `go` の開始時にワーカープールを指定スレッド数で作り直します。
> - `EvalPackage` を変更すると、古いワーカープールと評価器参照をその場で解放し、置換表もクリアします。
> - NNUE の出力スケール（除数 `16`）はエンジン内部で固定されており、`FV_SCALE` オプションは設けていません。

## 主なコマンドの仕様

### `position`（局面の指定）

平手初期局面（`position startpos`）または4フィールドの SFEN（`position sfen <盤面> <手番> <持ち駒> <手数>`）に続けて、任意の指し手列（`moves ...`）を受け付けます。

`src/position.rs` の `replay` は、入力された局面と各指し手に対して次の検証を行い、不正な場合は現在の局面を壊さずに `info string position error: ...` を返します。

- 先手玉・後手玉がそれぞれちょうど1枚ずつ存在すること
- 盤上と持ち駒の合計枚数が物理的な駒数上限（歩18、香・桂・銀・金4、角・飛2、玉以外の合計38枚）を超えないこと
- 現手番の合法手に「相手玉を取る手」が含まれないこと（王手放置の非合法局面を拒否）
- ルートの手数が `65407`（`u16::MAX - MAX_SEARCH_PLY`）以下であること

### `go`（探索の開始）

`go` には次の制限を組み合わせて指定できます。

| 指定 | 動作 |
| --- | --- |
| `btime` / `wtime`、`binc` / `winc`、`byoyomi`、`movestogo` | 持ち時間・フィッシャー増秒・秒読み・残り手数から1手の時間予算と soft / hard 締切を計算する（[時間管理と先読み](search/time.md)） |
| `movetime <ms>` | 指定ミリ秒（安全余裕差し引き後）を使い切る固定時間探索を行う |
| `depth <1..64>` | 反復深化の最大深さを指定する（上限64） |
| `nodes <n>` | 全ワーカー合計の訪問ノード数上限を指定する |
| `infinite` | `stop` が届くまで時間無制限で探索し、`bestmove` の出力を保留する |
| `ponder` | 相手番での先読みとして開始し、`ponderhit` で自分の時計の起点を引き直す |
| `searchmoves <m1> <m2> ...` | ルートで探索する候補手を指定した合法手だけに絞る |

時間・ノード数・`infinite`・`ponder`・`depth` のいずれも付かない素の `go` に対しては、暴走を防ぐため深さ4（`depth 4`）を既定の上限として探索します。また、詰将棋専用探索（`go mate`）は未実装のため `checkmate notimplemented` を返します。

### `info` と `bestmove`（探索結果の出力）

反復深化の各深さが完了するたび、および `bestmove` の直前に `info` 行を出力します。

- `depth`：完了した反復深化の深さ（depth 1 完了前に止まった場合は `0`）
- `score cp <値>` または `score mate <手数|+|- >`（詰み手数または距離0の `+` / `-`）：手番側から見た評価値。詰み帯（絶対値 `31,754` 以上）に入ったスコアは詰みまでの手数へ変換し、入玉宣言勝ちなど距離0の勝敗は `+` / `-` で表します
- `nodes` / `time` / `nps`：訪問ノード数、経過ミリ秒、1秒あたりのノード数
- `hashfull`：置換表の先頭最大1,000クラスタから概算した現世代スロットの使用率（千分率 `0..=1000`）
- `pv`：置換表から再構成した最大32手の読み筋。2手目がある場合は `bestmove <1手目> ponder <2手目>` として先読み候補も添えます（合法手がない場合は `bestmove resign`、ルートで入玉宣言勝ちが成立している場合は `bestmove win`）

## 調整・計測用のビルド機能（`tuning` / `stats`）

実験やパラメータ調整を行う際は、Cargo feature を付けてビルドすることで専用のコマンド・出力が加わります。

- **`--features tuning`（パラメータ調整ビルド）**
  - `usi` の応答に `Clear Hash` ボタンオプション（`usinewgame` と同じく置換表と探索履歴を消去）と、`src/search/params.rs` で定義された **51個の探索・時間管理パラメータ**（`SearchAspirationWindow`、`SearchLmrDivisor`、`TimeSoftPermille` など）が `spin` オプションとして追加されます。
  - 独自コマンド `usi_tunables` を送ると、[ShogiArena](https://github.com/nyoki-mtl/ShogiArena) の SPSA 調整器が読み取る JSON マニフェスト（各パラメータの最小・最大・初期値・摂動幅 `c_end`）を出力し、`usi_tunablesok` を返します。
- **`--features stats`（統計カウンタビルド）**
  - 主探索・静止探索のノード数、TT ヒット率、どの段階の手で beta cutoff が起きたか、null move / ProbCut / LMR / singular extension の試行回数と成功回数など34項目のアトミックカウンタ（`src/search/context.rs` の `stats` モジュール）を集計し、標準エラー出力へ `stats main_nodes=...` の形式で書き出します。通常ビルドではカウンタ加算自体がコンパイル時に消去されるため、速度への影響はありません。

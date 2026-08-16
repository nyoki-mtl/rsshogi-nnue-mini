# USI

## 対応command

- `usi` / `isready` / `setoption`
- `usinewgame` / `position`
- `go` / `stop` / `ponderhit`
- `gameover` / `quit`
- `go mate`は`checkmate notimplemented`

`USI_Ponder`をadvertiseし、通常探索の`bestmove`には探索済みで合法性を再確認したponder候補を付ける。
`go ponder`を受けた場合は`ponderhit`または`stop`まで`bestmove`を抑止する。
`ponderhit`は探索を停止せず、ponder状態の解除と時間予算の起点の引き直しを行う。
ShogiHomeが送る時刻引数付きの`ponderhit`も同じ扱いである。
node制限はponder中も数える。
time制限はponder中の到達判定を保留し、`ponderhit`の時点から予算を計り直す。
`go ponder`は相手の手番に走らせる探索なので、`go ponder`の時刻で締切を固定すると、相手が長考しただけで自分の思考時間を失う。
`go infinite`も`stop`まで`bestmove`を返さない。
`bestmove`の直前には、最終的に返すdepth、score、nodes、PVを同じ`info`として再送する。
詰み評価は通常評価値へ混ぜず、符号付きの`score mate N`で出力する。
rootですでに勝敗が決まっている場合は距離0に符号を保持できないため、勝ちは`score mate +`、負けは`score mate -`で出力する。

USIのscoreは手番側視点である。
ShogiArenaが出力するKIFの`**評価値=`も指し手側視点であり、KIF欄は整数だけを保持するため、USIで`score mate N`だった値もKIF上では319xxの内部値になる。
mate/cpの種別を確認するときはUSI transcriptを参照する。

## runtime option

- `USI_Hash`：共有TTのMiB数。既定は16、範囲は1から1024
- `Threads`：mainを含む探索worker数。既定は1、範囲は1から16
- `MoveOverhead`：GUIや中継との往復のために、各`go`の予算から引くms数。既定は500、範囲は0から5000
- `USI_Ponder`：GUIまたはbridgeがponder経路を使うかを示すcheck
- `EnteringKingRule`：CSA 24点法、27点法とその駒落ち版を選べる入玉宣言勝ちの規則で、既定はCSA 27点法
- `MaxMovesToDraw`：探索中にこの手数を超えた局面を引き分けとする。`0`で無効、範囲は0から65407

`EnteringKingRule`と`MaxMovesToDraw`のoption名と値は、やねうら王に合わせている。

通常ビルドはstandard NNUE、`eval/nn.bin`、`FV_SCALE=24`を固定する。
tuning binaryは評価値parameter 8個、探索parameter 12個、`UseNNUE`、`EvalFile`、`FV_SCALE`、`Clear Hash`を追加で広告する。
`Clear Hash`はSPSAのvariant間で置換表を初期化するために使う。

`go depth`は64以下に制限する。
`position`は、探索できる手数と物理的な駒数の範囲に収まるSFENを受理する。
`go nodes 0`は、着手と評価値が対応する必須fallbackを作るため1 nodeとして扱う。
時間予算は残り時間、増秒、秒読みから求め、`MoveOverhead`を安全余裕として差し引く。
反復深化は通常、予算の60%を過ぎた時点で新しいiterationの開始を止める。
`movetime`と純秒読みでは予算の全体を使う。
予算と締切の設計は[時間管理](search/time.md)で追う。

`USI_Hash`とruntime規則の変更は進行中の探索をcancelし、stale resultを公開せずに適用する。
`usinewgame`もTTを消去する。

## tunable manifest

`--features tuning`で作成したtuning binaryだけが、独自command`usi_tunables`へShogiArenaの`shogiarena.usi_tunables.v1` JSONと`usi_tunablesok`を返す。
manifestは探索parameter 12個と`FV_SCALE`を対象にし、parameterは`go`開始時にsnapshotされる。

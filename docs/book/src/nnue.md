# 評価関数（`.rsnn`）

miniは512幅・Threatなし・PSQTありのSFNNv15 `.rsnn` packageを使用する。評価ファイルはGitには含めず、配布条件を確認したうえでリリース時に別assetとして配布する予定である。内蔵版をビルドした場合は、指定したpackageが実行ファイルに含まれる。

## 読み込み

通常ビルドの既定のパスは、実行時の作業ディレクトリから見た`eval/model.rsnn`である。`embedded-rsnn`ビルドでは、実行ファイル内のpackageを表す`@default`が既定値となる。内蔵版では外部ファイルを配置する必要がない。別のpackageで試す場合は、`isready`の前に次のUSIコマンドを送る。

```text
setoption name EvalPackage value C:\path\to\model.rsnn
isready
```

読み込み時にcontainerのdigest、評価仕様、tensorの名前・型・形状と数値範囲の証明、`kp-progress.bin`を検証する。
未対応の形式や破損したpackageでは`readyok`を返さず、`go`には`bestmove resign`を返す。
`EvalPackage`を変更すると読み込み済みモデルと探索履歴を破棄する。

package内部の検証は破損と仕様違いを検出するが、期待する学習runのモデルかどうかは判定できない。
利用者は期待するpackageのSHA-256を別に記録し、配置後に照合する。

## 推論

HalfKA_hm direct特徴を双方の視点で抽出し、KP進行度で8個の出力層から1つを選ぶ。
512幅の量子化feature transformer、二段の全結合層、PSQT差分を整数演算で評価する。
参照consumerと同じ評価値を出す`FV_SCALE`は16で、通常ビルドの既定値は21である。
探索の枝刈りmarginと合わせて尺度を調整した結果、表示される通常評価の数値は尺度16の場合より約24%小さくなる。

探索中は特徴を毎回抽出し直さない。着手ごとに変わる特徴（動いた駒と、取った駒が持ち駒になる分）だけを記録し、評価するときにまとめてaccumulatorへ反映する。自玉が動くとその視点の特徴番号がすべて変わるため、自玉のマスごとに最後に作ったaccumulatorを覚えておき、そこからの駒配置の差分で作り直す。

## ベクトル命令

feature transformerの加減算、二つの半分の積によるpooling、全結合層の積和を、ビルド時に選んだ命令で実行する。どの経路もscalar実装と同じ整数値を返す。

poolingの出力は0が多いため、AVX2経路の全結合層は、4入力ごとのまとまりのうち0でないものの位置を先に表引きで並べ、その位置だけを積和する。
着手による差分は視点ごとに1組か2組なので、その数を固定した経路でaccumulatorを更新する。

| ビルド | 経路 |
| --- | --- |
| x86-64、`-C target-feature=+avx2` | AVX2 |
| aarch64（iOS、Android、Apple Silicon） | NEON。全結合層はCPUが`dotprod`を持てば`sdot` |
| 上記以外 | scalar |

aarch64ではNEONが必ず使えるため、追加の指定なしでNEON経路になる。`dotprod`（Armv8.2のint8内積命令）は起動後にOSへ問い合わせて判定するので、同じ実行ファイルが対応端末では`sdot`を、非対応端末では従来のNEON命令を使う。対象端末がすべて対応していると分かっている場合は、`-C target-feature=+dotprod`で判定を省ける。この指定をした実行ファイルは非対応CPUで不正命令により停止する。iOSとAndroid向けのcompile確認は次で行う。

```powershell
just check-mobile
```

NEON経路の数値一致はQEMU上のaarch64で確認している。iPhoneやAndroid実機での速度とメモリは未測定である。

## 実モデル検証

試験学習packageをローカルに用意した場合、fixture付きの検証は次で実行できる。現在のfixture testは2026年9月の100ステップ試験packageのdigestに固定されている。

```powershell
$env:RSSHOGI_RSNN_FIXTURE = (Resolve-Path ./path/to/pilot.rsnn).Path
cargo test pilot_package_evaluates_startpos -- --ignored
```

この試験は試験学習packageの読み込みと局面評価の一致を確認するものである。
84エポックの本学習packageは、学習時に固定版のrevision不一致で検査が失敗した。
固定版を揃えた後の再検査では、package digest、評価仕様、revisionの照合と、それぞれの不一致を拒否する検査を通過した。
miniでは本学習packageの5局面の評価値が参照実装と一致し、実モデルを使った6,000回のランダム操作（着手・取り消し・null手）で差分更新と全再計算の一致を確認した。
この検査はFloodgateでの棋力やモバイル実機の速度を保証しない。

リリース候補のpackageのSHA-256は`a4a61c91f85a1ee1eb67cb7c6483c66fdb6ed7c2832d3184901e2faf89dfb398`である。
同じpackageを用意した場合、mini側の実モデル検証を次で再実行できる。

```powershell
$env:RSSHOGI_RSNN_RELEASE_CANDIDATE = (Resolve-Path ./path/to/model.rsnn).Path
cargo test release_candidate_matches_reference_consumer -- --ignored
cargo test release_candidate_incremental_evaluation_matches_full_refresh -- --ignored
```

検証の経緯と対局結果は[開発時の測定結果](verification.md)を参照。

# 厳密な読み込みと水匠5互換性

`nn.bin`は他人が作ったbyte列である。
これを読み込んで探索の心臓部に据える以上、「開けたら動いた」で信用するわけにはいかない。
形式が少しでも想定と違えば読み込みを拒否し、読み込めた場合だけ互換を名乗る。
loaderはこの方針で書かれている。

## 検証する項目

loaderは`nn.bin`の先頭から順に、次をすべて検証する。

| 項目 | 期待値 |
| --- | --- |
| version | `0x7AF32F16` |
| network hash | `0x3E5AA6EE` |
| architecture文字列 | `HalfKP(Friend)[125388->256x2]`から始まる完全一致の記述子 |
| FeatureTransformer hash | `0x5D69D7B8` |
| network body hash | `0x63337156` |
| 層のshape | `125388 -> 256x2 -> 32 -> 32 -> 1`に固定 |
| 末尾 | 余剰byteが0であること |

architecture文字列は部分一致ではなく完全一致で照合する。
長さが一致しない入力は、記述子を読み込んだり誤り表示へechoしたりする前に拒否する。
このほか、fileの大きさは中身を読む前にmetadataで検査して128 MiBを上限とし、各読み取りはoffsetの溢れと途中終端（truncation）を検査する。

ここまで厳密にする理由は、失敗の質にある。
形式違いのfileを寛容に読むと、失敗は「読み込みエラー」ではなく「なんとなく弱い評価関数」として現れる。
その調査は読み込み拒否の何倍も高くつく。

## 読み込み失敗はfail-closed

通常ビルドは`isready`のときに、実行時の作業ディレクトリにある`eval/nn.bin`を読み込む。
失敗した場合は`info string NNUE load error: ...`を出して`readyok`を返さない。
その後に`go`が来てもmaterial評価へ黙って切り替えたりせず、`bestmove resign`でfail-closedする。

読み込み失敗時に停止することで、対局結果が指定したNNUEによるものだと確認できる。
material評価へ切り替わった対局が、水匠5を使った測定として記録される事態を防ぐためである。

## 「水匠5互換」の根拠

読み込めることと、公式実装と同じ評価値を出すことは別の主張である。
後者は外部parity testで確認している。

外部parity testでは、digestを固定した`nn.bin`と公式YaneuraOu実行ファイルを使い、開始局面の評価値`39`が一致することを確認している。
同じtestで、各操作後の差分更新とfull refreshも照合する。
再実行時は、公式releaseから取得したartifactを環境変数で指定する。

```powershell
$env:RSSHOGI_NNUE_MINI_TEST_NETWORK = (Resolve-Path eval/nn.bin).Path
cargo test external_suisho5_network_parity_and_incremental -- --ignored --nocapture
```

環境変数がない場合、このtestは互換性PASSを装わずskipされる。
fixtureなしでgreenになるtestは、互換の根拠として数えない。

## 取得元とdigest

水匠5の配布条件は、このリポジトリのMIT Licenseとは別である。
networkは[公式release](https://github.com/yaneurao/YaneuraOu/releases/tag/suisho5)から取得する。

| artifact | SHA-256 |
| --- | --- |
| `Suisho5.7z` | `6734E3A3D28E67B9206C3442F6D10F16148138327DFF811CADEDFCF581F79809` |
| 展開後の`nn.bin` | `768068F0D534A0603A5D38BCD143DE6BBCA820D5F1C95A14D40863E5B7892D76` |

releaseが更新された場合は、URL、内容、digestを再確認する。

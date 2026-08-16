# 量子化推論とAVX2

standard NNUEの推論は、浮動小数点を一切使わない。
重みは学習後に整数へ量子化されており、推論も整数演算だけで進む。
整数にする利点は速さだけではない。
同じ入力に対して、どのCPUでもbit単位で同じ評価値が出る。
この決定性が、後述するkernel実装の検証と、公式実装とのparity確認を可能にしている。

## 各層の整数演算

層ごとの型と演算は次のとおりである。

| 段階 | 重みと定数 | 演算 | 出力 |
| --- | --- | --- | --- |
| FeatureTransformer | 重みi16、biasi16 | 立った特徴量の重みベクトルをi16で加減算 | i16 × 256（視点ごと） |
| ClippedReLU | なし | 0〜127へclamp | u8 × 512（両視点を連結） |
| 隠れ層1（512→32） | 重みi8、biasi32 | biasとdot productをi64で合算、6bit右shift、0〜127へclamp | u8 × 32 |
| 隠れ層2（32→32） | 重みi8、biasi32 | 同上 | u8 × 32 |
| 出力層（32→1） | 重みi8、biasi32 | biasとdot productをi64で合算、FV_SCALEで除算 | i32（±31,753へclamp） |

途中の合算をi64で持つのは、敵対的な重みでも溢れないようにするためである。
正規の`nn.bin`なら i32 でも実用上は溢れないが、loaderが受理したbyte列に対して算術が未定義な経路を残さない、という方針でここは広めに取っている。

6bitの右shiftは量子化の逆操作で、重みが学習時の値の64倍で格納されていることに対応する。
最後のFV_SCALE除算まで済ませた値が、手番側から見たcentipawn相当の評価値になる。

## kernelは実行時に選ぶ

この推論の内側loopは4種類しかない。
accumulatorへのi16加算と減算、両視点をまとめる0〜127へのclamp、そしてu8とi8のdot productである。
既定のビルド（`just release`）はこの4つについてscalar実装とAVX2実装の両方をbinaryへ含め、起動したCPUのAVX2対応を見て実行時に選ぶ。

実行時に選ぶ理由は配布の単純さにある。
一つのbinaryがAVX2のない機械でもそのまま動き、選ばれた経路はNNUE読み込み時の`info string`に`scalar`または`avx2`として表示される。

AVX2対応CPU専用のbinaryも作れる。

```powershell
just release-avx2
```

こちらは`-C target-feature=+avx2`でcrate全体をビルドするため、コンパイラもAVX2を前提に最適化できる。
配布には実行時dispatch版、AVX2対応CPU上の計測には専用binaryを使い分けられる。

## kernelの検証

AVX2実装は、scalar実装をoracleとするbit-exact testを持つ。
clamp、i16の加減算、u8とi8のdot productのそれぞれについて、境界値（i16の両端、clamp境界の126〜128など）を含む入力で両実装の完全一致を確かめる。
これは「二つの実装が同じ関数である」ことの確認であって、NPS向上の証明ではない。
速さの主張は対局と計測の側で別に立てる。

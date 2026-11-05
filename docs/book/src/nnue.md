# NNUE評価関数

探索アルゴリズムが「数手先まで読んだ結果、どの変化を選ぶか」を決める頭脳だとすれば、末端の局面を見て「いま先手と後手のどちらが何点くらい有利か」を数値化する目にあたるのが**評価関数**です。

`src/eval.rs` の `Evaluator` は、テスト用の単純な駒割り評価（歩100・香300・桂320・銀450・金550・角800・飛1000・成りボーナス350）と、実戦用の **NNUE（Efficiently Updatable Neural Network）** 評価器を切り替えられるラッパーになっており、どちらも**手番側から見た整数スコア（`-31,753..=31,753`）**を返します。

---

## 1. なぜ「A-only + 進行度バケット」なのか

将棋の NNUE では長らく「自玉の位置 × 他の駒の位置（HalfKP など）」を入力特徴にとる方式が主流でした。玉との位置関係を直接表現できる反面、入力次元が10万次元規模に膨らむためモデルファイルが数十 MB になり、また**玉が1マス動くたびにアキュムレータを最初から全再計算しなければならない**という特徴があります。

これに対し、`rsshogi-nnue-mini` が採用している評価仕様 **`aonly1024_i8_progress8_v1`** は、次の3つの工夫で「高速な差分更新」「約 3.26 MB の軽量サイズ」「序中終盤の感覚の両立」を図っています。

1. **玉を特別扱いしない A-only 特徴量（2,344次元）**
   玉を含むすべての駒を単独の特徴として符号化します。どの合法局面でも ON になる特徴は最大40個で、**玉が動く手も他の駒と同じ「特徴1個の差し替え」として差分更新できる**ため、探索中に1,024次元の全再計算が発生しません。
2. **`i8` 重みによる省メモリ化（パッケージ全体で約 3.26 MB）**
   最も大きい第1層の重み `[2344, 1024]` を `i8`（約 2.40 MB）で保持し、加減算の瞬間にだけ `i16` アキュムレータへ拡幅します。CPU キャッシュに収まりやすく、WebAssembly やモバイルアプリへの内蔵にも向いています。
3. **進行度（KP progress）に応じた8本のヘッド切り替え + PSQT**
   A-only だけでは薄くなりがちな「序盤の駒組みか、終盤の寄せ合いか」の区別を補うため、固定の進行度テーブルから局面の進行度バケット `k`（`0..7`）を算出し、8組用意した全結合層スタックと8バケットの駒位置価値（PSQT）を切り替えて評価します。

---

## 2. 入力特徴量のエンコーディング（`src/eval/nnue/features.rs`）

ネットワークは「先手から見た盤面」と「後手から見た盤面」の2つの視点を並行して計算します。後手視点ではマス番号を `80 - sq` で180度回転し、自駒と相手駒を入れ替えることで、先後でまったく同じ重みテーブルを共有します。

1視点あたりの入力は合計 **2,344次元** で、盤上特徴と持ち駒特徴に分かれています。

- **盤上特徴（`0..2267`：2,268次元）**
  自駒（`owner = 0`）と相手駒（`owner = 1`）のそれぞれについて、14種類の駒種（歩・香・桂・銀・金・角・飛・玉・と・成香・成桂・成銀・馬・龍）× 81マスを `(owner * 14 + kind) * 81 + sq` で一意の番号に対応させます。
- **持ち駒特徴（`2268..2343`：76次元）**
  自駒38スロット・相手駒38スロット（歩18・香4・桂4・銀4・金4・角2・飛2）を**単調（unary）表現**で並べます。たとえば歩を3枚持っているときは「3枚目」だけを ON にするのではなく、「1枚目・2枚目・3枚目」の3つの特徴すべてを ON にします。こうしておくと、駒を取って持ち駒が1枚増えたときも、駒を打って1枚減ったときも、変化する特徴が常に1個だけで済みます。

---

## 3. ネットワーク構造と整数推論（`src/eval/nnue/network.rs`）

1局面の評価値が算出されるまでのデータの流れは次のとおりです。浮動小数点数は一切使わず、すべて整数演算とビットシフトだけで完結します。

<svg viewBox="0 0 760 290" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="aonly1024_i8_progress8_v1 のネットワーク構造と推論フロー">
  <defs>
    <marker id="nn-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 1 L 9 5 L 0 9 z" fill="#475569"/>
    </marker>
  </defs>
  <rect x="10" y="10" width="740" height="270" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <rect x="24" y="34" width="132" height="222" rx="8" fill="#eff6ff" stroke="#3b82f6" stroke-width="1.5"/>
  <text x="90" y="60" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">A-only 入力特徴</text>
  <text x="90" y="80" text-anchor="middle" font-size="11" fill="#1e293b">2,344 次元</text>
  <text x="90" y="98" text-anchor="middle" font-size="11" fill="#475569">(最大 40 個が ON)</text>
  <line x1="38" y1="118" x2="142" y2="118" stroke="#bfdbfe"/>
  <text x="90" y="142" text-anchor="middle" font-size="11" fill="#1e293b">盤上 2,268 + 持駒 76</text>
  <text x="90" y="162" text-anchor="middle" font-size="11" fill="#475569">先手視点 / 後手視点</text>
  <text x="90" y="210" text-anchor="middle" font-size="11" fill="#0f766e">玉移動も通常の</text>
  <text x="90" y="226" text-anchor="middle" font-size="11" fill="#0f766e">1特徴差分として更新</text>
  <rect x="184" y="28" width="170" height="72" rx="7" fill="#ffffff" stroke="#0284c7" stroke-width="1.5"/>
  <text x="269" y="49" text-anchor="middle" font-size="12" font-weight="bold" fill="#0c4a6e">特徴変換器 (FT)</text>
  <text x="269" y="67" text-anchor="middle" font-size="11" fill="#334155">重み i8 [2344, 1024]</text>
  <text x="269" y="84" text-anchor="middle" font-size="11" fill="#334155">先後 i16 アキュムレータ</text>
  <rect x="380" y="28" width="156" height="72" rx="7" fill="#ffffff" stroke="#0284c7" stroke-width="1.5"/>
  <text x="458" y="49" text-anchor="middle" font-size="12" font-weight="bold" fill="#0c4a6e">Pairwise Pooling</text>
  <text x="458" y="67" text-anchor="middle" font-size="11" fill="#334155">前半512 × 後半512 / 512</text>
  <text x="458" y="84" text-anchor="middle" font-size="11" fill="#334155">[手番 512, 相手 512] u8</text>
  <rect x="184" y="116" width="170" height="60" rx="7" fill="#fefce8" stroke="#ca8a04" stroke-width="1.5"/>
  <text x="269" y="138" text-anchor="middle" font-size="12" font-weight="bold" fill="#713f12">KP 進行度集計</text>
  <text x="269" y="157" text-anchor="middle" font-size="11" fill="#475569">kp-progress.bin → k ∈ 0..7</text>
  <rect x="184" y="192" width="170" height="64" rx="7" fill="#f0fdf4" stroke="#16a34a" stroke-width="1.5"/>
  <text x="269" y="214" text-anchor="middle" font-size="12" font-weight="bold" fill="#14532d">8バケット PSQT</text>
  <text x="269" y="232" text-anchor="middle" font-size="11" fill="#334155">重み i32 [2344, 8]</text>
  <text x="269" y="247" text-anchor="middle" font-size="11" fill="#475569">(stm[k] - nstm[k]) / 2</text>
  <rect x="380" y="116" width="224" height="92" rx="7" fill="#faf5ff" stroke="#9333ea" stroke-width="1.5"/>
  <text x="492" y="136" text-anchor="middle" font-size="12" font-weight="bold" fill="#581c87">バケット k 用の全結合スタック (×8組)</text>
  <text x="492" y="155" text-anchor="middle" font-size="11" fill="#334155">fc0 (1024→32) ──► P7 (二乗+CReLU → 64)</text>
  <text x="492" y="173" text-anchor="middle" font-size="11" fill="#334155">fc1 (64→32)   ──► P6 (二乗+CReLU → 64)</text>
  <text x="492" y="192" text-anchor="middle" font-size="11" fill="#334155">fc2 (concat 128→1) + (z0[30] - z0[31])</text>
  <rect x="628" y="136" width="108" height="120" rx="7" fill="#fff1f2" stroke="#e11d48" stroke-width="1.5"/>
  <text x="682" y="159" text-anchor="middle" font-size="12" font-weight="bold" fill="#881337">最終スコア合成</text>
  <text x="682" y="180" text-anchor="middle" font-size="11" fill="#334155">positional</text>
  <text x="682" y="196" text-anchor="middle" font-size="11" fill="#334155">+ linear</text>
  <text x="682" y="216" text-anchor="middle" font-size="11" font-weight="bold" fill="#1e293b">÷ 16</text>
  <text x="682" y="238" text-anchor="middle" font-size="10" fill="#475569">clamp(±31,753)</text>
  <line x1="156" y1="64" x2="182" y2="64" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
  <line x1="156" y1="146" x2="182" y2="146" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
  <line x1="156" y1="224" x2="182" y2="224" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
  <line x1="354" y1="64" x2="378" y2="64" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
  <line x1="458" y1="100" x2="458" y2="114" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
  <line x1="354" y1="146" x2="378" y2="146" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
  <line x1="604" y1="168" x2="626" y2="168" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
  <line x1="354" y1="232" x2="626" y2="232" stroke="#475569" stroke-width="1.5" marker-end="url(#nn-arrow)"/>
</svg>

各ステージの計算は次の順序で進みます。

1. **ペアワイズ乗算プーリング（`1024 i16 × 2` → `1024 u8`）**
   各視点の1,024次元アキュムレータ `a` を前半 `0..511` と後半 `512..1023` に分け、それぞれ `0..=255` にクランプしたうえで掛け合わせて `512` で割ります。
   `pool[i] = (clamp(a[i], 0, 255) * clamp(a[i + 512], 0, 255)) / 512`
   これにより特徴同士の掛け合わせ（2次の非線形性）を表現しながら次元を512に半減させ、`[手番側 512, 相手番側 512]` の順に連結した計1,024個の `u8` ベクトル `x` を作ります。
2. **進行度バケット `k`（`0..7`）の決定**
   エンジンに内蔵された固定テーブル `kp-progress.bin`（`RSKPPRG2` 形式、約 497 KB）から先手玉・後手玉それぞれの視点で進行度スカラー（`Q16.16` 固定小数点）を集計し、その合計値を7つの閾値 `[-127527, -71998, -33477, 0, 33478, 71999, 127528]` と照合して8段階のバケット番号 `k` を選びます。
3. **二乗つき活性化とスキップ結合を持つ全結合層（バケット `k` を使用）**
   選ばれたスタック `k` の重みを使って `fc0`（`1024 → 32`）・`fc1`（`64 → 32`）・`fc2`（`128 → 1`）を通します。中間層の活性化関数 `Ps(z)` は、通常の CReLU（`clamp(z >> s, 0, 127)`）の前に二乗項（`clamp(z^2 >> (2s + 7), 0, 127)`）を並べた64次元出力（`fc0` 後は `s = 7`、`fc1` 後は `s = 6`）です。最終層の `fc2` は2つの中間出力を連結した128次元を受け取り、さらに第1層の末尾2素子 `z0[30] - z0[31]` を直接足し込むスキップ経路を持っています。
4. **PSQT（駒位置価値）との合成**
   ニューラルネット側の出力 `positional = u * 9600 / 16384` と、入力特徴から直接引いた8バケット PSQT の手番差 `linear = (psqt_stm[k] - psqt_nstm[k]) / 2` を足し合わせ、出力スケール `16` で割って `[-31753, 31753]` に収めます（整数除算の端数は0方向への切り捨て）。

---

## 4. 遅延差分更新の仕組み（`src/eval/nnue/accumulator.rs`）

探索中に何百万局面も評価できる最大の理由は、親局面と子局面のあいだで「変化した特徴だけ」を足し引きする差分更新と、計算を本当に必要になるまで遅らせる **遅延評価（Lazy materialization）** にあります。

- **1手あたりの差分は最大2トランジション**
  駒を取らない移動や駒打ちは「元の特徴を1つ引き、移動先の特徴を1つ足す」の1トランジション、駒取りはそれに「取られた盤上駒を引き、持ち駒スロットを1つ足す」が加わる2トランジションで表現できます。A-only 特徴量なので、玉が動いた場合も1トランジション（玉で駒を取れば2トランジション）のままです。
- **使う局面でだけ計算するスタック構造**
  `AccumulatorStack::push`（`evaluator.advance`）は、その手で変わった特徴番号の組（`BaseFeatureDelta`）と玉マスだけをスタックに積み、`computed = false` の印を付けて即座に戻ります。もしその先が置換表ヒットや枝刈りで引き返せば、1,024次元の加算・減算は1回も実行されません。実際に `evaluate()` が呼ばれたときに初めて、直近の `computed = true` な祖先フレームまで遡り、途中の差分を順方向にまとめて適用します。
- **KP 進行度スカラーの最小限の再走査**
  バケット選択用の `kp-progress` スカラーだけは玉の位置を基準にしているため、玉が動かなかった視点は `i32` の加減算だけで更新し、玉が動いた視点に限り盤面と持ち駒（最大38アトム）を走査して再計算します。1,024次元ベクトルの再計算ではなくスカラー1個の集計であるため非常に軽量です。

---

## 5. `.rsnn` パッケージ検証とビット完全一致の SIMD

評価関数ファイル（`.rsnn`）は `src/eval/nnue/package/` で読み込まれ、探索に使われる前に厳格な検証を通ります。

- パッケージ全体の SHA-256 ダイジェストと、評価仕様（`aonly1024_i8_progress8_v1` の正規化 JSON）のハッシュ照合
- 全テンソルの型・要素数・バイト長の一致確認と、特徴変換器の `i8` 重みに `-128` が含まれないことの検査
- **オーバーフロー不在の静的証明**：合法局面で同時に ON になりうる最大40個の特徴（盤上・持ち駒の枚数制約を満たす最悪の組み合わせ）について、`i16` アキュムレータおよび `i32` PSQT が型の上下限を絶対に超えないことをロード時に計算して確かめます

このロード時証明によって、推論・更新カーネルの中で飽和加算ではなく通常の折り返し演算（`wrapping_add` / `wrapping_sub`）を使っても数学的に厳密な和と一致することが保証されます。その結果、`src/eval/nnue/simd/` にある3種類のバックエンド——汎用 `scalar`、x86_64 向け `avx2`、ARM64 向け `neon`（対応 CPU では `dotprod` 命令も併用）——のどれで動かしても、**1ビットのずれもなく完全に同一の評価値**が得られます。

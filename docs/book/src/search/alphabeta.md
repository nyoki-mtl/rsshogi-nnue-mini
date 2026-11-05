# alpha-beta法とPVS

合法手が平均80通りほどある将棋では、すべての枝を愚直に読む minimax 法のままでは数手先で局面数が爆発してしまいます。**評価結果（最善手とスコア）を一切変えずに、結果に影響しないことが確定した枝だけを読み飛ばす**基本アルゴリズムが **alpha-beta 法**であり、それをさらに推し進めたのが **PVS（Principal Variation Search）** です。

実装は主に `src/search/negamax.rs`、`src/search/root.rs`、`src/search.rs` にあります。

---

## 1. `(alpha, beta)` 窓と beta cutoff の原理

negamax 探索の各ノードは、常に **`(alpha, beta)`** という2つの境界値（探索窓）を受け取ります。どちらも「現手番側から見た点数」です。

- **`alpha`（確保済みの下限）**：「別の選択肢を選べば、少なくともこの点数以上になることがすでに分かっている」というライン。これ以下の手は選ぶ理由がありません。
- **`beta`（相手が許す上限）**：「自分の点数が `beta` 以上になるなら、1手前の相手はそもそもこの局面へ進んでこない（別の手を選ぶ）」というライン。

したがって、候補手を順番に調べていく途中で **`score >= beta`** となる手が1つでも見つかった瞬間、「この局面は相手にとって損すぎるため実現しない」と確定し、**残りの候補手を1手も読まずに探索を打ち切る**ことができます。これを **beta cutoff（fail-high）** と呼びます。

<svg viewBox="0 0 700 225" width="100%" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="alpha-beta探索でbeta cutoffが起きる例">
  <rect x="10" y="10" width="680" height="205" rx="10" fill="#f8fafc" stroke="#cbd5e1"/>
  <circle cx="350" cy="44" r="22" fill="#eff6ff" stroke="#2563eb" stroke-width="2"/>
  <text x="350" y="49" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">親</text>
  <text x="462" y="42" font-size="12" fill="#1e293b">手Aで +120 を確保済み → alpha = +120</text>
  <text x="462" y="59" font-size="11" fill="#475569">(子ノードへは反転窓 beta = -120 として渡る)</text>
  <line x1="332" y1="58" x2="180" y2="112" stroke="#2563eb" stroke-width="2"/>
  <line x1="368" y1="58" x2="500" y2="112" stroke="#475569" stroke-width="2"/>
  <rect x="115" y="112" width="130" height="44" rx="7" fill="#eff6ff" stroke="#2563eb" stroke-width="1.5"/>
  <text x="180" y="131" text-anchor="middle" font-size="12" font-weight="bold" fill="#1e3a8a">手A の結果</text>
  <text x="180" y="148" text-anchor="middle" font-size="12" fill="#1e293b">親視点 +120</text>
  <rect x="430" y="112" width="150" height="44" rx="7" fill="#fffbeb" stroke="#d97706" stroke-width="1.5"/>
  <text x="505" y="131" text-anchor="middle" font-size="12" font-weight="bold" fill="#78350f">手B (相手番ノード)</text>
  <text x="505" y="148" text-anchor="middle" font-size="11" fill="#475569">上限 beta = -120</text>
  <line x1="470" y1="156" x2="415" y2="186" stroke="#16a34a" stroke-width="2"/>
  <line x1="505" y1="156" x2="505" y2="186" stroke="#dc2626" stroke-width="1.8" stroke-dasharray="4 3"/>
  <line x1="540" y1="156" x2="595" y2="186" stroke="#dc2626" stroke-width="1.8" stroke-dasharray="4 3"/>
  <text x="410" y="203" text-anchor="middle" font-size="12" font-weight="bold" fill="#15803d">応手b1: -50 (≧ -120!)</text>
  <text x="552" y="203" text-anchor="middle" font-size="12" font-weight="bold" fill="#b91c1c">b2, b3... は読まずに打ち切り (cutoff)</text>
</svg>

子ノードへ降りるときは手番が入れ替わるため、親の窓 `(alpha, beta)` は符号と上下を入れ替えた **`(-beta, -alpha)`** として渡されます。

上の図では、親がすでに手Aで `+120` を確保しています。続く手Bを調べたところ、相手の最初の応手 b1 で相手視点 `-50`（＝親視点 `+50`）になりました。相手にはもっと良い応手（親にとって `+50` よりさらに悪い手）があるかもしれませんが、少なくとも手Bが `+120` に届かないことはこの瞬間に確定します。そのため b2 以降はすべて省略できます。

ここから分かるとおり、**最も有力な応手（b1）を最初に並べられるほど早く cutoff が起きる**ため、[手の並べ替え](ordering.md)が探索速度の生命線になります。

---

## 2. Principal Variation Search（PVS / ゼロ幅探索）

手の並べ替えが十分に機能しているなら、各ノードで**最初に調べる手（`index == 0`）がそのまま最善手であり、2番目以降の手（`index > 0`）はすべて `alpha` 以下に終わる**可能性が高くなります。

そこで PVS（`src/search/negamax.rs` および `src/search/root.rs`）では、1番目と2番目以降で窓の幅を使い分けます。

1. **1番目の手（`index == 0`）**：通常の窓 `(-beta, -alpha)` でしっかり探索し、基準となるスコア `alpha` を確定させます。
2. **2番目以降の手（`index > 0`）**：「この手はどうせ `alpha` を超えないはずだ」という仮説を立て、幅が1しかない**ゼロ幅窓（null window）`(-alpha - 1, -alpha)`** で軽く検査（scout）します。
   - **仮説どおり `score <= alpha` だった場合**：正確な点数が何点かを知る必要はないため、そのまま次の手へ進みます。窓が極限まで狭いので、内部で大量の cutoff が起きて高速に終わります。
   - **仮説が外れて `score > alpha` になった場合**：1番目の手より良い手が見つかった可能性があるため、`score < beta`（ルートでは常に真）であれば本来の広い窓 `(-beta, -alpha)` で**再探索（re-search）**して正確な評価値を求めます。

コードの中では、`beta - alpha > 1` である本筋のノードを **PV ノード（`pv_node = true`）**、幅1のゼロ幅探索中にあるノードを **非 PV ノード（`zero_window = !pv_node`）** として区別しています。非 PV ノードでは正確な点数の代わりに「閾値を超えるかどうか」だけを判定すればよいため、次章以降の置換表カットや枝刈りもより積極的に適用できます。

---

## 3. ルートの Aspiration Window（`src/search.rs`）

反復深化の `depth = 1` では手がかりがないため全窓 `(-INF, +INF)` で探索しますが、`depth >= 2` では「1つ前の深さの評価値 `previous_score` から大きくは動かないだろう」と予測できます。

そこでルート探索の初期窓も最初から全窓にはせず、前回の評価値を中心とした狭い **Aspiration Window（期待窓）** から始めます。

- **初期幅の適応調整（`adaptive_aspiration_delta`）**
  基本幅は `aspiration_window = 88` です。ただし直近の深さで評価値が激しく動いている局面では窓を外しやすいため、評価値変動の指数移動平均 `aspiration_change` に応じて初期の半幅 `delta` を `88`〜`264`（基本幅の1〜3倍）の間で自動的に広げます。
- **片側ずつの段階的拡大（`widen_aspiration_window`）**
  探索結果が `alpha` 以下に落ちた（fail-low）場合は下側だけを `score - delta` へ広げ、`beta` 以上に突き抜けた（fail-high）場合は上側だけを `score + delta` へ広げてから `delta` を2倍にし、窓の中に収まるまでルート探索をやり直します。反対側の境界を動かさないことで、再探索中の無駄な枝の広がりを抑えています。

---

## 4. ルート候補手の「探索労力」による並べ替え（`src/search/root.rs`）

ルート局面（`ply == 0`）での手の順序には、通常のノードとは異なる `order_root_moves` を使います。

1. 前回の反復（または置換表）で得られた最善手を先頭に置きます。
2. 2番目以降のルート候補手は、直前の深さで**その手の部分木につぎ込んだローカル訪問ノード数（`local_node_count`）が多い順**に並べ替えます。

なぜ評価値順ではなくノード数順なのでしょうか。PVS では2番目以降の手はゼロ幅探索で「`alpha` 以下」とだけ判定されるため、正確な評価値順位は分かりません。しかし、相手にとって受けにくく多くの変化を読まされた手（＝将来逆転して最善手に浮上しやすい手）ほど、打ち切るまでに多くのノード数を消費しています。労力のかかった手を前に寄せておくことで、次の深さでの並び順が安定しやすくなります。

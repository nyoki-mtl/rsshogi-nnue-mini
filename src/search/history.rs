//! 静かな手の履歴ヒューリスティクスを担う。
//!
//! main history(手番×from/to)をgravity式で更新し、値は±`HISTORY_MAX`へ
//! 自然に飽和する。βカット手には正のbonus、先に試して失敗した静かな手には
//! bonusに設定倍率を掛けたペナルティを与える。テーブルは対局中`go`をまたいで持続し、
//! 探索開始ごとに`age`で半減、`usinewgame`で破棄される。
//!
//! continuation history(1手前・2手前、piece-toキー)は、並べ替えの独立した層を作らず、
//! main historyへ`SearchContinuationWeight`の重みで加算する。重みが0なら寄与しない。
//! 直前の手は捕獲の有無で行を分け、交換中とそれ以外の手順を混ぜない。
//! 浅い探索では表が疎く、単独の層にすると一度の観測に引きずられるため。
//!
//! 静的評価のcorrection historyも同じ寿命でここに置く。

use rsshogi::board::Position;
use rsshogi::types::{Bitboard, Color, MOVE_NONE, Move32, PieceType};

use crate::params::SearchParams;

/// gravity更新の飽和上限。`|bonus| <= HISTORY_MAX`なら値はこの範囲を出ない。
pub(super) const HISTORY_MAX: i32 = 16_384;

/// βカットより先に試して失敗した静かな手を覚える上限。超過分は捨てる。
const MAX_TRIED_QUIETS: usize = 32;

/// main historyのindex空間。from(81マス+打ち駒種7)×to(81)。
const FROM_TO_SIZE: usize = Move32::FROM_TO_TABLE_SIZE;
/// main historyを分ける脅威の区分。動かす駒が相手に当たっているか×行き先が当たっているか。
const THREAT_LANES: usize = 4;

/// 相手の駒が利いているマスの集合。main historyを脅威の区分で分けるのに使う。
pub(super) fn threatened_squares(position: &Position) -> Bitboard {
    const PIECE_TYPES: [PieceType; 14] = [
        PieceType::PAWN,
        PieceType::LANCE,
        PieceType::KNIGHT,
        PieceType::SILVER,
        PieceType::GOLD,
        PieceType::BISHOP,
        PieceType::ROOK,
        PieceType::KING,
        PieceType::PRO_PAWN,
        PieceType::PRO_LANCE,
        PieceType::PRO_KNIGHT,
        PieceType::PRO_SILVER,
        PieceType::HORSE,
        PieceType::DRAGON,
    ];
    let them = position.turn().flip();
    PIECE_TYPES
        .iter()
        .fold(Bitboard::EMPTY, |acc, &piece_type| acc | position.attacks_by(them, piece_type))
}

/// 手の脅威の区分。打ち駒は動かす駒が当たっていないものとして扱う。
fn threat_lane(mv: Move32, threats: Bitboard) -> usize {
    let from = !mv.is_drop() && threats.test(mv.from_sq());
    let to = threats.test(mv.to_sq());
    usize::from(from) * 2 + usize::from(to)
}

/// βカットの前に試して失敗した静かな手の固定長バッファ。
pub(super) struct TriedQuiets {
    moves: [Move32; MAX_TRIED_QUIETS],
    len: usize,
}

impl TriedQuiets {
    pub(super) const fn new() -> Self {
        Self { moves: [MOVE_NONE; MAX_TRIED_QUIETS], len: 0 }
    }

    pub(super) fn push(&mut self, mv: Move32) {
        if self.len < MAX_TRIED_QUIETS {
            self.moves[self.len] = mv;
            self.len += 1;
        }
    }

    pub(super) fn as_slice(&self) -> &[Move32] {
        &self.moves[..self.len]
    }
}

/// continuation historyのindex空間。piece_after_move(32種)×to(81)。
const PIECE_TO_SIZE: usize = 32 * 81;
/// 直前の手は、捕獲かどうかでも行を分ける。
const CONTINUATION_SITE_SIZE: usize = PIECE_TO_SIZE * 2;
/// 参照する直前の手の数。1手前と2手前。
pub(super) const CONTINUATION_PLIES: usize = 2;

/// node内で変わらない参照行の先頭。表の値は保持せず、子探索後も最新の履歴を読む。
pub(super) struct ContinuationRows([Option<usize>; CONTINUATION_PLIES]);

impl ContinuationRows {
    pub(super) fn new(previous: &[Option<usize>; CONTINUATION_PLIES]) -> Self {
        Self(previous.map(|prev| prev.map(|index| index * PIECE_TO_SIZE)))
    }
}

/// 手を「動かした後の駒×到達マス」で表すindex。
///
/// from/toではなくpiece-toで持つのは、打ち駒にfromが無いのと、
/// 「その駒がそのマスへ来ること」がcontinuationの単位だからである。
pub(super) fn piece_to_index(mv: Move32) -> usize {
    mv.piece_after_move().to_index() * 81 + mv.to_sq().to_index()
}

/// 現在の手のpiece-toと、直前の手の捕獲文脈を別の軸として扱う。
pub(super) fn continuation_index(mv: Move32, is_capture: bool) -> usize {
    piece_to_index(mv) + usize::from(is_capture) * PIECE_TO_SIZE
}

/// 直前の手に続く静かな手の履歴。1手前・2手前それぞれの表を持つ。
///
/// main historyが「その手自体の良さ」を測るのに対し、これは
/// 「この手の後にこの手が良い」という組を測る。
pub(crate) struct ContinuationHistories {
    /// [直前の手の捕獲文脈とpiece-to][この手のpiece-to]。
    tables: [Box<[i16]>; CONTINUATION_PLIES],
}

impl ContinuationHistories {
    fn new() -> Self {
        Self {
            tables: std::array::from_fn(|_| {
                vec![0i16; CONTINUATION_SITE_SIZE * PIECE_TO_SIZE].into_boxed_slice()
            }),
        }
    }

    /// 行先頭を準備済みの、この手のcontinuation score。
    pub(super) fn score_rows(&self, rows: &ContinuationRows, cur: usize) -> i32 {
        let mut total = 0;
        for (table, start) in self.tables.iter().zip(rows.0.iter()) {
            if let Some(start) = start {
                total += i32::from(table[start + cur]);
            }
        }
        total
    }

    /// 行準備を使わない算術oracle。
    #[cfg(test)]
    pub(super) fn score(&self, previous: &[Option<usize>; CONTINUATION_PLIES], cur: usize) -> i32 {
        let mut total = 0;
        for (table, prev) in self.tables.iter().zip(previous.iter()) {
            if let Some(prev) = prev {
                total += i32::from(table[prev * PIECE_TO_SIZE + cur]);
            }
        }
        total
    }

    /// βカットした手へbonus、先に試して失敗した手へ同じ大きさのペナルティ。
    pub(super) fn record(
        &mut self,
        previous: &[Option<usize>; CONTINUATION_PLIES],
        cur: usize,
        bonus: i32,
    ) {
        for (table, prev) in self.tables.iter_mut().zip(previous.iter()) {
            if let Some(prev) = prev {
                let entry = &mut table[prev * PIECE_TO_SIZE + cur];
                let mut value = i32::from(*entry);
                apply_gravity(&mut value, bonus);
                *entry = value as i16;
            }
        }
    }
}

/// correction historyの表のindex空間。keyの下位ビットを取る。
const CORRECTION_SIZE: usize = 16_384;
/// 玉位置対の表のindex空間。`own_king * 81 + opp_king`。
const KING_PAIR_SIZE: usize = 81 * 81;
/// 補正値の格納粒度。評価値1点をこの倍率で持つ。
const CORRECTION_GRAIN: i32 = 256;
/// 1エントリが表せる補正の上限(評価値換算で±96)。
const CORRECTION_MAX: i32 = 96 * CORRECTION_GRAIN;
/// 1ノードが学習する誤差の上限(評価値換算)。
const CORRECTION_ERROR_MAX: i32 = 256;
/// 3表の和を評価値へ落とす除数。
const CORRECTION_DIVISOR: i32 = 512;

/// 静的評価の系統誤差を局面の特徴ごとに学習する表。
///
/// NNUEの評価は、手駒の偏り・歩の配置・玉の位置といった動きの遅い特徴について
/// 系統的にずれる。同じ特徴を持つ局面で「探索結果 - 静的評価」を平均し、
/// 次に同じ特徴の局面へ来たときの静的評価へ足し戻す。
///
/// keyはrsshogiが差分維持している資産をそのまま使う。手駒キーは
/// `key() ^ board_key()`(`key`は`board_key`と手駒寄与のXORで構成される)、
/// 歩キーは`partial_keys().pawn`、玉位置対はindexの直接計算。
pub(crate) struct CorrectionHistories {
    hand: Box<[[i32; CORRECTION_SIZE]; Color::COUNT]>,
    pawn: Box<[[i32; CORRECTION_SIZE]; Color::COUNT]>,
    king: Box<[[i32; KING_PAIR_SIZE]; Color::COUNT]>,
}

/// 一局面が引く3表のindex。
struct CorrectionIndices {
    side: usize,
    hand: usize,
    pawn: usize,
    king: usize,
}

fn correction_indices(position: &Position) -> CorrectionIndices {
    let stm = position.turn();
    // `key = board_key ^ hand_contribution`なので、XORで手駒寄与だけが残る。
    let hand_key = position.key().low_u64() ^ position.board_key().low_u64();
    let own_king = position.king_square(stm).to_index();
    let opp_king = position.king_square(stm.flip()).to_index();
    CorrectionIndices {
        side: stm.to_index(),
        hand: (hand_key as usize) & (CORRECTION_SIZE - 1),
        pawn: (position.partial_keys().pawn.low_u64() as usize) & (CORRECTION_SIZE - 1),
        king: own_king * 81 + opp_king,
    }
}

impl CorrectionHistories {
    fn new() -> Self {
        Self {
            hand: Box::new([[0; CORRECTION_SIZE]; Color::COUNT]),
            pawn: Box::new([[0; CORRECTION_SIZE]; Color::COUNT]),
            king: Box::new([[0; KING_PAIR_SIZE]; Color::COUNT]),
        }
    }

    /// この局面の静的評価へ足す補正。`apply_max`が0なら補正しない。
    pub(super) fn correction(&self, position: &Position, apply_max: i32) -> i32 {
        if apply_max <= 0 {
            return 0;
        }
        let index = correction_indices(position);
        let total = self.hand[index.side][index.hand]
            + self.pawn[index.side][index.pawn]
            + self.king[index.side][index.king];
        (total / CORRECTION_DIVISOR).clamp(-apply_max, apply_max)
    }

    /// 「探索結果 - 生の静的評価」をこの局面の特徴へ学習させる。
    ///
    /// 重みは深さに比例させ、深い探索の観測ほど強く効かせる。
    pub(super) fn record(&mut self, position: &Position, depth: u32, error: i32) {
        let index = correction_indices(position);
        let error = error.clamp(-CORRECTION_ERROR_MAX, CORRECTION_ERROR_MAX);
        let weight = depth.min(16) as i32;
        update_correction(&mut self.hand[index.side][index.hand], error, weight);
        update_correction(&mut self.pawn[index.side][index.pawn], error, weight);
        update_correction(&mut self.king[index.side][index.king], error, weight);
    }

    fn age(&mut self) {
        for side in self.hand.iter_mut().chain(self.pawn.iter_mut()) {
            halve(side);
        }
        for side in self.king.iter_mut() {
            halve(side);
        }
    }
}

/// 補正エントリの指数移動平均更新。`weight`が大きいほど新しい観測へ寄る。
fn update_correction(entry: &mut i32, error: i32, weight: i32) {
    let target = error * CORRECTION_GRAIN;
    *entry += (target - *entry) * weight / 1_024;
    *entry = (*entry).clamp(-CORRECTION_MAX, CORRECTION_MAX);
}

/// βカットのbonus `depth * per_depth - offset`を`[0, max]`へ収める形。
#[derive(Clone, Copy)]
struct BonusShape {
    per_depth: i32,
    offset: i32,
    max: i32,
    /// ペナルティのbonusに対する百分率。
    malus_percent: i32,
}

impl BonusShape {
    fn new(params: &SearchParams) -> Self {
        Self {
            per_depth: params.history_bonus_per_depth,
            offset: params.history_bonus_offset,
            max: params.history_bonus_max.clamp(0, HISTORY_MAX),
            malus_percent: params.history_malus_percent,
        }
    }
}

/// worker slotごとの履歴テーブル。対局中はUSIセッション側が保持して貸し出す。
pub(crate) struct HistoryTables {
    bonus: BonusShape,
    /// [手番][from/toの一意index][脅威の区分]。gravity更新で±`HISTORY_MAX`に収まる。
    main: Box<[[[i32; THREAT_LANES]; FROM_TO_SIZE]; Color::COUNT]>,
    /// 静的評価の補正表。main historyと同じ寿命で持つ。
    pub(super) corrections: CorrectionHistories,
    /// 直前の手に続く手の履歴。表が巨大なので`age`では減衰させず、
    /// gravity更新の飽和だけで古い観測を薄める。
    pub(super) continuations: ContinuationHistories,
}

impl HistoryTables {
    /// 確保領域を保ったまま対局用の学習値を消す。
    pub(super) fn clear(&mut self) {
        for side in self.main.iter_mut() {
            side.fill([0; THREAT_LANES]);
        }
        for side in self.corrections.hand.iter_mut().chain(self.corrections.pawn.iter_mut()) {
            side.fill(0);
        }
        for side in self.corrections.king.iter_mut() {
            side.fill(0);
        }
        for table in &mut self.continuations.tables {
            table.fill(0);
        }
    }

    pub(super) fn new() -> Self {
        Self {
            bonus: BonusShape::new(&SearchParams::default()),
            main: Box::new([[[0; THREAT_LANES]; FROM_TO_SIZE]; Color::COUNT]),
            corrections: CorrectionHistories::new(),
            continuations: ContinuationHistories::new(),
        }
    }

    /// 探索開始時に、βカットのbonusの形を探索パラメータから取り込む。
    pub(super) fn configure(&mut self, params: &SearchParams) {
        self.bonus = BonusShape::new(params);
    }

    /// βカット時のbonus。ペナルティはこの符号反転を使う。
    fn cutoff_bonus(&self, depth: u32) -> i32 {
        let shape = self.bonus;
        (i32::try_from(depth).unwrap_or(i32::MAX).saturating_mul(shape.per_depth) - shape.offset)
            .clamp(0, shape.max)
    }

    /// 先に試して失敗した静かな手へのペナルティ。`HISTORY_MAX`を超えない。
    fn cutoff_malus(&self, bonus: i32) -> i32 {
        (bonus * self.bonus.malus_percent.max(0) / 100).min(HISTORY_MAX)
    }

    /// 新しい探索の開始時に全エントリを半減する。
    ///
    /// 前の`go`で学んだ傾向を残しつつ、局面が進んで古くなった分の
    /// 重みを下げる。ゼロ方向への切り捨てなので符号は保存される。
    pub(super) fn age(&mut self) {
        for side in self.main.iter_mut() {
            halve(side.as_flattened_mut());
        }
        self.corrections.age();
    }

    /// 静かな手の並べ替えscore。手の脅威の区分で引いたmain historyの値。
    pub(super) fn quiet_score(&self, stm: Color, mv: Move32, threats: Bitboard) -> i32 {
        mv.from_to_index()
            .map_or(0, |index| self.main[stm.to_index()][index][threat_lane(mv, threats)])
    }

    /// 静かな手のβカットを記録する。
    ///
    /// カットした手へ正のbonus、先に試してalphaを上げられなかった
    /// 静かな手へ同じ大きさのペナルティをgravity更新で与える。
    pub(super) fn record_quiet_cutoff(
        &mut self,
        stm: Color,
        mv: Move32,
        tried_quiets: &[Move32],
        depth: u32,
        threats: Bitboard,
    ) {
        let bonus = self.cutoff_bonus(depth);
        let malus = self.cutoff_malus(bonus);
        self.update_quiet(stm, mv, threats, bonus);
        for tried in tried_quiets.iter().copied() {
            debug_assert_ne!(tried, mv, "カットした手自身へはペナルティを与えない");
            self.update_quiet(stm, tried, threats, -malus);
        }
    }

    /// 直前の手に続く静かな手のβカットを記録する。main historyと同じ
    /// bonus・ペナルティを、piece-toで引くcontinuationの表へも与える。
    pub(super) fn record_continuation_cutoff(
        &mut self,
        previous: &[Option<usize>; CONTINUATION_PLIES],
        mv: Move32,
        tried_quiets: &[Move32],
        depth: u32,
    ) {
        if previous.iter().all(Option::is_none) {
            return;
        }
        let bonus = self.cutoff_bonus(depth);
        let malus = self.cutoff_malus(bonus);
        self.continuations.record(previous, piece_to_index(mv), bonus);
        for tried in tried_quiets.iter().copied() {
            self.continuations.record(previous, piece_to_index(tried), -malus);
        }
    }

    fn update_quiet(&mut self, stm: Color, mv: Move32, threats: Bitboard, bonus: i32) {
        if let Some(index) = mv.from_to_index() {
            apply_gravity(&mut self.main[stm.to_index()][index][threat_lane(mv, threats)], bonus);
        }
    }
}

/// 新しい探索の開始時の減衰。ゼロ方向への切り捨てなので符号は保存される。
fn halve(entries: &mut [i32]) {
    for entry in entries {
        *entry /= 2;
    }
}

/// gravity更新 `h += bonus - h * |bonus| / MAX`。
///
/// `|h| <= MAX`かつ`|bonus| <= MAX`なら更新後も`|h| <= MAX`に収まる。
fn apply_gravity(entry: &mut i32, bonus: i32) {
    debug_assert!(bonus.abs() <= HISTORY_MAX);
    debug_assert!(entry.abs() <= HISTORY_MAX);
    *entry += bonus - *entry * bonus.abs() / HISTORY_MAX;
}

#[cfg(test)]
mod tests {
    use rsshogi::types::{Piece, PieceType, Square};

    /// 表の挙動を既定paramsから独立に確かめるための補正の適用上限。
    const fn apply_max() -> i32 {
        64
    }

    use super::*;

    // 履歴更新の算術を、調整される既定値から独立した固定条件で確かめる。
    fn history_fixture() -> HistoryTables {
        let mut tables = HistoryTables::new();
        tables.configure(&SearchParams {
            history_bonus_per_depth: 161,
            history_bonus_offset: 68,
            history_bonus_max: 1_615,
            history_malus_percent: 100,
            ..SearchParams::default()
        });
        tables
    }

    fn quiet_move(from: i8, to: i8, piece: Piece) -> Move32 {
        Move32::normal(Square::new(from), Square::new(to), piece)
    }

    #[test]
    fn gravity_updates_saturate_at_history_max_in_both_directions() {
        let mut positive = 0;
        let mut negative = 0;
        for _ in 0..1_000 {
            apply_gravity(&mut positive, 1_600);
            apply_gravity(&mut negative, -1_600);
            assert!(positive.abs() <= HISTORY_MAX);
            assert!(negative.abs() <= HISTORY_MAX);
        }
        assert!(positive > HISTORY_MAX * 9 / 10, "正のbonusは上限近くへ収束する");
        assert_eq!(positive, -negative, "gravityは符号について対称");
    }

    #[test]
    fn a_quiet_cutoff_rewards_the_move_and_penalizes_the_tried_quiets() {
        let mut tables = history_fixture();
        let cutoff = quiet_move(10, 20, Piece::B_SILVER);
        let tried = [quiet_move(30, 40, Piece::B_GOLD), quiet_move(50, 60, Piece::B_KNIGHT)];

        tables.record_quiet_cutoff(
            Color::BLACK,
            cutoff,
            &tried,
            4,
            rsshogi::types::Bitboard::EMPTY,
        );

        let bonus = history_fixture().cutoff_bonus(4);
        assert_eq!(bonus, 576);
        assert_eq!(
            tables.quiet_score(Color::BLACK, cutoff, rsshogi::types::Bitboard::EMPTY),
            bonus
        );
        for mv in tried {
            assert_eq!(
                tables.quiet_score(Color::BLACK, mv, rsshogi::types::Bitboard::EMPTY),
                -bonus
            );
        }
        assert_eq!(
            tables.quiet_score(Color::WHITE, cutoff, rsshogi::types::Bitboard::EMPTY),
            0,
            "main historyは手番で分かれる"
        );
    }

    #[test]
    fn cutoff_bonus_grows_with_depth_and_caps_at_the_maximum() {
        assert_eq!(history_fixture().cutoff_bonus(1), 93);
        assert_eq!(history_fixture().cutoff_bonus(10), 1_542);
        assert_eq!(history_fixture().cutoff_bonus(11), 1_615);
        assert_eq!(history_fixture().cutoff_bonus(64), 1_615);
    }

    #[test]
    fn a_recorded_error_moves_the_correction_toward_it_and_saturates() {
        let position = rsshogi::board::hirate_position();
        let mut tables = CorrectionHistories::new();
        assert_eq!(tables.correction(&position, apply_max()), 0);

        // 1回の観測では目標へ届かず、繰り返すと上限へ収束する。
        tables.record(&position, 8, 200);
        let once = tables.correction(&position, apply_max());
        assert!(once > 0 && once < 200);
        for _ in 0..200 {
            tables.record(&position, 16, 200);
        }
        let saturated = tables.correction(&position, apply_max());
        assert_eq!(saturated, apply_max(), "適用値は上限でclampされる");

        // 逆符号の観測は補正を反転させる。
        for _ in 0..400 {
            tables.record(&position, 16, -200);
        }
        assert_eq!(tables.correction(&position, apply_max()), -apply_max());
    }

    #[test]
    fn a_continuation_cutoff_rewards_the_move_after_the_same_previous_move() {
        let mut tables = history_fixture();
        let previous = [Some(10usize), Some(20usize)];
        let cutoff = quiet_move(10, 20, Piece::B_SILVER);
        let tried = [quiet_move(30, 40, Piece::B_GOLD)];
        let cur = piece_to_index(cutoff);

        tables.record_continuation_cutoff(&previous, cutoff, &tried, 4);

        let bonus = history_fixture().cutoff_bonus(4);
        // 1手前と2手前の両方の表に入るので、合計は2倍になる。
        assert_eq!(tables.continuations.score(&previous, cur), bonus * 2);
        assert_eq!(
            tables.continuations.score(&previous, piece_to_index(tried[0])),
            -bonus * 2,
            "先に試して失敗した手はペナルティを受ける"
        );
        assert_eq!(
            tables.continuations.score(&[Some(11), None], cur),
            0,
            "別の直前の手からは引かれない"
        );
    }

    #[test]
    fn a_continuation_cutoff_without_a_previous_move_is_ignored() {
        let mut tables = HistoryTables::new();
        let cutoff = quiet_move(10, 20, Piece::B_SILVER);

        tables.record_continuation_cutoff(&[None; CONTINUATION_PLIES], cutoff, &[], 4);

        assert_eq!(
            tables.continuations.score(&[None; CONTINUATION_PLIES], piece_to_index(cutoff)),
            0
        );
    }

    #[test]
    fn prepared_continuation_rows_match_the_oracle_and_observe_later_updates() {
        let mut histories = ContinuationHistories::new();
        for previous in [
            [None, None],
            [Some(0), None],
            [None, Some(CONTINUATION_SITE_SIZE - 1)],
            [Some(10), Some(20)],
        ] {
            let rows = ContinuationRows::new(&previous);
            for current in [0, 1, 81, PIECE_TO_SIZE - 1] {
                for bonus in [300, -700, 1_500] {
                    histories.record(&previous, current, bonus);
                    assert_eq!(
                        histories.score_rows(&rows, current),
                        histories.score(&previous, current),
                        "prepared rows must read live values, including negative feedback"
                    );
                }
            }
        }
    }

    #[test]
    fn continuation_separates_capture_and_quiet_previous_moves() {
        let mut histories = ContinuationHistories::new();
        let mv = quiet_move(10, 20, Piece::B_SILVER);
        let current = piece_to_index(mv);
        let quiet = [Some(continuation_index(mv, false)), None];
        let capture = [Some(continuation_index(mv, true)), None];
        histories.record(&quiet, current, 300);
        assert_eq!(histories.score(&quiet, current), 300);
        assert_eq!(histories.score(&capture, current), 0);
        histories.record(&capture, current, -500);
        assert_eq!(histories.score(&quiet, current), 300);
        assert_eq!(histories.score(&capture, current), -500);
    }

    #[test]
    fn a_dropped_piece_gets_a_continuation_index_of_its_own() {
        let drop = Move32::drop(PieceType::GOLD, Square::new(40), Color::BLACK);
        let board_move = quiet_move(30, 40, Piece::B_GOLD);

        // 打ち駒も盤上の手も「動かした後の駒×到達マス」なので同じ空間に入り、
        // 金が4一へ来る点で一致する。
        assert_eq!(piece_to_index(drop), piece_to_index(board_move));
        assert!(piece_to_index(drop) < 32 * 81);
    }

    #[test]
    fn a_zero_apply_max_turns_the_correction_off() {
        let position = rsshogi::board::hirate_position();
        let mut tables = CorrectionHistories::new();
        for _ in 0..8 {
            tables.record(&position, 16, 200);
        }
        assert!(tables.correction(&position, apply_max()) > 0);
        assert_eq!(tables.correction(&position, 0), 0);
    }

    #[test]
    fn a_recorded_error_is_clamped_before_it_reaches_the_table() {
        let position = rsshogi::board::hirate_position();
        let mut huge = CorrectionHistories::new();
        let mut capped = CorrectionHistories::new();
        for _ in 0..64 {
            huge.record(&position, 16, 100_000);
            capped.record(&position, 16, CORRECTION_ERROR_MAX);
        }
        assert_eq!(
            huge.correction(&position, apply_max()),
            capped.correction(&position, apply_max())
        );
    }

    #[test]
    fn corrections_are_keyed_by_hand_pawn_and_king_features() {
        let base = rsshogi::board::hirate_position();
        let mut tables = CorrectionHistories::new();
        for _ in 0..8 {
            tables.record(&base, 16, 200);
        }
        assert!(tables.correction(&base, apply_max()) > 0);

        // 手駒だけが違う局面は手駒表を共有しない。
        let hand = rsshogi::board::position_from_sfen(
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b P 1",
        )
        .expect("valid hand position");
        // 歩の配置だけが違う局面は歩表を共有しない。
        let pawn = rsshogi::board::position_from_sfen(
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL b - 1",
        )
        .expect("valid pawn position");
        assert!(tables.correction(&hand, apply_max()) < tables.correction(&base, apply_max()));
        assert!(tables.correction(&pawn, apply_max()) < tables.correction(&base, apply_max()));
    }

    #[test]
    fn aging_halves_the_corrections_too() {
        let position = rsshogi::board::hirate_position();
        let mut tables = HistoryTables::new();
        // 上限へ張り付くと半減が見えないので、飽和前の値で確かめる。
        for _ in 0..2 {
            tables.corrections.record(&position, 16, 200);
        }
        let before = tables.corrections.correction(&position, apply_max());
        assert!(before > 1 && before < apply_max());

        tables.age();

        assert_eq!(tables.corrections.correction(&position, apply_max()), before / 2);
    }

    #[test]
    fn a_drop_move_maps_into_the_main_history_index_space() {
        let mut tables = HistoryTables::new();
        let drop = Move32::drop(PieceType::GOLD, Square::new(40), Color::BLACK);

        tables.record_quiet_cutoff(Color::BLACK, drop, &[], 4, rsshogi::types::Bitboard::EMPTY);

        assert_eq!(
            tables.quiet_score(Color::BLACK, drop, rsshogi::types::Bitboard::EMPTY),
            HistoryTables::new().cutoff_bonus(4)
        );
    }

    #[test]
    fn aging_halves_entries_toward_zero_and_preserves_sign() {
        let mut tables = history_fixture();
        let rewarded = quiet_move(10, 20, Piece::B_SILVER);
        let penalized = quiet_move(30, 40, Piece::B_GOLD);
        tables.record_quiet_cutoff(
            Color::BLACK,
            rewarded,
            &[penalized],
            4,
            rsshogi::types::Bitboard::EMPTY,
        );
        let bonus = history_fixture().cutoff_bonus(4);

        tables.age();

        assert_eq!(
            tables.quiet_score(Color::BLACK, rewarded, rsshogi::types::Bitboard::EMPTY),
            bonus / 2
        );
        assert_eq!(
            tables.quiet_score(Color::BLACK, penalized, rsshogi::types::Bitboard::EMPTY),
            -(bonus / 2)
        );

        // 何回か半減すればゼロへ収束する。
        for _ in 0..16 {
            tables.age();
        }
        assert_eq!(tables.quiet_score(Color::BLACK, rewarded, rsshogi::types::Bitboard::EMPTY), 0);
        assert_eq!(tables.quiet_score(Color::BLACK, penalized, rsshogi::types::Bitboard::EMPTY), 0);
    }
}

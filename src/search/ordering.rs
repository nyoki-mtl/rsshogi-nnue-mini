//! 手の並べ替えと、そのための一手ごとの分類を担う。

use std::cmp::Reverse;
use std::collections::HashMap;

use rsshogi::board::Position;
use rsshogi::types::{Move32, Move32Metadata};

use crate::eval::EvalParams;
use crate::params::SearchParams;
use crate::see::static_exchange_eval;

use super::CaptureSee;

/// 並べ替え済みの一手と、その手について一度だけ求めた分類。
///
/// `metadata`、王手判定、SEEはここでまとめて求め、探索loopでは求め直さない。
#[derive(Clone, Copy)]
pub(super) struct OrderedMove {
    pub(super) mv: Move32,
    pub(super) metadata: Move32Metadata,
    pub(super) gives_check: bool,
    pub(super) see: CaptureSee,
    pub(super) quiet: bool,
}

pub(super) fn order_moves(
    position: &Position,
    moves: &[Move32],
    params: EvalParams,
    tt_move: Option<Move32>,
    killers: [Option<Move32>; 2],
    history: &HashMap<Move32, i32>,
    search_params: SearchParams,
) -> Vec<OrderedMove> {
    let capture_class_bonus = capture_class_bonus(params, search_params.check_ordering_bonus);
    let mut ordered = moves
        .iter()
        .copied()
        .map(|mv| {
            let metadata = position.move32_metadata(mv);
            let see = static_exchange_eval(position, mv, params);
            let gives_check = position.gives_check_move32(mv);
            let quiet = !metadata.is_capture() && !metadata.is_promotion();
            let capture = params.piece_value(metadata.captured_piece_type());
            let (exchange_class, exchange) = if metadata.is_capture() {
                match see {
                    Some(score) if score >= 0 => (capture_class_bonus, score),
                    Some(score) => (-capture_class_bonus, score),
                    None => (0, 0),
                }
            } else {
                (0, 0)
            };
            let promotion = if metadata.is_promotion() { params.promotion_bonus } else { 0 };
            let check = if gives_check { search_params.check_ordering_bonus } else { 0 };
            let principal = if Some(mv) == tt_move { 1_000_000 } else { 0 };
            let killer = if quiet && Some(mv) == killers[0] {
                80_000
            } else if quiet && Some(mv) == killers[1] {
                70_000
            } else {
                0
            };
            let history = if quiet { history.get(&mv).copied().unwrap_or(0) } else { 0 };
            let score = principal
                + capture * 16
                + exchange_class
                + exchange
                + promotion
                + check
                + killer
                + history;
            (Reverse(score), OrderedMove { mv, metadata, gives_check, see, quiet })
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|(score, _)| *score);
    ordered.into_iter().map(|(_, ordered)| ordered).collect()
}

fn capture_class_bonus(params: EvalParams, check_ordering_bonus: i32) -> i32 {
    let largest_base = [
        params.pawn,
        params.lance,
        params.knight,
        params.silver,
        params.gold,
        params.bishop,
        params.rook,
    ]
    .into_iter()
    .max()
    .expect("the material table has piece values");
    (largest_base + params.promotion_bonus) * 16 + params.promotion_bonus + check_ordering_bonus + 1
}

#[cfg(test)]
mod tests {
    use rsshogi::board;

    use super::*;

    #[test]
    fn capture_ordering_prefers_the_better_exchange() {
        let position = board::position_from_sfen("k8/9/4p4/4s2p1/4R2P1/9/9/9/8K b - 1")
            .expect("valid ordering position");
        let losing_high_victim =
            board::move_from_usi(&position, "5e5d").expect("legal losing capture");
        let profitable_low_victim =
            board::move_from_usi(&position, "2e2d").expect("legal profitable capture");
        let moves = [losing_high_victim, profitable_low_victim];
        let history = HashMap::from([(losing_high_victim, 100_000)]);

        let ordered = order_moves(
            &position,
            &moves,
            EvalParams::default(),
            None,
            [Some(losing_high_victim), None],
            &history,
            SearchParams::default(),
        );

        assert_eq!(
            ordered.iter().map(|ordered| ordered.mv).collect::<Vec<_>>(),
            [profitable_low_victim, losing_high_victim]
        );
    }

    #[test]
    fn capture_classes_stay_separate_when_bishop_is_worth_more_than_rook() {
        let params =
            EvalParams { bishop: 1_400, rook: 500, promotion_bonus: 0, ..EvalParams::default() };
        let check_bonus = SearchParams::default().check_ordering_bonus;
        let largest_unknown_capture = params.bishop * 16 + check_bonus;

        assert!(capture_class_bonus(params, check_bonus) > largest_unknown_capture);
    }
}

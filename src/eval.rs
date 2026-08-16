use std::sync::Arc;

use rsshogi::board::Position;
use rsshogi::types::{Color, Hand, HandPiece, Move32, PieceType};

use crate::nnue::{MAX_NNUE_EVAL, StandardAccumulator, StandardNetwork};

#[derive(Clone)]
pub struct Evaluator {
    params: EvalParams,
    nnue: Option<NnueEvaluator>,
}

#[derive(Clone)]
struct NnueEvaluator {
    network: Arc<StandardNetwork>,
    accumulator: Option<StandardAccumulator>,
    fv_scale: i32,
}

impl Evaluator {
    pub const fn material(params: EvalParams) -> Self {
        Self { params, nnue: None }
    }

    pub fn nnue(params: EvalParams, network: Arc<StandardNetwork>, fv_scale: i32) -> Self {
        Self { params, nnue: Some(NnueEvaluator { network, accumulator: None, fv_scale }) }
    }

    pub fn evaluate(&self, position: &Position) -> i32 {
        match &self.nnue {
            Some(nnue) => match &nnue.accumulator {
                Some(accumulator) => nnue
                    .network
                    .evaluate_accumulator(position, accumulator, nnue.fv_scale)
                    .expect("an incremental NNUE must evaluate a legal search position"),
                None => nnue
                    .network
                    .evaluate(position, nnue.fv_scale)
                    .expect("a loaded standard NNUE must evaluate a legal search position"),
            },
            None => evaluate_material(position, self.params),
        }
    }

    pub fn initialize(&mut self, position: &Position) {
        if let Some(nnue) = &mut self.nnue {
            nnue.accumulator = Some(
                nnue.network
                    .new_accumulator(position)
                    .expect("a loaded standard NNUE must initialize a legal search position"),
            );
        }
    }

    /// `mv`を適用した直後の`position`へaccumulatorを進める。
    pub fn advance(&mut self, position: &Position, mv: Move32) {
        if let Some(nnue) = &mut self.nnue {
            nnue.network
                .advance_accumulator(
                    nnue.accumulator.as_mut().expect("NNUE search must be initialized"),
                    position,
                    mv,
                )
                .expect("a legal searched position must have valid standard NNUE features");
        }
    }

    pub fn undo(&mut self) {
        if let Some(nnue) = &mut self.nnue {
            nnue.network.undo_accumulator(
                nnue.accumulator.as_mut().expect("NNUE search must be initialized"),
            );
        }
    }

    pub const fn params(&self) -> EvalParams {
        self.params
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalParams {
    pub pawn: i32,
    pub lance: i32,
    pub knight: i32,
    pub silver: i32,
    pub gold: i32,
    pub bishop: i32,
    pub rook: i32,
    pub promotion_bonus: i32,
}

impl Default for EvalParams {
    fn default() -> Self {
        Self {
            pawn: 100,
            lance: 300,
            knight: 320,
            silver: 450,
            gold: 550,
            bishop: 800,
            rook: 1_000,
            promotion_bonus: 350,
        }
    }
}

impl EvalParams {
    pub(crate) fn piece_value(self, piece_type: PieceType) -> i32 {
        let base = match piece_type.demote() {
            PieceType::PAWN => self.pawn,
            PieceType::LANCE => self.lance,
            PieceType::KNIGHT => self.knight,
            PieceType::SILVER => self.silver,
            PieceType::GOLD => self.gold,
            PieceType::BISHOP => self.bishop,
            PieceType::ROOK => self.rook,
            _ => 0,
        };
        if piece_type.is_promoted() { base + self.promotion_bonus } else { base }
    }
}

pub fn evaluate_material(position: &Position, params: EvalParams) -> i32 {
    evaluate_material_unclamped(position, params).clamp(-MAX_NNUE_EVAL, MAX_NNUE_EVAL)
}

/// NNUE評価値の定義域へclampする前の駒得。テストが端の値を検査するために分けている。
pub(crate) fn evaluate_material_unclamped(position: &Position, params: EvalParams) -> i32 {
    let black = material_for(position, Color::BLACK, params);
    let white = material_for(position, Color::WHITE, params);
    let black_score = black - white;
    if position.turn() == Color::BLACK { black_score } else { -black_score }
}

fn material_for(position: &Position, color: Color, params: EvalParams) -> i32 {
    let board = PieceType::iter()
        .filter(|piece_type| *piece_type != PieceType::NONE)
        .map(|piece_type| {
            let count = position.pieces_for(piece_type, color).count() as i32;
            count * params.piece_value(piece_type)
        })
        .sum::<i32>();

    let hand = HandPiece::iter()
        .map(|hand_piece| {
            let count = Hand::count_of(position.hand(color), hand_piece) as i32;
            count * params.piece_value(hand_piece.to_piece_type())
        })
        .sum::<i32>();

    board + hand
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsshogi::board;

    #[test]
    fn startpos_is_equal() {
        assert_eq!(evaluate_material(&board::hirate_position(), EvalParams::default()), 0);
    }

    #[test]
    fn maximum_material_configuration_stays_below_the_mate_domain() {
        let position = board::position_from_sfen(
            "k8/9/9/9/+P+P+P+P+P+P+P+P+P/+P+P+P+P+P+P3/9/9/4K4 b 2R2B4G4S4N4L3P 1",
        )
        .expect("valid maximum-material position");
        let params = EvalParams {
            pawn: 200,
            lance: 600,
            knight: 600,
            silver: 800,
            gold: 900,
            bishop: 1_400,
            rook: 1_600,
            promotion_bonus: 800,
        };

        assert_eq!(evaluate_material_unclamped(&position, params), 33_200);
        assert_eq!(evaluate_material(&position, params), MAX_NNUE_EVAL);
    }
}

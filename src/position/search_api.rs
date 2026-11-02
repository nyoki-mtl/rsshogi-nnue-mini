//! Mini-owned move facts and the pinned rsshogi 1.x search-null boundary.

use rsshogi::board::Position;
use rsshogi::board::position::MoveError;
use rsshogi::types::{Move32, Piece, PieceType};

#[derive(Clone, Copy)]
pub(crate) struct MoveFacts {
    mv: Move32,
    after: Piece,
    captured: PieceType,
}

impl MoveFacts {
    pub(crate) fn is_capture(self) -> bool {
        self.captured != PieceType::NONE
    }

    pub(crate) fn is_promotion(self) -> bool {
        self.mv.is_promotion()
    }

    pub(crate) fn is_drop(self) -> bool {
        self.mv.is_drop()
    }

    pub(crate) fn piece_after_move(self) -> Piece {
        self.after
    }

    pub(crate) fn captured_piece_type(self) -> PieceType {
        self.captured
    }
}

pub(crate) trait SearchPositionExt {
    /// Call before applying a normal, already validated move.
    fn move_facts(&self, mv: Move32) -> MoveFacts;
    fn start_search_null(&mut self) -> Result<(), MoveError>;
    fn finish_search_null(&mut self) -> Result<(), MoveError>;
}

impl SearchPositionExt for Position {
    fn move_facts(&self, mv: Move32) -> MoveFacts {
        let target = self.piece_on(mv.to_sq());
        MoveFacts {
            mv,
            after: self.moved_piece_after(mv),
            captured: if mv.is_drop() || target.is_empty() || target.color() == self.turn() {
                PieceType::NONE
            } else {
                target.piece_type()
            },
        }
    }

    // rsshogi is pinned to 1.7.2. Ordinary null moves advance game_ply and change
    // MaxMovesToDraw semantics, so keep these two 1.x operations until mini owns history.
    #[allow(deprecated)]
    fn start_search_null(&mut self) -> Result<(), MoveError> {
        self.try_apply_search_null_move()
    }

    #[allow(deprecated)]
    fn finish_search_null(&mut self) -> Result<(), MoveError> {
        self.undo_search_null_move()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsshogi::board;

    #[test]
    fn move_facts_cover_capture_promotion_drop_and_both_turns() {
        let mut position = board::hirate_position();
        for (text, captured, after, promotion, drop) in [
            ("7g7f", PieceType::NONE, PieceType::PAWN, false, false),
            ("3c3d", PieceType::NONE, PieceType::PAWN, false, false),
            ("8h2b+", PieceType::BISHOP, PieceType::HORSE, true, false),
            ("3a2b", PieceType::HORSE, PieceType::SILVER, false, false),
            ("B*4e", PieceType::NONE, PieceType::BISHOP, false, true),
            ("B*5d", PieceType::NONE, PieceType::BISHOP, false, true),
        ] {
            let mv = board::move_from_usi(&position, text).unwrap();
            assert!(position.is_legal_move32(mv));
            let facts = position.move_facts(mv);
            assert_eq!(facts.captured_piece_type(), captured);
            assert_eq!(facts.is_capture(), captured != PieceType::NONE);
            assert_eq!(facts.piece_after_move().piece_type(), after);
            assert_eq!(facts.piece_after_move().color(), position.turn());
            assert_eq!(facts.is_promotion(), promotion);
            assert_eq!(facts.is_drop(), drop);
            position.apply_move32(mv);
            assert_eq!(facts.piece_after_move(), position.piece_on(mv.to_sq()));
        }
    }

    #[test]
    fn search_null_preserves_move_limit_counter_and_restores_history() {
        let mut position = board::hirate_position();
        let before = position.to_sfen(None);
        let key = position.key();
        let turn = position.turn();
        let repetition = position.repetition_state();
        position.start_search_null().unwrap();
        assert_eq!(position.game_ply(), 1);
        assert_eq!(position.turn(), turn.flip());
        position.finish_search_null().unwrap();
        assert_eq!(position.to_sfen(None), before);
        assert_eq!(position.key(), key);
        assert_eq!(position.repetition_state(), repetition);
    }
}

//! HalfKPのfeature indexを局面と指し手から求める。

use rsshogi::board::Position;
use rsshogi::types::{Color, Hand, HandPiece, Move32, Piece, PieceType, Square};

use super::accumulator::FeatureChanges;
use super::{FEATURE_STRIDE, MAX_PIECES_WITHOUT_KINGS, NnueError};

/// 一手が`perspective`視点のfeature multisetへ与える差分を、指し手から直接求める。
///
/// 盤上の駒数と持ち駒の合計は指し手で変わらないため、padding分のfeatureは動かない。
pub(super) fn move_feature_changes(
    feature_zero: usize,
    position: &Position,
    mv: Move32,
    mover: Color,
    perspective: Color,
) -> Result<FeatureChanges, NnueError> {
    let mut changes = FeatureChanges::default();
    let to = mv.to_sq();

    if mv.is_drop() {
        let piece_type = mv
            .dropped_piece()
            .ok_or_else(|| NnueError::Invalid("drop move without a dropped piece".to_owned()))?;
        // 打った後の持ち駒枚数が、取り除かれるhand featureのindexそのものになる。
        let remaining = hand_count(position, mover, piece_type)?;
        changes.remove(hand_feature(feature_zero, mover, piece_type, remaining, perspective));
        changes.add(board_feature(
            feature_zero,
            Piece::from_parts(mover, piece_type),
            to,
            perspective,
        )?);
        return Ok(changes);
    }

    let moved_after = position.piece_on(to);
    if moved_after.piece_type() != PieceType::KING {
        let moved_before = if mv.is_promotion() {
            Piece::from_parts(moved_after.color(), moved_after.piece_type().demote())
        } else {
            moved_after
        };
        changes.remove(board_feature(feature_zero, moved_before, mv.from_sq(), perspective)?);
        changes.add(board_feature(feature_zero, moved_after, to, perspective)?);
    }

    let captured = position.captured_piece();
    if captured != Piece::NONE {
        changes.remove(board_feature(feature_zero, captured, to, perspective)?);
        let gained = captured.piece_type().demote();
        // 捕獲後の持ち駒枚数から1引いたindexが、新しく増えたhand featureになる。
        let held = hand_count(position, mover, gained)?;
        let index = held
            .checked_sub(1)
            .ok_or_else(|| NnueError::Invalid("capture without a hand gain".to_owned()))?;
        changes.add(hand_feature(feature_zero, mover, gained, index, perspective));
    }

    Ok(changes)
}

fn hand_count(
    position: &Position,
    owner: Color,
    piece_type: PieceType,
) -> Result<usize, NnueError> {
    let hand_piece = HandPiece::from_piece_type(piece_type).ok_or_else(|| {
        NnueError::Invalid(format!("piece type cannot be held in hand: {piece_type:?}"))
    })?;
    Ok(Hand::count_of(position.hand(owner), hand_piece) as usize)
}

fn board_feature(
    feature_zero: usize,
    piece: Piece,
    square: Square,
    perspective: Color,
) -> Result<usize, NnueError> {
    let base = board_base(piece, perspective)
        .ok_or_else(|| NnueError::Invalid(format!("unsupported board piece: {piece:?}")))?;
    Ok(feature_zero + base + board_offset(square, perspective))
}

fn hand_feature(
    feature_zero: usize,
    owner: Color,
    piece_type: PieceType,
    index: usize,
    perspective: Color,
) -> usize {
    feature_zero + hand_base(owner, piece_type, perspective) + index
}

fn board_offset(square: Square, perspective: Color) -> usize {
    if perspective == Color::BLACK {
        square.to_board_index()
    } else {
        square.flip().to_board_index()
    }
}

pub(super) fn feature_zero(position: &Position, perspective: Color) -> Result<usize, NnueError> {
    let king = position.king_square(perspective);
    if !king.is_on_board() {
        return Err(NnueError::Invalid(format!("missing {perspective} king")));
    }
    Ok(board_offset(king, perspective) * FEATURE_STRIDE)
}

pub(super) fn feature_indices(
    position: &Position,
    perspective: Color,
) -> Result<Vec<usize>, NnueError> {
    let feature_zero = feature_zero(position, perspective)?;
    let mut features = Vec::with_capacity(MAX_PIECES_WITHOUT_KINGS);

    for square in Square::iter() {
        let piece = position.piece_on(square);
        if piece == Piece::NONE || piece.piece_type() == PieceType::KING {
            continue;
        }
        features.push(board_feature(feature_zero, piece, square, perspective)?);
    }

    for owner in [Color::BLACK, Color::WHITE] {
        for hand_piece in HandPiece::iter() {
            let count = Hand::count_of(position.hand(owner), hand_piece);
            for index in 0..count {
                features.push(hand_feature(
                    feature_zero,
                    owner,
                    hand_piece.to_piece_type(),
                    index as usize,
                    perspective,
                ));
            }
        }
    }

    if features.len() > MAX_PIECES_WITHOUT_KINGS {
        return Err(NnueError::Invalid(format!("too many non-king pieces: {}", features.len())));
    }
    features.resize(MAX_PIECES_WITHOUT_KINGS, feature_zero);
    Ok(features)
}

fn board_base(piece: Piece, perspective: Color) -> Option<usize> {
    let (friend, enemy) = match piece.piece_type() {
        PieceType::PAWN => (90, 171),
        PieceType::LANCE => (252, 333),
        PieceType::KNIGHT => (414, 495),
        PieceType::SILVER => (576, 657),
        PieceType::GOLD
        | PieceType::PRO_PAWN
        | PieceType::PRO_LANCE
        | PieceType::PRO_KNIGHT
        | PieceType::PRO_SILVER => (738, 819),
        PieceType::BISHOP => (900, 981),
        PieceType::HORSE => (1062, 1143),
        PieceType::ROOK => (1224, 1305),
        PieceType::DRAGON => (1386, 1467),
        _ => return None,
    };
    Some(if piece.color() == perspective { friend } else { enemy })
}

fn hand_base(owner: Color, piece_type: PieceType, perspective: Color) -> usize {
    let (friend, enemy) = match piece_type {
        PieceType::PAWN => (1, 20),
        PieceType::LANCE => (39, 44),
        PieceType::KNIGHT => (49, 54),
        PieceType::SILVER => (59, 64),
        PieceType::GOLD => (69, 74),
        PieceType::BISHOP => (79, 82),
        PieceType::ROOK => (85, 88),
        _ => unreachable!("HandPiece only converts to a hand piece type"),
    };
    if owner == perspective { friend } else { enemy }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nnue::FEATURE_DIMENSIONS;
    use rsshogi::board;

    #[test]
    fn startpos_has_the_standard_fixed_feature_count() {
        let position = board::hirate_position();
        let black = feature_indices(&position, Color::BLACK).expect("black features");
        let white = feature_indices(&position, Color::WHITE).expect("white features");
        assert_eq!(black.len(), MAX_PIECES_WITHOUT_KINGS);
        assert_eq!(white.len(), MAX_PIECES_WITHOUT_KINGS);
        assert!(black.iter().all(|index| *index < FEATURE_DIMENSIONS));
        assert!(white.iter().all(|index| *index < FEATURE_DIMENSIONS));
        assert!(black.contains(&(44 * FEATURE_STRIDE + 90 + 6)));
        assert!(black.contains(&(44 * FEATURE_STRIDE + 171 + 2)));
    }

    #[test]
    fn hand_feature_uses_owner_relative_bases() {
        let position = board::position_from_sfen(
            "lnsgkgsnl/1r5b1/1pppppppp/9/9/9/1PPPPPPPP/1B5R1/LNSGKGSNL b Pp 1",
        )
        .expect("valid SFEN");
        let black = feature_indices(&position, Color::BLACK).expect("black features");
        let king_base = position.king_square(Color::BLACK).to_board_index() * FEATURE_STRIDE;
        assert!(black.contains(&(king_base + 1)));
        assert!(black.contains(&(king_base + 20)));
    }

    #[test]
    fn asymmetric_promoted_hand_and_white_perspective_indices_match_oracle() {
        let position = board::position_from_sfen("5k3/9/9/9/4+p4/9/9/9/4K4 w Rb 1")
            .expect("valid asymmetric SFEN");
        let black = feature_indices(&position, Color::BLACK).expect("black features");
        let white = feature_indices(&position, Color::WHITE).expect("white features");

        let black_zero = 44 * FEATURE_STRIDE;
        assert_eq!(position.king_square(Color::BLACK).to_string(), "5i");
        assert_eq!(black.iter().filter(|index| **index == black_zero).count(), 35);
        assert!(black.contains(&(black_zero + 819 + 40)));
        assert!(black.contains(&(black_zero + 85)));
        assert!(black.contains(&(black_zero + 82)));

        let white_zero = 53 * FEATURE_STRIDE;
        assert_eq!(position.king_square(Color::WHITE).to_string(), "4a");
        assert_eq!(white.iter().filter(|index| **index == white_zero).count(), 35);
        assert!(white.contains(&(white_zero + 738 + 40)));
        assert!(white.contains(&(white_zero + 88)));
        assert!(white.contains(&(white_zero + 79)));
    }
}

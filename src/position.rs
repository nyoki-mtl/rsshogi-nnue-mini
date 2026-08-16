use rsshogi::board::{self, Move32List, Position, generate_legal_all_move32};
use rsshogi::types::{Color, Hand, HandPiece, PieceType};
use rsshogi_usi::PositionSpec;

pub(crate) const MAX_SEARCH_DEPTH: u32 = 64;
pub(crate) const MAX_SEARCH_PLY: u32 = 128;
pub(crate) const MAX_ROOT_GAME_PLY: u32 = (u16::MAX as u32) - MAX_SEARCH_PLY;

pub fn replay(spec: &PositionSpec, moves: &[String]) -> Result<Position, String> {
    validate_sfen_hands(spec.as_sfen_parts().2)?;
    let mut position = board::position_from_sfen(&spec.to_sfen()).map_err(|err| err.to_string())?;
    validate_evaluable_position(&position)?;

    for (index, text) in moves.iter().enumerate() {
        let mv = board::move_from_usi(&position, text)
            .ok_or_else(|| format!("invalid move at index {index}: {text}"))?;
        if position.piece_on(mv.to_sq()).piece_type() == PieceType::KING {
            return Err(format!("king capture at index {index}: {text}"));
        }
        if !position.is_legal_move32(mv) {
            return Err(format!("illegal move at index {index}: {text}"));
        }
        position.apply_move32(mv);
        validate_evaluable_position(&position)?;
    }

    Ok(position)
}

fn validate_sfen_hands(hands: &str) -> Result<(), String> {
    if hands == "-" {
        return Ok(());
    }
    if hands.is_empty() {
        return Err("SFEN hands must not be empty".to_owned());
    }
    if !hands.is_ascii() {
        return Err("SFEN hands must use ASCII characters".to_owned());
    }

    let mut counts = [0_u8; 7];
    let mut total = 0_u8;
    let mut number = 0_u8;
    let mut has_number = false;
    for byte in hands.bytes() {
        if byte.is_ascii_digit() {
            if !has_number && byte == b'0' {
                return Err("SFEN hand count must not start with zero".to_owned());
            }
            number = number
                .checked_mul(10)
                .and_then(|value| value.checked_add(byte - b'0'))
                .filter(|value| *value <= 18)
                .ok_or_else(|| "SFEN hand count exceeds physical maximum".to_owned())?;
            has_number = true;
            continue;
        }

        let (index, maximum) = match byte {
            b'P' | b'p' => (0, 18),
            b'L' | b'l' => (1, 4),
            b'N' | b'n' => (2, 4),
            b'S' | b's' => (3, 4),
            b'G' | b'g' => (4, 4),
            b'B' | b'b' => (5, 2),
            b'R' | b'r' => (6, 2),
            _ => return Err("invalid SFEN hands grammar".to_owned()),
        };
        let count = if has_number { number } else { 1 };
        counts[index] = counts[index]
            .checked_add(count)
            .filter(|value| *value <= maximum)
            .ok_or_else(|| "SFEN hand count exceeds physical maximum".to_owned())?;
        total = total
            .checked_add(count)
            .filter(|value| *value <= 38)
            .ok_or_else(|| "SFEN hands exceed physical piece total".to_owned())?;
        number = 0;
        has_number = false;
    }
    if has_number {
        return Err("SFEN hand count is missing a piece".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_evaluable_position(position: &Position) -> Result<(), String> {
    if u32::from(position.game_ply()) > MAX_ROOT_GAME_PLY {
        return Err(format!(
            "game ply {} exceeds maximum root game ply {MAX_ROOT_GAME_PLY}",
            position.game_ply()
        ));
    }

    for color in [Color::BLACK, Color::WHITE] {
        let king_count = position.pieces_for(PieceType::KING, color).count();
        if king_count != 1 {
            return Err(format!("position must contain exactly one {color} king"));
        }
    }

    let mut physical_counts = [0_usize; 7];
    for piece_type in PieceType::iter()
        .filter(|piece_type| *piece_type != PieceType::NONE && *piece_type != PieceType::KING)
    {
        let index = physical_piece_index(piece_type.demote())
            .expect("every non-king board piece has a physical base type");
        physical_counts[index] += usize::try_from(
            position.pieces_for(piece_type, Color::BLACK).count()
                + position.pieces_for(piece_type, Color::WHITE).count(),
        )
        .expect("board piece count fits in usize");
    }
    for color in [Color::BLACK, Color::WHITE] {
        for hand_piece in HandPiece::iter() {
            let index = physical_piece_index(hand_piece.to_piece_type())
                .expect("every hand piece has a physical base type");
            physical_counts[index] +=
                usize::try_from(Hand::count_of(position.hand(color), hand_piece))
                    .expect("hand count fits in usize");
        }
    }
    for ((name, maximum), count) in [
        ("pawns", 18),
        ("lances", 4),
        ("knights", 4),
        ("silvers", 4),
        ("golds", 4),
        ("bishops", 2),
        ("rooks", 2),
    ]
    .into_iter()
    .zip(physical_counts)
    {
        if count > maximum {
            return Err(format!("position has too many {name}: {count}"));
        }
    }
    let non_king_pieces = physical_counts.into_iter().sum::<usize>();
    if non_king_pieces > 38 {
        return Err(format!("position has too many non-king pieces: {}", non_king_pieces));
    }

    let mut legal_moves = Move32List::new();
    generate_legal_all_move32(position, &mut legal_moves);
    if legal_moves
        .as_slice()
        .iter()
        .any(|mv| position.piece_on(mv.to_sq()).piece_type() == PieceType::KING)
    {
        return Err("position permits a move that captures the opposing king".to_owned());
    }

    Ok(())
}

const fn physical_piece_index(piece_type: PieceType) -> Option<usize> {
    match piece_type {
        PieceType::PAWN => Some(0),
        PieceType::LANCE => Some(1),
        PieceType::KNIGHT => Some(2),
        PieceType::SILVER => Some(3),
        PieceType::GOLD => Some(4),
        PieceType::BISHOP => Some(5),
        PieceType::ROOK => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_startpos_moves() {
        let moves = vec!["7g7f".to_owned(), "3c3d".to_owned()];
        let position = replay(&PositionSpec::StartPos, &moves).expect("moves should be legal");
        assert_eq!(position.game_ply(), 3);
    }

    #[test]
    fn rejects_illegal_move() {
        let moves = vec!["7g7e".to_owned()];
        let error = match replay(&PositionSpec::StartPos, &moves) {
            Ok(_) => panic!("move should be illegal"),
            Err(error) => error,
        };
        assert!(error.contains("illegal move"));
    }

    #[test]
    fn rejects_sfen_without_both_kings() {
        let spec = PositionSpec::Sfen {
            board: "9/9/9/9/9/9/9/9/4K4".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "-".to_owned(),
            ply: "1".to_owned(),
        };
        let error = match replay(&spec, &[]) {
            Ok(_) => panic!("missing white king must fail"),
            Err(error) => error,
        };
        assert!(!error.is_empty());
    }

    #[test]
    fn rejects_sfen_with_more_than_a_full_piece_set() {
        let spec = PositionSpec::Sfen {
            board: "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "Pp".to_owned(),
            ply: "1".to_owned(),
        };
        let error = match replay(&spec, &[]) {
            Ok(_) => panic!("extra hand pieces must fail"),
            Err(error) => error,
        };
        assert!(error.contains("too many pawns"));
    }

    #[test]
    fn rejects_too_many_of_one_piece_across_board_and_hands() {
        let spec = PositionSpec::Sfen {
            board: "4k4/LLLL4l/9/9/9/9/9/9/4K4".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "-".to_owned(),
            ply: "1".to_owned(),
        };
        let error = match replay(&spec, &[]) {
            Ok(_) => panic!("five lances must fail"),
            Err(error) => error,
        };
        assert!(error.contains("too many lances"));
    }

    #[test]
    fn rejects_move_that_targets_the_opposing_king() {
        let spec = PositionSpec::Sfen {
            board: "4k4/4R4/9/9/9/9/9/9/4K4".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "-".to_owned(),
            ply: "1".to_owned(),
        };
        let error = match replay(&spec, &["5b5a+".to_owned()]) {
            Ok(_) => panic!("king capture must fail"),
            Err(error) => error,
        };
        assert!(error.contains("captures the opposing king"));
    }

    #[test]
    fn rejects_sfen_that_permits_capturing_the_opposing_king() {
        let spec = PositionSpec::Sfen {
            board: "4k4/4R4/9/9/9/9/9/9/4K4".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "-".to_owned(),
            ply: "1".to_owned(),
        };
        let error = match replay(&spec, &[]) {
            Ok(_) => panic!("capturable opposing king must fail"),
            Err(error) => error,
        };
        assert!(error.contains("captures the opposing king"));
    }

    #[test]
    fn enforces_root_game_ply_boundary() {
        let spec = PositionSpec::Sfen {
            board: "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "-".to_owned(),
            ply: MAX_ROOT_GAME_PLY.to_string(),
        };
        assert!(replay(&spec, &[]).is_ok(), "maximum safe root ply must be accepted");

        let spec = PositionSpec::Sfen {
            board: "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "-".to_owned(),
            ply: (MAX_ROOT_GAME_PLY + 1).to_string(),
        };
        let error = match replay(&spec, &[]) {
            Ok(_) => panic!("unsafe root ply must fail"),
            Err(error) => error,
        };
        assert!(error.contains("maximum root game ply"));
    }

    #[test]
    fn rejects_huge_sfen_hand_counts_before_parsing() {
        let spec = PositionSpec::Sfen {
            board: "4k4/9/9/9/9/9/9/9/4K4".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "100000000P".to_owned(),
            ply: "1".to_owned(),
        };
        let error = match replay(&spec, &[]) {
            Ok(_) => panic!("huge hand count must fail"),
            Err(error) => error,
        };
        assert!(error.contains("hand count exceeds physical maximum"));
    }

    #[test]
    fn accepts_physical_sfen_hand_maxima() {
        let spec = PositionSpec::Sfen {
            board: "4k4/9/9/9/9/9/9/9/4K4".to_owned(),
            side_to_move: "b".to_owned(),
            hands: "18P4L4N4S4G2B2R".to_owned(),
            ply: "1".to_owned(),
        };
        assert!(replay(&spec, &[]).is_ok());
    }

    #[test]
    fn rejects_noncanonical_sfen_hand_grammar() {
        for hands in ["", "0P", "2", "P-", "Ｐ"] {
            let error = validate_sfen_hands(hands).expect_err("invalid hands must fail");
            assert!(!error.is_empty());
        }
    }
}

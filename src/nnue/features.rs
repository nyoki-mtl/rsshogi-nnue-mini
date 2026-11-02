//! Position and move conversion into perspective-independent A-only feature atoms.

use rsnn_package::aonly_feature_schema::{
    AbsoluteOwner, BaseFeatureDelta, BoardFeatureKind, HandFeatureKind, SemanticAtom,
};
use rsshogi::board::Position;
use rsshogi::types::{Color, Hand, HandPiece, Move32, Piece, PieceType, Square};

/// Upper bound on active base features: 38 board pieces, two kings, and no more.
pub(super) const MAX_ACTIVE: usize = rsnn_package::aonly_feature_schema::BASE_MAX_ACTIVE;

const HAND_DOMAINS: [(HandFeatureKind, HandPiece); 7] = [
    (HandFeatureKind::Pawn, HandPiece::PAWN),
    (HandFeatureKind::Lance, HandPiece::LANCE),
    (HandFeatureKind::Knight, HandPiece::KNIGHT),
    (HandFeatureKind::Silver, HandPiece::SILVER),
    (HandFeatureKind::Gold, HandPiece::GOLD),
    (HandFeatureKind::Bishop, HandPiece::BISHOP),
    (HandFeatureKind::Rook, HandPiece::ROOK),
];

/// Every active atom of `position`: board pieces, both kings, and unary hand slots.
pub(super) fn position_atoms(position: &Position) -> Result<Vec<SemanticAtom>, String> {
    let mut atoms = Vec::with_capacity(MAX_ACTIVE);
    for square in Square::iter() {
        let piece = position.piece_on(square);
        if piece.is_empty() {
            continue;
        }
        atoms.push(board_atom(piece, square)?);
    }
    for color in [Color::BLACK, Color::WHITE] {
        for &(kind, hand_piece) in &HAND_DOMAINS {
            let count = Hand::count_of(position.hand(color), hand_piece);
            if count > u32::from(rsnn_package::aonly_feature_schema::HAND_MAX[kind as usize]) {
                return Err("A-only hand count exceeds schema".to_owned());
            }
            for slot in 0..count {
                atoms.push(hand_atom(color, kind, slot)?);
            }
        }
    }
    if atoms.len() > MAX_ACTIVE {
        return Err("A-only position has more than 40 features".to_owned());
    }
    Ok(atoms)
}

/// Old-to-new atom transitions made by `mv`, read from the position after the move.
///
/// A move changes at most two atoms: the moved piece, and a captured piece that becomes
/// the next unary hand slot of the mover. A drop turns the last hand slot into a board piece.
pub(super) fn move_delta(position: &Position, mv: Move32) -> Result<BaseFeatureDelta, String> {
    let mover = position.turn().flip();
    let to = mv.to_sq();
    if mv.is_drop() {
        let piece_type =
            mv.dropped_piece().ok_or_else(|| "drop move without a dropped piece".to_owned())?;
        // 打った後の枚数が、消えるunary slotの番号そのものになる。
        let (kind, remaining) = hand_slot(position, mover, piece_type)?;
        let old = hand_atom(mover, kind, remaining)?;
        let new = board_atom(Piece::from_parts(mover, piece_type), to)?;
        return Ok(BaseFeatureDelta::one(old, new));
    }
    let moved_after = position.piece_on(to);
    let moved_before = if mv.is_promotion() {
        Piece::from_parts(moved_after.color(), moved_after.piece_type().demote())
    } else {
        moved_after
    };
    let moved = [board_atom(moved_before, mv.from_sq())?, board_atom(moved_after, to)?];
    let captured = position.captured_piece();
    if captured == Piece::NONE {
        return Ok(BaseFeatureDelta::one(moved[0], moved[1]));
    }
    // 取った後の枚数から1引いた番号が、新しく立つunary slotになる。
    let (kind, held) = hand_slot(position, mover, captured.piece_type().demote())?;
    let slot = held.checked_sub(1).ok_or_else(|| "capture without a hand gain".to_owned())?;
    let captured = [board_atom(captured, to)?, hand_atom(mover, kind, slot)?];
    Ok(BaseFeatureDelta::two(moved, captured))
}

fn hand_slot(
    position: &Position,
    owner: Color,
    piece_type: PieceType,
) -> Result<(HandFeatureKind, u32), String> {
    let &(kind, hand_piece) = HAND_DOMAINS
        .iter()
        .find(|(_, hand_piece)| hand_piece.to_piece_type() == piece_type)
        .ok_or_else(|| "piece type cannot be held in hand".to_owned())?;
    Ok((kind, Hand::count_of(position.hand(owner), hand_piece)))
}

fn owner(color: Color) -> AbsoluteOwner {
    if color == Color::BLACK { AbsoluteOwner::Black } else { AbsoluteOwner::White }
}

fn hand_atom(color: Color, kind: HandFeatureKind, slot: u32) -> Result<SemanticAtom, String> {
    u8::try_from(slot)
        .ok()
        .and_then(|slot| SemanticAtom::hand(owner(color), kind, slot))
        .ok_or_else(|| "invalid A-only hand feature".to_owned())
}

fn board_atom(piece: Piece, square: Square) -> Result<SemanticAtom, String> {
    let square = square.to_index() as u8;
    let atom = if piece.piece_type() == PieceType::KING {
        SemanticAtom::king(owner(piece.color()), square)
    } else {
        SemanticAtom::board(owner(piece.color()), board_kind(piece.piece_type())?, square)
    };
    atom.ok_or_else(|| "invalid A-only board feature".to_owned())
}

fn board_kind(piece_type: PieceType) -> Result<BoardFeatureKind, String> {
    match piece_type {
        PieceType::PAWN => Ok(BoardFeatureKind::Pawn),
        PieceType::LANCE => Ok(BoardFeatureKind::Lance),
        PieceType::KNIGHT => Ok(BoardFeatureKind::Knight),
        PieceType::SILVER => Ok(BoardFeatureKind::Silver),
        PieceType::GOLD => Ok(BoardFeatureKind::Gold),
        PieceType::PRO_PAWN => Ok(BoardFeatureKind::ProPawn),
        PieceType::PRO_LANCE => Ok(BoardFeatureKind::ProLance),
        PieceType::PRO_KNIGHT => Ok(BoardFeatureKind::ProKnight),
        PieceType::PRO_SILVER => Ok(BoardFeatureKind::ProSilver),
        PieceType::BISHOP => Ok(BoardFeatureKind::Bishop),
        PieceType::HORSE => Ok(BoardFeatureKind::Horse),
        PieceType::ROOK => Ok(BoardFeatureKind::Rook),
        PieceType::DRAGON => Ok(BoardFeatureKind::Dragon),
        _ => Err("unsupported A-only board piece".to_owned()),
    }
}

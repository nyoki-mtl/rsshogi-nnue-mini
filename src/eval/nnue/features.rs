//! Position and move conversion into perspective-independent A-only feature atoms.

use std::ops::Deref;

use super::package::schema::{
    AbsoluteOwner, BASE_MAX_ACTIVE, BaseFeatureDelta, BoardFeatureKind, HAND_MAX, HandFeatureKind,
    SemanticAtom,
};
use rsshogi::board::Position;
use rsshogi::types::{Color, Hand, HandPiece, Move32, Piece, PieceType, Square};

const HAND_DOMAINS: [(HandFeatureKind, HandPiece); 7] = [
    (HandFeatureKind::Pawn, HandPiece::PAWN),
    (HandFeatureKind::Lance, HandPiece::LANCE),
    (HandFeatureKind::Knight, HandPiece::KNIGHT),
    (HandFeatureKind::Silver, HandPiece::SILVER),
    (HandFeatureKind::Gold, HandPiece::GOLD),
    (HandFeatureKind::Bishop, HandPiece::BISHOP),
    (HandFeatureKind::Rook, HandPiece::ROOK),
];

/// Stack-allocated active-atom buffer (`<= 40` features for any legal Shogi position).
#[derive(Clone, Copy)]
pub(super) struct AtomList {
    atoms: [SemanticAtom; BASE_MAX_ACTIVE],
    len: u8,
}

impl AtomList {
    const FILLER: SemanticAtom = match SemanticAtom::king(AbsoluteOwner::Black, 0) {
        Some(atom) => atom,
        None => unreachable!(),
    };

    const fn new() -> Self {
        Self { atoms: [Self::FILLER; BASE_MAX_ACTIVE], len: 0 }
    }

    #[inline]
    fn push(&mut self, atom: SemanticAtom) -> Result<(), &'static str> {
        let index = usize::from(self.len);
        if index >= BASE_MAX_ACTIVE {
            return Err("A-only position has more than 40 features");
        }
        self.atoms[index] = atom;
        self.len += 1;
        Ok(())
    }
}

impl Deref for AtomList {
    type Target = [SemanticAtom];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.atoms[..usize::from(self.len)]
    }
}

/// Every active atom of `position`: board pieces, both kings, and unary hand slots.
pub(super) fn position_atoms(position: &Position) -> Result<AtomList, &'static str> {
    let mut atoms = AtomList::new();
    let mut occupied = position.pieces();
    while let Some(square) = occupied.pop_lsb() {
        let piece = position.piece_on(square);
        atoms.push(board_atom(piece, square)?)?;
    }
    for color in [Color::BLACK, Color::WHITE] {
        let hand = position.hand(color);
        if hand == Hand::ZERO {
            continue;
        }
        for &(kind, hand_piece) in &HAND_DOMAINS {
            let count = Hand::count_of(hand, hand_piece);
            if count > u32::from(HAND_MAX[kind as usize]) {
                return Err("A-only hand count exceeds schema");
            }
            for slot in 0..count {
                atoms.push(hand_atom(color, kind, slot)?)?;
            }
        }
    }
    Ok(atoms)
}

/// Old-to-new atom transitions made by `mv`, read from the position after the move.
///
/// A move changes at most two atoms: the moved piece, and a captured piece that becomes
/// the next unary hand slot of the mover. A drop turns the last hand slot into a board piece.
#[inline]
pub(super) fn move_delta(
    position: &Position,
    mv: Move32,
) -> Result<BaseFeatureDelta, &'static str> {
    let mover = position.turn().flip();
    let to = mv.to_sq();
    if mv.is_drop() {
        let piece_type = mv.dropped_piece().ok_or("drop move without a dropped piece")?;
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
    let slot = held.checked_sub(1).ok_or("capture without a hand gain")?;
    let captured = [board_atom(captured, to)?, hand_atom(mover, kind, slot)?];
    Ok(BaseFeatureDelta::two(moved, captured))
}

#[inline]
const fn hand_domain(piece_type: PieceType) -> Option<(HandFeatureKind, HandPiece)> {
    match piece_type {
        PieceType::PAWN => Some((HandFeatureKind::Pawn, HandPiece::PAWN)),
        PieceType::LANCE => Some((HandFeatureKind::Lance, HandPiece::LANCE)),
        PieceType::KNIGHT => Some((HandFeatureKind::Knight, HandPiece::KNIGHT)),
        PieceType::SILVER => Some((HandFeatureKind::Silver, HandPiece::SILVER)),
        PieceType::GOLD => Some((HandFeatureKind::Gold, HandPiece::GOLD)),
        PieceType::BISHOP => Some((HandFeatureKind::Bishop, HandPiece::BISHOP)),
        PieceType::ROOK => Some((HandFeatureKind::Rook, HandPiece::ROOK)),
        _ => None,
    }
}

#[inline]
fn hand_slot(
    position: &Position,
    owner: Color,
    piece_type: PieceType,
) -> Result<(HandFeatureKind, u32), &'static str> {
    let (kind, hand_piece) = hand_domain(piece_type).ok_or("piece type cannot be held in hand")?;
    Ok((kind, Hand::count_of(position.hand(owner), hand_piece)))
}

#[inline]
const fn owner(color: Color) -> AbsoluteOwner {
    if matches!(color, Color::BLACK) { AbsoluteOwner::Black } else { AbsoluteOwner::White }
}

#[inline]
fn hand_atom(color: Color, kind: HandFeatureKind, slot: u32) -> Result<SemanticAtom, &'static str> {
    u8::try_from(slot)
        .ok()
        .and_then(|slot| SemanticAtom::hand(owner(color), kind, slot))
        .ok_or("invalid A-only hand feature")
}

#[inline]
fn board_atom(piece: Piece, square: Square) -> Result<SemanticAtom, &'static str> {
    let square = square.to_index() as u8;
    let atom = if piece.piece_type() == PieceType::KING {
        SemanticAtom::king(owner(piece.color()), square)
    } else {
        SemanticAtom::board(owner(piece.color()), board_kind(piece.piece_type())?, square)
    };
    atom.ok_or("invalid A-only board feature")
}

#[inline]
const fn board_kind(piece_type: PieceType) -> Result<BoardFeatureKind, &'static str> {
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
        _ => Err("unsupported A-only board piece"),
    }
}

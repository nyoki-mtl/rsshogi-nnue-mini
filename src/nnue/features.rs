//! Position and move conversion into perspective-independent SFNNv15 feature atoms.

use rsnn_package::sfnnv15_feature_schema::{
    AbsoluteOwner, BaseFeatureDelta, BoardFeatureKind, HandFeatureKind, SemanticAtom,
};
use rsshogi::board::Position;
use rsshogi::types::{Color, Hand, HandPiece, Move32, Piece, PieceType, Square};

/// Upper bound on active base features: 38 board pieces, two kings, and no more.
pub(super) const MAX_ACTIVE: usize = rsnn_package::sfnnv15_feature_schema::BASE_MAX_ACTIVE;

const HAND_DOMAINS: [(HandFeatureKind, HandPiece); 7] = [
    (HandFeatureKind::Pawn, HandPiece::PAWN),
    (HandFeatureKind::Lance, HandPiece::LANCE),
    (HandFeatureKind::Knight, HandPiece::KNIGHT),
    (HandFeatureKind::Silver, HandPiece::SILVER),
    (HandFeatureKind::Gold, HandPiece::GOLD),
    (HandFeatureKind::Bishop, HandPiece::BISHOP),
    (HandFeatureKind::Rook, HandPiece::ROOK),
];

/// Board pieces and hand counts, kept by the refresh cache to diff against a new position.
#[derive(Clone, Copy)]
pub(super) struct Placement {
    board: [Piece; SQUARES],
    hands: [[u8; HAND_DOMAINS.len()]; 2],
}

/// Atoms of one side of a diff. A diff between two legal positions has at most
/// `MAX_ACTIVE` atoms on each side.
pub(super) struct AtomList {
    atoms: [SemanticAtom; MAX_ACTIVE],
    len: usize,
}

const SQUARES: usize = 81;

impl Placement {
    pub(super) const EMPTY: Self =
        Self { board: [Piece::NONE; SQUARES], hands: [[0; HAND_DOMAINS.len()]; 2] };

    pub(super) fn of(position: &Position) -> Self {
        let mut placement = Self::EMPTY;
        for square in Square::iter() {
            placement.board[square.to_index()] = position.piece_on(square);
        }
        for (hands, color) in placement.hands.iter_mut().zip([Color::BLACK, Color::WHITE]) {
            for (count, &(_, hand_piece)) in hands.iter_mut().zip(&HAND_DOMAINS) {
                *count = Hand::count_of(position.hand(color), hand_piece) as u8;
            }
        }
        placement
    }

    /// Atoms active in `self` but not in `next` go to `removed`, and the reverse to `added`.
    pub(super) fn diff(&self, next: &Self, removed: &mut AtomList, added: &mut AtomList) {
        for (square, (&old, &new)) in Square::iter().zip(self.board.iter().zip(&next.board)) {
            if old == new {
                continue;
            }
            if !old.is_empty() {
                removed.push(board_atom(old, square).expect("cached board piece is valid"));
            }
            if !new.is_empty() {
                added.push(board_atom(new, square).expect("legal board piece is valid"));
            }
        }
        for (color, (old_hands, new_hands)) in
            [Color::BLACK, Color::WHITE].into_iter().zip(self.hands.iter().zip(&next.hands))
        {
            for (&(kind, _), (&old, &new)) in
                HAND_DOMAINS.iter().zip(old_hands.iter().zip(new_hands))
            {
                // Hand features are unary, so only the slots between the two counts change.
                let (list, slots) =
                    if old > new { (&mut *removed, new..old) } else { (&mut *added, old..new) };
                for slot in slots {
                    list.push(hand_atom(color, kind, u32::from(slot)).expect("legal hand slot"));
                }
            }
        }
    }
}

impl AtomList {
    pub(super) fn new() -> Self {
        let filler = SemanticAtom::king(AbsoluteOwner::Black, 0).expect("square 0 is valid");
        Self { atoms: [filler; MAX_ACTIVE], len: 0 }
    }

    fn push(&mut self, atom: SemanticAtom) {
        self.atoms[self.len] = atom;
        self.len += 1;
    }

    pub(super) fn as_slice(&self) -> &[SemanticAtom] {
        &self.atoms[..self.len]
    }
}

/// Every active atom of `position`: board pieces, both kings, and unary hand slots.
#[cfg(test)]
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
            if count > u32::from(rsnn_package::sfnnv15_feature_schema::HAND_MAX[kind as usize]) {
                return Err("SFNNv15 hand count exceeds schema".to_owned());
            }
            for slot in 0..count {
                atoms.push(hand_atom(color, kind, slot)?);
            }
        }
    }
    if atoms.len() > MAX_ACTIVE {
        return Err("SFNNv15 position has more than 40 features".to_owned());
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
        .ok_or_else(|| "invalid SFNNv15 hand feature".to_owned())
}

fn board_atom(piece: Piece, square: Square) -> Result<SemanticAtom, String> {
    let square = square.to_index() as u8;
    let atom = if piece.piece_type() == PieceType::KING {
        SemanticAtom::king(owner(piece.color()), square)
    } else {
        SemanticAtom::board(owner(piece.color()), board_kind(piece.piece_type())?, square)
    };
    atom.ok_or_else(|| "invalid SFNNv15 board feature".to_owned())
}

fn board_kind(piece_type: PieceType) -> Result<BoardFeatureKind, String> {
    match piece_type {
        PieceType::PAWN => Ok(BoardFeatureKind::Pawn),
        PieceType::LANCE => Ok(BoardFeatureKind::Lance),
        PieceType::KNIGHT => Ok(BoardFeatureKind::Knight),
        PieceType::SILVER => Ok(BoardFeatureKind::Silver),
        PieceType::GOLD
        | PieceType::PRO_PAWN
        | PieceType::PRO_LANCE
        | PieceType::PRO_KNIGHT
        | PieceType::PRO_SILVER => Ok(BoardFeatureKind::Gold),
        PieceType::BISHOP => Ok(BoardFeatureKind::Bishop),
        PieceType::HORSE => Ok(BoardFeatureKind::Horse),
        PieceType::ROOK => Ok(BoardFeatureKind::Rook),
        PieceType::DRAGON => Ok(BoardFeatureKind::Dragon),
        _ => Err("unsupported SFNNv15 board piece".to_owned()),
    }
}

//! Pure feature-schema authority for the direct `SFNNv15` shogi contract.

pub const RECIPE_ID: &str = "sfnnv15_v2";
pub const RECIPE_VERSION: u32 = 2;
pub const FEATURE_SCHEMA_ID: &str = "sfnnv15_shogi";
pub const FEATURE_SCHEMA_VERSION: u32 = 2;
pub const BASE_INDEXING_ID: &str = "halfka_hm_direct_v2";
/// 一局面で同時に立つbase特徴の上限。盤上の駒38枚と両玉。
///
/// 推論側のactive容量であり、numeric rangeの証明もこの上限で行う。
pub const BASE_MAX_ACTIVE: usize = 40;
pub const PROGRESS_MAGIC: [u8; 8] = *b"RSKPPRG2";
pub const PROGRESS_VERSION: u32 = 2;

/// Maximum legal unary hand slots by [`HandFeatureKind`].
pub const HAND_MAX: [u8; 7] = [18, 4, 4, 4, 4, 2, 2];
/// First plane of each hand kind within one relative owner.
pub const HAND_PREFIX: [u16; 7] = [0, 18, 22, 26, 30, 34, 36];
pub const HAND_OWNER_STRIDE: u16 = 38;
pub const HAND_PLANES: u16 = 76;
pub const BOARD_KIND_COUNT: u16 = 9;
pub const SQUARE_COUNT: u16 = 81;
const SQUARE_COUNT_U8: u8 = 81;
pub const BASE_PLANES: u16 = 1_615;
pub const BASE_KING_BUCKETS: u16 = 45;
const BASE_KING_BUCKETS_U8: u8 = 45;
pub const BASE_INPUTS: u32 = 72_675;
pub const PROGRESS_PLANES: u16 = 1_534;
pub const PROGRESS_KING_BUCKETS: u16 = 81;
pub const PROGRESS_INPUTS: u32 = 124_254;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AbsoluteOwner {
    Black,
    White,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Perspective {
    Black,
    White,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RelativeOwner {
    Friend,
    Enemy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HandFeatureKind {
    Pawn,
    Lance,
    Knight,
    Silver,
    Gold,
    Bishop,
    Rook,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BoardFeatureKind {
    Pawn,
    Lance,
    Knight,
    Silver,
    Gold,
    Bishop,
    Horse,
    Rook,
    Dragon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FeatureClass {
    Hand,
    Board,
    King,
}

/// Perspective-independent semantic feature atom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct SemanticAtom(u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticAtomDecodeError {
    ReservedBits,
    InvalidClass,
    InvalidKind,
    InvalidPlace,
}

impl SemanticAtom {
    #[inline]
    #[must_use]
    pub const fn hand(owner: AbsoluteOwner, kind: HandFeatureKind, slot: u8) -> Option<Self> {
        if slot >= HAND_MAX[kind as usize] {
            return None;
        }
        Some(Self(pack_atom(slot, kind as u8, owner, FeatureClass::Hand)))
    }

    #[inline]
    #[must_use]
    pub const fn board(owner: AbsoluteOwner, kind: BoardFeatureKind, square: u8) -> Option<Self> {
        if square >= SQUARE_COUNT_U8 {
            return None;
        }
        Some(Self(pack_atom(square, kind as u8, owner, FeatureClass::Board)))
    }

    #[inline]
    #[must_use]
    pub const fn king(owner: AbsoluteOwner, square: u8) -> Option<Self> {
        if square >= SQUARE_COUNT_U8 {
            return None;
        }
        Some(Self(pack_atom(square, 0, owner, FeatureClass::King)))
    }

    #[must_use]
    pub const fn class(self) -> FeatureClass {
        match (self.0 >> 12) & 0b11 {
            0 => FeatureClass::Hand,
            1 => FeatureClass::Board,
            2 => FeatureClass::King,
            _ => unreachable!(),
        }
    }

    #[must_use]
    pub const fn owner(self) -> AbsoluteOwner {
        if self.0 & (1 << 11) == 0 { AbsoluteOwner::Black } else { AbsoluteOwner::White }
    }

    #[must_use]
    pub const fn place(self) -> u8 {
        (self.0 & 0x7f) as u8
    }

    #[must_use]
    pub const fn kind_code(self) -> u8 {
        ((self.0 >> 7) & 0x0f) as u8
    }

    #[must_use]
    pub const fn hand_kind(self) -> Option<HandFeatureKind> {
        if !matches!(self.class(), FeatureClass::Hand) {
            return None;
        }
        match self.kind_code() {
            0 => Some(HandFeatureKind::Pawn),
            1 => Some(HandFeatureKind::Lance),
            2 => Some(HandFeatureKind::Knight),
            3 => Some(HandFeatureKind::Silver),
            4 => Some(HandFeatureKind::Gold),
            5 => Some(HandFeatureKind::Bishop),
            6 => Some(HandFeatureKind::Rook),
            _ => None,
        }
    }

    #[must_use]
    pub const fn board_kind(self) -> Option<BoardFeatureKind> {
        if !matches!(self.class(), FeatureClass::Board) {
            return None;
        }
        match self.kind_code() {
            0 => Some(BoardFeatureKind::Pawn),
            1 => Some(BoardFeatureKind::Lance),
            2 => Some(BoardFeatureKind::Knight),
            3 => Some(BoardFeatureKind::Silver),
            4 => Some(BoardFeatureKind::Gold),
            5 => Some(BoardFeatureKind::Bishop),
            6 => Some(BoardFeatureKind::Horse),
            7 => Some(BoardFeatureKind::Rook),
            8 => Some(BoardFeatureKind::Dragon),
            _ => None,
        }
    }

    #[must_use]
    pub const fn packed(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for SemanticAtom {
    type Error = SemanticAtomDecodeError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value & 0xc000 != 0 {
            return Err(SemanticAtomDecodeError::ReservedBits);
        }
        let atom = Self(value);
        match (value >> 12) & 0b11 {
            0 => {
                let Some(kind) = atom.hand_kind() else {
                    return Err(SemanticAtomDecodeError::InvalidKind);
                };
                if atom.place() >= HAND_MAX[kind as usize] {
                    return Err(SemanticAtomDecodeError::InvalidPlace);
                }
            }
            1 => {
                if atom.board_kind().is_none() {
                    return Err(SemanticAtomDecodeError::InvalidKind);
                }
                if atom.place() >= SQUARE_COUNT_U8 {
                    return Err(SemanticAtomDecodeError::InvalidPlace);
                }
            }
            2 => {
                if atom.kind_code() != 0 {
                    return Err(SemanticAtomDecodeError::InvalidKind);
                }
                if atom.place() >= SQUARE_COUNT_U8 {
                    return Err(SemanticAtomDecodeError::InvalidPlace);
                }
            }
            _ => return Err(SemanticAtomDecodeError::InvalidClass),
        }
        Ok(atom)
    }
}

const fn pack_atom(place: u8, kind: u8, owner: AbsoluteOwner, class: FeatureClass) -> u16 {
    place as u16 | ((kind as u16) << 7) | ((owner as u16) << 11) | ((class as u16) << 12)
}

/// At most two old-to-new semantic transitions for one move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct BaseFeatureDelta {
    transitions: [[SemanticAtom; 2]; 2],
    len: u8,
}

impl BaseFeatureDelta {
    #[must_use]
    pub const fn empty() -> Self {
        let filler = SemanticAtom(0);
        Self { transitions: [[filler; 2]; 2], len: 0 }
    }

    #[must_use]
    pub const fn one(old: SemanticAtom, new: SemanticAtom) -> Self {
        let transition = [old, new];
        Self { transitions: [transition; 2], len: 1 }
    }

    #[must_use]
    pub const fn two(first: [SemanticAtom; 2], second: [SemanticAtom; 2]) -> Self {
        Self { transitions: [first, second], len: 2 }
    }

    #[must_use]
    pub fn transitions(&self) -> &[[SemanticAtom; 2]] {
        &self.transitions[..usize::from(self.len)]
    }
}

#[inline]
#[must_use]
pub const fn relative_owner(owner: AbsoluteOwner, perspective: Perspective) -> RelativeOwner {
    if owner as u8 == perspective as u8 { RelativeOwner::Friend } else { RelativeOwner::Enemy }
}

#[inline]
#[must_use]
pub const fn normalize_square(square: u8, perspective: Perspective) -> u8 {
    match perspective {
        Perspective::Black => square,
        Perspective::White => 80 - square,
    }
}

#[inline]
#[must_use]
pub const fn mirror_square(square: u8) -> u8 {
    (8 - square / 9) * 9 + square % 9
}

#[inline]
#[must_use]
pub const fn mirror_for_normalized_king(king_square: u8) -> bool {
    king_square >= BASE_KING_BUCKETS_U8
}

#[inline]
#[must_use]
pub const fn base_plane(atom: SemanticAtom, perspective: Perspective, mirror: bool) -> u16 {
    let relative = relative_owner(atom.owner(), perspective) as u16;
    match atom.class() {
        FeatureClass::Hand => {
            relative * HAND_OWNER_STRIDE
                + HAND_PREFIX[atom.kind_code() as usize]
                + atom.place() as u16
        }
        FeatureClass::Board => {
            let square = normalize_square(atom.place(), perspective);
            let square = if mirror { mirror_square(square) } else { square };
            HAND_PLANES
                + (relative * BOARD_KIND_COUNT + atom.kind_code() as u16) * SQUARE_COUNT
                + square as u16
        }
        FeatureClass::King => {
            let square = normalize_square(atom.place(), perspective);
            let square = if mirror { mirror_square(square) } else { square };
            HAND_PLANES + 2 * BOARD_KIND_COUNT * SQUARE_COUNT + square as u16
        }
    }
}

#[inline]
#[must_use]
pub const fn base_index(
    atom: SemanticAtom,
    perspective: Perspective,
    own_king_square: u8,
) -> Option<u32> {
    if own_king_square >= SQUARE_COUNT_U8 {
        return None;
    }
    let king = normalize_square(own_king_square, perspective);
    let mirror = mirror_for_normalized_king(king);
    let bucket = if mirror { mirror_square(king) } else { king };
    Some(bucket as u32 * BASE_PLANES as u32 + base_plane(atom, perspective, mirror) as u32)
}

#[inline]
#[must_use]
pub const fn progress_plane(atom: SemanticAtom, perspective: Perspective) -> Option<u16> {
    match atom.class() {
        FeatureClass::King => None,
        FeatureClass::Hand => {
            let relative = relative_owner(atom.owner(), perspective) as u16;
            Some(
                relative * HAND_OWNER_STRIDE
                    + HAND_PREFIX[atom.kind_code() as usize]
                    + atom.place() as u16,
            )
        }
        FeatureClass::Board => {
            let relative = relative_owner(atom.owner(), perspective) as u16;
            let square = normalize_square(atom.place(), perspective);
            Some(
                HAND_PLANES
                    + (relative * BOARD_KIND_COUNT + atom.kind_code() as u16) * SQUARE_COUNT
                    + square as u16,
            )
        }
    }
}

#[inline]
#[must_use]
pub const fn progress_index(
    atom: SemanticAtom,
    perspective: Perspective,
    own_king_square: u8,
) -> Option<u32> {
    if own_king_square >= SQUARE_COUNT_U8 {
        return None;
    }
    match progress_plane(atom, perspective) {
        Some(plane) => Some(
            normalize_square(own_king_square, perspective) as u32 * PROGRESS_PLANES as u32
                + plane as u32,
        ),
        None => None,
    }
}

/// Sort indices only when producing canonical fixtures or cross-repository evidence.
pub fn canonicalize_indices(indices: &mut [u32]) {
    indices.sort_unstable();
}

const _: () = assert!(size_of::<SemanticAtom>() == 2);
const _: () = assert!(align_of::<SemanticAtom>() == 2);
const _: () = assert!(size_of::<BaseFeatureDelta>() == 10);
const _: () = assert!(align_of::<BaseFeatureDelta>() == 2);
const _: () = assert!(BASE_PLANES as u32 * BASE_KING_BUCKETS as u32 == BASE_INPUTS);
const _: () = assert!(PROGRESS_PLANES as u32 * PROGRESS_KING_BUCKETS as u32 == PROGRESS_INPUTS);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const OWNERS: [AbsoluteOwner; 2] = [AbsoluteOwner::Black, AbsoluteOwner::White];
    const PERSPECTIVES: [Perspective; 2] = [Perspective::Black, Perspective::White];
    const HAND_KINDS: [HandFeatureKind; 7] = [
        HandFeatureKind::Pawn,
        HandFeatureKind::Lance,
        HandFeatureKind::Knight,
        HandFeatureKind::Silver,
        HandFeatureKind::Gold,
        HandFeatureKind::Bishop,
        HandFeatureKind::Rook,
    ];
    const BOARD_KINDS: [BoardFeatureKind; 9] = [
        BoardFeatureKind::Pawn,
        BoardFeatureKind::Lance,
        BoardFeatureKind::Knight,
        BoardFeatureKind::Silver,
        BoardFeatureKind::Gold,
        BoardFeatureKind::Bishop,
        BoardFeatureKind::Horse,
        BoardFeatureKind::Rook,
        BoardFeatureKind::Dragon,
    ];

    #[test]
    fn all_legal_atoms_are_unique_and_in_range() {
        let mut atoms = BTreeSet::new();
        for owner in OWNERS {
            for kind in HAND_KINDS {
                for slot in 0..HAND_MAX[kind as usize] {
                    assert!(atoms.insert(SemanticAtom::hand(owner, kind, slot).unwrap().0));
                }
            }
            for kind in BOARD_KINDS {
                for square in 0..81 {
                    assert!(atoms.insert(SemanticAtom::board(owner, kind, square).unwrap().0));
                }
            }
            for square in 0..81 {
                assert!(atoms.insert(SemanticAtom::king(owner, square).unwrap().0));
            }
        }
        assert_eq!(atoms.len(), 2 * (38 + 9 * 81 + 81));
        assert!(SemanticAtom::hand(AbsoluteOwner::Black, HandFeatureKind::Pawn, 18).is_none());
        assert!(SemanticAtom::board(AbsoluteOwner::Black, BoardFeatureKind::Pawn, 81).is_none());
        assert!(SemanticAtom::king(AbsoluteOwner::Black, 81).is_none());

        let hand = SemanticAtom::hand(AbsoluteOwner::White, HandFeatureKind::Rook, 1).unwrap();
        assert_eq!(hand.class(), FeatureClass::Hand);
        assert_eq!(hand.owner(), AbsoluteOwner::White);
        assert_eq!(hand.hand_kind(), Some(HandFeatureKind::Rook));
        assert_eq!(hand.board_kind(), None);
        assert_eq!(hand.place(), 1);

        let board =
            SemanticAtom::board(AbsoluteOwner::Black, BoardFeatureKind::Dragon, 80).unwrap();
        assert_eq!(board.board_kind(), Some(BoardFeatureKind::Dragon));
        assert_eq!(board.hand_kind(), None);
    }

    #[test]
    fn semantic_atom_checked_codec_round_trips_and_rejects_invalid_words() {
        for owner in OWNERS {
            for kind in HAND_KINDS {
                for slot in 0..HAND_MAX[kind as usize] {
                    let atom = SemanticAtom::hand(owner, kind, slot).unwrap();
                    assert_eq!(SemanticAtom::try_from(atom.packed()), Ok(atom));
                    assert_eq!(atom.packed() & 0xc000, 0);
                }
            }
            for kind in BOARD_KINDS {
                for square in 0..81 {
                    let atom = SemanticAtom::board(owner, kind, square).unwrap();
                    assert_eq!(SemanticAtom::try_from(atom.packed()), Ok(atom));
                    assert_eq!(atom.packed() & 0xc000, 0);
                }
            }
            for square in 0..81 {
                let atom = SemanticAtom::king(owner, square).unwrap();
                assert_eq!(SemanticAtom::try_from(atom.packed()), Ok(atom));
                assert_eq!(atom.packed() & 0xc000, 0);
            }
        }

        assert_eq!(SemanticAtom::try_from(0x4000), Err(SemanticAtomDecodeError::ReservedBits));
        assert_eq!(SemanticAtom::try_from(3 << 12), Err(SemanticAtomDecodeError::InvalidClass));
        assert_eq!(SemanticAtom::try_from(7 << 7), Err(SemanticAtomDecodeError::InvalidKind));
        assert_eq!(SemanticAtom::try_from(18), Err(SemanticAtomDecodeError::InvalidPlace));
        assert_eq!(
            SemanticAtom::try_from((1 << 12) | (9 << 7)),
            Err(SemanticAtomDecodeError::InvalidKind)
        );
        assert_eq!(
            SemanticAtom::try_from((1 << 12) | 0x51),
            Err(SemanticAtomDecodeError::InvalidPlace)
        );
        assert_eq!(
            SemanticAtom::try_from((2 << 12) | (1 << 7)),
            Err(SemanticAtomDecodeError::InvalidKind)
        );
    }

    #[test]
    fn base_planes_cover_each_perspective_exactly() {
        for perspective in PERSPECTIVES {
            for mirror in [false, true] {
                let mut planes = BTreeSet::new();
                for owner in OWNERS {
                    for kind in HAND_KINDS {
                        for slot in 0..HAND_MAX[kind as usize] {
                            planes.insert(base_plane(
                                SemanticAtom::hand(owner, kind, slot).unwrap(),
                                perspective,
                                mirror,
                            ));
                        }
                    }
                    for kind in BOARD_KINDS {
                        for square in 0..81 {
                            planes.insert(base_plane(
                                SemanticAtom::board(owner, kind, square).unwrap(),
                                perspective,
                                mirror,
                            ));
                        }
                    }
                }
                for square in 0..81 {
                    let black = base_plane(
                        SemanticAtom::king(AbsoluteOwner::Black, square).unwrap(),
                        perspective,
                        mirror,
                    );
                    let white = base_plane(
                        SemanticAtom::king(AbsoluteOwner::White, square).unwrap(),
                        perspective,
                        mirror,
                    );
                    assert_eq!(black, white);
                    planes.insert(black);
                }
                assert_eq!(planes, (0..BASE_PLANES).collect());
            }
        }
    }

    #[test]
    fn orientation_is_involutive_and_bucket_is_folded() {
        for square in 0..81 {
            assert_eq!(mirror_square(mirror_square(square)), square);
            assert_eq!(
                normalize_square(normalize_square(square, Perspective::White), Perspective::White),
                square
            );
            for perspective in PERSPECTIVES {
                let king = normalize_square(square, perspective);
                let bucket =
                    if mirror_for_normalized_king(king) { mirror_square(king) } else { king };
                assert!(bucket < 45);
            }
        }
        assert!(!mirror_for_normalized_king(44));
        assert!(mirror_for_normalized_king(45));
    }

    #[test]
    fn orientation_and_indices_match_independent_golden_values() {
        assert_eq!(normalize_square(0, Perspective::White), 80);
        assert_eq!(mirror_square(0), 72);
        assert_eq!(mirror_square(45), 27);

        let hand = SemanticAtom::hand(AbsoluteOwner::Black, HandFeatureKind::Pawn, 0).unwrap();
        assert_eq!(base_plane(hand, Perspective::Black, false), 0);
        assert_eq!(base_plane(hand, Perspective::White, false), 38);

        let board = SemanticAtom::board(AbsoluteOwner::Black, BoardFeatureKind::Pawn, 0).unwrap();
        assert_eq!(base_plane(board, Perspective::Black, false), 76);
        assert_eq!(base_plane(board, Perspective::Black, true), 148);
        assert_eq!(base_plane(board, Perspective::White, false), 885);
        assert_eq!(base_index(board, Perspective::Black, 0), Some(76));
        assert_eq!(base_index(board, Perspective::Black, 45), Some(43_753));
        assert_eq!(progress_index(board, Perspective::White, 80), Some(885));

        let king = SemanticAtom::king(AbsoluteOwner::White, 0).unwrap();
        assert_eq!(base_plane(king, Perspective::Black, false), 1_534);
        assert_eq!(progress_plane(king, Perspective::Black), None);
    }

    #[test]
    fn progress_excludes_kings_and_covers_non_king_planes() {
        for perspective in PERSPECTIVES {
            let mut planes = BTreeSet::new();
            for owner in OWNERS {
                for kind in HAND_KINDS {
                    for slot in 0..HAND_MAX[kind as usize] {
                        planes.insert(
                            progress_plane(
                                SemanticAtom::hand(owner, kind, slot).unwrap(),
                                perspective,
                            )
                            .unwrap(),
                        );
                    }
                }
                for kind in BOARD_KINDS {
                    for square in 0..81 {
                        planes.insert(
                            progress_plane(
                                SemanticAtom::board(owner, kind, square).unwrap(),
                                perspective,
                            )
                            .unwrap(),
                        );
                    }
                }
                assert_eq!(
                    progress_plane(SemanticAtom::king(owner, 0).unwrap(), perspective),
                    None
                );
            }
            assert_eq!(planes, (0..PROGRESS_PLANES).collect());
        }
    }

    #[test]
    fn indices_are_bounded_and_fixture_sorting_is_explicit() {
        let atom = SemanticAtom::board(AbsoluteOwner::Black, BoardFeatureKind::Pawn, 80).unwrap();
        for perspective in PERSPECTIVES {
            for king in 0..81 {
                assert!(base_index(atom, perspective, king).unwrap() < BASE_INPUTS);
                assert!(progress_index(atom, perspective, king).unwrap() < PROGRESS_INPUTS);
            }
            assert_eq!(base_index(atom, perspective, 81), None);
            assert_eq!(progress_index(atom, perspective, 81), None);
        }
        let mut indices = [9, 1, 4];
        canonicalize_indices(&mut indices);
        assert_eq!(indices, [1, 4, 9]);
    }
}

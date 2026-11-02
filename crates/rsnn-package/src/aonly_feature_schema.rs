//! A-only full14 board and unary-hand features; KP retains its original independent schema.
pub use crate::sfnnv15_feature_schema::{
    AbsoluteOwner, HAND_MAX, HAND_PREFIX, HandFeatureKind, PROGRESS_INPUTS, PROGRESS_MAGIC,
    PROGRESS_VERSION, Perspective,
};
pub const BASE_INPUTS: u32 = 2344;
pub const BASE_MAX_ACTIVE: usize = 40;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BoardFeatureKind {
    Pawn,
    Lance,
    Knight,
    Silver,
    Gold,
    Bishop,
    Rook,
    King,
    ProPawn,
    ProLance,
    ProKnight,
    ProSilver,
    Horse,
    Dragon,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticAtom {
    owner: AbsoluteOwner,
    kind: u8,
    place: u8,
    hand: bool,
}
impl SemanticAtom {
    pub const fn hand(owner: AbsoluteOwner, kind: HandFeatureKind, slot: u8) -> Option<Self> {
        if slot >= HAND_MAX[kind as usize] {
            return None;
        }
        Some(Self { owner, kind: kind as u8, place: slot, hand: true })
    }
    pub const fn board(owner: AbsoluteOwner, kind: BoardFeatureKind, square: u8) -> Option<Self> {
        if square >= 81 {
            return None;
        }
        Some(Self { owner, kind: kind as u8, place: square, hand: false })
    }
    pub const fn king(owner: AbsoluteOwner, square: u8) -> Option<Self> {
        Self::board(owner, BoardFeatureKind::King, square)
    }
}
pub fn base_index(atom: SemanticAtom, perspective: Perspective, _king: u8) -> Option<u32> {
    let owner = u32::from(atom.owner as u8 != perspective as u8);
    Some(if atom.hand {
        2268 + owner * 38 + u32::from(HAND_PREFIX[atom.kind as usize]) + u32::from(atom.place)
    } else {
        (owner * 14 + u32::from(atom.kind)) * 81
            + u32::from(if perspective == Perspective::White {
                80 - atom.place
            } else {
                atom.place
            })
    })
}
pub fn progress_index(atom: SemanticAtom, perspective: Perspective, king: u8) -> Option<u32> {
    use crate::sfnnv15_feature_schema::{self as kp, BoardFeatureKind as K};
    let legacy = if atom.hand {
        let kind = [
            HandFeatureKind::Pawn,
            HandFeatureKind::Lance,
            HandFeatureKind::Knight,
            HandFeatureKind::Silver,
            HandFeatureKind::Gold,
            HandFeatureKind::Bishop,
            HandFeatureKind::Rook,
        ][atom.kind as usize];
        kp::SemanticAtom::hand(atom.owner, kind, atom.place)?
    } else {
        if atom.kind == BoardFeatureKind::King as u8 {
            return None;
        }
        let kind = match atom.kind {
            0 => K::Pawn,
            1 => K::Lance,
            2 => K::Knight,
            3 => K::Silver,
            4 | 8..=11 => K::Gold,
            5 => K::Bishop,
            6 => K::Rook,
            12 => K::Horse,
            13 => K::Dragon,
            _ => unreachable!(),
        };
        kp::SemanticAtom::board(atom.owner, kind, atom.place)?
    };
    kp::progress_index(legacy, perspective, king)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct BaseFeatureDelta {
    transitions: [[SemanticAtom; 2]; 2],
    len: u8,
}

impl BaseFeatureDelta {
    #[must_use]
    pub const fn empty() -> Self {
        let filler = SemanticAtom { owner: AbsoluteOwner::Black, kind: 0, place: 0, hand: true };
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_indices_are_unique_and_rotate() {
        let kinds = [
            BoardFeatureKind::Pawn,
            BoardFeatureKind::Lance,
            BoardFeatureKind::Knight,
            BoardFeatureKind::Silver,
            BoardFeatureKind::Gold,
            BoardFeatureKind::Bishop,
            BoardFeatureKind::Rook,
            BoardFeatureKind::King,
            BoardFeatureKind::ProPawn,
            BoardFeatureKind::ProLance,
            BoardFeatureKind::ProKnight,
            BoardFeatureKind::ProSilver,
            BoardFeatureKind::Horse,
            BoardFeatureKind::Dragon,
        ];
        for perspective in [Perspective::Black, Perspective::White] {
            let mut seen = std::collections::BTreeSet::new();
            for owner in [AbsoluteOwner::Black, AbsoluteOwner::White] {
                for kind in kinds {
                    for square in 0..81 {
                        let a = SemanticAtom::board(owner, kind, square).unwrap();
                        assert!(seen.insert(base_index(a, perspective, 0).unwrap()));
                    }
                }
                for kind in [
                    HandFeatureKind::Pawn,
                    HandFeatureKind::Lance,
                    HandFeatureKind::Knight,
                    HandFeatureKind::Silver,
                    HandFeatureKind::Gold,
                    HandFeatureKind::Bishop,
                    HandFeatureKind::Rook,
                ] {
                    for slot in 0..HAND_MAX[kind as usize] {
                        assert!(
                            seen.insert(
                                base_index(
                                    SemanticAtom::hand(owner, kind, slot).unwrap(),
                                    perspective,
                                    0
                                )
                                .unwrap()
                            )
                        );
                    }
                }
            }
            assert_eq!(seen.into_iter().collect::<Vec<_>>(), (0..BASE_INPUTS).collect::<Vec<_>>());
        }
        assert_eq!(
            base_index(SemanticAtom::king(AbsoluteOwner::Black, 0).unwrap(), Perspective::White, 0),
            Some((14 + 7) * 81 + 80)
        );
    }
}

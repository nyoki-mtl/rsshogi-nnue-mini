//! A-only full14 board and unary-hand feature schema plus non-king KP progress indexing.

pub(crate) const BASE_INPUTS: u32 = 2_344;
pub(crate) const BASE_MAX_ACTIVE: usize = 40;

pub(crate) const HAND_MAX: [u8; 7] = [18, 4, 4, 4, 4, 2, 2];
const HAND_PREFIX: [u16; 7] = [0, 18, 22, 26, 30, 34, 36];

pub(crate) const PROGRESS_MAGIC: [u8; 8] = *b"RSKPPRG2";
pub(crate) const PROGRESS_VERSION: u32 = 2;
const PROGRESS_PLANES: u32 = 1_534;
pub(crate) const PROGRESS_INPUTS: u32 = 124_254;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum AbsoluteOwner {
    Black,
    White,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Perspective {
    Black,
    White,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum HandFeatureKind {
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
pub(crate) enum BoardFeatureKind {
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
pub(crate) struct SemanticAtom {
    owner: AbsoluteOwner,
    kind: u8,
    place: u8,
    hand: bool,
}

impl SemanticAtom {
    pub(crate) const fn hand(
        owner: AbsoluteOwner,
        kind: HandFeatureKind,
        slot: u8,
    ) -> Option<Self> {
        if slot >= HAND_MAX[kind as usize] {
            return None;
        }
        Some(Self { owner, kind: kind as u8, place: slot, hand: true })
    }

    pub(crate) const fn board(
        owner: AbsoluteOwner,
        kind: BoardFeatureKind,
        square: u8,
    ) -> Option<Self> {
        if square >= 81 {
            return None;
        }
        Some(Self { owner, kind: kind as u8, place: square, hand: false })
    }

    pub(crate) const fn king(owner: AbsoluteOwner, square: u8) -> Option<Self> {
        Self::board(owner, BoardFeatureKind::King, square)
    }
}

pub(crate) fn base_index(atom: SemanticAtom, perspective: Perspective) -> u32 {
    let owner = u32::from(atom.owner as u8 != perspective as u8);
    if atom.hand {
        2_268 + owner * 38 + u32::from(HAND_PREFIX[atom.kind as usize]) + u32::from(atom.place)
    } else {
        (owner * 14 + u32::from(atom.kind)) * 81
            + u32::from(if perspective == Perspective::White {
                80 - atom.place
            } else {
                atom.place
            })
    }
}

/// Non-king KP progress index (`kp_absolute_progress_q16_v2`).
///
/// Promoted minor pieces alias to gold (`4`), and horse (`6`) precedes rook (`7`) to match
/// the fixed KP selector table shared with the trainer.
pub(crate) fn progress_index(
    atom: SemanticAtom,
    perspective: Perspective,
    king: u8,
) -> Option<u32> {
    if king >= 81 {
        return None;
    }
    let relative = u32::from(atom.owner as u8 != perspective as u8);
    let plane = if atom.hand {
        relative * 38 + u32::from(HAND_PREFIX[atom.kind as usize]) + u32::from(atom.place)
    } else {
        let kp_kind = match atom.kind {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 | 8..=11 => 4,
            5 => 5,
            12 => 6,
            6 => 7,
            13 => 8,
            7 => return None,
            _ => unreachable!(),
        };
        let square = if perspective == Perspective::White { 80 - atom.place } else { atom.place };
        76 + (relative * 9 + kp_kind) * 81 + u32::from(square)
    };
    let normalized_king = if perspective == Perspective::White { 80 - king } else { king };
    Some(u32::from(normalized_king) * PROGRESS_PLANES + plane)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct BaseFeatureDelta {
    transitions: [[SemanticAtom; 2]; 2],
    len: u8,
}

impl BaseFeatureDelta {
    #[must_use]
    pub(crate) const fn empty() -> Self {
        let filler = SemanticAtom { owner: AbsoluteOwner::Black, kind: 0, place: 0, hand: true };
        Self { transitions: [[filler; 2]; 2], len: 0 }
    }

    #[must_use]
    pub(crate) const fn one(old: SemanticAtom, new: SemanticAtom) -> Self {
        let transition = [old, new];
        Self { transitions: [transition; 2], len: 1 }
    }

    #[must_use]
    pub(crate) const fn two(first: [SemanticAtom; 2], second: [SemanticAtom; 2]) -> Self {
        Self { transitions: [first, second], len: 2 }
    }

    #[must_use]
    pub(crate) fn transitions(&self) -> &[[SemanticAtom; 2]] {
        &self.transitions[..usize::from(self.len)]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const BOARD_KINDS: [BoardFeatureKind; 14] = [
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

    const HAND_KINDS: [HandFeatureKind; 7] = [
        HandFeatureKind::Pawn,
        HandFeatureKind::Lance,
        HandFeatureKind::Knight,
        HandFeatureKind::Silver,
        HandFeatureKind::Gold,
        HandFeatureKind::Bishop,
        HandFeatureKind::Rook,
    ];

    #[test]
    fn all_base_indices_are_unique_and_rotate() {
        for perspective in [Perspective::Black, Perspective::White] {
            let mut seen = BTreeSet::new();
            for owner in [AbsoluteOwner::Black, AbsoluteOwner::White] {
                for kind in BOARD_KINDS {
                    for square in 0..81 {
                        let atom = SemanticAtom::board(owner, kind, square).unwrap();
                        assert!(seen.insert(base_index(atom, perspective)));
                    }
                }
                for kind in HAND_KINDS {
                    for slot in 0..HAND_MAX[kind as usize] {
                        let atom = SemanticAtom::hand(owner, kind, slot).unwrap();
                        assert!(seen.insert(base_index(atom, perspective)));
                    }
                }
            }
            assert_eq!(seen.into_iter().collect::<Vec<_>>(), (0..BASE_INPUTS).collect::<Vec<_>>());
        }
        assert_eq!(
            base_index(SemanticAtom::king(AbsoluteOwner::Black, 0).unwrap(), Perspective::White),
            (14 + 7) * 81 + 80
        );
    }

    #[test]
    fn progress_excludes_kings_aliases_promoted_minors_and_covers_all_planes() {
        let distinct_kp_kinds = [
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
        for perspective in [Perspective::Black, Perspective::White] {
            for king in [0_u8, 40, 80] {
                let mut seen = BTreeSet::new();
                for owner in [AbsoluteOwner::Black, AbsoluteOwner::White] {
                    assert_eq!(
                        progress_index(SemanticAtom::king(owner, 40).unwrap(), perspective, king),
                        None
                    );
                    let gold = SemanticAtom::board(owner, BoardFeatureKind::Gold, 12).unwrap();
                    for promoted in [
                        BoardFeatureKind::ProPawn,
                        BoardFeatureKind::ProLance,
                        BoardFeatureKind::ProKnight,
                        BoardFeatureKind::ProSilver,
                    ] {
                        let atom = SemanticAtom::board(owner, promoted, 12).unwrap();
                        assert_eq!(
                            progress_index(atom, perspective, king),
                            progress_index(gold, perspective, king)
                        );
                    }
                    for kind in distinct_kp_kinds {
                        for square in 0..81 {
                            let atom = SemanticAtom::board(owner, kind, square).unwrap();
                            let idx = progress_index(atom, perspective, king).unwrap();
                            assert!(idx < PROGRESS_INPUTS);
                            assert!(seen.insert(idx % PROGRESS_PLANES));
                        }
                    }
                    for kind in HAND_KINDS {
                        for slot in 0..HAND_MAX[kind as usize] {
                            let atom = SemanticAtom::hand(owner, kind, slot).unwrap();
                            let idx = progress_index(atom, perspective, king).unwrap();
                            assert!(idx < PROGRESS_INPUTS);
                            assert!(seen.insert(idx % PROGRESS_PLANES));
                        }
                    }
                }
                assert_eq!(
                    seen.into_iter().collect::<Vec<_>>(),
                    (0..PROGRESS_PLANES).collect::<Vec<_>>()
                );
            }
        }
        assert_eq!(
            progress_index(
                SemanticAtom::board(AbsoluteOwner::Black, BoardFeatureKind::Pawn, 0).unwrap(),
                Perspective::Black,
                81,
            ),
            None
        );
    }
}

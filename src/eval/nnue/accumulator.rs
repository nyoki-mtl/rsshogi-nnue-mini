//! Lazy A-only FT/PSQT frames. KP progress is an independent scalar state.
use super::features::{move_delta, position_atoms};
use super::network::{AOnlyNetwork, BUCKETS, PERSPECTIVES};
use super::package::schema::BaseFeatureDelta;
use super::simd::Column;
#[cfg(test)]
use crate::position::SearchPositionExt;
use rsshogi::board::Position;
use rsshogi::types::{Color, Move32};
#[derive(Clone)]
struct Frame {
    accumulators: [Column; 2],
    psqt: [[i32; BUCKETS]; 2],
    progress: [i64; 2],
    kings: [u8; 2],
    computed: bool,
    progress_computed: [bool; 2],
    delta: BaseFeatureDelta,
}
impl Frame {
    fn empty() -> Self {
        Self {
            accumulators: [Column::ZERO; 2],
            psqt: [[0; BUCKETS]; 2],
            progress: [0; 2],
            kings: [0; 2],
            computed: false,
            progress_computed: [false; 2],
            delta: BaseFeatureDelta::empty(),
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct AccumulatorStack {
    frames: Vec<Frame>,
    top: usize,
    #[cfg(test)]
    trace_stats: TraceStats,
}
#[cfg(test)]
#[derive(Clone, Debug, Default)]
struct TraceStats {
    progress_rescans: usize,
    atom_collections: usize,
}
impl AccumulatorStack {
    pub(crate) fn reset(&mut self, network: &AOnlyNetwork, position: &Position) {
        self.top = 0;
        if self.frames.is_empty() {
            self.frames.push(Frame::empty());
        }
        let root = &mut self.frames[0];
        root.kings = kings(position);
        root.delta = BaseFeatureDelta::empty();
        let atoms = position_atoms(position).expect("legal A-only features");
        for (p, &perspective) in PERSPECTIVES.iter().enumerate() {
            root.progress[p] = network
                .accumulate_checked(
                    &atoms,
                    perspective,
                    root.kings[p],
                    &mut root.accumulators[p],
                    &mut root.psqt[p],
                )
                .expect("certified accumulator");
        }
        root.computed = true;
        root.progress_computed = [true; 2];
    }
    pub(crate) fn push(&mut self, position: &Position, mv: Move32) {
        self.top += 1;
        if self.frames.len() == self.top {
            self.frames.push(Frame::empty());
        }
        let frame = &mut self.frames[self.top];
        frame.kings = kings(position);
        frame.computed = false;
        frame.progress_computed = [false; 2];
        frame.delta = move_delta(position, mv).expect("legal A-only delta");
    }
    pub(crate) fn pop(&mut self) {
        self.top = self.top.checked_sub(1).expect("balanced accumulator stack");
    }
    pub(crate) fn evaluate(&mut self, network: &AOnlyNetwork, position: &Position) -> i32 {
        let mut source = self.top;
        while !self.frames[source].computed {
            source -= 1;
        }
        for index in source + 1..=self.top {
            let (parents, children) = self.frames.split_at_mut(index);
            let parent = &parents[index - 1];
            let child = &mut children[0];
            for (p, &perspective) in PERSPECTIVES.iter().enumerate() {
                network.apply_delta(
                    &child.delta,
                    perspective,
                    (&parent.accumulators[p], &parent.psqt[p]),
                    (&mut child.accumulators[p], &mut child.psqt[p]),
                );
            }
            child.computed = true;
        }
        // Replay KP through unevaluated frames independently of the FT cache. Only a
        // change of this perspective's king requires its current full scalar sum.
        let mut atoms = None;
        for (p, &perspective) in PERSPECTIVES.iter().enumerate() {
            let mut source = self.top;
            while !self.frames[source].progress_computed[p] {
                let parent = source - 1; // Both root perspectives are computed at reset.
                if self.frames[parent].kings[p] != self.frames[source].kings[p] {
                    #[cfg(test)]
                    {
                        self.trace_stats.progress_rescans += 1;
                        if atoms.is_none() {
                            self.trace_stats.atom_collections += 1;
                        }
                    }
                    let atoms = atoms
                        .get_or_insert_with(|| position_atoms(position).expect("legal features"));
                    let frame = &mut self.frames[self.top];
                    frame.progress[p] = atoms
                        .iter()
                        .map(|&a| network.progress_weight(a, perspective, frame.kings[p]))
                        .sum();
                    frame.progress_computed[p] = true;
                    source = self.top;
                    break;
                }
                source = parent;
            }
            for index in source + 1..=self.top {
                let (parents, children) = self.frames.split_at_mut(index);
                let parent = &parents[index - 1];
                let frame = &mut children[0];
                frame.progress[p] = parent.progress[p]
                    + frame
                        .delta
                        .transitions()
                        .iter()
                        .map(|&[old, new]| {
                            network.progress_weight(new, perspective, frame.kings[p])
                                - network.progress_weight(old, perspective, frame.kings[p])
                        })
                        .sum::<i64>();
                frame.progress_computed[p] = true;
            }
        }
        let frame = &self.frames[self.top];
        let stm = usize::from(position.turn() != Color::BLACK);
        network.output(
            [&frame.accumulators[stm], &frame.accumulators[1 - stm]],
            [&frame.psqt[stm], &frame.psqt[1 - stm]],
            frame.progress[0] + frame.progress[1],
        )
    }
}
fn kings(position: &Position) -> [u8; 2] {
    [Color::BLACK, Color::WHITE].map(|c| position.king_square(c).to_index() as u8)
}
#[cfg(test)]
mod tests {

    use rsshogi::board::{self, Move32List, generate_legal_all_move32};
    use rsshogi::types::{Piece, PieceType};

    use super::super::network::synthetic_network;
    use super::*;

    /// A random walk that mixes moves, undos, skipped evaluations, and search null moves
    /// (which flip the turn without touching the stack) must match the checked reference.
    /// Null moves are only tried out of check, matching the search precondition.
    #[test]
    fn lazy_incremental_evaluation_matches_full_refresh() {
        let network = synthetic_network(0x5eed);
        assert_random_walk_matches_full_refresh(&network, 600);
    }

    #[test]
    fn lazy_progress_replays_skipped_frames_and_shares_king_refresh_atoms() {
        board::init();
        let network = synthetic_network(0x65);
        let mut position = board::hirate_position();
        let mut stack = AccumulatorStack::default();
        stack.reset(&network, &position);
        for text in ["7g7f", "3c3d", "2g2f"] {
            let mv = board::move_from_usi(&position, text).expect("test move");
            assert!(position.is_legal_move32(mv));
            position.apply_move32(mv);
            stack.push(&position, mv);
        }
        assert_eq!(stack.evaluate(&network, &position), network.evaluate(&position).unwrap());
        assert_eq!(stack.trace_stats.progress_rescans, 0);
        assert!(stack.frames[..=stack.top].iter().all(|f| f.progress_computed == [true; 2]));

        // Undo the evaluated child and evaluate a different sibling with capture/promotion.
        let mv = board::move_from_usi(&board::hirate_position(), "2g2f").unwrap();
        position.undo_move32(mv).unwrap();
        stack.pop();
        let mv = board::move_from_usi(&position, "8h2b+").expect("capture sibling");
        assert!(position.is_legal_move32(mv));
        position.apply_move32(mv);
        stack.push(&position, mv);
        assert_eq!(stack.evaluate(&network, &position), network.evaluate(&position).unwrap());

        position = board::hirate_position();
        stack.reset(&network, &position);
        for text in ["5i6h", "5a4b"] {
            let mv = board::move_from_usi(&position, text).expect("king move");
            assert!(position.is_legal_move32(mv));
            position.apply_move32(mv);
            stack.push(&position, mv);
        }
        assert_eq!(stack.evaluate(&network, &position), network.evaluate(&position).unwrap());
        assert_eq!(stack.trace_stats.progress_rescans, 2);
        assert_eq!(stack.trace_stats.atom_collections, 1);
        let atoms = position_atoms(&position).unwrap();
        for (p, &perspective) in PERSPECTIVES.iter().enumerate() {
            let king = stack.frames[stack.top].kings[p];
            let expected: i64 =
                atoms.iter().map(|&a| network.progress_weight(a, perspective, king)).sum();
            assert_eq!(stack.frames[stack.top].progress[p], expected);
        }
    }

    #[test]
    #[ignore = "requires RSSHOGI_TEST_RSNN_PATH pointing to a trained A-only package"]
    fn trained_package_incremental_evaluation_matches_full_refresh() {
        let path = std::env::var_os("RSSHOGI_TEST_RSNN_PATH")
            .expect("set RSSHOGI_TEST_RSNN_PATH to the trained package");
        let network =
            AOnlyNetwork::load_file(std::path::Path::new(&path)).expect("validate trained package");
        assert_random_walk_matches_full_refresh(&network, 6_000);
    }

    fn assert_random_walk_matches_full_refresh(network: &AOnlyNetwork, steps: usize) {
        board::init();
        let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
        let mut random = move |bound: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % bound
        };
        let mut position = board::hirate_position();
        let mut stack = AccumulatorStack::default();
        stack.reset(network, &position);
        let mut played = Vec::new();
        let (mut captures, mut drops, mut promotions, mut king_moves) = (0, 0, 0, 0);
        let (mut evaluations, mut null_moves) = (0, 0);
        for _ in 0..steps {
            let mut moves = Move32List::new();
            generate_legal_all_move32(&position, &mut moves);
            let undo = !played.is_empty() && (moves.as_slice().is_empty() || random(10) < 3);
            if undo {
                let mv = played.pop().expect("played move");
                position.undo_move32(mv).expect("undo a played move");
                stack.pop();
            } else if !moves.as_slice().is_empty() {
                let mv = moves.as_slice()[random(moves.as_slice().len() as u64) as usize];
                position.apply_move32(mv);
                stack.push(&position, mv);
                played.push(mv);
                captures += usize::from(position.captured_piece() != Piece::NONE);
                drops += usize::from(mv.is_drop());
                promotions += usize::from(mv.is_promotion());
                king_moves += usize::from(
                    !mv.is_drop() && position.piece_on(mv.to_sq()).piece_type() == PieceType::KING,
                );
            }
            if random(3) != 0 {
                let expected = network.evaluate(&position).expect("reference");
                assert_eq!(stack.evaluate(network, &position), expected);
                evaluations += 1;
            }
            if random(8) == 0 && !position.is_in_check() && position.start_search_null().is_ok() {
                let expected = network.evaluate(&position).expect("reference");
                assert_eq!(stack.evaluate(network, &position), expected);
                position.finish_search_null().expect("undo a null move");
                null_moves += 1;
            }
        }
        let scale = (steps / 600).max(1);
        assert!(
            captures > 3 * scale
                && drops > 3 * scale
                && promotions >= scale
                && king_moves > 3 * scale
        );
        assert!(evaluations > 300 * scale && null_moves > 15 * scale);
    }
}

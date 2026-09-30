//! Per-worker accumulator stack with lazy incremental updates.
//!
//! `push` only records the move's feature delta. `evaluate` walks back to the nearest
//! computed frame and replays deltas forward, so nodes that are never evaluated cost no
//! accumulator work. A move by a perspective's own king changes every base index of that
//! perspective, so the walk stops there and the current frame is rebuilt from the position.
//!
//! Rebuilds go through a refresh cache keyed by perspective and own-king square. Each entry
//! remembers the accumulator of the last position rebuilt with that king square, so a rebuild
//! reads only the columns of pieces that differ instead of every active feature. The key is
//! the real square, not the mirrored king bucket, because mirroring changes the planes.

use rsnn_package::sfnnv15_feature_schema::BaseFeatureDelta;
use rsshogi::board::Position;
use rsshogi::types::{Color, Move32};

use super::features::{AtomList, Placement, move_delta};
use super::mobile::{BUCKETS, MobileNetwork, PERSPECTIVES};
use super::simd::Column;

#[derive(Clone)]
#[repr(C, align(64))]
struct Frame {
    accumulators: [Column; 2],
    psqt: [[i32; BUCKETS]; 2],
    progress: [i64; 2],
    kings: [u8; 2],
    computed: [bool; 2],
    /// Delta of the move that produced this frame from the previous one.
    delta: BaseFeatureDelta,
}

impl Frame {
    const fn empty() -> Self {
        Self {
            accumulators: [Column::ZERO; 2],
            psqt: [[0; BUCKETS]; 2],
            progress: [0; 2],
            kings: [0; 2],
            computed: [false; 2],
            delta: BaseFeatureDelta::empty(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Backward,
}

#[derive(Clone)]
struct RefreshEntry {
    accumulator: Column,
    psqt: [i32; BUCKETS],
    progress: i64,
    placement: Placement,
}

const KING_SQUARES: usize = 81;

#[derive(Clone, Default)]
pub(crate) struct AccumulatorStack {
    frames: Vec<Frame>,
    top: usize,
    /// `perspective * 81 + own king square`; filled from the empty board at first reset.
    refresh_cache: Vec<RefreshEntry>,
}

impl AccumulatorStack {
    /// Starts a new search from `position` and computes the root frame, so every later
    /// walk back ends at a computed frame or at an own-king move.
    pub(crate) fn reset(&mut self, network: &MobileNetwork, position: &Position) {
        if self.frames.is_empty() {
            self.frames.push(Frame::empty());
            let empty = RefreshEntry {
                accumulator: *network.biases(),
                psqt: [0; BUCKETS],
                progress: 0,
                placement: Placement::EMPTY,
            };
            self.refresh_cache = vec![empty; 2 * KING_SQUARES];
        }
        self.top = 0;
        let root = &mut self.frames[0];
        root.kings = kings(position);
        root.delta = BaseFeatureDelta::empty();
        for perspective in 0..2 {
            self.refresh_top(network, position, perspective);
        }
    }

    /// Records the move that produced `position`.
    pub(crate) fn push(&mut self, position: &Position, mv: Move32) {
        let delta = move_delta(position, mv).expect("a legal move has valid SFNNv15 features");
        self.top += 1;
        if self.frames.len() == self.top {
            self.frames.push(Frame::empty());
        }
        let frame = &mut self.frames[self.top];
        frame.kings = kings(position);
        frame.computed = [false; 2];
        frame.delta = delta;
    }

    pub(crate) fn pop(&mut self) {
        self.top = self.top.checked_sub(1).expect("accumulator push/pop must be balanced");
    }

    pub(crate) fn evaluate(
        &mut self,
        network: &MobileNetwork,
        position: &Position,
        fv_scale: i32,
    ) -> i32 {
        debug_assert_eq!(self.frames[self.top].kings, kings(position));
        for perspective in 0..2 {
            self.update(network, position, perspective);
        }
        let frame = &self.frames[self.top];
        let stm = usize::from(position.turn() != Color::BLACK);
        network.output(
            [&frame.accumulators[stm], &frame.accumulators[1 - stm]],
            [&frame.psqt[stm], &frame.psqt[1 - stm]],
            frame.progress[0] + frame.progress[1],
            fv_scale,
        )
    }

    fn update(&mut self, network: &MobileNetwork, position: &Position, perspective: usize) {
        let mut source = self.top;
        while !self.frames[source].computed[perspective] {
            let parent = source.checked_sub(1).expect("the root frame is computed at reset");
            if self.frames[source].kings[perspective] != self.frames[parent].kings[perspective] {
                // The own king moved into `source`. Rebuild the current frame, then fill the
                // frames back to `source` in reverse, since they share this king square and
                // siblings deeper than the king move can reuse them.
                self.refresh_top(network, position, perspective);
                for index in (source + 1..=self.top).rev() {
                    self.derive(network, perspective, index, Direction::Backward);
                }
                return;
            }
            source = parent;
        }
        for index in source + 1..=self.top {
            self.derive(network, perspective, index, Direction::Forward);
        }
    }

    /// Computes frame `index` from `index - 1` (forward) or `index - 1` from `index`
    /// (backward) through the delta of the move between them.
    fn derive(
        &mut self,
        network: &MobileNetwork,
        perspective: usize,
        index: usize,
        direction: Direction,
    ) {
        let (parents, children) = self.frames.split_at_mut(index);
        let (parent, child) = (&mut parents[index - 1], &mut children[0]);
        let (delta, king) = (child.delta, child.kings[perspective]);
        let (source, target) = match direction {
            Direction::Forward => (&*parent, child),
            Direction::Backward => (&*child, parent),
        };
        let progress = network.apply_delta(
            &delta,
            direction == Direction::Backward,
            PERSPECTIVES[perspective],
            king,
            (&source.accumulators[perspective], &source.psqt[perspective]),
            (&mut target.accumulators[perspective], &mut target.psqt[perspective]),
        );
        target.progress[perspective] = source.progress[perspective] + progress;
        target.computed[perspective] = true;
    }

    fn refresh_top(&mut self, network: &MobileNetwork, position: &Position, perspective: usize) {
        let frame = &mut self.frames[self.top];
        let king = frame.kings[perspective];
        let entry = &mut self.refresh_cache[perspective * KING_SQUARES + usize::from(king)];
        let placement = Placement::of(position);
        let (mut removed, mut added) = (AtomList::new(), AtomList::new());
        entry.placement.diff(&placement, &mut removed, &mut added);
        let progress = network.apply_changes(
            removed.as_slice(),
            added.as_slice(),
            PERSPECTIVES[perspective],
            king,
            (&entry.accumulator, &entry.psqt),
            (&mut frame.accumulators[perspective], &mut frame.psqt[perspective]),
        );
        frame.progress[perspective] = entry.progress + progress;
        frame.computed[perspective] = true;
        entry.accumulator = frame.accumulators[perspective];
        entry.psqt = frame.psqt[perspective];
        entry.progress = frame.progress[perspective];
        entry.placement = placement;
    }
}

fn kings(position: &Position) -> [u8; 2] {
    [Color::BLACK, Color::WHITE].map(|color| position.king_square(color).to_index() as u8)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rsnn_package::Digest;
    use rsshogi::board::{self, Move32List, generate_legal_all_move32};
    use rsshogi::types::{Piece, PieceType};

    use super::*;
    use crate::nnue::DEFAULT_FV_SCALE;
    use crate::nnue::mobile::synthetic_network;

    /// A random walk that mixes moves, undos, skipped evaluations, and search null moves
    /// (which flip the turn without touching the stack) must match the checked reference.
    /// Null moves are only tried out of check, matching the search precondition.
    #[test]
    fn lazy_incremental_evaluation_matches_full_refresh() {
        let network = synthetic_network(0x5eed);
        assert_random_walk_matches_full_refresh(&network);
    }

    #[test]
    #[ignore = "requires RSSHOGI_RSNN_RELEASE_CANDIDATE"]
    fn release_candidate_incremental_evaluation_matches_full_refresh() {
        let path = std::env::var_os("RSSHOGI_RSNN_RELEASE_CANDIDATE")
            .expect("set RSSHOGI_RSNN_RELEASE_CANDIDATE to the 84-epoch package");
        let digest =
            Digest::from_hex("a4a61c91f85a1ee1eb67cb7c6483c66fdb6ed7c2832d3184901e2faf89dfb398")
                .expect("release candidate digest");
        let network = MobileNetwork::load(Path::new(&path), Some(digest))
            .expect("release candidate package loads");
        assert_random_walk_matches_full_refresh(&network);
    }

    fn assert_random_walk_matches_full_refresh(network: &MobileNetwork) {
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
        for _ in 0..6_000 {
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
                let expected = network.evaluate(&position, DEFAULT_FV_SCALE).expect("reference");
                assert_eq!(stack.evaluate(network, &position, DEFAULT_FV_SCALE), expected);
                evaluations += 1;
            }
            if random(8) == 0
                && !position.is_in_check()
                && position.try_apply_search_null_move().is_ok()
            {
                let expected = network.evaluate(&position, DEFAULT_FV_SCALE).expect("reference");
                assert_eq!(stack.evaluate(network, &position, DEFAULT_FV_SCALE), expected);
                position.undo_search_null_move().expect("undo a null move");
                null_moves += 1;
            }
        }
        assert!(captures > 50 && drops > 50 && promotions > 20 && king_moves > 50);
        assert!(evaluations > 3_000 && null_moves > 200);
    }
}

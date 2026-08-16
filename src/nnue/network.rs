//! networkの重みの保持と、全結合層・accumulatorを使った推論。

use rsshogi::board::Position;
use rsshogi::types::{Color, Move32, PieceType};

use super::accumulator::{AccumulatorDelta, FeatureChanges, PerspectiveDelta, StandardAccumulator};
use super::features::{feature_indices, feature_zero, move_feature_changes};
use super::{
    CLIPPED_RELU_MAX, HIDDEN_DIMENSIONS, MAX_NNUE_EVAL, NnueError, TRANSFORMED_DIMENSIONS,
    WEIGHT_SCALE_BITS, simd,
};

#[derive(Debug)]
pub(super) struct DenseLayer {
    pub(super) input_dimensions: usize,
    pub(super) biases: Box<[i32]>,
    pub(super) weights: Box<[i8]>,
}

#[derive(Debug)]
pub struct StandardNetwork {
    pub(super) feature_biases: Box<[i16]>,
    pub(super) feature_weights: Box<[i16]>,
    pub(super) hidden_1: DenseLayer,
    pub(super) hidden_2: DenseLayer,
    pub(super) output_bias: i32,
    pub(super) output_weights: Box<[i8]>,
}

impl StandardNetwork {
    pub fn inference_route() -> &'static str {
        simd::active_route()
    }

    pub fn evaluate(&self, position: &Position, fv_scale: i32) -> Result<i32, NnueError> {
        let black = self.accumulate(position, Color::BLACK)?;
        let white = self.accumulate(position, Color::WHITE)?;
        self.evaluate_accumulations(position, &[black, white], fv_scale)
    }

    pub fn new_accumulator(&self, position: &Position) -> Result<StandardAccumulator, NnueError> {
        let black_features = feature_indices(position, Color::BLACK)?;
        let white_features = feature_indices(position, Color::WHITE)?;
        Ok(StandardAccumulator {
            accumulations: [
                self.accumulate_features(&black_features),
                self.accumulate_features(&white_features),
            ],
            feature_zero: [
                feature_zero(position, Color::BLACK)?,
                feature_zero(position, Color::WHITE)?,
            ],
            history: Vec::new(),
        })
    }

    pub fn evaluate_accumulator(
        &self,
        position: &Position,
        accumulator: &StandardAccumulator,
        fv_scale: i32,
    ) -> Result<i32, NnueError> {
        self.evaluate_accumulations(position, &accumulator.accumulations, fv_scale)
    }

    /// 直前に`position`へ適用された`mv`の分だけaccumulatorを進める。
    ///
    /// 玉が動いた側は視点のfeature originが変わるため、その側だけをfull refreshし、
    /// もう一方の側は移動元・移動先・捕獲・持ち駒の差分だけを加減する。
    pub fn advance_accumulator(
        &self,
        accumulator: &mut StandardAccumulator,
        position: &Position,
        mv: Move32,
    ) -> Result<(), NnueError> {
        let mover = position.turn().flip();
        let king_move =
            !mv.is_drop() && position.piece_on(mv.to_sq()).piece_type() == PieceType::KING;
        let previous_feature_zero = accumulator.feature_zero;
        let mut perspectives = [None, None];

        for (index, perspective) in [Color::BLACK, Color::WHITE].into_iter().enumerate() {
            if king_move && perspective == mover {
                let previous = Box::new(accumulator.accumulations[index]);
                let features = feature_indices(position, perspective)?;
                accumulator.accumulations[index] = self.accumulate_features(&features);
                accumulator.feature_zero[index] = feature_zero(position, perspective)?;
                perspectives[index] = Some(PerspectiveDelta::Refreshed(previous));
            } else {
                let changes = move_feature_changes(
                    accumulator.feature_zero[index],
                    position,
                    mv,
                    mover,
                    perspective,
                )?;
                self.apply_changes(&mut accumulator.accumulations[index], &changes);
                perspectives[index] = Some(PerspectiveDelta::Changed(changes));
            }
        }

        accumulator.history.push(AccumulatorDelta {
            previous_feature_zero,
            perspectives: perspectives.map(|delta| delta.expect("both perspectives are built")),
        });
        Ok(())
    }

    pub fn undo_accumulator(&self, accumulator: &mut StandardAccumulator) {
        let delta = accumulator.history.pop().expect("NNUE accumulator history must match do/undo");
        for (index, perspective) in delta.perspectives.into_iter().enumerate() {
            match perspective {
                PerspectiveDelta::Changed(changes) => {
                    self.apply_changes(&mut accumulator.accumulations[index], &changes.reversed());
                }
                PerspectiveDelta::Refreshed(previous) => {
                    accumulator.accumulations[index] = *previous;
                }
            }
        }
        accumulator.feature_zero = delta.previous_feature_zero;
    }

    fn evaluate_accumulations(
        &self,
        position: &Position,
        accumulations: &[[i16; TRANSFORMED_DIMENSIONS]; 2],
        fv_scale: i32,
    ) -> Result<i32, NnueError> {
        if !(1..=128).contains(&fv_scale) {
            return Err(NnueError::Invalid(format!("FVScale must be in 1..=128, got {fv_scale}")));
        }
        let black = &accumulations[0];
        let white = &accumulations[1];
        let (friend, enemy) =
            if position.turn() == Color::BLACK { (black, white) } else { (white, black) };

        let transformed = simd::clamp_pair(friend, enemy);

        let hidden_1 = propagate(&self.hidden_1, &transformed);
        let hidden_2 = propagate(&self.hidden_2, &hidden_1);
        let raw =
            i64::from(self.output_bias) + i64::from(simd::dot(&self.output_weights, &hidden_2));
        let evaluation =
            (raw / i64::from(fv_scale)).clamp(-i64::from(MAX_NNUE_EVAL), i64::from(MAX_NNUE_EVAL));
        Ok(i32::try_from(evaluation).expect("clamped NNUE evaluation fits in i32"))
    }

    fn accumulate(
        &self,
        position: &Position,
        perspective: Color,
    ) -> Result<[i16; TRANSFORMED_DIMENSIONS], NnueError> {
        let features = feature_indices(position, perspective)?;
        Ok(self.accumulate_features(&features))
    }

    fn accumulate_features(&self, features: &[usize]) -> [i16; TRANSFORMED_DIMENSIONS] {
        let mut accumulation = [0i16; TRANSFORMED_DIMENSIONS];
        accumulation.copy_from_slice(&self.feature_biases);
        for feature in features {
            let start = *feature * TRANSFORMED_DIMENSIONS;
            let weights = &self.feature_weights[start..start + TRANSFORMED_DIMENSIONS];
            simd::add_assign(&mut accumulation, weights);
        }
        accumulation
    }

    fn apply_changes(
        &self,
        accumulation: &mut [i16; TRANSFORMED_DIMENSIONS],
        changes: &FeatureChanges,
    ) {
        for feature in changes.removed() {
            let start = *feature * TRANSFORMED_DIMENSIONS;
            simd::sub_assign(
                accumulation,
                &self.feature_weights[start..start + TRANSFORMED_DIMENSIONS],
            );
        }
        for feature in changes.added() {
            let start = *feature * TRANSFORMED_DIMENSIONS;
            simd::add_assign(
                accumulation,
                &self.feature_weights[start..start + TRANSFORMED_DIMENSIONS],
            );
        }
    }
}

fn propagate(layer: &DenseLayer, input: &[u8]) -> [u8; HIDDEN_DIMENSIONS] {
    let mut output = [0u8; HIDDEN_DIMENSIONS];
    for (neuron, slot) in output.iter_mut().enumerate() {
        let start = neuron * layer.input_dimensions;
        let value = i64::from(layer.biases[neuron])
            + i64::from(simd::dot(&layer.weights[start..start + layer.input_dimensions], input));
        *slot = (value >> WEIGHT_SCALE_BITS).clamp(0, i64::from(CLIPPED_RELU_MAX)) as u8;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nnue::DEFAULT_FV_SCALE;
    use crate::nnue::test_support::{deterministic_feature_network, zero_network_bytes};
    use rsshogi::board::{self, Move32List, generate_legal_all_move32};

    #[test]
    fn hostile_bias_plus_positive_dot_clamps_without_overflow() {
        let mut network =
            StandardNetwork::from_bytes(&zero_network_bytes()).expect("valid synthetic network");
        network.hidden_1.biases.fill(i32::MAX);
        network.hidden_1.weights.fill(1);
        network.hidden_2.biases.fill(i32::MAX);
        network.hidden_2.weights.fill(1);
        network.output_bias = i32::MAX;
        network.output_weights.fill(1);

        let position = board::hirate_position();
        let accumulations = [[1; TRANSFORMED_DIMENSIONS]; 2];
        assert_eq!(
            network
                .evaluate_accumulations(&position, &accumulations, 1)
                .expect("hostile weights must evaluate"),
            MAX_NNUE_EVAL
        );
    }

    /// test fixtureの重み付けだけに使うmultiset差分。探索経路では使わない。
    fn feature_multiset_difference(previous: &[usize], next: &[usize]) -> (Vec<usize>, Vec<usize>) {
        let mut previous = previous.to_vec();
        let mut next = next.to_vec();
        previous.sort_unstable();
        next.sort_unstable();

        let (mut removed, mut added) = (Vec::new(), Vec::new());
        let (mut old_index, mut new_index) = (0, 0);
        while old_index < previous.len() && new_index < next.len() {
            match previous[old_index].cmp(&next[new_index]) {
                std::cmp::Ordering::Less => {
                    removed.push(previous[old_index]);
                    old_index += 1;
                }
                std::cmp::Ordering::Greater => {
                    added.push(next[new_index]);
                    new_index += 1;
                }
                std::cmp::Ordering::Equal => {
                    old_index += 1;
                    new_index += 1;
                }
            }
        }
        removed.extend_from_slice(&previous[old_index..]);
        added.extend_from_slice(&next[new_index..]);
        (removed, added)
    }

    #[test]
    fn feature_difference_preserves_duplicate_multiplicity() {
        let (removed, added) = feature_multiset_difference(&[1, 1, 2, 4], &[1, 2, 2, 5]);
        assert_eq!(removed, [1, 4]);
        assert_eq!(added, [2, 5]);
    }

    /// 指し手駆動の差分が、full refreshとの一致だけでなく、
    /// 局面から求めたmultiset差分とも同じfeature集合を動かすことを確かめる。
    fn assert_move_changes_match_multiset(
        previous: &Position,
        next: &Position,
        mv: Move32,
        expect_refresh_for: Option<Color>,
    ) {
        let mover = previous.turn();
        for perspective in [Color::BLACK, Color::WHITE] {
            if expect_refresh_for == Some(perspective) {
                continue;
            }
            let previous_features =
                feature_indices(previous, perspective).expect("previous features");
            let next_features = feature_indices(next, perspective).expect("next features");
            let (mut expected_removed, mut expected_added) =
                feature_multiset_difference(&previous_features, &next_features);
            let changes = move_feature_changes(
                feature_zero(previous, perspective).expect("previous feature origin"),
                next,
                mv,
                mover,
                perspective,
            )
            .expect("move driven changes");
            let mut actual_removed = changes.removed().to_vec();
            let mut actual_added = changes.added().to_vec();
            expected_removed.sort_unstable();
            expected_added.sort_unstable();
            actual_removed.sort_unstable();
            actual_added.sort_unstable();
            assert_eq!(
                actual_removed,
                expected_removed,
                "{perspective} removed for {}",
                mv.to_usi()
            );
            assert_eq!(actual_added, expected_added, "{perspective} added for {}", mv.to_usi());
        }
    }

    fn refreshed_perspective(position: &Position, mv: Move32) -> Option<Color> {
        (!mv.is_drop() && position.piece_on(mv.from_sq()).piece_type() == PieceType::KING)
            .then(|| position.turn())
    }

    /// 一手を進めて差分・full refresh・undoの三つが一致することを確かめる。
    fn assert_move_keeps_accumulator_exact(
        network: &StandardNetwork,
        position: &mut Position,
        accumulator: &mut StandardAccumulator,
        mv: Move32,
    ) {
        let baseline = accumulator.accumulations;
        let baseline_feature_zero = accumulator.feature_zero;
        let previous = position.clone();
        let refreshed = refreshed_perspective(&previous, mv);

        position.apply_move32(mv);
        network.advance_accumulator(accumulator, position, mv).expect("advance accumulator");

        let refreshed_black = matches!(
            accumulator.history.last().expect("delta").perspectives[0],
            PerspectiveDelta::Refreshed(_)
        );
        let refreshed_white = matches!(
            accumulator.history.last().expect("delta").perspectives[1],
            PerspectiveDelta::Refreshed(_)
        );
        assert_eq!(
            refreshed_black,
            refreshed == Some(Color::BLACK),
            "black refresh must happen exactly on a black king move: {}",
            mv.to_usi()
        );
        assert_eq!(
            refreshed_white,
            refreshed == Some(Color::WHITE),
            "white refresh must happen exactly on a white king move: {}",
            mv.to_usi()
        );

        assert_eq!(
            accumulator.accumulations,
            network.new_accumulator(position).expect("refresh").accumulations,
            "incremental mismatch after {}",
            mv.to_usi()
        );
        assert_move_changes_match_multiset(&previous, position, mv, refreshed);

        position.undo_move32(mv).expect("undo move");
        network.undo_accumulator(accumulator);
        assert_eq!(accumulator.accumulations, baseline, "undo mismatch after {}", mv.to_usi());
        assert_eq!(accumulator.feature_zero, baseline_feature_zero);
    }

    #[test]
    fn move_driven_delta_matches_full_refresh_for_every_legal_move() {
        board::init();
        let network = deterministic_feature_network();
        let cases = [
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1",
            "lr6l/4g1pkp/2ns1s3/4pp1SP/PPBP2Pp1/2gpPP3/2B2S3/2K6/L6RL b 4P2g3np 1",
            "lnsG5/4g4/prpp1p1pp/1p4p1k/4+B4/2P1P3P/P+b1PSP1L1/4K2SL/2G2G1r1 b SP3nl3p 73",
            "k8/9/9/9/4p4/4S4/9/9/K8 w - 1",
            "4k4/9/9/9/9/9/9/4r4/4K4 b - 1",
            "4k4/9/4P4/9/9/9/9/9/4K4 b RBGSNLP 1",
        ];

        for sfen in cases {
            let mut position = board::position_from_sfen(sfen).expect("valid SFEN");
            let mut accumulator = network.new_accumulator(&position).expect("root accumulator");
            let mut moves = Move32List::new();
            generate_legal_all_move32(&position, &mut moves);
            assert!(!moves.as_slice().is_empty(), "case must have legal moves: {sfen}");
            for mv in moves.as_slice().iter().copied() {
                assert_move_keeps_accumulator_exact(&network, &mut position, &mut accumulator, mv);
            }
        }
    }

    #[test]
    fn nested_do_undo_keeps_the_accumulator_aligned() {
        board::init();
        let network = deterministic_feature_network();
        let mut position = board::position_from_sfen(
            "lr6l/4g1pkp/2ns1s3/4pp1SP/PPBP2Pp1/2gpPP3/2B2S3/2K6/L6RL b 4P2g3np 1",
        )
        .expect("valid SFEN");
        let mut accumulator = network.new_accumulator(&position).expect("root accumulator");
        let root = accumulator.accumulations;

        walk(&network, &mut position, &mut accumulator, 3);

        assert_eq!(accumulator.accumulations, root, "the search walk must restore the root");
        assert!(accumulator.history.is_empty(), "do/undo must be balanced");
    }

    /// 探索と同じ順序で数手潜り、各nodeで差分とfull refreshの一致を確かめる。
    fn walk(
        network: &StandardNetwork,
        position: &mut Position,
        accumulator: &mut StandardAccumulator,
        depth: u32,
    ) {
        if depth == 0 {
            return;
        }
        let mut moves = Move32List::new();
        generate_legal_all_move32(position, &mut moves);
        for mv in moves.as_slice().iter().copied().take(6) {
            let previous = position.clone();
            let refreshed = refreshed_perspective(&previous, mv);
            position.apply_move32(mv);
            network.advance_accumulator(accumulator, position, mv).expect("advance");
            assert_eq!(
                accumulator.accumulations,
                network.new_accumulator(position).expect("refresh").accumulations,
                "incremental mismatch at depth {depth} after {}",
                mv.to_usi()
            );
            assert_move_changes_match_multiset(&previous, position, mv, refreshed);
            walk(network, position, accumulator, depth - 1);
            position.undo_move32(mv).expect("undo");
            network.undo_accumulator(accumulator);
        }
    }

    fn give_transition_features_nonzero_weights(
        network: &mut StandardNetwork,
        previous: &Position,
        next: &Position,
    ) {
        for perspective in [Color::BLACK, Color::WHITE] {
            let previous_features =
                feature_indices(previous, perspective).expect("previous features");
            let next_features = feature_indices(next, perspective).expect("next features");
            let (removed, added) = feature_multiset_difference(&previous_features, &next_features);
            for feature in removed.iter().chain(&added) {
                network.feature_weights[*feature * TRANSFORMED_DIMENSIONS] =
                    (*feature % 127) as i16 + 1;
            }
        }
    }

    #[test]
    fn synthetic_nonzero_accumulator_reuses_root_state_for_siblings() {
        let mut network =
            StandardNetwork::from_bytes(&zero_network_bytes()).expect("valid synthetic network");
        let mut position = board::hirate_position();
        let first = board::move_from_usi(&position, "7g7f").expect("valid first root move");
        let second = board::move_from_usi(&position, "2g2f").expect("valid second root move");

        let mut first_position = position.clone();
        first_position.apply_move32(first);
        let mut second_position = position.clone();
        second_position.apply_move32(second);
        give_transition_features_nonzero_weights(&mut network, &position, &first_position);
        give_transition_features_nonzero_weights(&mut network, &position, &second_position);

        let mut accumulator =
            network.new_accumulator(&position).expect("initialize root accumulator");
        let root_accumulations = accumulator.accumulations;

        position.apply_move32(first);
        network
            .advance_accumulator(&mut accumulator, &position, first)
            .expect("advance first sibling");
        assert_eq!(
            accumulator.accumulations,
            network.new_accumulator(&position).expect("refresh first sibling").accumulations,
            "first sibling must match a fresh full refresh"
        );
        assert_ne!(
            accumulator.accumulations, root_accumulations,
            "the synthetic first sibling must exercise nonzero feature weights"
        );

        position.undo_move32(first).expect("undo first sibling");
        network.undo_accumulator(&mut accumulator);
        assert_eq!(
            accumulator.accumulations, root_accumulations,
            "undo must restore the root accumulation before the next sibling"
        );

        position.apply_move32(second);
        network
            .advance_accumulator(&mut accumulator, &position, second)
            .expect("advance second sibling");
        assert_eq!(
            accumulator.accumulations,
            network.new_accumulator(&position).expect("refresh second sibling").accumulations,
            "second sibling must match a fresh full refresh after reusing the root accumulator"
        );
        assert_ne!(
            accumulator.accumulations, root_accumulations,
            "the synthetic second sibling must exercise nonzero feature weights"
        );
    }

    #[test]
    fn synthetic_accumulator_stays_aligned_across_search_null_and_real_child() {
        let mut network =
            StandardNetwork::from_bytes(&zero_network_bytes()).expect("valid synthetic network");
        let mut position = board::hirate_position();
        let setup = board::move_from_usi(&position, "7g7f").expect("legal setup move");
        position.apply_move32(setup);

        let black_features = feature_indices(&position, Color::BLACK).expect("black features");
        let white_features = feature_indices(&position, Color::WHITE).expect("white features");
        let black_only = *black_features
            .iter()
            .find(|feature| !white_features.contains(feature))
            .expect("black-only feature");
        let white_only = *white_features
            .iter()
            .find(|feature| !black_features.contains(feature))
            .expect("white-only feature");
        network.feature_weights[black_only * TRANSFORMED_DIMENSIONS] = 100;
        network.feature_weights[white_only * TRANSFORMED_DIMENSIONS] = 20;
        network.hidden_1.weights[0] = 1;
        network.hidden_1.weights[TRANSFORMED_DIMENSIONS] = -1;
        network.hidden_2.weights[0] = 64;
        network.output_weights[0] = i8::try_from(DEFAULT_FV_SCALE).expect("scale fits i8");

        position.try_apply_search_null_move().expect("search null move");
        let real_child = board::move_from_usi(&position, "2g2f").expect("legal real child");
        let mut child = position.clone();
        child.apply_move32(real_child);
        for perspective in [Color::BLACK, Color::WHITE] {
            let before = feature_indices(&position, perspective).expect("null features");
            let after = feature_indices(&child, perspective).expect("child features");
            let (removed, added) = feature_multiset_difference(&before, &after);
            for feature in removed.iter().chain(&added) {
                network.feature_weights[*feature * TRANSFORMED_DIMENSIONS + 1] =
                    (*feature % 127) as i16 + 1;
            }
        }
        position.undo_search_null_move().expect("undo setup null move");

        let mut accumulator = network.new_accumulator(&position).expect("root accumulator");
        let original_accumulations = accumulator.accumulations;
        let original_eval = network
            .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
            .expect("original incremental evaluation");
        assert_eq!(
            original_eval,
            network.evaluate(&position, DEFAULT_FV_SCALE).expect("original refresh")
        );

        position.try_apply_search_null_move().expect("search null move");
        let null_eval = network
            .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
            .expect("null incremental evaluation");
        assert_eq!(null_eval, network.evaluate(&position, DEFAULT_FV_SCALE).expect("null refresh"));
        assert_ne!(
            null_eval, original_eval,
            "turn must select the opposite accumulator perspective"
        );

        position.apply_move32(real_child);
        network
            .advance_accumulator(&mut accumulator, &position, real_child)
            .expect("advance real child");
        assert_ne!(accumulator.accumulations, original_accumulations);
        assert_eq!(
            network
                .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
                .expect("child incremental evaluation"),
            network.evaluate(&position, DEFAULT_FV_SCALE).expect("child refresh")
        );

        position.undo_move32(real_child).expect("undo real child");
        network.undo_accumulator(&mut accumulator);
        assert_eq!(accumulator.accumulations, original_accumulations);
        assert_eq!(
            network
                .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
                .expect("restored null evaluation"),
            null_eval
        );

        position.undo_search_null_move().expect("undo search null move");
        assert_eq!(accumulator.accumulations, original_accumulations);
        assert_eq!(
            network
                .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
                .expect("restored original evaluation"),
            original_eval
        );
    }

    fn assert_incremental_move(network: &StandardNetwork, sfen: &str, usi: &str) {
        let mut position = board::position_from_sfen(sfen).expect("valid SFEN");
        let original_key = position.key();
        let mut accumulator = network.new_accumulator(&position).expect("initialize accumulator");
        let mv = board::move_from_usi(&position, usi).expect("valid move text");
        assert!(position.is_legal_move32(mv), "move must be legal: {usi}");

        position.apply_move32(mv);
        network.advance_accumulator(&mut accumulator, &position, mv).expect("advance accumulator");
        let incremental = network
            .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
            .expect("incremental evaluation");
        let refreshed =
            network.evaluate(&position, DEFAULT_FV_SCALE).expect("full refresh evaluation");
        assert_eq!(incremental, refreshed, "incremental mismatch after {usi}");

        position.undo_move32(mv).expect("undo move");
        network.undo_accumulator(&mut accumulator);
        assert_eq!(position.key(), original_key);
        assert_eq!(
            network
                .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
                .expect("restored incremental evaluation"),
            network.evaluate(&position, DEFAULT_FV_SCALE).expect("restored full refresh"),
            "incremental mismatch after undoing {usi}"
        );
    }

    #[test]
    #[ignore = "requires a separately downloaded, digest-verified standard NNUE file"]
    fn external_suisho5_network_parity_and_incremental() {
        let path = std::env::var("RSSHOGI_NNUE_MINI_TEST_NETWORK")
            .expect("set RSSHOGI_NNUE_MINI_TEST_NETWORK to the verified nn.bin path");
        let network = StandardNetwork::load(path).expect("load external standard NNUE");
        let startpos = board::hirate_position();
        let score = network
            .evaluate(&startpos, DEFAULT_FV_SCALE)
            .expect("evaluate startpos with the release-recommended scale");
        assert_eq!(score, 39, "YaneuraOu V8.30 oracle with FV_SCALE=24");

        assert_incremental_move(&network, "8k/9/9/9/4p4/4P4/9/9/K8 b - 1", "5f5e");
        assert_incremental_move(&network, "8k/9/9/9/9/9/9/9/K8 b P 1", "P*5e");
        assert_incremental_move(&network, "8k/9/4P4/9/9/9/9/9/K8 b - 1", "5c5b+");
        assert_incremental_move(&network, "8k/9/9/9/9/9/9/9/K8 b - 1", "9i9h");

        let mut root_moves = Move32List::new();
        generate_legal_all_move32(&startpos, &mut root_moves);
        for mv in root_moves.as_slice() {
            assert_incremental_move(&network, &startpos.to_sfen(None), &mv.to_usi());
        }
    }
}

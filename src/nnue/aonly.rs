//! 1024-wide A-only integer inference from a validated `.rsnn` package.

#[cfg(test)]
#[path = "aonly_bench.rs"]
mod bench;

#[cfg(test)]
use crate::position::SearchPositionExt;
use std::path::Path;

use rsnn_package::aonly_feature_schema::{
    self as schema, BaseFeatureDelta, Perspective, SemanticAtom,
};
use rsnn_package::{Digest, EvaluationDescriptor, EvaluatorExpectation};
#[cfg(test)]
use rsshogi::board::Position;
#[cfg(test)]
use rsshogi::types::Color;

#[cfg(test)]
use super::features::position_atoms;
use super::simd::{self, Column, CompactColumn, DENSE_OUTPUTS};
use super::{MAX_NNUE_EVAL, RsnnPackage};

use super::simd::WIDTH;
pub(super) const BUCKETS: usize = 8;
pub(super) const PERSPECTIVES: [Perspective; 2] = [Perspective::Black, Perspective::White];
const HIDDEN: usize = DENSE_OUTPUTS;
/// fc0の出力を対にした活性化。fc1への入力で、fc2への入力の前半になる。
const FC1_INPUTS: usize = 2 * HIDDEN;
const FC2_INPUTS: usize = 128;
const PROGRESS_HEADER: usize = 32;
const PROGRESS_THRESHOLDS: [i64; 7] = [-127_527, -71_998, -33_477, 0, 33_478, 71_999, 127_528];

/// One immutable network shared by the search workers. Tensor bytes are decoded once into
/// the layouts the kernels read: one aligned column per base feature and chunk-major dense
/// weights per layer stack.
#[derive(Debug)]
pub struct AonlyNetwork {
    digest: Digest,
    base_weights: Vec<CompactColumn>,
    biases: Column,
    psqt_weights: Vec<[i32; BUCKETS]>,
    progress_weights: Vec<i32>,
    stacks: Vec<LayerStack>,
}

#[derive(Debug)]
struct LayerStack {
    fc0: Vec<i8>,
    fc0_bias: [i32; HIDDEN],
    fc1: Vec<i8>,
    fc1_bias: [i32; HIDDEN],
    fc2: [i8; FC2_INPUTS],
    fc2_bias: i32,
}

/// Tensors decoded from the package in row-major order, before kernel layout.
struct DecodedTensors {
    digest: Digest,
    base_weights: Vec<i8>,
    biases: Vec<i16>,
    psqt_weights: Vec<i32>,
    progress_weights: Vec<i32>,
    /// Per layer stack, `[output][input]`.
    fc0: Vec<Vec<i8>>,
    fc0_bias: Vec<i32>,
    fc1: Vec<Vec<i8>>,
    fc1_bias: Vec<i32>,
    fc2: Vec<Vec<i8>>,
    fc2_bias: Vec<i32>,
}

impl AonlyNetwork {
    pub fn load_bytes(bytes: &[u8]) -> Result<Self, String> {
        Self::from_package(RsnnPackage::load_bytes(bytes, rsnn_package::sha256(bytes))?)
    }

    pub fn load(path: &Path, expected_digest: Option<Digest>) -> Result<Self, String> {
        let package = RsnnPackage::load(path, expected_digest)?;
        Self::from_package(package)
    }

    #[cfg(feature = "embedded-rsnn")]
    pub fn load_embedded() -> Result<Self, String> {
        let bytes = include_bytes!(env!("RSSHOGI_EMBEDDED_RSNN_PATH"));
        let digest = Digest::from_hex(env!("RSSHOGI_EMBEDDED_RSNN_SHA256"))
            .map_err(|error| error.to_string())?;
        Self::from_package(RsnnPackage::load_bytes(bytes, digest)?)
    }

    pub(super) fn from_package(package: RsnnPackage) -> Result<Self, String> {
        let expected =
            EvaluatorExpectation::new(EvaluationDescriptor::aonly1024_i8_progress8_v1_schema())
                .map_err(|error| error.to_string())?;
        expected
            .verify(&package.contents.manifest.evaluation_descriptor)
            .map_err(|error| error.to_string())?;
        let contents = &package.contents;
        if contents.manifest.auxiliary_artifacts.len() != 1
            || contents.manifest.auxiliary_artifacts[0].name != "kp-progress.bin"
        {
            return Err("A-only .rsnn requires exactly kp-progress.bin".to_owned());
        }
        let tensor = |name: &str| -> Result<&[u8], String> {
            contents.tensor(name).map(|entry| entry.data).map_err(|error| error.to_string())
        };
        let progress = contents.auxiliary("kp-progress.bin").map_err(|error| error.to_string())?;
        let progress_weights = decode_progress(progress.data)?;
        Ok(Self::from_tensors(DecodedTensors {
            digest: package.digest(),
            base_weights: tensor("ft_base_weight")?.iter().map(|&v| v as i8).collect(),
            biases: decode_i16(tensor("ft_bias")?),
            psqt_weights: decode_i32(tensor("psqt_weight")?),
            progress_weights,
            fc0: decode_stacked_matrix(tensor("fc0_weight")?, HIDDEN, WIDTH),
            fc0_bias: decode_i32(tensor("fc0_bias")?),
            fc1: decode_stacked_matrix(tensor("fc1_weight")?, HIDDEN, FC1_INPUTS),
            fc1_bias: decode_i32(tensor("fc1_bias")?),
            fc2: decode_stacked_matrix(tensor("fc2_weight")?, 1, FC2_INPUTS),
            fc2_bias: decode_i32(tensor("fc2_bias")?),
        }))
    }

    fn from_tensors(tensors: DecodedTensors) -> Self {
        let stacks = (0..BUCKETS)
            .map(|bucket| LayerStack {
                fc0: chunk_major(&tensors.fc0[bucket], WIDTH),
                fc0_bias: tensors.fc0_bias[bucket * HIDDEN..][..HIDDEN].try_into().expect("HIDDEN"),
                fc1: chunk_major(&tensors.fc1[bucket], FC1_INPUTS),
                fc1_bias: tensors.fc1_bias[bucket * HIDDEN..][..HIDDEN].try_into().expect("HIDDEN"),
                fc2: tensors.fc2[bucket].as_slice().try_into().expect("128 fc2 weights"),
                fc2_bias: tensors.fc2_bias[bucket],
            })
            .collect();
        Self {
            digest: tensors.digest,
            base_weights: tensors
                .base_weights
                .as_chunks::<WIDTH>()
                .0
                .iter()
                .map(|column| CompactColumn(*column))
                .collect(),
            biases: Column(tensors.biases.as_slice().try_into().expect("1024 biases")),
            psqt_weights: tensors.psqt_weights.as_chunks::<BUCKETS>().0.to_vec(),
            progress_weights: tensors.progress_weights,
            stacks,
        }
    }

    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Full-position reference path: checked wide accumulation and scalar kernels only.
    #[cfg(test)]
    pub(super) fn evaluate(&self, position: &Position) -> Result<i32, String> {
        let atoms = position_atoms(position)?;
        let mut accumulators = [Column::ZERO; 2];
        let mut psqt = [[0_i32; BUCKETS]; 2];
        let mut progress = 0_i64;
        for (index, color) in [Color::BLACK, Color::WHITE].into_iter().enumerate() {
            let king = position.king_square(color).to_index() as u8;
            progress += self.accumulate_checked(
                &atoms,
                PERSPECTIVES[index],
                king,
                &mut accumulators[index],
                &mut psqt[index],
            )?;
        }
        let stm = usize::from(position.turn() != Color::BLACK);
        Ok(self.output_with::<ScalarKernels>(
            [&accumulators[stm], &accumulators[1 - stm]],
            [&psqt[stm], &psqt[1 - stm]],
            progress,
        ))
    }

    /// Evaluates side-to-move-first accumulators. `progress` is the sum of both perspectives.
    pub(super) fn output(
        &self,
        accumulators: [&Column; 2],
        psqt: [&[i32; BUCKETS]; 2],
        progress: i64,
    ) -> i32 {
        self.output_with::<VectorKernels>(accumulators, psqt, progress)
    }

    fn output_with<K: Kernels>(
        &self,
        accumulators: [&Column; 2],
        psqt: [&[i32; BUCKETS]; 2],
        progress: i64,
    ) -> i32 {
        let bucket =
            PROGRESS_THRESHOLDS.iter().position(|&threshold| progress < threshold).unwrap_or(7);
        let stack = &self.stacks[bucket];
        let psqt = (i64::from(psqt[0][bucket]) - i64::from(psqt[1][bucket])) / 2;
        let mut input = [0_u8; WIDTH];
        K::pool(accumulators, &mut input);
        let mut fc0 = [0_i32; HIDDEN];
        K::dense(&stack.fc0, &stack.fc0_bias, &input, &mut fc0);
        let mut fc2_input = [0_u8; FC2_INPUTS];
        let fc1_input: &mut [u8; FC1_INPUTS] =
            (&mut fc2_input[..FC1_INPUTS]).try_into().expect("fc1 inputs");
        paired_activation(&fc0, 7, fc1_input);
        let mut fc1 = [0_i32; HIDDEN];
        K::dense(&stack.fc1, &stack.fc1_bias, fc1_input, &mut fc1);
        paired_activation(
            &fc1,
            6,
            (&mut fc2_input[FC1_INPUTS..]).try_into().expect("fc1 activations"),
        );
        let fc2 = stack.fc2_bias + K::dot(&stack.fc2, &fc2_input);
        let unscaled = i64::from(fc2) + i64::from(fc0[30]) - i64::from(fc0[31]);
        let positional = unscaled * 600 * i64::from(super::OUTPUT_SCALE) / (128 * 64 * 2);
        let raw = positional + psqt;
        (raw / i64::from(super::OUTPUT_SCALE))
            .clamp(-i64::from(MAX_NNUE_EVAL), i64::from(MAX_NNUE_EVAL)) as i32
    }

    /// Applies the one or two atom transitions of a legal move on one perspective.
    /// KP progress is updated independently; A-only never needs an FT king refresh.
    ///
    /// The package reader certified that every legal feature set keeps the accumulator and
    /// PSQT within `i16`/`i32`, so wrapping arithmetic gives the exact values.
    pub(super) fn apply_delta(
        &self,
        delta: &BaseFeatureDelta,
        perspective: Perspective,
        king: u8,
        source: (&Column, &[i32; BUCKETS]),
        target: (&mut Column, &mut [i32; BUCKETS]),
    ) {
        let transitions = delta.transitions();
        let mut removed_columns = [&self.base_weights[0]; 2];
        let mut added_columns = [&self.base_weights[0]; 2];
        *target.1 = *source.1;
        for (i, &[old, new]) in transitions.iter().enumerate() {
            let old = base_index(old, perspective, king);
            let new = base_index(new, perspective, king);
            removed_columns[i] = &self.base_weights[old];
            added_columns[i] = &self.base_weights[new];
            for (b, value) in target.1.iter_mut().enumerate() {
                *value = value
                    .wrapping_sub(self.psqt_weights[old][b])
                    .wrapping_add(self.psqt_weights[new][b]);
            }
        }
        simd::update(
            target.0,
            source.0,
            &removed_columns[..transitions.len()],
            &added_columns[..transitions.len()],
        );
    }

    pub(super) fn progress_weight(
        &self,
        atom: SemanticAtom,
        perspective: Perspective,
        king: u8,
    ) -> i64 {
        schema::progress_index(atom, perspective, king)
            .map_or(0, |index| i64::from(self.progress_weights[index as usize]))
    }

    pub(super) fn accumulate_checked(
        &self,
        atoms: &[SemanticAtom],
        perspective: Perspective,
        king: u8,
        accumulation: &mut Column,
        psqt: &mut [i32; BUCKETS],
    ) -> Result<i64, String> {
        let mut values = self.biases.0.map(i32::from);
        let mut psqt_wide = [0_i64; BUCKETS];
        let mut progress = 0_i64;
        for &atom in atoms {
            let index = base_index(atom, perspective, king);
            for (value, &weight) in values.iter_mut().zip(&self.base_weights[index].0) {
                *value += i32::from(weight);
            }
            for (value, &weight) in psqt_wide.iter_mut().zip(&self.psqt_weights[index]) {
                *value += i64::from(weight);
            }
            progress += self.progress_weight(atom, perspective, king);
        }
        for (dst, value) in accumulation.0.iter_mut().zip(values) {
            *dst = i16::try_from(value).map_err(|_| "A-only accumulator overflow".to_owned())?;
        }
        for (dst, value) in psqt.iter_mut().zip(psqt_wide) {
            *dst = i32::try_from(value).map_err(|_| "A-only PSQT overflow".to_owned())?;
        }
        Ok(progress)
    }
}

/// Kernel set used by one evaluation path; the reference path pins the scalar set.
trait Kernels {
    fn pool(accumulators: [&Column; 2], output: &mut [u8; WIDTH]);
    fn dense<const IN: usize>(
        weights: &[i8],
        bias: &[i32; HIDDEN],
        input: &[u8; IN],
        output: &mut [i32; HIDDEN],
    );
    fn dot(weights: &[i8; FC2_INPUTS], input: &[u8; FC2_INPUTS]) -> i32;
}

struct VectorKernels;

impl Kernels for VectorKernels {
    fn pool(accumulators: [&Column; 2], output: &mut [u8; WIDTH]) {
        simd::pool(accumulators, output);
    }

    fn dense<const IN: usize>(
        weights: &[i8],
        bias: &[i32; HIDDEN],
        input: &[u8; IN],
        output: &mut [i32; HIDDEN],
    ) {
        simd::dense(weights, bias, input, output);
    }

    fn dot(weights: &[i8; FC2_INPUTS], input: &[u8; FC2_INPUTS]) -> i32 {
        simd::dot(weights, input)
    }
}

#[cfg(test)]
struct ScalarKernels;

#[cfg(test)]
impl Kernels for ScalarKernels {
    fn pool(accumulators: [&Column; 2], output: &mut [u8; WIDTH]) {
        simd::scalar::pool(accumulators, output);
    }

    fn dense<const IN: usize>(
        weights: &[i8],
        bias: &[i32; HIDDEN],
        input: &[u8; IN],
        output: &mut [i32; HIDDEN],
    ) {
        simd::scalar::dense(weights, bias, input, output);
    }

    fn dot(weights: &[i8; FC2_INPUTS], input: &[u8; FC2_INPUTS]) -> i32 {
        simd::scalar::dot(weights, input)
    }
}

/// Atoms come from a legal position, so every index exists for an on-board king square.
fn base_index(atom: SemanticAtom, perspective: Perspective, king: u8) -> usize {
    schema::base_index(atom, perspective, king).expect("king square is on the board") as usize
}

/// Reorders one row-major `[32][inputs]` matrix into `[inputs / 4][32][4]` for the kernels.
fn chunk_major(rows: &[i8], inputs: usize) -> Vec<i8> {
    let mut chunks = vec![0_i8; rows.len()];
    for output in 0..HIDDEN {
        for input in 0..inputs {
            chunks[(input / 4) * HIDDEN * 4 + output * 4 + input % 4] =
                rows[output * inputs + input];
        }
    }
    chunks
}

fn decode_i16(bytes: &[u8]) -> Vec<i16> {
    bytes.as_chunks::<2>().0.iter().copied().map(i16::from_le_bytes).collect()
}

fn decode_i32(bytes: &[u8]) -> Vec<i32> {
    bytes.as_chunks::<4>().0.iter().copied().map(i32::from_le_bytes).collect()
}

fn decode_progress(bytes: &[u8]) -> Result<Vec<i32>, String> {
    if bytes.len() != PROGRESS_HEADER + schema::PROGRESS_INPUTS as usize * 4
        || bytes[..8] != schema::PROGRESS_MAGIC
        || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != schema::PROGRESS_VERSION
        || u32::from_le_bytes(bytes[12..16].try_into().unwrap()) != schema::PROGRESS_INPUTS
        || u32::from_le_bytes(bytes[16..20].try_into().unwrap()) != BUCKETS as u32
        || u32::from_le_bytes(bytes[20..24].try_into().unwrap()) != 16
        || bytes[24..PROGRESS_HEADER].iter().any(|&byte| byte != 0)
    {
        return Err("invalid kp-progress.bin".to_owned());
    }
    if rsnn_package::sha256(bytes).to_hex()
        != "441b3dd485a32f48d6065f794df3b54d6aae9ac08514f5c3b22ca43f3ef04bb9"
    {
        return Err("kp-progress.bin digest differs from fixed A-only progress artifact".into());
    }
    Ok(decode_i32(&bytes[PROGRESS_HEADER..]))
}

fn decode_stacked_matrix(bytes: &[u8], outputs: usize, inputs: usize) -> Vec<Vec<i8>> {
    let mut stacks = (0..BUCKETS).map(|_| vec![0_i8; outputs * inputs]).collect::<Vec<_>>();
    for (stack, weights) in stacks.iter_mut().enumerate() {
        for output in 0..outputs {
            for input in 0..inputs {
                weights[output * inputs + input] =
                    bytes[(stack * outputs + output) * inputs + input] as i8;
            }
        }
    }
    stacks
}

fn paired_activation(input: &[i32; HIDDEN], scale_bits: u32, output: &mut [u8; 64]) {
    for (index, &value) in input.iter().enumerate() {
        let value64 = i64::from(value);
        output[index] = ((value64 * value64) >> (2 * scale_bits + 7)).clamp(0, 127) as u8;
        output[index + 32] = (value >> scale_bits).clamp(0, 127) as u8;
    }
}

/// Random weights in ranges that keep every legal accumulator inside `i16` while spreading
/// activations and progress buckets, for tests that compare evaluation paths.
#[cfg(test)]
pub(crate) fn synthetic_network(seed: u64) -> AonlyNetwork {
    AonlyNetwork::from_tensors(synthetic_tensors(seed))
}

#[cfg(test)]
fn synthetic_tensors(seed: u64) -> DecodedTensors {
    let mut state = seed | 1;
    let mut next = move |low: i64, high: i64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        low + (state % (high - low + 1) as u64) as i64
    };
    let mut fill = |len: usize, low: i64, high: i64| -> Vec<i64> {
        (0..len).map(|_| next(low, high)).collect()
    };
    let base_inputs = schema::BASE_INPUTS as usize;
    let matrix = |values: Vec<i64>, outputs: usize, inputs: usize| {
        values
            .chunks_exact(outputs * inputs)
            .map(|stack| stack.iter().map(|&v| v as i8).collect())
            .collect()
    };
    DecodedTensors {
        digest: Digest::from_hex(&"0".repeat(64)).expect("digest literal"),
        base_weights: fill(base_inputs * WIDTH, -40, 40).into_iter().map(|v| v as i8).collect(),
        biases: fill(WIDTH, 40, 220).into_iter().map(|v| v as i16).collect(),
        psqt_weights: fill(base_inputs * BUCKETS, -3_000, 3_000)
            .into_iter()
            .map(|v| v as i32)
            .collect(),
        progress_weights: fill(schema::PROGRESS_INPUTS as usize, -5_000, 5_000)
            .into_iter()
            .map(|v| v as i32)
            .collect(),
        fc0: matrix(fill(BUCKETS * HIDDEN * WIDTH, -128, 127), HIDDEN, WIDTH),
        fc0_bias: fill(BUCKETS * HIDDEN, -8_000, 8_000).into_iter().map(|v| v as i32).collect(),
        fc1: matrix(fill(BUCKETS * HIDDEN * FC1_INPUTS, -128, 127), HIDDEN, FC1_INPUTS),
        fc1_bias: fill(BUCKETS * HIDDEN, -8_000, 8_000).into_iter().map(|v| v as i32).collect(),
        fc2: matrix(fill(BUCKETS * FC2_INPUTS, -128, 127), 1, FC2_INPUTS),
        fc2_bias: fill(BUCKETS, -8_000, 8_000).into_iter().map(|v| v as i32).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnn_package::{Manifest, Package, PackageReader, PackageWriter, Provenance, sha256};

    #[test]
    #[ignore = "requires RSSHOGI_TEST_KP_PROGRESS_PATH pointing to the fixed KP artifact"]
    fn compact_package_matches_unpacked_reference() {
        use rsnn_package::AuxiliaryDescriptor;

        let path = std::env::var_os("RSSHOGI_TEST_KP_PROGRESS_PATH").expect("fixed KP path");
        let progress = std::fs::read(path).expect("read KP artifact");
        let mut tensors = synthetic_tensors(0x127);
        tensors.progress_weights = decode_progress(&progress).expect("fixed KP digest");
        // Exercise the signed endpoints in the package decoder and production updates.
        tensors.base_weights[0] = -127;
        tensors.base_weights[1] = 127;
        let mut descriptor = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
        let mut payload = Vec::new();
        for tensor in &mut descriptor.tensors {
            let data: Vec<u8> = match tensor.name.as_str() {
                "ft_base_weight" => tensors.base_weights.iter().map(|&v| v as u8).collect(),
                "ft_bias" => tensors.biases.iter().flat_map(|v| v.to_le_bytes()).collect(),
                "psqt_weight" => {
                    tensors.psqt_weights.iter().flat_map(|v| v.to_le_bytes()).collect()
                }
                "fc0_weight" => tensors.fc0.iter().flatten().map(|&v| v as u8).collect(),
                "fc1_weight" => tensors.fc1.iter().flatten().map(|&v| v as u8).collect(),
                "fc2_weight" => tensors.fc2.iter().flatten().map(|&v| v as u8).collect(),
                "fc0_bias" => tensors.fc0_bias.iter().flat_map(|v| v.to_le_bytes()).collect(),
                "fc1_bias" => tensors.fc1_bias.iter().flat_map(|v| v.to_le_bytes()).collect(),
                "fc2_bias" => tensors.fc2_bias.iter().flat_map(|v| v.to_le_bytes()).collect(),
                _ => unreachable!(),
            };
            assert_eq!(data.len() as u64, tensor.length);
            tensor.digest = sha256(&data);
            payload.extend(data);
        }
        let auxiliary = AuxiliaryDescriptor {
            name: "kp-progress.bin".into(),
            media_type: "application/octet-stream".into(),
            offset: payload.len() as u64,
            length: progress.len() as u64,
            digest: sha256(&progress),
        };
        payload.extend(progress);
        let manifest = Manifest::new(
            descriptor.clone(),
            Provenance {
                training_spec_hash: Digest::ZERO,
                export_spec_hash: Digest::ZERO,
                source_plan_hash: Digest::ZERO,
                source_checkpoint_digest: Digest::ZERO,
                source_dataset_identity: "synthetic compact inference fixture; not trained".into(),
            },
            descriptor.tensors,
            vec![auxiliary],
            sha256(&payload),
        )
        .unwrap();
        let bytes = PackageWriter::write(&Package { manifest, payload }).unwrap();
        assert!(bytes.len() < 3_300_000, "metadata included: {} bytes", bytes.len());
        let contents = PackageReader::default().read(&bytes, Some(sha256(&bytes))).unwrap();
        let decoded = AonlyNetwork::from_package(RsnnPackage { contents }).unwrap();
        let reference = AonlyNetwork::from_tensors(tensors);
        let mut position = rsshogi::board::hirate_position();
        let mut stack = super::super::AccumulatorStack::default();
        stack.reset(&decoded, &position);
        for text in ["7g7f", "3c3d", "8h2b+", "3a2b", "B*4e", "B*5d", "5i6h"] {
            let mv = rsshogi::board::move_from_usi(&position, text).unwrap();
            assert!(position.is_legal_move32(mv));
            position.apply_move32(mv);
            stack.push(&position, mv);
            assert_eq!(stack.evaluate(&decoded, &position), reference.evaluate(&position).unwrap());
            if !position.is_in_check() {
                position.start_search_null().unwrap();
                assert_eq!(
                    stack.evaluate(&decoded, &position),
                    reference.evaluate(&position).unwrap()
                );
                position.finish_search_null().unwrap();
            }
            position.undo_move32(mv).unwrap();
            stack.pop();
            assert_eq!(stack.evaluate(&decoded, &position), reference.evaluate(&position).unwrap());
            position.apply_move32(mv);
            stack.push(&position, mv);
        }
        println!("synthetic package: {} bytes, SHA256 {}", bytes.len(), sha256(&bytes));
        if let Some(path) = std::env::var_os("RSSHOGI_TEST_SYNTHETIC_PACKAGE_OUT") {
            std::fs::write(path, bytes).unwrap();
        }
    }

    #[test]
    fn reused_evaluator_refreshes_old_frames_with_the_current_network_and_root() {
        use crate::eval::{EvalParams, Evaluator};
        use std::sync::Arc;

        let first = Arc::new(synthetic_network(17));
        let next = Arc::new(synthetic_network(29));
        let params = EvalParams::default();
        let mut evaluator = Evaluator::nnue(params, Arc::clone(&first));
        let mut position = rsshogi::board::hirate_position();
        evaluator.initialize(&position);
        for _ in 0..8 {
            let mut moves = rsshogi::board::Move32List::new();
            rsshogi::board::generate_legal_all_move32(&position, &mut moves);
            let mv = moves.as_slice()[0];
            position.apply_move32(mv);
            evaluator.advance(&position, mv);
            assert_eq!(evaluator.evaluate(&position), first.evaluate(&position).unwrap());
        }
        // 深いframeが残り、networkもrootも異なる次のgoを模す。
        evaluator.prepare(&Evaluator::nnue(params, Arc::clone(&next)));
        let root = rsshogi::board::hirate_position();
        evaluator.initialize(&root);
        assert_eq!(evaluator.evaluate(&root), next.evaluate(&root).unwrap());
        evaluator.initialize(&position);
        assert_eq!(evaluator.evaluate(&position), next.evaluate(&position).unwrap());

        evaluator.prepare(&Evaluator::material(params));
        assert_eq!(evaluator.evaluate(&root), crate::eval::evaluate_material(&root, params));
        evaluator.prepare(&Evaluator::nnue(params, Arc::clone(&first)));
        evaluator.initialize(&position);
        assert_eq!(evaluator.evaluate(&position), first.evaluate(&position).unwrap());
    }
    #[test]
    fn synthetic_package_roundtrip_requires_numeric_certificate() {
        let mut descriptor = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
        let mut payload = Vec::new();
        for t in &mut descriptor.tensors {
            let data = vec![0; t.length as usize];
            t.digest = sha256(&data);
            payload.extend(data);
        }
        let manifest = Manifest::new(
            descriptor.clone(),
            Provenance {
                training_spec_hash: Digest::ZERO,
                export_spec_hash: Digest::ZERO,
                source_plan_hash: Digest::ZERO,
                source_checkpoint_digest: Digest::ZERO,
                source_dataset_identity: "synthetic A-only test".into(),
            },
            descriptor.tensors.clone(),
            vec![],
            sha256(&payload),
        )
        .unwrap();
        let bytes = PackageWriter::write(&Package { manifest, payload }).unwrap();
        let contents = PackageReader::default().read(&bytes, Some(sha256(&bytes))).unwrap();
        assert!(contents.numeric_range_report.is_some());
        assert!(
            AonlyNetwork::from_package(RsnnPackage { contents })
                .unwrap_err()
                .contains("kp-progress.bin")
        );
        let mut corrupt = bytes;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(PackageReader::default().read(&corrupt, None).is_err());
    }
    #[test]
    fn fixed_output_scale_preserves_signed_rounding_and_clamping() {
        let mut network = synthetic_network(0x16);
        let accumulators = [Column::ZERO; 2];
        for stack in &mut network.stacks {
            stack.fc0.fill(0);
            stack.fc0_bias.fill(0);
            stack.fc1.fill(0);
            stack.fc1_bias.fill(0);
            stack.fc2.fill(0);
        }
        for (bias, linear, expected) in [
            (16384, 3230, 700),
            (-16384, -3230, -700),
            (0, 3230, 100),
            (0, -3230, -100),
            (0, i32::MAX, 31753),
            (0, i32::MIN, -31753),
        ] {
            for stack in &mut network.stacks {
                stack.fc2_bias = bias;
            }
            assert_eq!(
                network.output([&accumulators[0], &accumulators[1]], [&[linear; 8], &[0; 8]], 0,),
                expected,
            );
        }
    }

    #[test]
    fn integer_output_matches_independent_row_major_reference() {
        let network = synthetic_network(0x12345678);
        let mut a = [Column::ZERO; 2];
        for (p, col) in a.iter_mut().enumerate() {
            for (i, v) in col.0.iter_mut().enumerate() {
                *v = ((i * 37 + p * 71) % 750) as i16 - 200;
            }
        }
        let psqt = [[1397; 8], [-413; 8]];
        for progress in [
            -127528, -127527, -71999, -71998, -33478, -33477, -1, 0, 33477, 33478, 71998, 71999,
            127527, 127528,
        ] {
            let k = PROGRESS_THRESHOLDS.iter().filter(|&&t| progress >= t).count();
            let stack = &network.stacks[k];
            let mut x = Vec::new();
            for col in &a {
                for i in 0..512 {
                    x.push(
                        (i32::from(col.0[i]).clamp(0, 255)
                            * i32::from(col.0[i + 512]).clamp(0, 255)
                            / 512) as u8,
                    );
                }
            }
            let dense = |w: &[i8], b: &[i32; 32], x: &[u8]| -> Vec<i32> {
                (0..32)
                    .map(|o| {
                        b[o] + x
                            .iter()
                            .enumerate()
                            .map(|(i, &v)| {
                                i32::from(w[(i / 4) * 128 + o * 4 + i % 4]) * i32::from(v)
                            })
                            .sum::<i32>()
                    })
                    .collect()
            };
            let activation = |z: &[i32], shift: u32| -> Vec<u8> {
                z.iter()
                    .map(|&v| {
                        ((i64::from(v) * i64::from(v)) >> (2 * shift + 7)).clamp(0, 127) as u8
                    })
                    .chain(z.iter().map(|&v| (v >> shift).clamp(0, 127) as u8))
                    .collect()
            };
            let z0 = dense(&stack.fc0, &stack.fc0_bias, &x);
            let u0 = activation(&z0, 7);
            let z1 = dense(&stack.fc1, &stack.fc1_bias, &u0);
            let u1 = activation(&z1, 6);
            let u = i64::from(stack.fc2_bias)
                + u0.iter()
                    .chain(&u1)
                    .zip(stack.fc2)
                    .map(|(&v, w)| i64::from(v) * i64::from(w))
                    .sum::<i64>()
                + i64::from(z0[30])
                - i64::from(z0[31]);
            let expected = ((u * 9600 / 16384 + (1397 + 413) / 2) / 16).clamp(-31753, 31753) as i32;
            assert_eq!(network.output([&a[0], &a[1]], [&psqt[0], &psqt[1]], progress), expected);
        }
        let bytes: Vec<u8> = (0..64).collect();
        let decoded = decode_stacked_matrix(&bytes, 2, 4);
        for (bucket, matrix) in decoded.iter().enumerate() {
            assert_eq!(matrix, &(bucket * 8..bucket * 8 + 8).map(|v| v as i8).collect::<Vec<_>>());
        }
    }
    #[test]
    fn signed_square_and_progress_digest_are_mandatory() {
        let mut z = [0; 32];
        z[0] = -4096;
        let mut u = [0; 64];
        paired_activation(&z, 7, &mut u);
        assert_eq!((u[0], u[32]), (8, 0));
        let mut bytes = vec![0; 32 + schema::PROGRESS_INPUTS as usize * 4];
        bytes[..8].copy_from_slice(&schema::PROGRESS_MAGIC);
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&schema::PROGRESS_INPUTS.to_le_bytes());
        bytes[16..20].copy_from_slice(&8u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&16u32.to_le_bytes());
        assert!(decode_progress(&bytes).unwrap_err().contains("digest"));
    }
}

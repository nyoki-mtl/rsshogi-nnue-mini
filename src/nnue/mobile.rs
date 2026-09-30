//! 512-wide, base-only SFNNv15 integer inference from a validated `.rsnn` package.

use std::path::Path;

use rsnn_package::sfnnv15_feature_schema::{
    self as schema, BaseFeatureDelta, Perspective, SemanticAtom,
};
use rsnn_package::{Digest, EvaluationDescriptor, EvaluatorExpectation};
#[cfg(test)]
use rsshogi::board::Position;
#[cfg(test)]
use rsshogi::types::Color;

use super::features::MAX_ACTIVE;
#[cfg(test)]
use super::features::position_atoms;
use super::simd::{self, Column, DENSE_OUTPUTS};
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
pub struct MobileNetwork {
    digest: Digest,
    base_weights: Vec<Column>,
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
    base_weights: Vec<i16>,
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

impl MobileNetwork {
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

    fn from_package(package: RsnnPackage) -> Result<Self, String> {
        let expected = EvaluatorExpectation::new(EvaluationDescriptor::sfnnv15_mobile_v1_schema())
            .map_err(|error| error.to_string())?;
        expected
            .verify(&package.contents.manifest.evaluation_descriptor)
            .map_err(|error| error.to_string())?;
        let contents = &package.contents;
        if contents.manifest.auxiliary_artifacts.len() != 1
            || contents.manifest.auxiliary_artifacts[0].name != "kp-progress.bin"
        {
            return Err("mobile .rsnn requires exactly kp-progress.bin".to_owned());
        }
        let tensor = |name: &str| -> Result<&[u8], String> {
            contents.tensor(name).map(|entry| entry.data).map_err(|error| error.to_string())
        };
        let progress = contents.auxiliary("kp-progress.bin").map_err(|error| error.to_string())?;
        let progress_weights = decode_progress(progress.data)?;
        Ok(Self::from_tensors(DecodedTensors {
            digest: package.digest(),
            base_weights: decode_i16(tensor("ft_base_weight")?),
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
                .chunks_exact(WIDTH)
                .map(|column| Column(column.try_into().expect("512 lanes")))
                .collect(),
            biases: Column(tensors.biases.as_slice().try_into().expect("512 biases")),
            psqt_weights: tensors
                .psqt_weights
                .chunks_exact(BUCKETS)
                .map(|column| column.try_into().expect("8 buckets"))
                .collect(),
            progress_weights: tensors.progress_weights,
            stacks,
        }
    }

    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Full-position reference path: checked wide accumulation and scalar kernels only.
    #[cfg(test)]
    pub(super) fn evaluate(&self, position: &Position, fv_scale: i32) -> Result<i32, String> {
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
            fv_scale,
        ))
    }

    /// Evaluates side-to-move-first accumulators. `progress` is the sum of both perspectives.
    pub(super) fn output(
        &self,
        accumulators: [&Column; 2],
        psqt: [&[i32; BUCKETS]; 2],
        progress: i64,
        fv_scale: i32,
    ) -> i32 {
        self.output_with::<VectorKernels>(accumulators, psqt, progress, fv_scale)
    }

    fn output_with<K: Kernels>(
        &self,
        accumulators: [&Column; 2],
        psqt: [&[i32; BUCKETS]; 2],
        progress: i64,
        fv_scale: i32,
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
        let positional = unscaled * 600 * 16 / (128 * 64 * 2);
        let raw = positional + psqt;
        (raw / i64::from(fv_scale)).clamp(-i64::from(MAX_NNUE_EVAL), i64::from(MAX_NNUE_EVAL))
            as i32
    }

    pub(super) const fn biases(&self) -> &Column {
        &self.biases
    }

    /// Removes and adds feature columns on one perspective, writing `source` plus the change
    /// into `target`, and returns the progress change.
    ///
    /// The package reader certified that every legal feature set keeps the accumulator and
    /// PSQT within `i16`/`i32`, so wrapping arithmetic gives the exact values.
    pub(super) fn apply_changes(
        &self,
        removed: &[SemanticAtom],
        added: &[SemanticAtom],
        perspective: Perspective,
        king: u8,
        source: (&Column, &[i32; BUCKETS]),
        target: (&mut Column, &mut [i32; BUCKETS]),
    ) -> i64 {
        let mut removed_columns = [&self.biases; MAX_ACTIVE];
        let mut added_columns = [&self.biases; MAX_ACTIVE];
        *target.1 = *source.1;
        let mut progress = 0_i64;
        for (column, &atom) in removed_columns.iter_mut().zip(removed) {
            let index = base_index(atom, perspective, king);
            *column = &self.base_weights[index];
            for (value, &weight) in target.1.iter_mut().zip(&self.psqt_weights[index]) {
                *value = value.wrapping_sub(weight);
            }
            progress -= self.progress_weight(atom, perspective, king);
        }
        for (column, &atom) in added_columns.iter_mut().zip(added) {
            let index = base_index(atom, perspective, king);
            *column = &self.base_weights[index];
            for (value, &weight) in target.1.iter_mut().zip(&self.psqt_weights[index]) {
                *value = value.wrapping_add(weight);
            }
            progress += self.progress_weight(atom, perspective, king);
        }
        simd::update(
            target.0,
            source.0,
            &removed_columns[..removed.len()],
            &added_columns[..added.len()],
        );
        progress
    }

    /// Applies one move delta, or its inverse when `reverse`, to one perspective.
    pub(super) fn apply_delta(
        &self,
        delta: &BaseFeatureDelta,
        reverse: bool,
        perspective: Perspective,
        king: u8,
        source: (&Column, &[i32; BUCKETS]),
        target: (&mut Column, &mut [i32; BUCKETS]),
    ) -> i64 {
        let transitions = delta.transitions();
        // A one-transition move repeats it in the second slot; only `..count` is read.
        let old = [transitions[0][0], transitions[transitions.len() - 1][0]];
        let new = [transitions[0][1], transitions[transitions.len() - 1][1]];
        let (removed, added) = if reverse { (new, old) } else { (old, new) };
        let count = transitions.len();
        self.apply_changes(&removed[..count], &added[..count], perspective, king, source, target)
    }

    fn progress_weight(&self, atom: SemanticAtom, perspective: Perspective, king: u8) -> i64 {
        schema::progress_index(atom, perspective, king)
            .map_or(0, |index| i64::from(self.progress_weights[index as usize]))
    }

    #[cfg(test)]
    fn accumulate_checked(
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
            *dst = i16::try_from(value).map_err(|_| "SFNNv15 accumulator overflow".to_owned())?;
        }
        for (dst, value) in psqt.iter_mut().zip(psqt_wide) {
            *dst = i32::try_from(value).map_err(|_| "SFNNv15 PSQT overflow".to_owned())?;
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
    bytes.chunks_exact(2).map(|part| i16::from_le_bytes([part[0], part[1]])).collect()
}

fn decode_i32(bytes: &[u8]) -> Vec<i32> {
    bytes
        .chunks_exact(4)
        .map(|part| i32::from_le_bytes(part.try_into().expect("four bytes")))
        .collect()
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
    Ok(decode_i32(&bytes[PROGRESS_HEADER..]))
}

fn decode_stacked_matrix(bytes: &[u8], outputs: usize, inputs: usize) -> Vec<Vec<i8>> {
    let mut stacks = (0..BUCKETS).map(|_| vec![0_i8; outputs * inputs]).collect::<Vec<_>>();
    for input in 0..inputs {
        for (stack, weights) in stacks.iter_mut().enumerate() {
            for output in 0..outputs {
                weights[output * inputs + input] =
                    bytes[input * BUCKETS * outputs + stack * outputs + output] as i8;
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
pub(super) fn synthetic_network(seed: u64) -> MobileNetwork {
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
    MobileNetwork::from_tensors(DecodedTensors {
        digest: Digest::from_hex(&"0".repeat(64)).expect("digest literal"),
        base_weights: fill(base_inputs * WIDTH, -40, 40).into_iter().map(|v| v as i16).collect(),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 参照consumerの評価値を再現する除数。engineの既定値とは別に、parityの基準として固定する。
    const REFERENCE_FV_SCALE: i32 = 16;

    #[test]
    #[ignore = "requires RSSHOGI_RSNN_FIXTURE"]
    fn pilot_package_evaluates_startpos() {
        rsshogi::board::init();
        let path = std::env::var_os("RSSHOGI_RSNN_FIXTURE").expect("pilot fixture path");
        let expected =
            Digest::from_hex("9e3a3f8f74088a8c430ddd06617ff6cc8e35a979623c2082e1fb1d34befc5a36")
                .expect("digest literal");
        let network =
            MobileNetwork::load(Path::new(&path), Some(expected)).expect("load pilot model");
        assert_eq!(network.digest(), expected);
        for (sfen, consumer_score) in [
            ("lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1", 49),
            ("lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1", 49),
            ("4k4/9/9/4+P4/9/4+p4/9/9/4K4 b - 1", -3),
        ] {
            let position = rsshogi::board::position_from_sfen(sfen).expect("SFEN");
            let score = network.evaluate(&position, REFERENCE_FV_SCALE).expect("evaluate position");
            assert_eq!(score, consumer_score, "{sfen}");
        }
    }

    #[test]
    #[ignore = "requires RSSHOGI_RSNN_RELEASE_CANDIDATE"]
    fn release_candidate_matches_reference_consumer() {
        rsshogi::board::init();
        let path = std::env::var_os("RSSHOGI_RSNN_RELEASE_CANDIDATE")
            .expect("set RSSHOGI_RSNN_RELEASE_CANDIDATE to the 84-epoch package");
        let digest =
            Digest::from_hex("a4a61c91f85a1ee1eb67cb7c6483c66fdb6ed7c2832d3184901e2faf89dfb398")
                .expect("release candidate digest");
        let network = MobileNetwork::load(Path::new(&path), Some(digest))
            .expect("release candidate package loads");
        for (sfen, consumer_raw_output) in [
            ("lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1", 1873),
            ("lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1", 1873),
            ("4k4/9/9/4+P4/9/4+p4/9/9/4K4 b - 1", 6024),
            ("8k/9/9/9/6r1P/6B2/9/9/K8 b - 1", 2590),
            ("4k4/9/9/9/9/9/9/9/4K4 w RB2G2S2N2L9P 1", -60469),
        ] {
            let position = rsshogi::board::position_from_sfen(sfen).expect("SFEN");
            let score = network.evaluate(&position, REFERENCE_FV_SCALE).expect("evaluate position");
            assert_eq!(score, consumer_raw_output / REFERENCE_FV_SCALE, "{sfen}");
        }
    }
}

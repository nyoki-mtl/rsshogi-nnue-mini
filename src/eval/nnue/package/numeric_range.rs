//! Numeric range qualification for `aonly1024_i8_progress8_v1` packages.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::descriptor::verify_descriptor;
use super::hash::Digest;
use super::reader::{Manifest, PackageError, Result};
use super::schema::{BASE_INPUTS, BASE_MAX_ACTIVE};

const ACCUMULATOR_DIMS: usize = 1_024;
const PSQT_BUCKETS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    evaluation_spec_hash: Digest,
    tensors: [Digest; 9],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AOnlyNumericRangeReport {
    #[cfg(test)]
    accumulator_min: i64,
    #[cfg(test)]
    accumulator_max: i64,
}

static CERTIFICATE_CACHE: OnceLock<Mutex<HashMap<CacheKey, AOnlyNumericRangeReport>>> =
    OnceLock::new();

#[derive(Clone)]
struct Extrema<const K: usize> {
    positive: [i128; K],
    positive_len: usize,
    positive_min: usize,
    negative: [i128; K],
    negative_len: usize,
    negative_max: usize,
}

impl<const K: usize> Default for Extrema<K> {
    fn default() -> Self {
        Self {
            positive: [0; K],
            positive_len: 0,
            positive_min: 0,
            negative: [0; K],
            negative_len: 0,
            negative_max: 0,
        }
    }
}

impl<const K: usize> Extrema<K> {
    fn push(&mut self, value: i128) {
        if value > 0 {
            if self.positive_len < K {
                let index = self.positive_len;
                self.positive[index] = value;
                self.positive_len += 1;
                if value < self.positive[self.positive_min] {
                    self.positive_min = index;
                }
            } else if value > self.positive[self.positive_min] {
                self.positive[self.positive_min] = value;
                self.positive_min = self.positive[..self.positive_len]
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, item)| *item)
                    .map_or(0, |(index, _)| index);
            }
        } else if value < 0 {
            if self.negative_len < K {
                let index = self.negative_len;
                self.negative[index] = value;
                self.negative_len += 1;
                if value > self.negative[self.negative_max] {
                    self.negative_max = index;
                }
            } else if value < self.negative[self.negative_max] {
                self.negative[self.negative_max] = value;
                self.negative_max = self.negative[..self.negative_len]
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, item)| *item)
                    .map_or(0, |(index, _)| index);
            }
        }
    }

    fn minimum(&self) -> i128 {
        self.negative[..self.negative_len].iter().sum()
    }

    fn maximum(&self) -> i128 {
        self.positive[..self.positive_len].iter().sum()
    }
}

// Hold the cache lock across the first scan so concurrent loads share one verification pass.
#[allow(clippy::significant_drop_tightening)]
pub(super) fn certify_numeric_range(
    manifest: &Manifest,
    payload: &[u8],
) -> Result<AOnlyNumericRangeReport> {
    verify_descriptor(&manifest.evaluation_descriptor)?;

    let base = tensor_data(manifest, payload, "ft_base_weight")?;
    if base.contains(&128) {
        return Err(PackageError::InvalidDescriptor(
            "A-only FT requires symmetric I8 (-127..127)".into(),
        ));
    }

    let tensor_names = [
        "ft_base_weight",
        "ft_bias",
        "psqt_weight",
        "fc0_weight",
        "fc0_bias",
        "fc1_weight",
        "fc1_bias",
        "fc2_weight",
        "fc2_bias",
    ];
    let mut digests = [Digest::ZERO; 9];
    for (slot, name) in digests.iter_mut().zip(tensor_names) {
        *slot = tensor_digest(manifest, name)?;
    }
    let key = CacheKey { evaluation_spec_hash: manifest.evaluation_spec_hash, tensors: digests };
    let mut cache = CERTIFICATE_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("numeric range certificate cache mutex must not be poisoned");
    if let Some(&report) = cache.get(&key) {
        return Ok(report);
    }

    certify_dense(manifest, payload)?;

    let bias = tensor_data(manifest, payload, "ft_bias")?;
    let base_psqt = tensor_data(manifest, payload, "psqt_weight")?;
    let rows = BASE_INPUTS as usize;

    let _accumulator = certify_accumulator(
        rows,
        ACCUMULATOR_DIMS,
        |row, lane| i128::from(base[row * ACCUMULATOR_DIMS + lane] as i8),
        |lane| i128::from(read_i16(bias, lane)),
    )?;
    certify_psqt(rows, PSQT_BUCKETS, |row, lane| {
        i128::from(read_i32(base_psqt, row * PSQT_BUCKETS + lane))
    })?;

    let report = AOnlyNumericRangeReport {
        #[cfg(test)]
        accumulator_min: i64::try_from(_accumulator.0).expect("certified i16 bound fits i64"),
        #[cfg(test)]
        accumulator_max: i64::try_from(_accumulator.1).expect("certified i16 bound fits i64"),
    };
    cache.insert(key, report);
    Ok(report)
}

fn certify_dense(manifest: &Manifest, payload: &[u8]) -> Result<()> {
    for (name, inputs, outputs) in [("fc0", 1_024, 32), ("fc1", 64, 32), ("fc2", 128, 1)] {
        let weights = tensor_data(manifest, payload, &format!("{name}_weight"))?;
        let biases = tensor_data(manifest, payload, &format!("{name}_bias"))?;
        for row in 0..8 * outputs {
            let bias = i128::from(read_i32(biases, row));
            let mut lo = bias;
            let mut hi = bias;
            for &w in &weights[row * inputs..(row + 1) * inputs] {
                let w = i128::from(w as i8) * 127;
                lo += w.min(0);
                hi += w.max(0);
            }
            require_range(
                format!("A-only {name} row {row}"),
                lo,
                hi,
                i128::from(i32::MIN),
                i128::from(i32::MAX),
            )?;
        }
    }
    Ok(())
}

fn certify_accumulator(
    rows: usize,
    lanes: usize,
    base: impl Fn(usize, usize) -> i128,
    bias: impl Fn(usize) -> i128,
) -> Result<(i128, i128)> {
    let extrema = collect_extrema::<BASE_MAX_ACTIVE>(rows, lanes, &base);
    let mut overall_min = i128::MAX;
    let mut overall_max = i128::MIN;
    for (lane, target) in extrema.iter().enumerate() {
        let minimum = bias(lane) + target.minimum();
        let maximum = bias(lane) + target.maximum();
        require_range(
            format!("base accumulator channel {lane}"),
            minimum,
            maximum,
            i128::from(i16::MIN),
            i128::from(i16::MAX),
        )?;
        overall_min = overall_min.min(minimum);
        overall_max = overall_max.max(maximum);
    }
    Ok((overall_min, overall_max))
}

fn certify_psqt(rows: usize, lanes: usize, base: impl Fn(usize, usize) -> i128) -> Result<()> {
    let extrema = collect_extrema::<BASE_MAX_ACTIVE>(rows, lanes, &base);
    for (lane, target) in extrema.iter().enumerate() {
        let minimum = target.minimum();
        let maximum = target.maximum();
        require_range(
            format!("base PSQT bucket {lane}"),
            minimum,
            maximum,
            i128::from(i32::MIN),
            i128::from(i32::MAX),
        )?;
        if maximum - minimum > i128::from(i32::MAX) {
            return Err(PackageError::NumericRangeViolation {
                context: format!("PSQT perspective subtraction bucket {lane}"),
                minimum,
                maximum,
                allowed_minimum: i128::from(i32::MIN),
                allowed_maximum: i128::from(i32::MAX),
            });
        }
    }
    Ok(())
}

fn collect_extrema<const K: usize>(
    rows: usize,
    lanes: usize,
    value: &impl Fn(usize, usize) -> i128,
) -> Vec<Extrema<K>> {
    let mut extrema = vec![Extrema::default(); lanes];
    for row in 0..rows {
        for (lane, target) in extrema.iter_mut().enumerate() {
            target.push(value(row, lane));
        }
    }
    extrema
}

fn require_range(
    context: String,
    minimum: i128,
    maximum: i128,
    allowed_minimum: i128,
    allowed_maximum: i128,
) -> Result<()> {
    if minimum < allowed_minimum || maximum > allowed_maximum {
        return Err(PackageError::NumericRangeViolation {
            context,
            minimum,
            maximum,
            allowed_minimum,
            allowed_maximum,
        });
    }
    Ok(())
}

fn tensor_data<'a>(manifest: &Manifest, payload: &'a [u8], name: &str) -> Result<&'a [u8]> {
    let tensor = manifest
        .tensors
        .iter()
        .find(|tensor| tensor.name == name)
        .ok_or_else(|| PackageError::MissingEntry(name.into()))?;
    let start = usize::try_from(tensor.offset).map_err(|_| PackageError::LengthOverflow)?;
    let elements = tensor
        .shape
        .iter()
        .try_fold(1_u64, |product, dimension| product.checked_mul(*dimension))
        .ok_or(PackageError::LengthOverflow)?;
    let bytes =
        elements.checked_mul(tensor.dtype.byte_width()).ok_or(PackageError::LengthOverflow)?;
    let end = start
        .checked_add(usize::try_from(bytes).map_err(|_| PackageError::LengthOverflow)?)
        .ok_or(PackageError::LengthOverflow)?;
    payload.get(start..end).ok_or_else(|| PackageError::InvalidEntryRange { name: name.into() })
}

fn tensor_digest(manifest: &Manifest, name: &str) -> Result<Digest> {
    manifest
        .tensors
        .iter()
        .find(|tensor| tensor.name == name)
        .map(|tensor| tensor.digest)
        .ok_or_else(|| PackageError::MissingEntry(name.into()))
}

fn read_i16(bytes: &[u8], index: usize) -> i16 {
    let offset = index * 2;
    i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_i32(bytes: &[u8], index: usize) -> i32 {
    let offset = index * 4;
    i32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::nnue::package::descriptor::{DType, EvaluationDescriptor};
    use crate::eval::nnue::package::hash::sha256;
    use crate::eval::nnue::package::reader::Provenance;

    fn manifest(descriptor: EvaluationDescriptor) -> Manifest {
        let tensors = descriptor.tensors.clone();
        Manifest::new(
            descriptor,
            Provenance {
                training_spec_hash: Digest::ZERO,
                export_spec_hash: Digest::ZERO,
                source_plan_hash: Digest::ZERO,
                source_checkpoint_digest: Digest::ZERO,
                source_dataset_identity: "test".into(),
            },
            tensors,
            Vec::new(),
            sha256(&[]),
        )
        .unwrap()
    }

    #[test]
    fn aonly_contract_requires_exact_recipe_and_shape() {
        let exact = manifest(EvaluationDescriptor::aonly1024_i8_progress8_v1_schema());
        assert!(matches!(
            certify_numeric_range(&exact, &[]),
            Err(PackageError::InvalidEntryRange { .. })
        ));
        let mut wrong = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
        wrong.tensors[0].shape[1] = 512;
        assert_eq!(
            certify_numeric_range(&manifest(wrong), &[]),
            Err(PackageError::DescriptorMismatch { field: "tensor_schema" })
        );
        let mut old = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
        old.recipe_id = "aonly1024_progress8_v1".into();
        assert!(matches!(
            certify_numeric_range(&manifest(old), &[]),
            Err(PackageError::UnsupportedRecipe(_))
        ));
    }

    #[test]
    fn aonly_i8_certificate_sign_extends_weights_and_rejects_minus_128() {
        let mut m = manifest(EvaluationDescriptor::aonly1024_i8_progress8_v1_schema());
        let mut payload = vec![0; m.tensors.iter().map(|t| t.length).sum::<u64>() as usize];
        let base = m.tensors.iter().find(|t| t.name == "ft_base_weight").unwrap().offset as usize;
        for row in 0..40 {
            payload[base + row * 1_024] = (-127_i8) as u8;
            payload[base + row * 1_024 + 1] = 127;
        }
        m.tensors[0].digest = sha256(&payload[base..base + m.tensors[0].length as usize]);
        let report = certify_numeric_range(&m, &payload).unwrap();
        assert_eq!((report.accumulator_min, report.accumulator_max), (-5_080, 5_080));

        // Even with a cached digest, -128 must be rejected up front.
        payload[base] = 128;
        assert!(matches!(
            certify_numeric_range(&m, &payload),
            Err(PackageError::InvalidDescriptor(_))
        ));
        m.evaluation_descriptor.tensors[0].dtype = DType::I16;
        assert_eq!(
            certify_numeric_range(&m, &payload),
            Err(PackageError::DescriptorMismatch { field: "tensor_schema" })
        );
    }

    #[test]
    fn aonly_dense_and_accumulator_and_psqt_bounds_are_enforced() {
        assert_eq!(
            certify_accumulator(1, 1, |_, _| i128::from(i16::MAX), |_| 0).unwrap(),
            (0, i128::from(i16::MAX))
        );
        assert!(matches!(
            certify_accumulator(1, 1, |_, _| i128::from(i16::MAX), |_| 1),
            Err(PackageError::NumericRangeViolation { .. })
        ));
        assert!(matches!(
            certify_psqt(2, 1, |row, _| {
                if row == 0 { i128::from(i32::MAX) } else { -1 }
            }),
            Err(PackageError::NumericRangeViolation { context, .. }) if context.contains("subtraction")
        ));

        let mut m = manifest(EvaluationDescriptor::aonly1024_i8_progress8_v1_schema());
        let mut payload = vec![0; m.tensors.iter().map(|t| t.length).sum::<u64>() as usize];
        let weight = m.tensors.iter().find(|t| t.name == "fc0_weight").unwrap().offset as usize;
        let bias = m.tensors.iter().find(|t| t.name == "fc0_bias").unwrap().offset as usize;
        payload[weight] = 1;
        payload[bias..bias + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        m.tensors[3].digest = sha256(&[1]);
        assert!(matches!(
            certify_numeric_range(&m, &payload),
            Err(PackageError::NumericRangeViolation { .. })
        ));
    }
}

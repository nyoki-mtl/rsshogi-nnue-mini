//! Numeric range qualification for the fixed `SFNNv15` feature transformer.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::sfnnv15_feature_schema::{self as schema, BASE_MAX_ACTIVE};
use crate::{Digest, Error, EvaluationDescriptor, EvaluatorExpectation, Manifest, Result};

// 証明は「任意の <=BASE_MAX_ACTIVE 行 x 任意の <=THREAT_MAX_ACTIVE 行」の
// worst-case 包絡なので、上限を上げると保守的な top-K bound が広がり、
// 現行 package が不合格になり得る。BASE_MAX_ACTIVE は推論側の active 容量と共有する。
// threat の定数は threat 特徴を持つ `sfnnv15_v2` package の証明にだけ使う。
const THREAT_FEATURES: usize = 114_880;
const THREAT_MAX_ACTIVE: usize = 128;
const ACCUMULATOR_DIMS: usize = 1_024;
const PSQT_BUCKETS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    evaluation_spec_hash: Digest,
    tensors: [Digest; 5],
}

static CERTIFICATE_CACHE: OnceLock<Mutex<HashMap<CacheKey, Sfnnv15NumericRangeReport>>> =
    OnceLock::new();

/// Conservative final-value bounds proved from one `SFNNv15` package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sfnnv15NumericRangeReport {
    pub accumulator_min: i64,
    pub accumulator_max: i64,
    pub psqt_sum_min: i64,
    pub psqt_sum_max: i64,
    pub psqt_difference_span_max: i64,
}

/// Whether a package received the mandatory `SFNNv15` numeric certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sfnnv15NumericRangeVerification {
    Certified(Sfnnv15NumericRangeReport),
    NotApplicable,
}

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

// Keep the cache lock across the first calculation so concurrent engine/test
// sessions do not all scan the same large tensors at once.
#[allow(clippy::significant_drop_tightening)]
pub fn verify_sfnnv15_numeric_range(
    manifest: &Manifest,
    payload: &[u8],
) -> Result<Sfnnv15NumericRangeVerification> {
    let (expected, base_buckets, base_features_per_bucket, accumulator_dims, threat_rows) =
        match manifest.evaluation_descriptor.recipe_id.as_str() {
            "sfnnv15_v2" => (
                EvaluationDescriptor::sfnnv15_v2_schema(),
                usize::from(schema::BASE_KING_BUCKETS),
                usize::from(schema::BASE_PLANES),
                ACCUMULATOR_DIMS,
                THREAT_FEATURES,
            ),
            "sfnnv15_mobile_v1" => (
                EvaluationDescriptor::sfnnv15_mobile_v1_schema(),
                usize::from(schema::BASE_KING_BUCKETS),
                usize::from(schema::BASE_PLANES),
                512,
                0,
            ),
            recipe
                if recipe.starts_with("sfnnv15_")
                    || manifest.evaluation_descriptor.feature_schema.id == "sfnnv15_shogi" =>
            {
                return Err(Error::UnsupportedRecipe(format!(
                    "{}@{}",
                    recipe, manifest.evaluation_descriptor.recipe_version
                )));
            }
            _ => return Ok(Sfnnv15NumericRangeVerification::NotApplicable),
        };
    let expectation = EvaluatorExpectation::new(expected)?;
    expectation.verify(&manifest.evaluation_descriptor)?;

    let key = CacheKey {
        evaluation_spec_hash: manifest.evaluation_spec_hash,
        tensors: [
            tensor_digest(manifest, "ft_base_weight")?,
            if threat_rows == 0 {
                Digest::ZERO
            } else {
                tensor_digest(manifest, "ft_threat_weight")?
            },
            tensor_digest(manifest, "ft_bias")?,
            tensor_digest(manifest, "psqt_weight")?,
            if threat_rows == 0 {
                Digest::ZERO
            } else {
                tensor_digest(manifest, "psqt_threat_weight")?
            },
        ],
    };
    let mut cache = CERTIFICATE_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("numeric range certificate cache mutex must not be poisoned by verifier-only code");
    if let Some(report) = cache.get(&key) {
        return Ok(Sfnnv15NumericRangeVerification::Certified(*report));
    }

    let base = tensor_data(manifest, payload, "ft_base_weight")?;
    let threat = if threat_rows == 0 {
        &[][..]
    } else {
        tensor_data(manifest, payload, "ft_threat_weight")?
    };
    let bias = tensor_data(manifest, payload, "ft_bias")?;
    let base_psqt = tensor_data(manifest, payload, "psqt_weight")?;
    let threat_psqt = if threat_rows == 0 {
        &[][..]
    } else {
        tensor_data(manifest, payload, "psqt_threat_weight")?
    };

    let accumulator = certify_accumulator(
        |row, lane| i128::from(read_i16(base, row * accumulator_dims + lane)),
        |row, lane| i128::from(i8::from_le_bytes([threat[row * accumulator_dims + lane]])),
        |lane| i128::from(read_i16(bias, lane)),
        NumericLayout {
            base_buckets,
            base_rows_per_bucket: base_features_per_bucket,
            threat_rows,
            lanes: accumulator_dims,
        },
    )?;
    let psqt = certify_psqt(
        |row, lane| i128::from(read_i32(base_psqt, row * PSQT_BUCKETS + lane)),
        |row, lane| i128::from(read_i32(threat_psqt, row * PSQT_BUCKETS + lane)),
        NumericLayout {
            base_buckets,
            base_rows_per_bucket: base_features_per_bucket,
            threat_rows,
            lanes: PSQT_BUCKETS,
        },
    )?;
    let report = Sfnnv15NumericRangeReport {
        accumulator_min: fixed_bound_to_i64(accumulator.0),
        accumulator_max: fixed_bound_to_i64(accumulator.1),
        psqt_sum_min: fixed_bound_to_i64(psqt.0),
        psqt_sum_max: fixed_bound_to_i64(psqt.1),
        psqt_difference_span_max: fixed_bound_to_i64(psqt.2),
    };
    cache.insert(key, report);
    Ok(Sfnnv15NumericRangeVerification::Certified(report))
}

#[derive(Clone, Copy)]
struct NumericLayout {
    base_buckets: usize,
    base_rows_per_bucket: usize,
    threat_rows: usize,
    lanes: usize,
}

fn certify_accumulator<Base, Threat, Bias>(
    base: Base,
    threat: Threat,
    bias: Bias,
    layout: NumericLayout,
) -> Result<(i128, i128)>
where
    Base: Fn(usize, usize) -> i128,
    Threat: Fn(usize, usize) -> i128,
    Bias: Fn(usize) -> i128,
{
    let threat_extrema =
        collect_extrema::<THREAT_MAX_ACTIVE, _>(layout.threat_rows, layout.lanes, &threat);
    let mut overall_min = i128::MAX;
    let mut overall_max = i128::MIN;
    for king_bucket in 0..layout.base_buckets {
        let first_row = king_bucket * layout.base_rows_per_bucket;
        let base_extrema = collect_extrema::<BASE_MAX_ACTIVE, _>(
            layout.base_rows_per_bucket,
            layout.lanes,
            &|row, lane| base(first_row + row, lane),
        );
        for lane in 0..layout.lanes {
            let base_minimum = bias(lane) + base_extrema[lane].minimum();
            let base_maximum = bias(lane) + base_extrema[lane].maximum();
            require_range(
                format!("base accumulator king_bucket {king_bucket} channel {lane}"),
                base_minimum,
                base_maximum,
                i128::from(i16::MIN),
                i128::from(i16::MAX),
            )?;
            let minimum = base_minimum + threat_extrema[lane].minimum();
            let maximum = base_maximum + threat_extrema[lane].maximum();
            require_range(
                format!("combined accumulator king_bucket {king_bucket} channel {lane}"),
                minimum,
                maximum,
                i128::from(i16::MIN),
                i128::from(i16::MAX),
            )?;
            overall_min = overall_min.min(minimum);
            overall_max = overall_max.max(maximum);
        }
    }
    Ok((overall_min, overall_max))
}

fn certify_psqt<Base, Threat>(
    base: Base,
    threat: Threat,
    layout: NumericLayout,
) -> Result<(i128, i128, i128)>
where
    Base: Fn(usize, usize) -> i128,
    Threat: Fn(usize, usize) -> i128,
{
    let threat_extrema =
        collect_extrema::<THREAT_MAX_ACTIVE, _>(layout.threat_rows, layout.lanes, &threat);
    let mut minima = vec![i128::MAX; layout.lanes];
    let mut maxima = vec![i128::MIN; layout.lanes];
    for king_bucket in 0..layout.base_buckets {
        let first_row = king_bucket * layout.base_rows_per_bucket;
        let base_extrema = collect_extrema::<BASE_MAX_ACTIVE, _>(
            layout.base_rows_per_bucket,
            layout.lanes,
            &|row, lane| base(first_row + row, lane),
        );
        for lane in 0..layout.lanes {
            let base_minimum = base_extrema[lane].minimum();
            let base_maximum = base_extrema[lane].maximum();
            require_range(
                format!("base PSQT king_bucket {king_bucket} bucket {lane}"),
                base_minimum,
                base_maximum,
                i128::from(i32::MIN),
                i128::from(i32::MAX),
            )?;
            let minimum = base_minimum + threat_extrema[lane].minimum();
            let maximum = base_maximum + threat_extrema[lane].maximum();
            require_range(
                format!("combined PSQT king_bucket {king_bucket} bucket {lane}"),
                minimum,
                maximum,
                i128::from(i32::MIN),
                i128::from(i32::MAX),
            )?;
            minima[lane] = minima[lane].min(minimum);
            maxima[lane] = maxima[lane].max(maximum);
        }
    }
    let mut maximum_span = 0;
    for lane in 0..layout.lanes {
        let span = maxima[lane] - minima[lane];
        if span > i128::from(i32::MAX) {
            return Err(Error::NumericRangeViolation {
                context: format!("PSQT perspective subtraction bucket {lane}"),
                minimum: minima[lane],
                maximum: maxima[lane],
                allowed_minimum: i128::from(i32::MIN),
                allowed_maximum: i128::from(i32::MAX),
            });
        }
        maximum_span = maximum_span.max(span);
    }
    Ok((minima.into_iter().min().unwrap_or(0), maxima.into_iter().max().unwrap_or(0), maximum_span))
}

fn collect_extrema<const K: usize, Value>(
    rows: usize,
    lanes: usize,
    value: &Value,
) -> Vec<Extrema<K>>
where
    Value: Fn(usize, usize) -> i128,
{
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
        return Err(Error::NumericRangeViolation {
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
        .ok_or_else(|| Error::MissingEntry(name.into()))?;
    let start = usize::try_from(tensor.offset).map_err(|_| Error::LengthOverflow)?;
    let elements = tensor
        .shape
        .iter()
        .try_fold(1_u64, |product, dimension| product.checked_mul(*dimension))
        .ok_or(Error::LengthOverflow)?;
    let bytes = elements.checked_mul(tensor.dtype.byte_width()).ok_or(Error::LengthOverflow)?;
    let end = start
        .checked_add(usize::try_from(bytes).map_err(|_| Error::LengthOverflow)?)
        .ok_or(Error::LengthOverflow)?;
    payload.get(start..end).ok_or_else(|| Error::InvalidEntryRange { name: name.into() })
}

fn tensor_digest(manifest: &Manifest, name: &str) -> Result<Digest> {
    manifest
        .tensors
        .iter()
        .find(|tensor| tensor.name == name)
        .map(|tensor| tensor.digest)
        .ok_or_else(|| Error::MissingEntry(name.into()))
}

fn read_i16(bytes: &[u8], index: usize) -> i16 {
    let offset = index * 2;
    i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_i32(bytes: &[u8], index: usize) -> i32 {
    let offset = index * 4;
    i32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn fixed_bound_to_i64(value: i128) -> i64 {
    i64::try_from(value).expect("fixed SFNNv15 dimensions and element types fit i64")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Provenance, sha256};

    const SMALL: NumericLayout =
        NumericLayout { base_buckets: 2, base_rows_per_bucket: 3, threat_rows: 4, lanes: 2 };

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
    fn sfnnv15_contract_mismatches_are_not_silently_skipped() {
        let exact = manifest(EvaluationDescriptor::sfnnv15_v2_schema());
        assert!(matches!(
            verify_sfnnv15_numeric_range(&exact, &[]),
            Err(Error::InvalidEntryRange { .. })
        ));

        let mut wrong_version = EvaluationDescriptor::sfnnv15_v2_schema();
        wrong_version.recipe_version = 1;
        assert!(matches!(
            verify_sfnnv15_numeric_range(&manifest(wrong_version), &[]),
            Err(Error::UnsupportedRecipe(_))
        ));

        let mut wrong_feature = EvaluationDescriptor::sfnnv15_v2_schema();
        wrong_feature.feature_schema.blocks[0].indexing = "wrong".into();
        assert_eq!(
            verify_sfnnv15_numeric_range(&manifest(wrong_feature), &[]),
            Err(Error::DescriptorMismatch { field: "feature_schema" })
        );

        let mut wrong_tensor = EvaluationDescriptor::sfnnv15_v2_schema();
        wrong_tensor.tensors[0].shape[0] += 1;
        assert_eq!(
            verify_sfnnv15_numeric_range(&manifest(wrong_tensor), &[]),
            Err(Error::DescriptorMismatch { field: "tensor_schema" })
        );
    }

    #[test]
    fn mobile_contract_requires_numeric_range_data_and_exact_shape() {
        let exact = manifest(EvaluationDescriptor::sfnnv15_mobile_v1_schema());
        assert!(matches!(
            verify_sfnnv15_numeric_range(&exact, &[]),
            Err(Error::InvalidEntryRange { .. })
        ));
        let mut wrong = EvaluationDescriptor::sfnnv15_mobile_v1_schema();
        wrong.tensors[0].shape[1] = 1_024;
        assert_eq!(
            verify_sfnnv15_numeric_range(&manifest(wrong), &[]),
            Err(Error::DescriptorMismatch { field: "tensor_schema" })
        );
    }

    #[test]
    fn non_sfnnv15_contract_is_typed_not_applicable() {
        let mut descriptor = EvaluationDescriptor::sfnnv15_v2_schema();
        descriptor.recipe_id = "other_v1".into();
        descriptor.feature_schema.id = "other_shogi".into();
        assert_eq!(
            verify_sfnnv15_numeric_range(&manifest(descriptor), &[]).unwrap(),
            Sfnnv15NumericRangeVerification::NotApplicable
        );
    }

    #[test]
    fn extrema_pad_unused_active_slots_with_zero() {
        let mut extrema = Extrema::<2>::default();
        extrema.push(10);
        extrema.push(-1);
        assert_eq!(extrema.maximum(), 10);
        assert_eq!(extrema.minimum(), -1);
    }

    #[test]
    fn small_certificate_reports_conservative_bounds() {
        let base = [[1, -2], [3, -4], [-5, 6], [7, -8], [9, 10], [-11, 12]];
        let threat = [[1, -1], [2, -2], [-3, 3], [4, -4]];
        let accumulator = certify_accumulator(
            |row, lane| base[row][lane],
            |row, lane| threat[row][lane],
            |lane| [10, -10][lane],
            SMALL,
        )
        .unwrap();
        assert_eq!(accumulator, (-25, 33));
        let psqt = certify_psqt(|row, lane| base[row][lane], |row, lane| threat[row][lane], SMALL)
            .unwrap();
        assert_eq!(psqt, (-15, 25, 40));
    }

    #[test]
    fn king_buckets_are_not_mixed() {
        let result = certify_accumulator(
            |_, _| 20_000,
            |_, _| 0,
            |_| 0,
            NumericLayout { base_buckets: 2, base_rows_per_bucket: 1, threat_rows: 0, lanes: 1 },
        )
        .unwrap();
        assert_eq!(result, (0, 20_000));
    }

    #[test]
    fn base_accumulator_overflow_is_rejected_before_threat_cancellation() {
        let error = certify_accumulator(
            |_, _| 20_000,
            |_, _| -20_000,
            |_| 0,
            NumericLayout { base_buckets: 1, base_rows_per_bucket: 2, threat_rows: 2, lanes: 1 },
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::NumericRangeViolation { context, .. } if context.contains("base accumulator"))
        );
    }

    #[test]
    fn accumulator_i16_boundary_is_inclusive_and_plus_one_is_rejected() {
        let layout =
            NumericLayout { base_buckets: 1, base_rows_per_bucket: 1, threat_rows: 0, lanes: 1 };
        assert_eq!(
            certify_accumulator(|_, _| i128::from(i16::MAX), |_, _| 0, |_| 0, layout).unwrap(),
            (0, i128::from(i16::MAX))
        );
        let error =
            certify_accumulator(|_, _| i128::from(i16::MAX), |_, _| 0, |_| 1, layout).unwrap_err();
        assert!(matches!(error, Error::NumericRangeViolation { .. }));
    }

    #[test]
    fn combined_accumulator_overflow_is_rejected() {
        let error = certify_accumulator(
            |_, _| i128::from(i16::MAX),
            |_, _| 1,
            |_| 0,
            NumericLayout { base_buckets: 1, base_rows_per_bucket: 1, threat_rows: 1, lanes: 1 },
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::NumericRangeViolation { context, .. } if context.contains("combined accumulator"))
        );
    }

    #[test]
    fn base_psqt_overflow_is_rejected_before_threat_cancellation() {
        let error = certify_psqt(
            |_, _| i128::from(i32::MAX),
            |_, _| -i128::from(i32::MAX),
            NumericLayout { base_buckets: 1, base_rows_per_bucket: 2, threat_rows: 2, lanes: 1 },
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::NumericRangeViolation { context, .. } if context.contains("base PSQT"))
        );
    }

    #[test]
    fn psqt_i32_boundary_is_inclusive_and_combined_plus_one_is_rejected() {
        let layout =
            NumericLayout { base_buckets: 1, base_rows_per_bucket: 1, threat_rows: 0, lanes: 1 };
        assert_eq!(
            certify_psqt(|_, _| i128::from(i32::MAX), |_, _| 0, layout).unwrap(),
            (0, i128::from(i32::MAX), i128::from(i32::MAX))
        );
        let error = certify_psqt(
            |_, _| i128::from(i32::MAX),
            |_, _| 1,
            NumericLayout { threat_rows: 1, ..layout },
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::NumericRangeViolation { context, .. } if context.contains("combined PSQT"))
        );
    }

    #[test]
    fn psqt_subtraction_span_violation_is_rejected() {
        let error = certify_psqt(
            |row, _| if row == 0 { i128::from(i32::MAX) } else { i128::from(i32::MIN) },
            |_, _| 0,
            NumericLayout { base_buckets: 2, base_rows_per_bucket: 1, threat_rows: 0, lanes: 1 },
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::NumericRangeViolation { context, .. } if context.contains("subtraction"))
        );
    }
}

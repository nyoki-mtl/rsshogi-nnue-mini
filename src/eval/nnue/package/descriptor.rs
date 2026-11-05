//! Evaluation descriptor contract for the `aonly1024_i8_progress8_v1` recipe.

use serde::{Deserialize, Serialize};

use super::hash::{Digest, evaluation_spec_hash};
use super::reader::{PackageError, Result};
use super::schema::BASE_INPUTS;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DType {
    I8,
    I16,
    I32,
    I64,
    U8,
}

impl DType {
    #[must_use]
    pub(crate) const fn byte_width(&self) -> u64 {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 => 2,
            Self::I32 => 4,
            Self::I64 => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TensorRole {
    FeatureTransformer,
    HiddenWeight,
    HiddenBias,
    OutputWeight,
    OutputBias,
    Psqt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rational {
    pub(crate) numerator: i64,
    pub(crate) denominator: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ActivationSpec {
    pub(crate) feature_transform: String,
    pub(crate) first_clamp: Rational,
    pub(crate) first_shift: u32,
    pub(crate) second_clamp: Rational,
    pub(crate) second_shift: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IntegerEvaluationSpec {
    pub(crate) accumulator_bits: u16,
    pub(crate) intermediate_bits: u16,
    pub(crate) activation: ActivationSpec,
    pub(crate) positive_skip_channel: u32,
    pub(crate) negative_skip_channel: u32,
    pub(crate) output_multiplier: i64,
    pub(crate) output_divisor: u64,
    pub(crate) division: String,
    pub(crate) psqt_addition: String,
    pub(crate) output_units: String,
    pub(crate) engine_scale: Rational,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FeatureSchema {
    pub(crate) id: String,
    pub(crate) version: u32,
    pub(crate) perspective: String,
    pub(crate) blocks: Vec<FeatureBlock>,
    pub(crate) bucket_rule: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FeatureBlock {
    pub(crate) name: String,
    pub(crate) feature_count: u64,
    pub(crate) indexing: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TensorDescriptor {
    pub(crate) name: String,
    pub(crate) role: TensorRole,
    pub(crate) dtype: DType,
    pub(crate) shape: Vec<u64>,
    pub(crate) scale: Rational,
    pub(crate) offset: u64,
    pub(crate) length: u64,
    pub(crate) digest: Digest,
    pub(crate) padding: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AuxiliaryDescriptor {
    pub(crate) name: String,
    pub(crate) media_type: String,
    pub(crate) offset: u64,
    pub(crate) length: u64,
    pub(crate) digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvaluationDescriptor {
    pub(crate) schema_version: u32,
    pub(crate) recipe_id: String,
    pub(crate) recipe_version: u32,
    pub(crate) feature_schema: FeatureSchema,
    pub(crate) tensors: Vec<TensorDescriptor>,
    pub(crate) integer_evaluation: IntegerEvaluationSpec,
}

impl EvaluationDescriptor {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(PackageError::InvalidDescriptor(format!(
                "unsupported evaluation descriptor schema version {}",
                self.schema_version
            )));
        }
        if self.recipe_id.is_empty() {
            return Err(PackageError::InvalidDescriptor("recipe_id is empty".into()));
        }
        if self.feature_schema.id.is_empty()
            || self.feature_schema.blocks.is_empty()
            || self.feature_schema.blocks.iter().any(|block| {
                block.name.is_empty() || block.indexing.is_empty() || block.feature_count == 0
            })
        {
            return Err(PackageError::InvalidDescriptor(
                "feature schema identity or block is invalid".into(),
            ));
        }
        if self.integer_evaluation.activation.first_clamp.denominator == 0
            || self.integer_evaluation.activation.second_clamp.denominator == 0
            || self.integer_evaluation.output_divisor == 0
            || self.integer_evaluation.engine_scale.denominator == 0
        {
            return Err(PackageError::InvalidDescriptor("a rational denominator is zero".into()));
        }
        if self.tensors.iter().any(|tensor| tensor.name.is_empty() || tensor.scale.denominator == 0)
        {
            return Err(PackageError::InvalidDescriptor(
                "tensor identity or scale is invalid".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn hash(&self) -> Result<Digest> {
        evaluation_spec_hash(self)
    }

    /// Runtime descriptor for A-only FT1024 with symmetric `i8` FT weights and fixed KP progress.
    #[must_use]
    pub(crate) fn aonly1024_i8_progress8_v1(tensors: Vec<TensorDescriptor>) -> Self {
        Self {
            schema_version: 1,
            recipe_id: "aonly1024_i8_progress8_v1".into(),
            recipe_version: 1,
            feature_schema: FeatureSchema {
                id: "aonly_shogi".into(),
                version: 1,
                perspective: "friend_enemy_rotate180".into(),
                blocks: vec![FeatureBlock {
                    name: "base".into(),
                    feature_count: u64::from(BASE_INPUTS),
                    indexing: "aonly_full14_unaryhand_v1".into(),
                }],
                bucket_rule: "kp_absolute_progress_q16_v2".into(),
            },
            tensors,
            integer_evaluation: IntegerEvaluationSpec {
                accumulator_bits: 32,
                intermediate_bits: 64,
                activation: ActivationSpec {
                    feature_transform: "pairwise_product_pool_clamp_0_255_divide_512".into(),
                    first_clamp: Rational { numerator: 255, denominator: 256 },
                    first_shift: 7,
                    second_clamp: Rational { numerator: 127, denominator: 128 },
                    second_shift: 6,
                },
                positive_skip_channel: 30,
                negative_skip_channel: 31,
                output_multiplier: 9_600,
                output_divisor: 16_384,
                division: "multiply_then_truncate_toward_zero".into(),
                psqt_addition: "halved_perspective_difference_after_positional_scaling".into(),
                output_units: "engine_value".into(),
                engine_scale: Rational { numerator: 1, denominator: 1 },
            },
        }
    }

    /// Logical tensor table and schema for `aonly1024_i8_progress8_v1`.
    #[must_use]
    pub(crate) fn aonly1024_i8_progress8_v1_schema() -> Self {
        let base_inputs = u64::from(BASE_INPUTS);
        let definitions = [
            (
                "ft_base_weight",
                TensorRole::FeatureTransformer,
                DType::I8,
                vec![base_inputs, 1_024],
                Rational { numerator: 1, denominator: 256 },
            ),
            (
                "ft_bias",
                TensorRole::FeatureTransformer,
                DType::I16,
                vec![1_024],
                Rational { numerator: 1, denominator: 256 },
            ),
            (
                "psqt_weight",
                TensorRole::Psqt,
                DType::I32,
                vec![base_inputs, 8],
                Rational { numerator: 1, denominator: 9_600 },
            ),
            (
                "fc0_weight",
                TensorRole::HiddenWeight,
                DType::I8,
                vec![8, 32, 1_024],
                Rational { numerator: 1, denominator: 128 },
            ),
            (
                "fc0_bias",
                TensorRole::HiddenBias,
                DType::I32,
                vec![8, 32],
                Rational { numerator: 1, denominator: 16_384 },
            ),
            (
                "fc1_weight",
                TensorRole::HiddenWeight,
                DType::I8,
                vec![8, 32, 64],
                Rational { numerator: 1, denominator: 64 },
            ),
            (
                "fc1_bias",
                TensorRole::HiddenBias,
                DType::I32,
                vec![8, 32],
                Rational { numerator: 1, denominator: 8_192 },
            ),
            (
                "fc2_weight",
                TensorRole::OutputWeight,
                DType::I8,
                vec![8, 1, 128],
                Rational { numerator: 1, denominator: 128 },
            ),
            (
                "fc2_bias",
                TensorRole::OutputBias,
                DType::I32,
                vec![8, 1],
                Rational { numerator: 1, denominator: 16_384 },
            ),
        ];
        let mut offset = 0_u64;
        let tensors = definitions
            .into_iter()
            .map(|(name, role, dtype, shape, scale)| {
                let length = shape.iter().copied().product::<u64>() * dtype.byte_width();
                let tensor = TensorDescriptor {
                    name: name.into(),
                    role,
                    dtype,
                    shape,
                    scale,
                    offset,
                    length,
                    digest: Digest::ZERO,
                    padding: 0,
                };
                offset += length;
                tensor
            })
            .collect();
        Self::aonly1024_i8_progress8_v1(tensors)
    }
}

pub(super) fn verify_descriptor(actual: &EvaluationDescriptor) -> Result<()> {
    let expected = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
    if actual.recipe_id != expected.recipe_id || actual.recipe_version != expected.recipe_version {
        return Err(PackageError::UnsupportedRecipe(format!(
            "{}@{}",
            actual.recipe_id, actual.recipe_version
        )));
    }
    if actual.schema_version != expected.schema_version {
        return Err(PackageError::DescriptorMismatch { field: "schema_version" });
    }
    if actual.feature_schema != expected.feature_schema {
        return Err(PackageError::DescriptorMismatch { field: "feature_schema" });
    }
    if actual.integer_evaluation != expected.integer_evaluation {
        return Err(PackageError::DescriptorMismatch { field: "integer_evaluation" });
    }
    if actual
        .tensors
        .iter()
        .map(|tensor| (&tensor.name, &tensor.role, &tensor.dtype, &tensor.shape, &tensor.scale))
        .ne(expected.tensors.iter().map(|tensor| {
            (&tensor.name, &tensor.role, &tensor.dtype, &tensor.shape, &tensor.scale)
        }))
    {
        return Err(PackageError::DescriptorMismatch { field: "tensor_schema" });
    }
    if actual.hash()? != expected.hash()? {
        return Err(PackageError::DescriptorMismatch { field: "evaluation_spec_hash" });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aonly_schema_shapes_size_and_hash_are_golden() {
        let descriptor = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
        assert_eq!(descriptor.tensors[0].shape, [2344, 1024]);
        assert_eq!(descriptor.tensors[0].dtype, DType::I8);
        assert_eq!(descriptor.tensors.iter().map(|tensor| tensor.length).sum::<u64>(), 2_758_944);
        assert_eq!(
            descriptor.hash().unwrap().to_hex(),
            "b82e7ecc070e3810a6bbf7ca4fa1d840649802fbfcf5706e1478203bbbf995e2"
        );
    }
}

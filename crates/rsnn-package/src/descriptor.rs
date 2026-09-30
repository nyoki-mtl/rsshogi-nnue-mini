use serde::{Deserialize, Serialize};

use crate::sfnnv15_feature_schema as schema;
use crate::{Digest, Error, Result, evaluation_spec_hash};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DType {
    I8,
    I16,
    I32,
    I64,
    U8,
}

impl DType {
    #[must_use]
    pub const fn byte_width(&self) -> u64 {
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
pub enum TensorRole {
    FeatureTransformer,
    HiddenWeight,
    HiddenBias,
    OutputWeight,
    OutputBias,
    Psqt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rational {
    pub numerator: i64,
    pub denominator: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationSpec {
    pub feature_transform: String,
    pub first_clamp: Rational,
    pub first_shift: u32,
    pub second_clamp: Rational,
    pub second_shift: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegerEvaluationSpec {
    pub accumulator_bits: u16,
    pub intermediate_bits: u16,
    pub activation: ActivationSpec,
    pub positive_skip_channel: u32,
    pub negative_skip_channel: u32,
    pub output_multiplier: i64,
    pub output_divisor: u64,
    pub division: String,
    pub psqt_addition: String,
    pub output_units: String,
    pub engine_scale: Rational,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureSchema {
    pub id: String,
    pub version: u32,
    pub perspective: String,
    pub blocks: Vec<FeatureBlock>,
    pub bucket_rule: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureBlock {
    pub name: String,
    pub feature_count: u64,
    pub indexing: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TensorDescriptor {
    pub name: String,
    pub role: TensorRole,
    pub dtype: DType,
    pub shape: Vec<u64>,
    pub scale: Rational,
    pub offset: u64,
    pub length: u64,
    pub digest: Digest,
    pub padding: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxiliaryDescriptor {
    pub name: String,
    pub media_type: String,
    pub offset: u64,
    pub length: u64,
    pub digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationDescriptor {
    pub schema_version: u32,
    pub recipe_id: String,
    pub recipe_version: u32,
    pub feature_schema: FeatureSchema,
    pub tensors: Vec<TensorDescriptor>,
    pub integer_evaluation: IntegerEvaluationSpec,
}

impl EvaluationDescriptor {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(Error::InvalidDescriptor(format!(
                "unsupported evaluation descriptor schema version {}",
                self.schema_version
            )));
        }
        if self.recipe_id.is_empty() {
            return Err(Error::InvalidDescriptor("recipe_id is empty".into()));
        }
        if self.feature_schema.id.is_empty()
            || self.feature_schema.blocks.is_empty()
            || self.feature_schema.blocks.iter().any(|block| {
                block.name.is_empty() || block.indexing.is_empty() || block.feature_count == 0
            })
        {
            return Err(Error::InvalidDescriptor(
                "feature schema identity or block is invalid".into(),
            ));
        }
        if self.integer_evaluation.activation.first_clamp.denominator == 0
            || self.integer_evaluation.activation.second_clamp.denominator == 0
            || self.integer_evaluation.output_divisor == 0
            || self.integer_evaluation.engine_scale.denominator == 0
        {
            return Err(Error::InvalidDescriptor("a rational denominator is zero".into()));
        }
        if self.tensors.iter().any(|tensor| tensor.name.is_empty() || tensor.scale.denominator == 0)
        {
            return Err(Error::InvalidDescriptor("tensor identity or scale is invalid".into()));
        }
        Ok(())
    }

    pub fn hash(&self) -> Result<Digest> {
        evaluation_spec_hash(self)
    }

    /// Runtime descriptor for the direct-feature `SFNNv15` contract.
    ///
    /// Payload ranges and digests are filled by the exporter.
    #[must_use]
    pub fn sfnnv15_v2(tensors: Vec<TensorDescriptor>) -> Self {
        Self {
            schema_version: 1,
            recipe_id: schema::RECIPE_ID.into(),
            recipe_version: schema::RECIPE_VERSION,
            feature_schema: FeatureSchema {
                id: schema::FEATURE_SCHEMA_ID.into(),
                version: schema::FEATURE_SCHEMA_VERSION,
                // Each perspective reads its own side's piece list and its own
                // king, so a friendly piece lands in the same plane range for
                // both sides.
                perspective: "friend_enemy_king_relative".into(),
                blocks: vec![
                    FeatureBlock {
                        name: "base".into(),
                        feature_count: u64::from(schema::BASE_INPUTS),
                        // `HalfKA` with the horizontal king mirror that folds
                        // 81 king squares onto 45. `indexing` names the index
                        // formula, not the block; the block is already `base`.
                        indexing: schema::BASE_INDEXING_ID.into(),
                    },
                    FeatureBlock {
                        name: "threat".into(),
                        feature_count: 114_880,
                        indexing: "occupied_target_threat_v1".into(),
                    },
                ],
                bucket_rule: "base_threat_horizontal_king_bucket_v1".into(),
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

    /// Golden logical tensor schema for `sfnnv15_v2`.
    ///
    /// Offsets are densely packed and digests are zero because this descriptor
    /// is an evaluation-contract fixture, not a publishable package manifest.
    #[must_use]
    pub fn sfnnv15_v2_schema() -> Self {
        let mut tensors = logical_tensors([
            (
                "ft_base_weight",
                TensorRole::FeatureTransformer,
                DType::I16,
                vec![u64::from(schema::BASE_INPUTS), 1_024],
            ),
            ("ft_threat_weight", TensorRole::FeatureTransformer, DType::I8, vec![114_880, 1_024]),
            ("ft_bias", TensorRole::FeatureTransformer, DType::I16, vec![1_024]),
            ("psqt_weight", TensorRole::Psqt, DType::I32, vec![u64::from(schema::BASE_INPUTS), 8]),
            ("psqt_threat_weight", TensorRole::Psqt, DType::I32, vec![114_880, 8]),
            ("fc0_weight", TensorRole::HiddenWeight, DType::I8, vec![8, 32, 1_024]),
            ("fc0_bias", TensorRole::HiddenBias, DType::I32, vec![8, 32]),
            ("fc1_weight", TensorRole::HiddenWeight, DType::I8, vec![8, 32, 64]),
            ("fc1_bias", TensorRole::HiddenBias, DType::I32, vec![8, 32]),
            ("fc2_weight", TensorRole::OutputWeight, DType::I8, vec![8, 1, 128]),
            ("fc2_bias", TensorRole::OutputBias, DType::I32, vec![8]),
        ]);
        for tensor in &mut tensors {
            tensor.scale = match tensor.name.as_str() {
                "ft_base_weight" | "ft_threat_weight" | "ft_bias" => {
                    Rational { numerator: 1, denominator: 256 }
                }
                "psqt_weight" | "psqt_threat_weight" => {
                    Rational { numerator: 1, denominator: 9_600 }
                }
                "fc0_weight" | "fc2_weight" => Rational { numerator: 1, denominator: 128 },
                "fc0_bias" | "fc2_bias" => Rational { numerator: 1, denominator: 16_384 },
                "fc1_weight" => Rational { numerator: 1, denominator: 64 },
                "fc1_bias" => Rational { numerator: 1, denominator: 8_192 },
                _ => unreachable!("fixed SFNNv15 tensor name"),
            };
        }
        Self::sfnnv15_v2(tensors)
    }

    /// 512-wide, base-only mobile `SFNNv15` contract.
    #[must_use]
    pub fn sfnnv15_mobile_v1(tensors: Vec<TensorDescriptor>) -> Self {
        let mut descriptor = Self::sfnnv15_v2(tensors);
        descriptor.recipe_id = "sfnnv15_mobile_v1".into();
        descriptor.recipe_version = 1;
        descriptor.feature_schema.blocks.retain(|block| block.name != "threat");
        descriptor.feature_schema.bucket_rule = "base_horizontal_king_bucket_v1".into();
        descriptor
    }

    #[must_use]
    pub fn sfnnv15_mobile_v1_schema() -> Self {
        let mut descriptor = Self::sfnnv15_v2_schema();
        descriptor = Self::sfnnv15_mobile_v1(descriptor.tensors);
        descriptor.tensors.retain(|tensor| {
            tensor.name != "ft_threat_weight" && tensor.name != "psqt_threat_weight"
        });
        let mut offset = 0_u64;
        for tensor in &mut descriptor.tensors {
            match tensor.name.as_str() {
                "ft_base_weight" => tensor.shape[1] = 512,
                "ft_bias" => tensor.shape[0] = 512,
                "fc0_weight" => tensor.shape[2] = 512,
                _ => {}
            }
            tensor.offset = offset;
            tensor.length = tensor.shape.iter().product::<u64>() * tensor.dtype.byte_width();
            offset += tensor.length;
        }
        descriptor
    }
}

fn logical_tensors<const N: usize>(
    definitions: [(&str, TensorRole, DType, Vec<u64>); N],
) -> Vec<TensorDescriptor> {
    let mut offset = 0_u64;
    definitions
        .into_iter()
        .map(|(name, role, dtype, shape)| {
            let length = shape.iter().copied().product::<u64>() * dtype.byte_width();
            let tensor = TensorDescriptor {
                name: name.into(),
                role,
                dtype,
                shape,
                scale: Rational { numerator: 1, denominator: 1 },
                offset,
                length,
                digest: Digest::ZERO,
                padding: 0,
            };
            offset += length;
            tensor
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluatorExpectation {
    pub descriptor: EvaluationDescriptor,
    pub evaluation_spec_hash: Digest,
}

impl EvaluatorExpectation {
    pub fn new(descriptor: EvaluationDescriptor) -> Result<Self> {
        let evaluation_spec_hash = descriptor.hash()?;
        Ok(Self { descriptor, evaluation_spec_hash })
    }

    pub fn verify(&self, actual: &EvaluationDescriptor) -> Result<()> {
        if actual.recipe_id != self.descriptor.recipe_id
            || actual.recipe_version != self.descriptor.recipe_version
        {
            return Err(Error::UnsupportedRecipe(format!(
                "{}@{}",
                actual.recipe_id, actual.recipe_version
            )));
        }
        if actual.feature_schema != self.descriptor.feature_schema {
            return Err(Error::DescriptorMismatch { field: "feature_schema" });
        }
        if actual.integer_evaluation != self.descriptor.integer_evaluation {
            return Err(Error::DescriptorMismatch { field: "integer_evaluation" });
        }
        if actual
            .tensors
            .iter()
            .map(|tensor| (&tensor.name, &tensor.role, &tensor.dtype, &tensor.shape, &tensor.scale))
            .ne(self.descriptor.tensors.iter().map(|tensor| {
                (&tensor.name, &tensor.role, &tensor.dtype, &tensor.shape, &tensor.scale)
            }))
        {
            return Err(Error::DescriptorMismatch { field: "tensor_schema" });
        }
        if actual.hash()? != self.evaluation_spec_hash {
            return Err(Error::DescriptorMismatch { field: "evaluation_spec_hash" });
        }
        Ok(())
    }
}

#[cfg(test)]
mod mobile_tests {
    use super::*;

    #[test]
    fn mobile_schema_matches_trainer_contract() {
        let descriptor = EvaluationDescriptor::sfnnv15_mobile_v1_schema();
        assert_eq!(descriptor.tensors.len(), 9);
        assert_eq!(descriptor.tensors[0].shape, [72_675, 512]);
        assert_eq!(
            descriptor.hash().unwrap().to_hex(),
            "c13f6a8108dc14ce2c4a057d73f6b5fc526516fbc094fa50c1c66a06fc7486fd"
        );
    }
}

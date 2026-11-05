//! Canonical JSON serialization, SHA-256 digests, and domain-separated spec hashing.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use super::descriptor::{DType, EvaluationDescriptor, TensorRole};
use super::reader::{PackageError, Result};

// Protocol identifier; changing this changes every derived evaluation_spec_hash.
const EVALUATION_SPEC_HASH_DOMAIN: &[u8] = b"rsnn/evaluation-spec";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct Digest([u8; 32]);

impl Digest {
    pub(crate) const ZERO: Self = Self([0; 32]);

    #[must_use]
    const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) fn from_hex(value: &str) -> Result<Self> {
        if value.len() != 64 {
            return Err(PackageError::InvalidManifestField(
                "SHA-256 digest must contain 64 hexadecimal characters".into(),
            ));
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            bytes[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub(crate) fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }
}

fn hex_nibble(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(PackageError::InvalidManifestField(
            "SHA-256 digest contains a non-lowercase-hex character".into(),
        )),
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Digest").field(&self.to_hex()).finish()
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for Digest {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(serde::de::Error::custom)
    }
}

#[must_use]
pub(crate) fn sha256(bytes: &[u8]) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Digest::from_bytes(hasher.finalize().into())
}

pub(super) fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json_canonicalizer::to_vec(value)
        .map_err(|error| PackageError::InvalidManifest(error.to_string()))
}

struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn new(domain: &[u8], version: u32) -> Self {
        let mut encoder = Self { bytes: Vec::new() };
        encoder.bytes(domain);
        encoder.u32(version);
        encoder
    }

    fn bytes(&mut self, value: &[u8]) {
        self.u64(value.len() as u64);
        self.bytes.extend_from_slice(value);
    }

    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn finish(self) -> Digest {
        sha256(&self.bytes)
    }
}

fn encode_rational(encoder: &mut Encoder, numerator: i64, denominator: u64) {
    encoder.i64(numerator);
    encoder.u64(denominator);
}

const fn dtype_tag(dtype: &DType) -> u8 {
    match dtype {
        DType::I8 => 1,
        DType::I16 => 2,
        DType::I32 => 3,
        DType::I64 => 4,
        DType::U8 => 5,
    }
}

const fn role_tag(role: &TensorRole) -> u8 {
    match role {
        TensorRole::FeatureTransformer => 1,
        TensorRole::HiddenWeight => 2,
        TensorRole::HiddenBias => 3,
        TensorRole::OutputWeight => 4,
        TensorRole::OutputBias => 5,
        TensorRole::Psqt => 6,
    }
}

pub(super) fn evaluation_spec_hash(descriptor: &EvaluationDescriptor) -> Result<Digest> {
    descriptor.validate()?;
    let mut encoder = Encoder::new(EVALUATION_SPEC_HASH_DOMAIN, descriptor.schema_version);
    encoder.string(&descriptor.recipe_id);
    encoder.u32(descriptor.recipe_version);
    encoder.string(&descriptor.feature_schema.id);
    encoder.u32(descriptor.feature_schema.version);
    encoder.string(&descriptor.feature_schema.perspective);
    encoder.u64(descriptor.feature_schema.blocks.len() as u64);
    for block in &descriptor.feature_schema.blocks {
        encoder.string(&block.name);
        encoder.u64(block.feature_count);
        encoder.string(&block.indexing);
    }
    encoder.string(&descriptor.feature_schema.bucket_rule);
    encoder.u64(descriptor.tensors.len() as u64);
    for tensor in &descriptor.tensors {
        encoder.string(&tensor.name);
        encoder.u8(role_tag(&tensor.role));
        encoder.u8(dtype_tag(&tensor.dtype));
        encoder.u64(tensor.shape.len() as u64);
        for dimension in &tensor.shape {
            encoder.u64(*dimension);
        }
        encode_rational(&mut encoder, tensor.scale.numerator, tensor.scale.denominator);
    }
    let evaluation = &descriptor.integer_evaluation;
    encoder.u16(evaluation.accumulator_bits);
    encoder.u16(evaluation.intermediate_bits);
    encoder.string(&evaluation.activation.feature_transform);
    encode_rational(
        &mut encoder,
        evaluation.activation.first_clamp.numerator,
        evaluation.activation.first_clamp.denominator,
    );
    encoder.u32(evaluation.activation.first_shift);
    encode_rational(
        &mut encoder,
        evaluation.activation.second_clamp.numerator,
        evaluation.activation.second_clamp.denominator,
    );
    encoder.u32(evaluation.activation.second_shift);
    encoder.u32(evaluation.positive_skip_channel);
    encoder.u32(evaluation.negative_skip_channel);
    encoder.i64(evaluation.output_multiplier);
    encoder.u64(evaluation.output_divisor);
    encoder.string(&evaluation.division);
    encoder.string(&evaluation.psqt_addition);
    encoder.string(&evaluation.output_units);
    encode_rational(
        &mut encoder,
        evaluation.engine_scale.numerator,
        evaluation.engine_scale.denominator,
    );
    Ok(encoder.finish())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn digest_requires_canonical_lowercase_hex() {
        assert_eq!(Digest::from_hex(&"00".repeat(32)), Ok(Digest::ZERO));
        assert!(Digest::from_hex(&"AA".repeat(32)).is_err());
        assert!(Digest::from_hex("00").is_err());
    }

    #[test]
    fn canonical_json_sorts_keys_and_rejects_non_finite_numbers() {
        let entries = [("zeta", "last"), ("alpha", "first"), ("日本語", "value")];
        let forward: HashMap<_, _> = entries.into_iter().collect();
        let reverse: HashMap<_, _> = entries.into_iter().rev().collect();
        assert_eq!(canonical_json(&forward).unwrap(), canonical_json(&reverse).unwrap());
        assert!(canonical_json(&f32::NAN).is_err());
        assert!(canonical_json(&f64::INFINITY).is_err());
    }
}

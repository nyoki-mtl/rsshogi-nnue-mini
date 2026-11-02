//! Canonical, host-only reader and writer for portable `.rsnn` packages.
//!
//! This crate owns container version 1, manifest serialization, package
//! validation, and evaluation/export specification hashing. It intentionally
//! has no dependency on the consumer runtime, trainer, or CUDA.

pub mod aonly_feature_schema;
mod canonical;
mod descriptor;
mod error;
mod hash;
mod numeric_range;
mod package;
pub mod sfnnv15_feature_schema;
mod verifier;

pub use descriptor::{
    ActivationSpec, AuxiliaryDescriptor, DType, EvaluationDescriptor, EvaluatorExpectation,
    ExportSpec, FeatureBlock, FeatureSchema, IntegerEvaluationSpec, PaddingPolicy, Rational,
    SFNNV15_THREAT_SCHEMA_SHA256, TensorDescriptor, TensorRole, verify_sfnnv15_threat_schema,
};
pub use error::{Error, Result};
pub use hash::{Digest, evaluation_spec_hash, export_spec_hash, sha256};
pub use numeric_range::Sfnnv15NumericRangeReport;
pub use package::{
    AuxiliaryPayload, CONTAINER_VERSION, HEADER_LEN, MAGIC, Manifest, Package, PackageLimits,
    PackageReader, PackageWriter, Provenance, ReadPackage, TensorPayload,
};
pub use verifier::{VERIFIER_PROTOCOL_VERSION, VerifierReport};

/// Serialize a typed contract with the shared canonical JSON encoder.
pub fn canonical_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    canonical::to_vec(value)
}

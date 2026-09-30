//! Canonical, host-only reader for portable `.rsnn` packages.
//!
//! This crate owns container version 1, manifest parsing, package validation,
//! and evaluation specification hashing. It intentionally has no dependency on
//! the consumer runtime, trainer, or CUDA.

mod canonical;
mod descriptor;
mod error;
mod hash;
mod numeric_range;
mod package;
pub mod sfnnv15_feature_schema;

pub use descriptor::{
    ActivationSpec, AuxiliaryDescriptor, DType, EvaluationDescriptor, EvaluatorExpectation,
    FeatureBlock, FeatureSchema, IntegerEvaluationSpec, Rational, TensorDescriptor, TensorRole,
};
pub use error::{Error, Result};
pub use hash::{Digest, evaluation_spec_hash, sha256};
pub use numeric_range::Sfnnv15NumericRangeReport;
pub use package::{
    AuxiliaryPayload, CONTAINER_VERSION, HEADER_LEN, MAGIC, Manifest, PackageLimits, PackageReader,
    Provenance, ReadPackage, TensorPayload,
};

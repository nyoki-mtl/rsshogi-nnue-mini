//! `.rsnn` container validation before the A-only inference layer is materialized.

use std::path::Path;

use rsnn_package::{Digest, EvaluationDescriptor, PackageReader, ReadPackage};

/// Validated portable package whose descriptor matches the A-only recipe.
#[derive(Debug)]
pub struct RsnnPackage {
    pub(super) contents: ReadPackage,
}

impl RsnnPackage {
    pub fn load(path: &Path, expected_digest: Option<Digest>) -> Result<Self, String> {
        let contents = PackageReader::default()
            .read_file(path, expected_digest)
            .map_err(|error| error.to_string())?;
        Self::from_read_package(contents)
    }

    pub fn load_bytes(bytes: &[u8], expected_digest: Digest) -> Result<Self, String> {
        let contents = PackageReader::default()
            .read(bytes, Some(expected_digest))
            .map_err(|error| error.to_string())?;
        Self::from_read_package(contents)
    }

    fn from_read_package(contents: ReadPackage) -> Result<Self, String> {
        let expected = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
        let actual = &contents.manifest.evaluation_descriptor;
        if actual.schema_version != expected.schema_version
            || actual.recipe_id != expected.recipe_id
            || actual.recipe_version != expected.recipe_version
            || actual.feature_schema != expected.feature_schema
            || actual.integer_evaluation != expected.integer_evaluation
        {
            return Err("unsupported .rsnn evaluation recipe".to_owned());
        }
        if actual.tensors.len() != expected.tensors.len()
            || actual.tensors.iter().zip(&expected.tensors).any(|(got, want)| {
                got.name != want.name
                    || got.role != want.role
                    || got.dtype != want.dtype
                    || got.shape != want.shape
                    || got.scale != want.scale
            })
        {
            return Err("unsupported .rsnn tensor schema".to_owned());
        }
        if contents.numeric_range_report.is_none() {
            return Err("missing .rsnn numeric range verification".to_owned());
        }
        Ok(Self { contents })
    }

    pub fn digest(&self) -> Digest {
        self.contents.package_digest
    }
}

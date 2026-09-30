//! `.rsnn` container validation before the SFNNv15 inference layer is materialized.

use std::path::Path;

use rsnn_package::{Digest, EvaluationDescriptor, PackageReader, ReadPackage};

/// Validated portable package whose descriptor matches the SFNNv15 recipe.
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

    #[cfg(feature = "embedded-rsnn")]
    pub fn load_bytes(bytes: &[u8], expected_digest: Digest) -> Result<Self, String> {
        let contents = PackageReader::default()
            .read(bytes, Some(expected_digest))
            .map_err(|error| error.to_string())?;
        Self::from_read_package(contents)
    }

    fn from_read_package(contents: ReadPackage) -> Result<Self, String> {
        let expected = EvaluationDescriptor::sfnnv15_mobile_v1_schema();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires RSSHOGI_RSNN_FIXTURE"]
    fn trained_package_matches_the_supported_recipe() {
        let path = std::env::var_os("RSSHOGI_RSNN_FIXTURE")
            .expect("set RSSHOGI_RSNN_FIXTURE to a qualified .rsnn package");
        let expected_digest =
            Digest::from_hex("9e3a3f8f74088a8c430ddd06617ff6cc8e35a979623c2082e1fb1d34befc5a36")
                .expect("literal digest");
        let package = RsnnPackage::load(Path::new(&path), Some(expected_digest))
            .expect("qualified SFNNv15 package loads");
        assert_eq!(package.digest(), expected_digest);
        let mut unsupported = package.contents;
        unsupported.manifest.evaluation_descriptor.recipe_version += 1;
        assert!(RsnnPackage::from_read_package(unsupported).is_err());
    }
}

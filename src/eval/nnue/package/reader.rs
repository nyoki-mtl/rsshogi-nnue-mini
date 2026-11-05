//! Portable `.rsnn` v1 container reader and manifest validator.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::descriptor::{AuxiliaryDescriptor, EvaluationDescriptor, TensorDescriptor};
use super::hash::{Digest, canonical_json, sha256};
use super::numeric_range::certify_numeric_range;

const MAGIC: [u8; 8] = *b"RSNNPKG\0";
const CONTAINER_VERSION: u16 = 1;
const HEADER_LEN: usize = 28;
const MANIFEST_SCHEMA_VERSION: u32 = 1;

pub(crate) type Result<T> = std::result::Result<T, PackageError>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PackageError {
    TruncatedHeader,
    InvalidMagic,
    UnsupportedContainerVersion(u16),
    NonZeroFlags(u16),
    ManifestTooLarge {
        found: u64,
        limit: u64,
    },
    PayloadTooLarge {
        found: u64,
        limit: u64,
    },
    LengthOverflow,
    TruncatedPackage {
        declared: u64,
        found: u64,
    },
    TrailingData {
        declared: u64,
        found: u64,
    },
    InvalidManifest(String),
    NonCanonicalManifest,
    UnsupportedManifestVersion(u32),
    UnknownRequiredFeature(String),
    TooManyEntries {
        found: usize,
        limit: usize,
    },
    DuplicateEntryName(String),
    MissingEntry(String),
    InvalidEntryRange {
        name: String,
    },
    OverlappingEntries {
        first: String,
        second: String,
    },
    TensorShapeLengthMismatch {
        name: String,
    },
    NonZeroPadding {
        name: String,
    },
    PayloadDigestMismatch,
    EntryDigestMismatch(String),
    EvaluationSpecHashMismatch,
    PackageDigestMismatch,
    UnsupportedRecipe(String),
    DescriptorMismatch {
        field: &'static str,
    },
    InvalidDescriptor(String),
    InvalidManifestField(String),
    NumericRangeViolation {
        context: String,
        minimum: i128,
        maximum: i128,
        allowed_minimum: i128,
        allowed_maximum: i128,
    },
    Io(String),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader => write!(f, "package is shorter than the fixed header"),
            Self::InvalidMagic => write!(f, "package magic is invalid"),
            Self::UnsupportedContainerVersion(v) => write!(f, "unsupported container version {v}"),
            Self::NonZeroFlags(flags) => {
                write!(f, "container flags must be zero, found {flags:#06x}")
            }
            Self::ManifestTooLarge { found, limit } => {
                write!(f, "manifest length {found} exceeds limit {limit}")
            }
            Self::PayloadTooLarge { found, limit } => {
                write!(f, "payload length {found} exceeds limit {limit}")
            }
            Self::LengthOverflow => write!(f, "declared package length overflows"),
            Self::TruncatedPackage { declared, found } => {
                write!(f, "package is truncated: declared {declared} bytes, found {found}")
            }
            Self::TrailingData { declared, found } => {
                write!(f, "package has trailing data: declared {declared} bytes, found {found}")
            }
            Self::InvalidManifest(msg) => write!(f, "manifest is not valid UTF-8 JSON: {msg}"),
            Self::NonCanonicalManifest => write!(f, "manifest is not in canonical JSON form"),
            Self::UnsupportedManifestVersion(v) => {
                write!(f, "manifest schema version {v} is unsupported")
            }
            Self::UnknownRequiredFeature(feat) => write!(f, "unknown required feature {feat:?}"),
            Self::TooManyEntries { found, limit } => {
                write!(f, "manifest has {found} entries, exceeding limit {limit}")
            }
            Self::DuplicateEntryName(name) => write!(f, "duplicate payload entry name {name:?}"),
            Self::MissingEntry(name) => write!(f, "required payload entry {name:?} is missing"),
            Self::InvalidEntryRange { name } => {
                write!(f, "payload entry {name:?} has an invalid range")
            }
            Self::OverlappingEntries { first, second } => {
                write!(f, "payload entries {first:?} and {second:?} overlap")
            }
            Self::TensorShapeLengthMismatch { name } => {
                write!(f, "tensor {name:?} shape byte length is inconsistent with payload length")
            }
            Self::NonZeroPadding { name } => {
                write!(f, "tensor {name:?} padding is not zero-filled")
            }
            Self::PayloadDigestMismatch => write!(f, "payload digest does not match manifest"),
            Self::EntryDigestMismatch(name) => {
                write!(f, "entry digest does not match for {name:?}")
            }
            Self::EvaluationSpecHashMismatch => {
                write!(f, "evaluation descriptor hash does not match manifest")
            }
            Self::PackageDigestMismatch => {
                write!(f, "package digest does not match caller expectation")
            }
            Self::UnsupportedRecipe(recipe) => {
                write!(f, "descriptor recipe {recipe:?} is not supported by this evaluator")
            }
            Self::DescriptorMismatch { field } => {
                write!(f, "descriptor field {field:?} does not match compile-in evaluator")
            }
            Self::InvalidDescriptor(msg) => write!(f, "descriptor is invalid: {msg}"),
            Self::InvalidManifestField(msg) => write!(f, "manifest is invalid: {msg}"),
            Self::NumericRangeViolation {
                context,
                minimum,
                maximum,
                allowed_minimum,
                allowed_maximum,
            } => write!(
                f,
                "numeric range certificate failed for {context}: [{minimum}, {maximum}] is outside [{allowed_minimum}, {allowed_maximum}]"
            ),
            Self::Io(msg) => write!(f, "I/O failed: {msg}"),
        }
    }
}

impl std::error::Error for PackageError {}

impl From<std::io::Error> for PackageError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackageLimits {
    max_manifest_bytes: u64,
    max_payload_bytes: u64,
    max_entries: usize,
}

impl Default for PackageLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 16 * 1024 * 1024,
            max_payload_bytes: 16 * 1024 * 1024 * 1024,
            max_entries: 4_096,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Provenance {
    pub(crate) training_spec_hash: Digest,
    pub(crate) export_spec_hash: Digest,
    pub(crate) source_plan_hash: Digest,
    pub(crate) source_checkpoint_digest: Digest,
    pub(crate) source_dataset_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) schema_version: u32,
    pub(crate) required_features: Vec<String>,
    pub(crate) evaluation_descriptor: EvaluationDescriptor,
    pub(crate) evaluation_spec_hash: Digest,
    pub(crate) provenance: Provenance,
    pub(crate) tensors: Vec<TensorDescriptor>,
    pub(crate) auxiliary_artifacts: Vec<AuxiliaryDescriptor>,
    pub(crate) payload_digest: Digest,
}

impl Manifest {
    #[cfg(test)]
    pub(crate) fn new(
        evaluation_descriptor: EvaluationDescriptor,
        provenance: Provenance,
        tensors: Vec<TensorDescriptor>,
        auxiliary_artifacts: Vec<AuxiliaryDescriptor>,
        payload_digest: Digest,
    ) -> Result<Self> {
        let evaluation_spec_hash = evaluation_descriptor.hash()?;
        Ok(Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            required_features: Vec::new(),
            evaluation_descriptor,
            evaluation_spec_hash,
            provenance,
            tensors,
            auxiliary_artifacts,
            payload_digest,
        })
    }

    fn canonical_json(&self) -> Result<Vec<u8>> {
        canonical_json(self)
    }
}

#[derive(Debug)]
pub(crate) struct ReadPackage {
    pub(crate) manifest: Manifest,
    payload: Vec<u8>,
    pub(crate) package_digest: Digest,
}

impl ReadPackage {
    pub(crate) fn tensor(&self, name: &str) -> Result<&[u8]> {
        let descriptor = self
            .manifest
            .tensors
            .iter()
            .find(|tensor| tensor.name == name)
            .ok_or_else(|| PackageError::MissingEntry(name.into()))?;
        let (start, _) =
            entry_range(name, descriptor.offset, descriptor.length, self.payload.len())?;
        let data_len = tensor_data_length(descriptor)?;
        let data_end = start.checked_add(data_len).ok_or(PackageError::LengthOverflow)?;
        Ok(&self.payload[start..data_end])
    }

    pub(crate) fn auxiliary(&self, name: &str) -> Result<&[u8]> {
        let descriptor = self
            .manifest
            .auxiliary_artifacts
            .iter()
            .find(|artifact| artifact.name == name)
            .ok_or_else(|| PackageError::MissingEntry(name.into()))?;
        let (start, end) =
            entry_range(name, descriptor.offset, descriptor.length, self.payload.len())?;
        Ok(&self.payload[start..end])
    }
}

#[cfg(test)]
pub(crate) fn encode_package_for_test(manifest: &Manifest, payload: &[u8]) -> Result<Vec<u8>> {
    let manifest_bytes = manifest.canonical_json()?;
    let mut bytes = Vec::with_capacity(HEADER_LEN + manifest_bytes.len() + payload.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&(manifest_bytes.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&manifest_bytes);
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

pub(crate) fn read_package_file(path: &Path) -> Result<ReadPackage> {
    let limits = PackageLimits::default();
    let metadata = fs::metadata(path)?;
    let maximum = u64::try_from(HEADER_LEN)
        .ok()
        .and_then(|header| header.checked_add(limits.max_manifest_bytes))
        .and_then(|value| value.checked_add(limits.max_payload_bytes))
        .ok_or(PackageError::LengthOverflow)?;
    if metadata.len() > maximum {
        return Err(PackageError::PayloadTooLarge { found: metadata.len(), limit: maximum });
    }
    let bytes = fs::read(path)?;
    read_package_bytes(&bytes, None)
}

pub(crate) fn read_package_bytes(
    bytes: &[u8],
    expected_package_digest: Option<Digest>,
) -> Result<ReadPackage> {
    let limits = PackageLimits::default();
    let manifest_len = parse_header(bytes, limits)?;
    let package_digest = sha256(bytes);
    if expected_package_digest.is_some_and(|expected| expected != package_digest) {
        return Err(PackageError::PackageDigestMismatch);
    }
    let manifest_end = HEADER_LEN.checked_add(manifest_len).ok_or(PackageError::LengthOverflow)?;
    let manifest_bytes = &bytes[HEADER_LEN..manifest_end];
    let manifest: Manifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| PackageError::InvalidManifest(error.to_string()))?;
    if manifest.canonical_json()? != manifest_bytes {
        return Err(PackageError::NonCanonicalManifest);
    }
    let payload = &bytes[manifest_end..];
    validate_manifest(&manifest, payload, limits)?;
    certify_numeric_range(&manifest, payload)?;
    Ok(ReadPackage { manifest, payload: payload.to_vec(), package_digest })
}

fn parse_header(bytes: &[u8], limits: PackageLimits) -> Result<usize> {
    if bytes.len() < HEADER_LEN {
        return Err(PackageError::TruncatedHeader);
    }
    if bytes[..8] != MAGIC {
        return Err(PackageError::InvalidMagic);
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != CONTAINER_VERSION {
        return Err(PackageError::UnsupportedContainerVersion(version));
    }
    let flags = u16::from_le_bytes([bytes[10], bytes[11]]);
    if flags != 0 {
        return Err(PackageError::NonZeroFlags(flags));
    }
    let manifest_len = u64::from_le_bytes(bytes[12..20].try_into().expect("fixed header slice"));
    let payload_len = u64::from_le_bytes(bytes[20..28].try_into().expect("fixed header slice"));
    if manifest_len > limits.max_manifest_bytes {
        return Err(PackageError::ManifestTooLarge {
            found: manifest_len,
            limit: limits.max_manifest_bytes,
        });
    }
    if payload_len > limits.max_payload_bytes {
        return Err(PackageError::PayloadTooLarge {
            found: payload_len,
            limit: limits.max_payload_bytes,
        });
    }
    let declared = u64::try_from(HEADER_LEN)
        .ok()
        .and_then(|header| header.checked_add(manifest_len))
        .and_then(|value| value.checked_add(payload_len))
        .ok_or(PackageError::LengthOverflow)?;
    let found = u64::try_from(bytes.len()).map_err(|_| PackageError::LengthOverflow)?;
    if found < declared {
        return Err(PackageError::TruncatedPackage { declared, found });
    }
    if found > declared {
        return Err(PackageError::TrailingData { declared, found });
    }
    usize::try_from(manifest_len).map_err(|_| PackageError::LengthOverflow)
}

fn validate_manifest(manifest: &Manifest, payload: &[u8], limits: PackageLimits) -> Result<()> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(PackageError::UnsupportedManifestVersion(manifest.schema_version));
    }
    if let Some(feature) = manifest.required_features.first() {
        return Err(PackageError::UnknownRequiredFeature(feature.clone()));
    }
    let entry_count = manifest
        .tensors
        .len()
        .checked_add(manifest.auxiliary_artifacts.len())
        .ok_or(PackageError::LengthOverflow)?;
    if entry_count > limits.max_entries {
        return Err(PackageError::TooManyEntries { found: entry_count, limit: limits.max_entries });
    }
    manifest.evaluation_descriptor.validate()?;
    if manifest.evaluation_descriptor.hash()? != manifest.evaluation_spec_hash {
        return Err(PackageError::EvaluationSpecHashMismatch);
    }
    if manifest.evaluation_descriptor.tensors != manifest.tensors {
        return Err(PackageError::InvalidManifestField(
            "evaluation descriptor tensor table differs from manifest tensor table".into(),
        ));
    }
    if sha256(payload) != manifest.payload_digest {
        return Err(PackageError::PayloadDigestMismatch);
    }

    let mut names = HashSet::with_capacity(entry_count);
    let mut ranges = Vec::with_capacity(entry_count);
    for tensor in &manifest.tensors {
        validate_name(&mut names, &tensor.name)?;
        let range = entry_range(&tensor.name, tensor.offset, tensor.length, payload.len())?;
        let data_length =
            u64::try_from(tensor_data_length(tensor)?).map_err(|_| PackageError::LengthOverflow)?;
        let expected_length =
            data_length.checked_add(tensor.padding).ok_or(PackageError::LengthOverflow)?;
        if expected_length != tensor.length {
            return Err(PackageError::TensorShapeLengthMismatch { name: tensor.name.clone() });
        }
        let data_end = usize::try_from(
            tensor.offset.checked_add(data_length).ok_or(PackageError::LengthOverflow)?,
        )
        .map_err(|_| PackageError::LengthOverflow)?;
        if payload[data_end..range.1].iter().any(|byte| *byte != 0) {
            return Err(PackageError::NonZeroPadding { name: tensor.name.clone() });
        }
        if sha256(&payload[range.0..range.1]) != tensor.digest {
            return Err(PackageError::EntryDigestMismatch(tensor.name.clone()));
        }
        ranges.push((tensor.name.as_str(), range));
    }
    for auxiliary in &manifest.auxiliary_artifacts {
        validate_name(&mut names, &auxiliary.name)?;
        let range =
            entry_range(&auxiliary.name, auxiliary.offset, auxiliary.length, payload.len())?;
        if sha256(&payload[range.0..range.1]) != auxiliary.digest {
            return Err(PackageError::EntryDigestMismatch(auxiliary.name.clone()));
        }
        ranges.push((auxiliary.name.as_str(), range));
    }
    ranges.sort_unstable_by_key(|(_, range)| range.0);
    for pair in ranges.windows(2) {
        if pair[0].1.1 > pair[1].1.0 {
            return Err(PackageError::OverlappingEntries {
                first: pair[0].0.into(),
                second: pair[1].0.into(),
            });
        }
    }
    Ok(())
}

fn tensor_data_length(tensor: &TensorDescriptor) -> Result<usize> {
    let element_count = tensor.shape.iter().try_fold(1_u64, |product, dimension| {
        product.checked_mul(*dimension).ok_or(PackageError::LengthOverflow)
    })?;
    let bytes =
        element_count.checked_mul(tensor.dtype.byte_width()).ok_or(PackageError::LengthOverflow)?;
    usize::try_from(bytes).map_err(|_| PackageError::LengthOverflow)
}

fn validate_name(names: &mut HashSet<String>, name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(PackageError::InvalidManifestField("payload entry name is empty".into()));
    }
    if !names.insert(name.to_owned()) {
        return Err(PackageError::DuplicateEntryName(name.into()));
    }
    Ok(())
}

fn entry_range(name: &str, offset: u64, length: u64, payload_len: usize) -> Result<(usize, usize)> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| PackageError::InvalidEntryRange { name: name.into() })?;
    let start = usize::try_from(offset)
        .map_err(|_| PackageError::InvalidEntryRange { name: name.into() })?;
    let end =
        usize::try_from(end).map_err(|_| PackageError::InvalidEntryRange { name: name.into() })?;
    if end > payload_len {
        return Err(PackageError::InvalidEntryRange { name: name.into() });
    }
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_aonly_manifest_and_payload() -> (Manifest, Vec<u8>) {
        let mut descriptor = EvaluationDescriptor::aonly1024_i8_progress8_v1_schema();
        let mut payload = Vec::new();
        for tensor in &mut descriptor.tensors {
            let data = vec![0_u8; tensor.length as usize];
            tensor.digest = sha256(&data);
            payload.extend(data);
        }
        let manifest = Manifest::new(
            descriptor.clone(),
            Provenance {
                training_spec_hash: Digest::ZERO,
                export_spec_hash: Digest::ZERO,
                source_plan_hash: Digest::ZERO,
                source_checkpoint_digest: Digest::ZERO,
                source_dataset_identity: "synthetic A-only test".into(),
            },
            descriptor.tensors,
            Vec::new(),
            sha256(&payload),
        )
        .unwrap();
        (manifest, payload)
    }

    #[test]
    fn container_header_manifest_and_digests_are_enforced() {
        let (manifest, payload) = zero_aonly_manifest_and_payload();
        let bytes = encode_package_for_test(&manifest, &payload).unwrap();
        let digest = sha256(&bytes);
        let read = read_package_bytes(&bytes, Some(digest)).unwrap();
        assert_eq!(read.package_digest, digest);
        assert_eq!(read.tensor("missing"), Err(PackageError::MissingEntry("missing".into())));
        assert_eq!(
            read_package_bytes(&bytes, Some(Digest::ZERO)).unwrap_err(),
            PackageError::PackageDigestMismatch
        );

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(matches!(
            read_package_bytes(&trailing, None),
            Err(PackageError::TrailingData { .. })
        ));
        assert_eq!(
            read_package_bytes(&bytes[..10], None).unwrap_err(),
            PackageError::TruncatedHeader
        );

        let mut corrupt = bytes;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert_eq!(
            read_package_bytes(&corrupt, None).unwrap_err(),
            PackageError::PayloadDigestMismatch
        );

        let mut bad_hash = manifest;
        bad_hash.evaluation_spec_hash = Digest::ZERO;
        let bad_bytes = encode_package_for_test(&bad_hash, &payload).unwrap();
        assert_eq!(
            read_package_bytes(&bad_bytes, None).unwrap_err(),
            PackageError::EvaluationSpecHashMismatch
        );
    }
}

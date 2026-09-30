use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::canonical;
use crate::numeric_range::{Sfnnv15NumericRangeVerification, verify_sfnnv15_numeric_range};
use crate::{
    AuxiliaryDescriptor, Digest, Error, EvaluationDescriptor, Result, Sfnnv15NumericRangeReport,
    TensorDescriptor, sha256,
};

pub const MAGIC: [u8; 8] = *b"RSNNPKG\0";
pub const CONTAINER_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 28;
const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackageLimits {
    pub max_manifest_bytes: u64,
    pub max_payload_bytes: u64,
    pub max_entries: usize,
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
pub struct Provenance {
    pub training_spec_hash: Digest,
    pub export_spec_hash: Digest,
    pub source_plan_hash: Digest,
    pub source_checkpoint_digest: Digest,
    pub source_dataset_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub required_features: Vec<String>,
    pub evaluation_descriptor: EvaluationDescriptor,
    pub evaluation_spec_hash: Digest,
    pub provenance: Provenance,
    pub tensors: Vec<TensorDescriptor>,
    pub auxiliary_artifacts: Vec<AuxiliaryDescriptor>,
    pub payload_digest: Digest,
}

impl Manifest {
    pub fn new(
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

    pub fn canonical_json(&self) -> Result<Vec<u8>> {
        canonical::to_vec(self)
    }
}

/// A parsed and validated `.rsnn` container with its digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadPackage {
    pub manifest: Manifest,
    pub payload: Vec<u8>,
    pub package_digest: Digest,
    pub numeric_range_report: Option<Sfnnv15NumericRangeReport>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TensorPayload<'a> {
    pub descriptor: &'a TensorDescriptor,
    pub data: &'a [u8],
    pub padding: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuxiliaryPayload<'a> {
    pub descriptor: &'a AuxiliaryDescriptor,
    pub data: &'a [u8],
}

impl ReadPackage {
    /// Resolve a tensor by semantic name and return logical bytes separately
    /// from its validated zero-filled padding.
    pub fn tensor(&self, name: &str) -> Result<TensorPayload<'_>> {
        let descriptor = self
            .manifest
            .tensors
            .iter()
            .find(|tensor| tensor.name == name)
            .ok_or_else(|| Error::MissingEntry(name.into()))?;
        let (start, end) =
            entry_range(name, descriptor.offset, descriptor.length, self.payload.len())?;
        let data_len = tensor_data_length(descriptor)?;
        let data_end = start.checked_add(data_len).ok_or(Error::LengthOverflow)?;
        Ok(TensorPayload {
            descriptor,
            data: &self.payload[start..data_end],
            padding: &self.payload[data_end..end],
        })
    }

    /// Resolve an auxiliary artifact by semantic name.
    pub fn auxiliary(&self, name: &str) -> Result<AuxiliaryPayload<'_>> {
        let descriptor = self
            .manifest
            .auxiliary_artifacts
            .iter()
            .find(|artifact| artifact.name == name)
            .ok_or_else(|| Error::MissingEntry(name.into()))?;
        let (start, end) =
            entry_range(name, descriptor.offset, descriptor.length, self.payload.len())?;
        Ok(AuxiliaryPayload { descriptor, data: &self.payload[start..end] })
    }
}

pub struct PackageReader {
    limits: PackageLimits,
}

impl Default for PackageReader {
    fn default() -> Self {
        Self::new(PackageLimits::default())
    }
}

impl PackageReader {
    #[must_use]
    pub const fn new(limits: PackageLimits) -> Self {
        Self { limits }
    }

    pub fn read_file(
        &self,
        path: impl AsRef<Path>,
        expected_package_digest: Option<Digest>,
    ) -> Result<ReadPackage> {
        let metadata = fs::metadata(&path)?;
        let maximum = u64::try_from(HEADER_LEN)
            .ok()
            .and_then(|header| header.checked_add(self.limits.max_manifest_bytes))
            .and_then(|value| value.checked_add(self.limits.max_payload_bytes))
            .ok_or(Error::LengthOverflow)?;
        if metadata.len() > maximum {
            return Err(Error::PayloadTooLarge { found: metadata.len(), limit: maximum });
        }
        let bytes = fs::read(path)?;
        self.read(&bytes, expected_package_digest)
    }

    pub fn read(
        &self,
        bytes: &[u8],
        expected_package_digest: Option<Digest>,
    ) -> Result<ReadPackage> {
        let header = parse_header(bytes, self.limits)?;
        let package_digest = sha256(bytes);
        if expected_package_digest.is_some_and(|expected| expected != package_digest) {
            return Err(Error::PackageDigestMismatch);
        }
        let manifest_end =
            HEADER_LEN.checked_add(header.manifest_len).ok_or(Error::LengthOverflow)?;
        let manifest_bytes = &bytes[HEADER_LEN..manifest_end];
        let manifest: Manifest = serde_json::from_slice(manifest_bytes)
            .map_err(|error| Error::InvalidManifest(error.to_string()))?;
        if manifest.canonical_json()? != manifest_bytes {
            return Err(Error::NonCanonicalManifest);
        }
        let payload = &bytes[manifest_end..];
        let numeric_range_report = validate_manifest(&manifest, payload, self.limits)?;
        Ok(ReadPackage {
            manifest,
            payload: payload.to_vec(),
            package_digest,
            numeric_range_report,
        })
    }
}

struct Header {
    manifest_len: usize,
}

fn parse_header(bytes: &[u8], limits: PackageLimits) -> Result<Header> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::TruncatedHeader);
    }
    if bytes[..8] != MAGIC {
        return Err(Error::InvalidMagic);
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != CONTAINER_VERSION {
        return Err(Error::UnsupportedContainerVersion(version));
    }
    let flags = u16::from_le_bytes([bytes[10], bytes[11]]);
    if flags != 0 {
        return Err(Error::NonZeroFlags(flags));
    }
    let manifest_len = u64::from_le_bytes(bytes[12..20].try_into().expect("fixed header slice"));
    let payload_len = u64::from_le_bytes(bytes[20..28].try_into().expect("fixed header slice"));
    if manifest_len > limits.max_manifest_bytes {
        return Err(Error::ManifestTooLarge {
            found: manifest_len,
            limit: limits.max_manifest_bytes,
        });
    }
    if payload_len > limits.max_payload_bytes {
        return Err(Error::PayloadTooLarge { found: payload_len, limit: limits.max_payload_bytes });
    }
    let declared = u64::try_from(HEADER_LEN)
        .ok()
        .and_then(|header| header.checked_add(manifest_len))
        .and_then(|value| value.checked_add(payload_len))
        .ok_or(Error::LengthOverflow)?;
    let found = u64::try_from(bytes.len()).map_err(|_| Error::LengthOverflow)?;
    if found < declared {
        return Err(Error::TruncatedPackage { declared, found });
    }
    if found > declared {
        return Err(Error::TrailingData { declared, found });
    }
    Ok(Header { manifest_len: usize::try_from(manifest_len).map_err(|_| Error::LengthOverflow)? })
}

fn validate_manifest(
    manifest: &Manifest,
    payload: &[u8],
    limits: PackageLimits,
) -> Result<Option<Sfnnv15NumericRangeReport>> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(Error::UnsupportedManifestVersion(manifest.schema_version));
    }
    if let Some(feature) = manifest.required_features.first() {
        return Err(Error::UnknownRequiredFeature(feature.clone()));
    }
    let entry_count = manifest
        .tensors
        .len()
        .checked_add(manifest.auxiliary_artifacts.len())
        .ok_or(Error::LengthOverflow)?;
    if entry_count > limits.max_entries {
        return Err(Error::TooManyEntries { found: entry_count, limit: limits.max_entries });
    }
    manifest.evaluation_descriptor.validate()?;
    if manifest.evaluation_descriptor.hash()? != manifest.evaluation_spec_hash {
        return Err(Error::EvaluationSpecHashMismatch);
    }
    if manifest.evaluation_descriptor.tensors != manifest.tensors {
        return Err(Error::InvalidManifestField(
            "evaluation descriptor tensor table differs from manifest tensor table".into(),
        ));
    }
    if sha256(payload) != manifest.payload_digest {
        return Err(Error::PayloadDigestMismatch);
    }

    let mut names = HashSet::with_capacity(entry_count);
    let mut ranges = Vec::with_capacity(entry_count);
    for tensor in &manifest.tensors {
        validate_name(&mut names, &tensor.name)?;
        let range = entry_range(&tensor.name, tensor.offset, tensor.length, payload.len())?;
        let data_length =
            u64::try_from(tensor_data_length(tensor)?).map_err(|_| Error::LengthOverflow)?;
        let expected_length =
            data_length.checked_add(tensor.padding).ok_or(Error::LengthOverflow)?;
        if expected_length != tensor.length {
            return Err(Error::TensorShapeLengthMismatch { name: tensor.name.clone() });
        }
        let data_end =
            usize::try_from(tensor.offset.checked_add(data_length).ok_or(Error::LengthOverflow)?)
                .map_err(|_| Error::LengthOverflow)?;
        if payload[data_end..range.1].iter().any(|byte| *byte != 0) {
            return Err(Error::NonZeroPadding { name: tensor.name.clone() });
        }
        if sha256(&payload[range.0..range.1]) != tensor.digest {
            return Err(Error::EntryDigestMismatch(tensor.name.clone()));
        }
        ranges.push((tensor.name.as_str(), range));
    }
    for auxiliary in &manifest.auxiliary_artifacts {
        validate_name(&mut names, &auxiliary.name)?;
        let range =
            entry_range(&auxiliary.name, auxiliary.offset, auxiliary.length, payload.len())?;
        if sha256(&payload[range.0..range.1]) != auxiliary.digest {
            return Err(Error::EntryDigestMismatch(auxiliary.name.clone()));
        }
        ranges.push((auxiliary.name.as_str(), range));
    }
    ranges.sort_unstable_by_key(|(_, range)| range.0);
    for pair in ranges.windows(2) {
        if pair[0].1.1 > pair[1].1.0 {
            return Err(Error::OverlappingEntries {
                first: pair[0].0.into(),
                second: pair[1].0.into(),
            });
        }
    }
    match verify_sfnnv15_numeric_range(manifest, payload)? {
        Sfnnv15NumericRangeVerification::Certified(report) => Ok(Some(report)),
        Sfnnv15NumericRangeVerification::NotApplicable => Ok(None),
    }
}

fn tensor_data_length(tensor: &TensorDescriptor) -> Result<usize> {
    let element_count = tensor.shape.iter().try_fold(1_u64, |product, dimension| {
        product.checked_mul(*dimension).ok_or(Error::LengthOverflow)
    })?;
    let bytes =
        element_count.checked_mul(tensor.dtype.byte_width()).ok_or(Error::LengthOverflow)?;
    usize::try_from(bytes).map_err(|_| Error::LengthOverflow)
}

fn validate_name(names: &mut HashSet<String>, name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidManifestField("payload entry name is empty".into()));
    }
    if !names.insert(name.to_owned()) {
        return Err(Error::DuplicateEntryName(name.into()));
    }
    Ok(())
}

fn entry_range(name: &str, offset: u64, length: u64, payload_len: usize) -> Result<(usize, usize)> {
    let end =
        offset.checked_add(length).ok_or_else(|| Error::InvalidEntryRange { name: name.into() })?;
    let start =
        usize::try_from(offset).map_err(|_| Error::InvalidEntryRange { name: name.into() })?;
    let end = usize::try_from(end).map_err(|_| Error::InvalidEntryRange { name: name.into() })?;
    if end > payload_len {
        return Err(Error::InvalidEntryRange { name: name.into() });
    }
    Ok((start, end))
}

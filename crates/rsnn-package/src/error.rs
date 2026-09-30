use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("package is shorter than the fixed header")]
    TruncatedHeader,
    #[error("package magic is invalid")]
    InvalidMagic,
    #[error("unsupported container version {0}")]
    UnsupportedContainerVersion(u16),
    #[error("container flags must be zero, found {0:#06x}")]
    NonZeroFlags(u16),
    #[error("manifest length {found} exceeds limit {limit}")]
    ManifestTooLarge { found: u64, limit: u64 },
    #[error("payload length {found} exceeds limit {limit}")]
    PayloadTooLarge { found: u64, limit: u64 },
    #[error("declared package length overflows")]
    LengthOverflow,
    #[error("package is truncated: declared {declared} bytes, found {found}")]
    TruncatedPackage { declared: u64, found: u64 },
    #[error("package has trailing data: declared {declared} bytes, found {found}")]
    TrailingData { declared: u64, found: u64 },
    #[error("manifest is not valid UTF-8 JSON: {0}")]
    InvalidManifest(String),
    #[error("manifest is not in canonical JSON form")]
    NonCanonicalManifest,
    #[error("manifest schema version {0} is unsupported")]
    UnsupportedManifestVersion(u32),
    #[error("unknown required feature {0:?}")]
    UnknownRequiredFeature(String),
    #[error("manifest has {found} entries, exceeding limit {limit}")]
    TooManyEntries { found: usize, limit: usize },
    #[error("duplicate payload entry name {0:?}")]
    DuplicateEntryName(String),
    #[error("required payload entry {0:?} is missing")]
    MissingEntry(String),
    #[error("payload entry {name:?} has an invalid range")]
    InvalidEntryRange { name: String },
    #[error("payload entries {first:?} and {second:?} overlap")]
    OverlappingEntries { first: String, second: String },
    #[error("tensor {name:?} shape byte length is inconsistent with its payload length")]
    TensorShapeLengthMismatch { name: String },
    #[error("tensor {name:?} padding is not zero-filled")]
    NonZeroPadding { name: String },
    #[error("payload digest does not match manifest")]
    PayloadDigestMismatch,
    #[error("entry digest does not match for {0:?}")]
    EntryDigestMismatch(String),
    #[error("evaluation descriptor hash does not match manifest")]
    EvaluationSpecHashMismatch,
    #[error("package digest does not match caller expectation")]
    PackageDigestMismatch,
    #[error("descriptor recipe {0:?} is not supported by this evaluator")]
    UnsupportedRecipe(String),
    #[error("descriptor field {field:?} does not match compile-in evaluator expectation")]
    DescriptorMismatch { field: &'static str },
    #[error("descriptor is invalid: {0}")]
    InvalidDescriptor(String),
    #[error("manifest is invalid: {0}")]
    InvalidManifestField(String),
    #[error(
        "numeric range certificate failed for {context}: [{minimum}, {maximum}] is outside [{allowed_minimum}, {allowed_maximum}]"
    )]
    NumericRangeViolation {
        context: String,
        minimum: i128,
        maximum: i128,
        allowed_minimum: i128,
        allowed_maximum: i128,
    },
    #[error("I/O failed: {0}")]
    Io(String),
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

//! `.rsnn` v1 container reader, canonical hashing, feature schema, and A-only numeric certificate.

pub(super) mod descriptor;
pub(super) mod hash;
mod numeric_range;
pub(super) mod reader;
pub(super) mod schema;

pub(crate) use hash::Digest;
pub(super) use hash::sha256;
pub(super) use reader::{ReadPackage, read_package_bytes, read_package_file};

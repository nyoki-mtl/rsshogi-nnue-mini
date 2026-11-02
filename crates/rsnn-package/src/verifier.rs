use serde::{Deserialize, Serialize};

use crate::{Digest, Error, Result};

pub const VERIFIER_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierReport {
    pub protocol_version: u32,
    pub consumer_revision: String,
    pub package_digest: Digest,
    pub evaluation_spec_hash: Digest,
}

impl VerifierReport {
    pub fn validate(&self) -> Result<()> {
        if self.protocol_version != VERIFIER_PROTOCOL_VERSION {
            return Err(Error::InvalidManifestField(format!(
                "unsupported verifier protocol version {}",
                self.protocol_version
            )));
        }
        if self.consumer_revision.len() != 40
            || !self
                .consumer_revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::InvalidManifestField(
                "consumer revision must be a 40-character lowercase Git object ID".into(),
            ));
        }
        Ok(())
    }
}

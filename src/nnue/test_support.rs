//! 子module間で共有するtest fixture。

use super::network::StandardNetwork;
use super::{
    FEATURE_DIMENSIONS, FEATURE_TRANSFORMER_HASH, HIDDEN_DIMENSIONS, NETWORK_BODY_HASH,
    NETWORK_HASH, SUPPORTED_ARCHITECTURE, TRANSFORMED_DIMENSIONS, VERSION,
};

pub(super) fn header(version: u32, hash: u32, architecture: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&hash.to_le_bytes());
    bytes.extend_from_slice(&(architecture.len() as u32).to_le_bytes());
    bytes.extend_from_slice(architecture.as_bytes());
    bytes
}

pub(super) fn zero_network_bytes() -> Vec<u8> {
    let mut bytes = header(VERSION, NETWORK_HASH, SUPPORTED_ARCHITECTURE);
    bytes.extend_from_slice(&FEATURE_TRANSFORMER_HASH.to_le_bytes());
    bytes.resize(bytes.len() + TRANSFORMED_DIMENSIONS * 2, 0);
    bytes.resize(bytes.len() + FEATURE_DIMENSIONS * TRANSFORMED_DIMENSIONS * 2, 0);
    bytes.extend_from_slice(&NETWORK_BODY_HASH.to_le_bytes());
    bytes.resize(bytes.len() + HIDDEN_DIMENSIONS * 4, 0);
    bytes.resize(bytes.len() + HIDDEN_DIMENSIONS * TRANSFORMED_DIMENSIONS * 2, 0);
    bytes.resize(bytes.len() + HIDDEN_DIMENSIONS * 4, 0);
    bytes.resize(bytes.len() + HIDDEN_DIMENSIONS * HIDDEN_DIMENSIONS, 0);
    bytes.resize(bytes.len() + 4 + HIDDEN_DIMENSIONS, 0);
    bytes
}

/// 全featureへ決定的な非零weightを与え、index誤りがaccumulatorへ必ず現れるようにする。
pub(super) fn deterministic_feature_network() -> StandardNetwork {
    let mut network =
        StandardNetwork::from_bytes(&zero_network_bytes()).expect("valid synthetic network");
    for (index, weight) in network.feature_weights.iter_mut().enumerate() {
        *weight = (index as u64).wrapping_mul(0x9E37_79B9) as i16 % 97 - 48;
    }
    network
}

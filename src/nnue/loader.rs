//! 標準NNUE fileの読み込みとformat検証。

use std::fs;
use std::path::Path;

use super::network::{DenseLayer, StandardNetwork};
use super::{
    FEATURE_DIMENSIONS, FEATURE_TRANSFORMER_HASH, HIDDEN_DIMENSIONS, NETWORK_BODY_HASH,
    NETWORK_HASH, NnueError, SUPPORTED_ARCHITECTURE, TRANSFORMED_DIMENSIONS, VERSION,
    ensure_loadable_size,
};

impl StandardNetwork {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, NnueError> {
        let path = path.as_ref();
        // 中身を読む前にmetadataで大きさを検査し、巨大fileの読み込み自体を避ける。
        ensure_loadable_size(fs::metadata(path)?.len())?;
        let bytes = fs::read(path)?;
        ensure_loadable_size(bytes.len() as u64)?;
        Self::from_bytes(&bytes)
    }

    pub(super) fn from_bytes(bytes: &[u8]) -> Result<Self, NnueError> {
        let mut reader = Reader::new(bytes);
        let version = reader.u32("version")?;
        if version != VERSION {
            return Err(NnueError::Invalid(format!(
                "unsupported version: expected {VERSION:#010x}, got {version:#010x}"
            )));
        }

        let network_hash = reader.u32("network hash")?;
        if network_hash != NETWORK_HASH {
            return Err(NnueError::Invalid(format!(
                "unexpected network hash: expected {NETWORK_HASH:#010x}, got {network_hash:#010x}"
            )));
        }

        let architecture_length = reader.u32("architecture length")? as usize;
        if architecture_length != SUPPORTED_ARCHITECTURE.len() {
            return Err(NnueError::Invalid(format!(
                "unsupported architecture length: expected {}, got {architecture_length}",
                SUPPORTED_ARCHITECTURE.len()
            )));
        }
        let architecture_bytes = reader.take(architecture_length, "architecture")?;
        if architecture_bytes != SUPPORTED_ARCHITECTURE.as_bytes() {
            return Err(NnueError::Invalid("unsupported architecture descriptor".to_owned()));
        }

        let feature_hash = reader.u32("FeatureTransformer hash")?;
        if feature_hash != FEATURE_TRANSFORMER_HASH {
            return Err(NnueError::Invalid(format!(
                "unexpected FeatureTransformer hash: expected {FEATURE_TRANSFORMER_HASH:#010x}, got {feature_hash:#010x}"
            )));
        }
        let feature_biases = reader.i16s(TRANSFORMED_DIMENSIONS, "FeatureTransformer biases")?;
        let feature_weights = reader
            .i16s(FEATURE_DIMENSIONS * TRANSFORMED_DIMENSIONS, "FeatureTransformer weights")?;

        let body_hash = reader.u32("network body hash")?;
        if body_hash != NETWORK_BODY_HASH {
            return Err(NnueError::Invalid(format!(
                "unexpected network body hash: expected {NETWORK_BODY_HASH:#010x}, got {body_hash:#010x}"
            )));
        }
        let hidden_1 = reader.dense_layer(TRANSFORMED_DIMENSIONS * 2, "HiddenLayer1")?;
        let hidden_2 = reader.dense_layer(HIDDEN_DIMENSIONS, "HiddenLayer2")?;
        let output_bias = reader.i32("OutputLayer bias")?;
        let output_weights = reader.i8s(HIDDEN_DIMENSIONS, "OutputLayer weights")?;

        if reader.remaining() != 0 {
            return Err(NnueError::Invalid(format!(
                "unexpected trailing data: {} bytes",
                reader.remaining()
            )));
        }

        Ok(Self {
            feature_biases,
            feature_weights,
            hidden_1,
            hidden_2,
            output_bias,
            output_weights,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.cursor
    }

    fn take(&mut self, length: usize, context: &str) -> Result<&'a [u8], NnueError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| NnueError::Invalid(format!("{context} length overflow")))?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| NnueError::Invalid(format!("unexpected EOF while reading {context}")))?;
        self.cursor = end;
        Ok(value)
    }

    fn u32(&mut self, context: &str) -> Result<u32, NnueError> {
        let bytes: [u8; 4] = self.take(4, context)?.try_into().expect("slice length is fixed");
        Ok(u32::from_le_bytes(bytes))
    }

    fn i32(&mut self, context: &str) -> Result<i32, NnueError> {
        let bytes: [u8; 4] = self.take(4, context)?.try_into().expect("slice length is fixed");
        Ok(i32::from_le_bytes(bytes))
    }

    fn i16s(&mut self, count: usize, context: &str) -> Result<Box<[i16]>, NnueError> {
        let length = count
            .checked_mul(2)
            .ok_or_else(|| NnueError::Invalid(format!("{context} size overflow")))?;
        let bytes = self.take(length, context)?;
        Ok(bytes.chunks_exact(2).map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]])).collect())
    }

    fn i32s(&mut self, count: usize, context: &str) -> Result<Box<[i32]>, NnueError> {
        let length = count
            .checked_mul(4)
            .ok_or_else(|| NnueError::Invalid(format!("{context} size overflow")))?;
        let bytes = self.take(length, context)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|chunk| i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }

    fn i8s(&mut self, count: usize, context: &str) -> Result<Box<[i8]>, NnueError> {
        Ok(self.take(count, context)?.iter().map(|byte| i8::from_le_bytes([*byte])).collect())
    }

    fn dense_layer(
        &mut self,
        input_dimensions: usize,
        context: &str,
    ) -> Result<DenseLayer, NnueError> {
        let biases = self.i32s(HIDDEN_DIMENSIONS, &format!("{context} biases"))?;
        let weights =
            self.i8s(HIDDEN_DIMENSIONS * input_dimensions, &format!("{context} weights"))?;
        Ok(DenseLayer { input_dimensions, biases, weights })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nnue::test_support::{header, zero_network_bytes};
    use crate::nnue::{DEFAULT_FV_SCALE, MAX_FILE_SIZE};
    use rsshogi::board;

    #[test]
    fn rejects_wrong_version() {
        let error = StandardNetwork::from_bytes(&header(0, NETWORK_HASH, SUPPORTED_ARCHITECTURE))
            .expect_err("wrong version must fail");
        assert!(error.to_string().contains("unsupported version"));
    }

    #[test]
    fn accepts_header_before_reporting_truncated_body() {
        let error =
            StandardNetwork::from_bytes(&header(VERSION, NETWORK_HASH, SUPPORTED_ARCHITECTURE))
                .expect_err("missing body must fail");
        assert!(error.to_string().contains("FeatureTransformer hash"));
    }

    #[test]
    fn rejects_misleading_architecture_with_legacy_substrings() {
        let error = StandardNetwork::from_bytes(&header(
            VERSION,
            NETWORK_HASH,
            "Features=HalfKP(Friend),Network=AffineTransform",
        ))
        .expect_err("non-exact architecture must fail");
        assert!(error.to_string().contains("unsupported architecture"));
    }

    #[test]
    fn rejects_oversized_architecture_before_reading_or_echoing_it() {
        let architecture_length =
            u32::try_from(SUPPORTED_ARCHITECTURE.len() + 1).expect("architecture length fits u32");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&NETWORK_HASH.to_le_bytes());
        bytes.extend_from_slice(&architecture_length.to_le_bytes());

        let error =
            StandardNetwork::from_bytes(&bytes).expect_err("oversized descriptor must fail");
        assert_eq!(
            error.to_string(),
            format!(
                "invalid standard NNUE: unsupported architecture length: expected {}, got {architecture_length}",
                SUPPORTED_ARCHITECTURE.len()
            )
        );
    }

    #[test]
    fn rejects_an_overlarge_size_and_accepts_the_boundary() {
        assert!(ensure_loadable_size(MAX_FILE_SIZE).is_ok());
        assert!(matches!(
            ensure_loadable_size(MAX_FILE_SIZE + 1),
            Err(NnueError::FileTooLarge { size }) if size == MAX_FILE_SIZE + 1
        ));
    }

    #[test]
    fn synthetic_zero_network_loads_and_rejects_trailing_data() {
        let mut bytes = zero_network_bytes();
        {
            let network = StandardNetwork::from_bytes(&bytes).expect("valid synthetic network");
            let mut position = board::hirate_position();
            assert_eq!(
                network.evaluate(&position, DEFAULT_FV_SCALE).expect("evaluate startpos"),
                0
            );
            let mut accumulator = network.new_accumulator(&position).expect("initialize");
            let mv = board::move_from_usi(&position, "7g7f").expect("valid move");
            position.apply_move32(mv);
            network.advance_accumulator(&mut accumulator, &position, mv).expect("advance");
            assert_eq!(
                network
                    .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
                    .expect("incremental evaluation"),
                network.evaluate(&position, DEFAULT_FV_SCALE).expect("full refresh")
            );
            position.undo_move32(mv).expect("undo move");
            network.undo_accumulator(&mut accumulator);
            assert_eq!(
                network
                    .evaluate_accumulator(&position, &accumulator, DEFAULT_FV_SCALE)
                    .expect("restored evaluation"),
                0
            );
        }

        bytes.push(0);
        let error = StandardNetwork::from_bytes(&bytes).expect_err("trailing data must fail");
        assert!(error.to_string().contains("trailing data"));
    }
}

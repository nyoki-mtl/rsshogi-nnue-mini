//! 標準NNUE評価関数のmodule。format定数とerror型をここに置き、
//! 読み込み・推論・feature index・accumulatorは子moduleへ分ける。

use std::fmt;
use std::io;

mod accumulator;
mod features;
mod loader;
mod network;
mod simd;
#[cfg(test)]
mod test_support;

pub use accumulator::StandardAccumulator;
pub use network::StandardNetwork;

const VERSION: u32 = 0x7af3_2f16;
const NETWORK_HASH: u32 = 0x3e5a_a6ee;
const FEATURE_TRANSFORMER_HASH: u32 = 0x5d69_d7b8;
const NETWORK_BODY_HASH: u32 = 0x6333_7156;
pub(crate) const SUPPORTED_ARCHITECTURE: &str = "Features=HalfKP(Friend)[125388->256x2],Network=AffineTransform[1<-32](ClippedReLU[32](AffineTransform[32<-32](ClippedReLU[32](AffineTransform[32<-512](InputSlice[512(0:512)])))))";
const FEATURE_DIMENSIONS: usize = 125_388;
const TRANSFORMED_DIMENSIONS: usize = 256;
const HIDDEN_DIMENSIONS: usize = 32;
const FEATURE_STRIDE: usize = 1_548;
const MAX_FILE_SIZE: u64 = 128 * 1024 * 1024;
const MAX_PIECES_WITHOUT_KINGS: usize = 38;
const WEIGHT_SCALE_BITS: u32 = 6;
const CLIPPED_RELU_MAX: i32 = 127;
/// `FV_SCALE`の既定値。水匠5系の標準NNUEで慣例的に使われる値に合わせている。
pub const DEFAULT_FV_SCALE: i32 = 24;
/// NNUE評価値の定義域の上限(YaneuraOuの`VALUE_MAX_EVAL`と同じ値)。
///
/// 評価値をこの絶対値へclampすることで、探索の詰みスコア帯
/// (`MATE_TT_THRESHOLD`以上)と重ならないことを保証する。
pub const MAX_NNUE_EVAL: i32 = 31_753;

/// fileの大きさが読み込み上限に収まるかを検査する。
fn ensure_loadable_size(size: u64) -> Result<(), NnueError> {
    if size > MAX_FILE_SIZE { Err(NnueError::FileTooLarge { size }) } else { Ok(()) }
}

#[derive(Debug)]
pub enum NnueError {
    Io(io::Error),
    FileTooLarge { size: u64 },
    Invalid(String),
}

impl fmt::Display for NnueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::FileTooLarge { size } => {
                write!(formatter, "NNUE file is too large: {size} bytes (max {MAX_FILE_SIZE})")
            }
            Self::Invalid(reason) => write!(formatter, "invalid standard NNUE: {reason}"),
        }
    }
}

impl std::error::Error for NnueError {}

impl From<io::Error> for NnueError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

//! Portable `.rsnn` package validation and A-only FT1024 inference.

mod accumulator;
mod aonly;
mod features;
mod rsnn;
mod simd;

pub(crate) use accumulator::AccumulatorStack;
pub use aonly::AonlyNetwork;
#[cfg(test)]
pub(crate) use aonly::synthetic_network;
pub use rsnn::RsnnPackage;

/// Fixed conversion from the integer network output to the search evaluation scale.
pub const OUTPUT_SCALE: i32 = 16;
/// Keep evaluation scores below the mate score band.
pub const MAX_NNUE_EVAL: i32 = 31_753;

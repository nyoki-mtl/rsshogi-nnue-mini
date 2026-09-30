//! Portable `.rsnn` package validation and mobile SFNNv15 inference.

mod accumulator;
mod features;
mod mobile;
mod rsnn;
mod simd;

pub(crate) use accumulator::AccumulatorStack;
pub use mobile::MobileNetwork;
pub use rsnn::RsnnPackage;

/// Default divisor used by the engine. It is tuned together with the search margins, so the
/// engine's centipawn scale is about 24% smaller than the reference consumer's.
pub const DEFAULT_FV_SCALE: i32 = 21;
/// Keep evaluation scores below the mate score band.
pub const MAX_NNUE_EVAL: i32 = 31_753;

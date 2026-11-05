//! Portable `.rsnn` package validation and A-only FT1024 inference.

mod accumulator;
mod features;
mod network;
mod package;
mod simd;

pub(super) use accumulator::AccumulatorStack;
pub(crate) use network::AOnlyNetwork;
#[cfg(test)]
pub(crate) use network::synthetic_network;

/// Fixed conversion from the integer network output to the search evaluation scale.
const OUTPUT_SCALE: i32 = 16;
/// Keep evaluation scores below the mate score band.
pub(crate) const MAX_NNUE_EVAL: i32 = 31_753;

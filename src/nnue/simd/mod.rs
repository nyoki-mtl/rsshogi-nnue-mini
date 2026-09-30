//! Vector kernels for SFNNv15 inference, chosen at compile time.
//!
//! - x86-64 with `-C target-feature=+avx2`: [`avx2`]
//! - aarch64 (iOS, Android, Apple Silicon): [`neon`]; the dense layers use `sdot` when the
//!   CPU has `dotprod`, detected at run time unless the build already enables it
//! - anything else: [`scalar`]
//!
//! Every backend returns exactly the scalar values. Two input contracts make that hold:
//! accumulator sums wrap in `i16` but the package reader certified the final values fit, and
//! dense-layer inputs never exceed 127, so `u8` inputs are also valid `i8` and saturating
//! `i16` pair sums cannot saturate.

#[cfg_attr(
    any(all(target_arch = "x86_64", target_feature = "avx2"), target_arch = "aarch64"),
    allow(dead_code, reason = "vector builds keep the scalar kernels as the test reference")
)]
pub(crate) mod scalar;

#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
mod avx2;
#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
use avx2 as backend;
#[cfg(target_arch = "aarch64")]
use neon as backend;
#[cfg(not(any(
    all(target_arch = "x86_64", target_feature = "avx2"),
    target_arch = "aarch64"
)))]
use scalar as backend;

pub(crate) use backend::{dense, dot, pool, update};

pub(crate) const WIDTH: usize = 512;
pub(crate) const DENSE_OUTPUTS: usize = 32;

/// Name of the route this process runs. Tests use it to catch a build that silently fell back.
#[cfg(test)]
pub(crate) fn route() -> &'static str {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    return "avx2";
    #[cfg(target_arch = "aarch64")]
    return neon::route();
    #[cfg(not(any(
        all(target_arch = "x86_64", target_feature = "avx2"),
        target_arch = "aarch64"
    )))]
    return "scalar";
}

/// One accumulator perspective or one feature-transformer column, cache-line aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(64))]
pub(crate) struct Column(pub(crate) [i16; WIDTH]);

impl Column {
    pub(crate) const ZERO: Self = Self([0; WIDTH]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_stream(seed: u64) -> impl FnMut(i64, i64) -> i64 {
        let mut state = seed | 1;
        move |low, high| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            low + (state % (high - low + 1) as u64) as i64
        }
    }

    fn column(next: &mut impl FnMut(i64, i64) -> i64, low: i64, high: i64) -> Column {
        Column(std::array::from_fn(|_| next(low, high) as i16))
    }

    /// Set `RSSHOGI_EXPECT_SIMD_ROUTE` (for example `neon-dotprod` under `qemu-aarch64 -cpu max`)
    /// to prove which route the kernel tests below exercised.
    #[test]
    fn route_matches_the_target_and_the_expected_cpu() {
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        assert_eq!(route(), "avx2");
        #[cfg(target_arch = "aarch64")]
        assert!(route().starts_with("neon"));
        if let Some(expected) = std::env::var_os("RSSHOGI_EXPECT_SIMD_ROUTE") {
            assert_eq!(route(), expected);
        }
    }

    #[test]
    fn accumulator_kernels_match_scalar_including_wrapping() {
        let mut next = random_stream(11);
        for _ in 0..200 {
            let src = column(&mut next, i64::from(i16::MIN), i64::from(i16::MAX));
            let columns: Vec<Column> = (0..40)
                .map(|_| column(&mut next, i64::from(i16::MIN), i64::from(i16::MAX)))
                .collect();
            let refs: Vec<&Column> = columns.iter().collect();
            // Move deltas use up to two columns per side; cache refreshes use up to 40.
            let removed = &refs[..next(0, 20) as usize];
            let added = &refs[20..20 + next(0, 20) as usize];
            let (mut expected, mut actual) = (Column::ZERO, Column::ZERO);
            scalar::update(&mut expected, &src, removed, added);
            update(&mut actual, &src, removed, added);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn pool_matches_scalar_over_the_clamp_edges() {
        let mut next = random_stream(12);
        for _ in 0..500 {
            let left = column(&mut next, -400, 400);
            let right = column(&mut next, i64::from(i16::MIN), i64::from(i16::MAX));
            let (mut expected, mut actual) = ([0_u8; WIDTH], [0_u8; WIDTH]);
            scalar::pool([&left, &right], &mut expected);
            pool([&left, &right], &mut actual);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn dense_and_dot_match_scalar_at_the_input_bound() {
        let mut next = random_stream(13);
        for round in 0..300 {
            let weights: Vec<i8> =
                (0..WIDTH * DENSE_OUTPUTS).map(|_| next(-128, 127) as i8).collect();
            let bias: [i32; DENSE_OUTPUTS] = std::array::from_fn(|_| next(-50_000, 50_000) as i32);
            // Extreme rounds use the bound on every input; others mix zeros like pooled data.
            let input: [u8; WIDTH] = std::array::from_fn(|_| match round % 3 {
                0 => 127,
                1 => next(0, 127) as u8,
                _ => (next(-127, 127).max(0)) as u8,
            });
            let (mut expected, mut actual) = ([0; DENSE_OUTPUTS], [0; DENSE_OUTPUTS]);
            scalar::dense(&weights, &bias, &input, &mut expected);
            dense(&weights, &bias, &input, &mut actual);
            assert_eq!(actual, expected);
            let short: &[u8; 64] = input[..64].try_into().unwrap();
            scalar::dense(&weights[..64 * DENSE_OUTPUTS], &bias, short, &mut expected);
            dense(&weights[..64 * DENSE_OUTPUTS], &bias, short, &mut actual);
            assert_eq!(actual, expected);
            let row: &[i8; 128] = weights[..128].try_into().unwrap();
            let fc2_input: &[u8; 128] = input[..128].try_into().unwrap();
            assert_eq!(dot(row, fc2_input), scalar::dot(row, fc2_input));
        }
    }
}

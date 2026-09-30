//! aarch64 NEON kernels. NEON is mandatory on aarch64, so iOS and Android builds always
//! use this backend. The dense layers use `sdot` on CPUs with `dotprod`.

use std::arch::aarch64::*;

use super::{Column, DENSE_OUTPUTS, WIDTH};

const LANES: usize = 8;

#[inline]
#[target_feature(enable = "neon")]
fn load(column: &Column, offset: usize) -> int16x8_t {
    debug_assert!(offset + LANES <= WIDTH);
    // SAFETY: `offset + 8 <= WIDTH` for every caller loop.
    unsafe { vld1q_s16(column.0.as_ptr().add(offset)) }
}

#[inline]
#[target_feature(enable = "neon")]
fn store(column: &mut Column, offset: usize, value: int16x8_t) {
    debug_assert!(offset + LANES <= WIDTH);
    // SAFETY: as in `load`.
    unsafe { vst1q_s16(column.0.as_mut_ptr().add(offset), value) }
}

pub(crate) fn update(dst: &mut Column, src: &Column, removed: &[&Column], added: &[&Column]) {
    // SAFETY: this module is compiled only for builds that enable NEON.
    unsafe { update_neon(dst, src, removed, added) }
}

#[target_feature(enable = "neon")]
fn update_neon(dst: &mut Column, src: &Column, removed: &[&Column], added: &[&Column]) {
    for offset in (0..WIDTH).step_by(LANES) {
        let mut value = load(src, offset);
        for column in removed {
            value = vsubq_s16(value, load(column, offset));
        }
        for column in added {
            value = vaddq_s16(value, load(column, offset));
        }
        store(dst, offset, value);
    }
}

pub(crate) fn pool(accumulators: [&Column; 2], output: &mut [u8; WIDTH]) {
    // SAFETY: this module is compiled only for builds that enable NEON.
    unsafe { pool_neon(accumulators, output) }
}

#[target_feature(enable = "neon")]
fn pool_neon(accumulators: [&Column; 2], output: &mut [u8; WIDTH]) {
    let zero = vdupq_n_s16(0);
    let max = vdupq_n_s16(255);
    let clamp = |value: int16x8_t| vreinterpretq_u16_s16(vminq_s16(vmaxq_s16(value, zero), max));
    for (half, accumulator) in accumulators.into_iter().enumerate() {
        for offset in (0..WIDTH / 2).step_by(2 * LANES) {
            let product = |offset: usize| {
                let left = clamp(load(accumulator, offset));
                let right = clamp(load(accumulator, WIDTH / 2 + offset));
                // 255 * 255 fits in u16, and the shift leaves at most 127. Narrowing shifts
                // stop at 8 bits, so shift and narrow separately.
                vmovn_u16(vshrq_n_u16::<9>(vmulq_u16(left, right)))
            };
            let packed = vcombine_u8(product(offset), product(offset + LANES));
            // SAFETY: 16 bytes at `half * 256 + offset` stay inside the 512-byte output.
            unsafe { vst1q_u8(output.as_mut_ptr().add(half * WIDTH / 2 + offset), packed) };
        }
    }
}

/// Whether this CPU has the Armv8.2 dot-product instructions. Builds with `+dotprod` assume
/// them; other builds ask the OS (std caches the answer), so a default iOS or Android build
/// still uses `sdot` on the devices that have it.
#[inline]
fn has_dotprod() -> bool {
    cfg!(target_feature = "dotprod") || std::arch::is_aarch64_feature_detected!("dotprod")
}

/// Adds the four dot products of one 16-byte weight block (4 outputs × 4 inputs) and the
/// matching inputs into `sum` with plain NEON widening multiplies. Inputs are at most 127,
/// so they are valid `i8`, and each i16 pair sum stays within 2 * 128 * 127.
#[inline]
#[target_feature(enable = "neon")]
fn dot4_widening(sum: int32x4_t, weights: int8x16_t, inputs: int8x16_t) -> int32x4_t {
    let low = vmull_s8(vget_low_s8(weights), vget_low_s8(inputs));
    let high = vmull_high_s8(weights, inputs);
    vpadalq_s16(sum, vpaddq_s16(low, high))
}

/// The same four dot products with one `sdot`. `vdotq_s32` is still unstable in Rust, so
/// the instruction is written directly.
#[inline]
#[target_feature(enable = "neon,dotprod")]
fn dot4_sdot(mut sum: int32x4_t, weights: int8x16_t, inputs: int8x16_t) -> int32x4_t {
    // SAFETY: this function requires `dotprod`; the instruction only touches these registers.
    unsafe {
        std::arch::asm!(
            "sdot {sum:v}.4s, {weights:v}.16b, {inputs:v}.16b",
            sum = inout(vreg) sum,
            weights = in(vreg) weights,
            inputs = in(vreg) inputs,
            options(pure, nomem, nostack, preserves_flags),
        );
    }
    sum
}

pub(crate) fn dense<const IN: usize>(
    weights: &[i8],
    bias: &[i32; DENSE_OUTPUTS],
    input: &[u8; IN],
    output: &mut [i32; DENSE_OUTPUTS],
) {
    assert_eq!(weights.len(), IN * DENSE_OUTPUTS);
    // SAFETY: NEON is part of every aarch64 target, and `dotprod` is checked before use.
    unsafe {
        if has_dotprod() {
            dense_sdot(weights, bias, input, output);
        } else {
            dense_widening(weights, bias, input, output);
        }
    }
}

#[target_feature(enable = "neon")]
fn dense_widening<const IN: usize>(
    weights: &[i8],
    bias: &[i32; DENSE_OUTPUTS],
    input: &[u8; IN],
    output: &mut [i32; DENSE_OUTPUTS],
) {
    dense_with(weights, bias, input, output, |sum, weight, inputs| {
        dot4_widening(sum, weight, inputs)
    });
}

#[target_feature(enable = "neon,dotprod")]
fn dense_sdot<const IN: usize>(
    weights: &[i8],
    bias: &[i32; DENSE_OUTPUTS],
    input: &[u8; IN],
    output: &mut [i32; DENSE_OUTPUTS],
) {
    dense_with(weights, bias, input, output, |sum, weight, inputs| dot4_sdot(sum, weight, inputs));
}

/// Chunk-major dense layer shared by both dot-product variants.
#[inline]
#[target_feature(enable = "neon")]
fn dense_with<const IN: usize>(
    weights: &[i8],
    bias: &[i32; DENSE_OUTPUTS],
    input: &[u8; IN],
    output: &mut [i32; DENSE_OUTPUTS],
    dot4: impl Fn(int32x4_t, int8x16_t, int8x16_t) -> int32x4_t,
) {
    // SAFETY: bias holds 32 i32 values, read as eight 4-lane registers.
    let mut sums: [int32x4_t; 8] =
        std::array::from_fn(|index| unsafe { vld1q_s32(bias.as_ptr().add(index * 4)) });
    for (chunk, inputs) in input.chunks_exact(4).enumerate() {
        let packed = u32::from_le_bytes(inputs.try_into().expect("four inputs"));
        // Pooled activations are often zero; skipping their chunk is exact.
        if packed == 0 {
            continue;
        }
        let broadcast = vreinterpretq_s8_u32(vdupq_n_u32(packed));
        let block = chunk * DENSE_OUTPUTS * 4;
        for (index, sum) in sums.iter_mut().enumerate() {
            // SAFETY: `block + 128 <= weights.len()`, asserted by `dense`.
            let weight = unsafe { vld1q_s8(weights.as_ptr().add(block + index * 16)) };
            *sum = dot4(*sum, weight, broadcast);
        }
    }
    for (index, sum) in sums.into_iter().enumerate() {
        // SAFETY: output holds 32 i32 values.
        unsafe { vst1q_s32(output.as_mut_ptr().add(index * 4), sum) };
    }
}

pub(crate) fn dot<const IN: usize>(weights: &[i8; IN], input: &[u8; IN]) -> i32 {
    // SAFETY: NEON is part of every aarch64 target, and `dotprod` is checked before use.
    unsafe { if has_dotprod() { dot_sdot(weights, input) } else { dot_widening(weights, input) } }
}

#[target_feature(enable = "neon")]
fn dot_widening<const IN: usize>(weights: &[i8; IN], input: &[u8; IN]) -> i32 {
    dot_with(weights, input, |sum, weight, input| dot4_widening(sum, weight, input))
}

#[target_feature(enable = "neon,dotprod")]
fn dot_sdot<const IN: usize>(weights: &[i8; IN], input: &[u8; IN]) -> i32 {
    dot_with(weights, input, |sum, weight, input| dot4_sdot(sum, weight, input))
}

#[inline]
#[target_feature(enable = "neon")]
fn dot_with<const IN: usize>(
    weights: &[i8; IN],
    input: &[u8; IN],
    dot4: impl Fn(int32x4_t, int8x16_t, int8x16_t) -> int32x4_t,
) -> i32 {
    const { assert!(IN.is_multiple_of(16)) };
    let mut sum = vdupq_n_s32(0);
    for offset in (0..IN).step_by(16) {
        // SAFETY: `offset + 16 <= IN` for both arrays.
        let (weight, input) = unsafe {
            (
                vld1q_s8(weights.as_ptr().add(offset)),
                vreinterpretq_s8_u8(vld1q_u8(input.as_ptr().add(offset))),
            )
        };
        sum = dot4(sum, weight, input);
    }
    vaddvq_s32(sum)
}

/// Name of the dense-layer route this process uses, for tests that guard against fallback.
#[cfg(test)]
pub(crate) fn route() -> &'static str {
    if has_dotprod() { "neon-dotprod" } else { "neon" }
}

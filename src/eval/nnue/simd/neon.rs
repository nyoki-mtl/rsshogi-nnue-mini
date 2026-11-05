//! aarch64 NEON kernels. NEON is mandatory on aarch64, so iOS and Android builds always
//! use this backend. The dense layers use `sdot` on CPUs with `dotprod`.

use std::arch::aarch64::*;

use super::{Column, CompactColumn, DENSE_OUTPUTS, WIDTH};

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

#[inline]
#[target_feature(enable = "neon")]
fn load_weight_pair(column: &CompactColumn, offset: usize) -> (int16x8_t, int16x8_t) {
    debug_assert!(offset + 2 * LANES <= WIDTH);
    // SAFETY: 16 I8 lanes stay within this column.
    let raw = unsafe { vld1q_s8(column.0.as_ptr().add(offset)) };
    (vmovl_s8(vget_low_s8(raw)), vmovl_high_s8(raw))
}

pub(crate) fn update(
    dst: &mut Column,
    src: &Column,
    removed: &[&CompactColumn],
    added: &[&CompactColumn],
) {
    // SAFETY: this module is compiled only for builds that enable NEON.
    unsafe {
        match (removed, added) {
            ([r], [a]) => update_fixed_neon(dst, src, [*r], [*a]),
            ([r0, r1], [a0, a1]) => update_fixed_neon(dst, src, [*r0, *r1], [*a0, *a1]),
            _ => update_neon(dst, src, removed, added),
        }
    }
}

#[target_feature(enable = "neon")]
fn update_fixed_neon<const N: usize>(
    dst: &mut Column,
    src: &Column,
    removed: [&CompactColumn; N],
    added: [&CompactColumn; N],
) {
    update_neon(dst, src, &removed, &added);
}

#[inline]
#[target_feature(enable = "neon")]
fn update_neon(
    dst: &mut Column,
    src: &Column,
    removed: &[&CompactColumn],
    added: &[&CompactColumn],
) {
    const { assert!(WIDTH.is_multiple_of(4 * LANES)) };
    for offset in (0..WIDTH).step_by(4 * LANES) {
        let mut v0 = load(src, offset);
        let mut v1 = load(src, offset + LANES);
        let mut v2 = load(src, offset + 2 * LANES);
        let mut v3 = load(src, offset + 3 * LANES);
        for column in removed {
            let (w0, w1) = load_weight_pair(column, offset);
            let (w2, w3) = load_weight_pair(column, offset + 2 * LANES);
            v0 = vsubq_s16(v0, w0);
            v1 = vsubq_s16(v1, w1);
            v2 = vsubq_s16(v2, w2);
            v3 = vsubq_s16(v3, w3);
        }
        for column in added {
            let (w0, w1) = load_weight_pair(column, offset);
            let (w2, w3) = load_weight_pair(column, offset + 2 * LANES);
            v0 = vaddq_s16(v0, w0);
            v1 = vaddq_s16(v1, w1);
            v2 = vaddq_s16(v2, w2);
            v3 = vaddq_s16(v3, w3);
        }
        store(dst, offset, v0);
        store(dst, offset + LANES, v1);
        store(dst, offset + 2 * LANES, v2);
        store(dst, offset + 3 * LANES, v3);
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
            // SAFETY: 16 bytes at `half * WIDTH / 2 + offset` stay inside the WIDTH-byte output.
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
    for (chunk, inputs) in input.as_chunks::<4>().0.iter().enumerate() {
        let packed = u32::from_le_bytes(*inputs);
        // Pooled FC0 activations are often zero; FC1 (64 inputs) is small and mostly nonzero.
        if IN != 64 && packed == 0 {
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

pub(crate) fn activate<const SCALE_BITS: u32>(
    input: &[i32; DENSE_OUTPUTS],
    output: &mut [u8; 2 * DENSE_OUTPUTS],
) {
    // SAFETY: NEON is mandatory on every aarch64 target.
    unsafe { activate_neon::<SCALE_BITS>(input, output) }
}

#[target_feature(enable = "neon")]
fn activate_neon<const SCALE_BITS: u32>(
    input: &[i32; DENSE_OUTPUTS],
    output: &mut [u8; 2 * DENSE_OUTPUTS],
) {
    const { assert!(SCALE_BITS == 6 || SCALE_BITS == 7) };
    let sq_min = vdupq_n_s32(-16_384);
    let sq_max = vdupq_n_s32(16_384);
    let max127 = vdupq_n_u8(127);

    let pack16 = |a: int32x4_t, b: int32x4_t, c: int32x4_t, d: int32x4_t| {
        let ab = vqmovn_u16(vcombine_u16(vqmovun_s32(a), vqmovun_s32(b)));
        let cd = vqmovn_u16(vcombine_u16(vqmovun_s32(c), vqmovun_s32(d)));
        vminq_u8(vcombine_u8(ab, cd), max127)
    };

    for half in 0..2 {
        let base = half * 16;
        // SAFETY: `base + 16 <= DENSE_OUTPUTS` (32) and `output` has 64 bytes.
        let v: [int32x4_t; 4] =
            std::array::from_fn(|i| unsafe { vld1q_s32(input.as_ptr().add(base + i * 4)) });
        let sq: [int32x4_t; 4] = std::array::from_fn(|i| {
            let clamped = vminq_s32(vmaxq_s32(v[i], sq_min), sq_max);
            let prod = vmulq_s32(clamped, clamped);
            match SCALE_BITS {
                6 => vshrq_n_s32::<19>(prod),
                7 => vshrq_n_s32::<21>(prod),
                _ => unreachable!(),
            }
        });
        let lin: [int32x4_t; 4] = std::array::from_fn(|i| match SCALE_BITS {
            6 => vshrq_n_s32::<6>(v[i]),
            7 => vshrq_n_s32::<7>(v[i]),
            _ => unreachable!(),
        });
        unsafe {
            vst1q_u8(output.as_mut_ptr().add(base), pack16(sq[0], sq[1], sq[2], sq[3]));
            vst1q_u8(
                output.as_mut_ptr().add(DENSE_OUTPUTS + base),
                pack16(lin[0], lin[1], lin[2], lin[3]),
            );
        }
    }
}

/// Name of the dense-layer route this process uses, for tests that guard against fallback.
#[cfg(test)]
pub(crate) fn route() -> &'static str {
    if has_dotprod() { "neon-dotprod" } else { "neon" }
}

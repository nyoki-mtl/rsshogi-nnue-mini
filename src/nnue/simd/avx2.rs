//! x86-64 AVX2 kernels. Selected at compile time by `-C target-feature=+avx2`.

use std::arch::x86_64::*;

use super::{Column, DENSE_OUTPUTS, WIDTH};

const LANES: usize = 16;

#[inline]
#[target_feature(enable = "avx2")]
fn load(column: &Column, offset: usize) -> __m256i {
    debug_assert!(offset + LANES <= WIDTH);
    // SAFETY: `offset + 16 <= WIDTH` for every caller loop, and loadu has no alignment demand.
    unsafe { _mm256_loadu_si256(column.0.as_ptr().add(offset).cast()) }
}

#[inline]
#[target_feature(enable = "avx2")]
fn store(column: &mut Column, offset: usize, value: __m256i) {
    debug_assert!(offset + LANES <= WIDTH);
    // SAFETY: as in `load`.
    unsafe { _mm256_storeu_si256(column.0.as_mut_ptr().add(offset).cast(), value) }
}

pub(crate) fn update(dst: &mut Column, src: &Column, removed: &[&Column], added: &[&Column]) {
    // A move changes one or two features per perspective. Fixing those counts at compile
    // time unrolls the per-column loops; refreshes with other counts take the general path.
    // SAFETY: this module is compiled only for builds that enable AVX2.
    unsafe {
        match (removed, added) {
            ([r], [a]) => update_fixed_avx2(dst, src, [*r], [*a]),
            ([r0, r1], [a0, a1]) => update_fixed_avx2(dst, src, [*r0, *r1], [*a0, *a1]),
            _ => update_avx2(dst, src, removed, added),
        }
    }
}

#[target_feature(enable = "avx2")]
fn update_fixed_avx2<const N: usize>(
    dst: &mut Column,
    src: &Column,
    removed: [&Column; N],
    added: [&Column; N],
) {
    update_avx2(dst, src, &removed, &added);
}

#[inline]
#[target_feature(enable = "avx2")]
fn update_avx2(dst: &mut Column, src: &Column, removed: &[&Column], added: &[&Column]) {
    for offset in (0..WIDTH).step_by(LANES) {
        let mut value = load(src, offset);
        for column in removed {
            value = _mm256_sub_epi16(value, load(column, offset));
        }
        for column in added {
            value = _mm256_add_epi16(value, load(column, offset));
        }
        store(dst, offset, value);
    }
}

pub(crate) fn pool(accumulators: [&Column; 2], output: &mut [u8; WIDTH]) {
    // SAFETY: this module is compiled only for builds that enable AVX2.
    unsafe { pool_avx2(accumulators, output) }
}

#[target_feature(enable = "avx2")]
fn pool_avx2(accumulators: [&Column; 2], output: &mut [u8; WIDTH]) {
    let zero = _mm256_setzero_si256();
    let max = _mm256_set1_epi16(255);
    let clamp = |value: __m256i| _mm256_min_epi16(_mm256_max_epi16(value, zero), max);
    for (half, accumulator) in accumulators.into_iter().enumerate() {
        for offset in (0..WIDTH / 2).step_by(2 * LANES) {
            let product = |offset: usize| {
                // (a << 7) * b >> 16 == a * b / 512 for 0 <= a, b <= 255.
                let left = _mm256_slli_epi16::<7>(clamp(load(accumulator, offset)));
                let right = clamp(load(accumulator, WIDTH / 2 + offset));
                _mm256_mulhi_epi16(left, right)
            };
            let packed = _mm256_packus_epi16(product(offset), product(offset + LANES));
            // packus interleaves 128-bit lanes; restore channel order.
            let ordered = _mm256_permute4x64_epi64::<0b11_01_10_00>(packed);
            // SAFETY: 32 bytes at `half * 256 + offset` stay inside the 512-byte output.
            unsafe {
                _mm256_storeu_si256(
                    output.as_mut_ptr().add(half * WIDTH / 2 + offset).cast(),
                    ordered,
                );
            }
        }
    }
}

pub(crate) fn dense<const IN: usize>(
    weights: &[i8],
    bias: &[i32; DENSE_OUTPUTS],
    input: &[u8; IN],
    output: &mut [i32; DENSE_OUTPUTS],
) {
    // SAFETY: this module is compiled only for builds that enable AVX2.
    unsafe { dense_avx2::<IN>(weights, bias, input, output) }
}

/// For each 8-bit mask, the positions of its set bits, padded with zeros.
const SET_BIT_POSITIONS: [[u16; 8]; 256] = {
    let mut table = [[0_u16; 8]; 256];
    let mut mask = 0;
    while mask < 256 {
        let (mut bit, mut count) = (0, 0);
        while bit < 8 {
            if mask & (1 << bit) != 0 {
                table[mask][count] = bit as u16;
                count += 1;
            }
            bit += 1;
        }
        mask += 1;
    }
    table
};

#[target_feature(enable = "avx2")]
fn dense_avx2<const IN: usize>(
    weights: &[i8],
    bias: &[i32; DENSE_OUTPUTS],
    input: &[u8; IN],
    output: &mut [i32; DENSE_OUTPUTS],
) {
    const { assert!(IN.is_multiple_of(32) && IN <= WIDTH) };
    assert_eq!(weights.len(), IN * DENSE_OUTPUTS);
    // Pooled activations are often zero, and skipping a zero chunk of four inputs is exact.
    // List the nonzero chunks first so the product loop has no data-dependent branch.
    // The eight spare slots take the last full-width store of positions.
    let mut chunks = [0_u16; WIDTH / 4 + 8];
    let mut count = 0;
    let zero = _mm256_setzero_si256();
    let mut base = _mm_setzero_si128();
    for offset in (0..IN).step_by(32) {
        // SAFETY: `offset + 32 <= IN`.
        let values = unsafe { _mm256_loadu_si256(input.as_ptr().add(offset).cast()) };
        // Inputs are at most 127, so a chunk read as `i32` is nonzero exactly when positive.
        let mask = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpgt_epi32(values, zero)));
        let mask = mask as u8;
        // SAFETY: the table row holds eight `u16`; `count + 8 <= offset / 4 + 8` fits `chunks`.
        unsafe {
            let positions = _mm_loadu_si128(SET_BIT_POSITIONS[usize::from(mask)].as_ptr().cast());
            _mm_storeu_si128(chunks.as_mut_ptr().add(count).cast(), _mm_add_epi16(base, positions));
        }
        count += mask.count_ones() as usize;
        base = _mm_add_epi16(base, _mm_set1_epi16(8));
    }
    let ones = _mm256_set1_epi16(1);
    // SAFETY: bias holds 32 i32 values, read as four 8-lane registers.
    let mut sums: [__m256i; 4] = std::array::from_fn(|index| unsafe {
        _mm256_loadu_si256(bias.as_ptr().add(index * 8).cast())
    });
    for &chunk in &chunks[..count] {
        let chunk = usize::from(chunk);
        // SAFETY: `chunk < IN / 4`, so the four bytes are inside `input`.
        let packed = unsafe { input.as_ptr().add(chunk * 4).cast::<i32>().read_unaligned() };
        let broadcast = _mm256_set1_epi32(packed);
        let block = chunk * DENSE_OUTPUTS * 4;
        for (index, sum) in sums.iter_mut().enumerate() {
            // SAFETY: `block + 128 <= weights.len()` by the length assertion.
            let weight =
                unsafe { _mm256_loadu_si256(weights.as_ptr().add(block + index * 32).cast()) };
            // Inputs are at most 127, so the saturating i16 pair sums are exact.
            let pairs = _mm256_maddubs_epi16(broadcast, weight);
            *sum = _mm256_add_epi32(*sum, _mm256_madd_epi16(pairs, ones));
        }
    }
    for (index, sum) in sums.into_iter().enumerate() {
        // SAFETY: output holds 32 i32 values.
        unsafe { _mm256_storeu_si256(output.as_mut_ptr().add(index * 8).cast(), sum) };
    }
}

pub(crate) fn dot<const IN: usize>(weights: &[i8; IN], input: &[u8; IN]) -> i32 {
    // SAFETY: this module is compiled only for builds that enable AVX2.
    unsafe { dot_avx2::<IN>(weights, input) }
}

#[target_feature(enable = "avx2")]
fn dot_avx2<const IN: usize>(weights: &[i8; IN], input: &[u8; IN]) -> i32 {
    const { assert!(IN.is_multiple_of(32)) };
    let ones = _mm256_set1_epi16(1);
    let mut sum = _mm256_setzero_si256();
    for offset in (0..IN).step_by(32) {
        // SAFETY: `offset + 32 <= IN` for both arrays.
        let (input, weight) = unsafe {
            (
                _mm256_loadu_si256(input.as_ptr().add(offset).cast()),
                _mm256_loadu_si256(weights.as_ptr().add(offset).cast()),
            )
        };
        sum = _mm256_add_epi32(sum, _mm256_madd_epi16(_mm256_maddubs_epi16(input, weight), ones));
    }
    let half = _mm_add_epi32(_mm256_castsi256_si128(sum), _mm256_extracti128_si256::<1>(sum));
    let quarter = _mm_add_epi32(half, _mm_unpackhi_epi64(half, half));
    _mm_cvtsi128_si32(_mm_add_epi32(quarter, _mm_shuffle_epi32::<0b01>(quarter)))
}

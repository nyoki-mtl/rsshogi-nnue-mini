//! Portable reference kernels. Every vector backend must return exactly these values.

use super::{Column, DENSE_OUTPUTS, WIDTH};

pub(crate) fn update(dst: &mut Column, src: &Column, removed: &[&Column], added: &[&Column]) {
    dst.0.copy_from_slice(&src.0);
    for column in removed {
        for (value, &weight) in dst.0.iter_mut().zip(&column.0) {
            *value = value.wrapping_sub(weight);
        }
    }
    for column in added {
        for (value, &weight) in dst.0.iter_mut().zip(&column.0) {
            *value = value.wrapping_add(weight);
        }
    }
}

/// `clamp(a, 0, 255) * clamp(b, 0, 255) / 512` for the two halves of each accumulator.
pub(crate) fn pool(accumulators: [&Column; 2], output: &mut [u8; WIDTH]) {
    for (half, accumulator) in accumulators.into_iter().enumerate() {
        let (left, right) = accumulator.0.split_at(WIDTH / 2);
        for ((out, &a), &b) in
            output[half * WIDTH / 2..][..WIDTH / 2].iter_mut().zip(left).zip(right)
        {
            let a = i32::from(a).clamp(0, 255);
            let b = i32::from(b).clamp(0, 255);
            *out = (a * b / 512) as u8;
        }
    }
}

/// `output[o] = bias[o] + sum_i weights(o, i) * input[i]` with chunk-major weights:
/// four consecutive inputs of all outputs are stored together (`[IN / 4][32][4]`).
pub(crate) fn dense<const IN: usize>(
    weights: &[i8],
    bias: &[i32; DENSE_OUTPUTS],
    input: &[u8; IN],
    output: &mut [i32; DENSE_OUTPUTS],
) {
    *output = *bias;
    for (chunk, inputs) in input.chunks_exact(4).enumerate() {
        let block = &weights[chunk * DENSE_OUTPUTS * 4..][..DENSE_OUTPUTS * 4];
        for (value, row) in output.iter_mut().zip(block.chunks_exact(4)) {
            *value += row
                .iter()
                .zip(inputs)
                .map(|(&weight, &input)| i32::from(weight) * i32::from(input))
                .sum::<i32>();
        }
    }
}

pub(crate) fn dot<const IN: usize>(weights: &[i8; IN], input: &[u8; IN]) -> i32 {
    weights.iter().zip(input).map(|(&weight, &input)| i32::from(weight) * i32::from(input)).sum()
}

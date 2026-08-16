//! NNUEの内側loopと、その実行時選択。
//!
//! AVX2 kernelは常にbinaryへ入れ、実行時のCPU機能で選ぶ。
//! 一つのbinaryをどの機械でも動かせるようにするためで、
//! `-C target-feature=+avx2`でbuildした場合も選択の結果は変わらない。

/// AVX2 kernelを使えるかどうか。判定結果はstdが内部でcacheする。
///
/// `-C target-feature=+avx2`でbuildしたときは第一項が定数になるので、
/// 分岐も`#[target_feature]`の呼び出し境界も消え、compile時に選ぶのと同じcodeになる。
#[cfg(target_arch = "x86_64")]
fn avx2_available() -> bool {
    cfg!(target_feature = "avx2") || std::arch::is_x86_feature_detected!("avx2")
}

#[cfg(not(target_arch = "x86_64"))]
fn avx2_available() -> bool {
    false
}

pub(crate) fn active_route() -> &'static str {
    if avx2_available() { "avx2" } else { "scalar" }
}

use super::TRANSFORMED_DIMENSIONS;

pub(crate) fn clamp_pair(
    friend: &[i16; TRANSFORMED_DIMENSIONS],
    enemy: &[i16; TRANSFORMED_DIMENSIONS],
) -> [u8; TRANSFORMED_DIMENSIONS * 2] {
    #[cfg(target_arch = "x86_64")]
    if avx2_available() {
        // SAFETY: 直前にAVX2の有無を確認している。
        return unsafe { avx2::clamp_pair(friend, enemy) };
    }
    scalar::clamp_pair(friend, enemy)
}

pub(crate) fn add_assign(values: &mut [i16], weights: &[i16]) {
    assert_eq!(values.len(), weights.len());
    #[cfg(target_arch = "x86_64")]
    if avx2_available() {
        // SAFETY: 直前にAVX2の有無を確認している。
        unsafe { avx2::add_assign(values, weights) };
        return;
    }
    scalar::add_assign(values, weights);
}

pub(crate) fn sub_assign(values: &mut [i16], weights: &[i16]) {
    assert_eq!(values.len(), weights.len());
    #[cfg(target_arch = "x86_64")]
    if avx2_available() {
        // SAFETY: 直前にAVX2の有無を確認している。
        unsafe { avx2::sub_assign(values, weights) };
        return;
    }
    scalar::sub_assign(values, weights);
}

pub(crate) fn dot(weights: &[i8], input: &[u8]) -> i32 {
    assert_eq!(weights.len(), input.len());
    #[cfg(target_arch = "x86_64")]
    if avx2_available() {
        // SAFETY: 直前にAVX2の有無を確認している。
        return unsafe { avx2::dot(weights, input) };
    }
    scalar::dot(weights, input)
}

mod scalar {
    use super::TRANSFORMED_DIMENSIONS;

    pub(super) fn clamp_pair(
        friend: &[i16; TRANSFORMED_DIMENSIONS],
        enemy: &[i16; TRANSFORMED_DIMENSIONS],
    ) -> [u8; TRANSFORMED_DIMENSIONS * 2] {
        let mut output = [0u8; TRANSFORMED_DIMENSIONS * 2];
        for (slot, value) in output[..TRANSFORMED_DIMENSIONS].iter_mut().zip(friend) {
            *slot = i32::from(*value).clamp(0, 127) as u8;
        }
        for (slot, value) in output[TRANSFORMED_DIMENSIONS..].iter_mut().zip(enemy) {
            *slot = i32::from(*value).clamp(0, 127) as u8;
        }
        output
    }

    pub(super) fn add_assign(values: &mut [i16], weights: &[i16]) {
        for (value, weight) in values.iter_mut().zip(weights) {
            *value = value.wrapping_add(*weight);
        }
    }

    pub(super) fn sub_assign(values: &mut [i16], weights: &[i16]) {
        for (value, weight) in values.iter_mut().zip(weights) {
            *value = value.wrapping_sub(*weight);
        }
    }

    pub(super) fn dot(weights: &[i8], input: &[u8]) -> i32 {
        weights.iter().zip(input).map(|(w, x)| i32::from(*w) * i32::from(*x)).sum()
    }
}

/// AVX2 kernel。呼び出し側が[`avx2_available`]を確認したときだけ呼べる。
#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::TRANSFORMED_DIMENSIONS;
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn clamp_pair(
        friend: &[i16; TRANSFORMED_DIMENSIONS],
        enemy: &[i16; TRANSFORMED_DIMENSIONS],
    ) -> [u8; TRANSFORMED_DIMENSIONS * 2] {
        let mut output = [0u8; TRANSFORMED_DIMENSIONS * 2];
        let (friend_output, enemy_output) = output.split_at_mut(TRANSFORMED_DIMENSIONS);
        unsafe {
            let zero = _mm256_setzero_si256();
            let max = _mm256_set1_epi16(127);
            for (source, destination) in
                [(friend.as_slice(), friend_output), (enemy.as_slice(), enemy_output)]
            {
                let mut offset = 0;
                while offset + 32 <= TRANSFORMED_DIMENSIONS {
                    let first = _mm256_loadu_si256(source.as_ptr().add(offset).cast());
                    let second = _mm256_loadu_si256(source.as_ptr().add(offset + 16).cast());
                    let first = _mm256_min_epi16(_mm256_max_epi16(first, zero), max);
                    let second = _mm256_min_epi16(_mm256_max_epi16(second, zero), max);
                    let packed = _mm256_packus_epi16(first, second);
                    let ordered = _mm256_permute4x64_epi64::<0xd8>(packed);
                    _mm256_storeu_si256(destination.as_mut_ptr().add(offset).cast(), ordered);
                    offset += 32;
                }
                for (slot, value) in destination[offset..].iter_mut().zip(&source[offset..]) {
                    *slot = i32::from(*value).clamp(0, 127) as u8;
                }
            }
        }
        output
    }

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn add_assign(values: &mut [i16], weights: &[i16]) {
        add_sub_assign(values, weights, true);
    }

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn sub_assign(values: &mut [i16], weights: &[i16]) {
        add_sub_assign(values, weights, false);
    }

    #[target_feature(enable = "avx2")]
    fn add_sub_assign(values: &mut [i16], weights: &[i16], add: bool) {
        let mut offset = 0;
        unsafe {
            while offset + 16 <= values.len() {
                let value = _mm256_loadu_si256(values.as_ptr().add(offset).cast());
                let weight = _mm256_loadu_si256(weights.as_ptr().add(offset).cast());
                let result = if add {
                    _mm256_add_epi16(value, weight)
                } else {
                    _mm256_sub_epi16(value, weight)
                };
                _mm256_storeu_si256(values.as_mut_ptr().add(offset).cast(), result);
                offset += 16;
            }
        }
        for (value, weight) in values[offset..].iter_mut().zip(&weights[offset..]) {
            *value = if add { value.wrapping_add(*weight) } else { value.wrapping_sub(*weight) };
        }
    }

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn dot(weights: &[i8], input: &[u8]) -> i32 {
        let mut offset = 0;
        let mut lanes = [0i32; 8];
        unsafe {
            let ones = _mm256_set1_epi16(1);
            let mut sum = _mm256_setzero_si256();
            while offset + 16 <= input.len() {
                let bytes = _mm_loadu_si128(input.as_ptr().add(offset).cast());
                let signed = _mm_loadu_si128(weights.as_ptr().add(offset).cast());
                let products =
                    _mm256_mullo_epi16(_mm256_cvtepu8_epi16(bytes), _mm256_cvtepi8_epi16(signed));
                sum = _mm256_add_epi32(sum, _mm256_madd_epi16(products, ones));
                offset += 16;
            }
            _mm256_storeu_si256(lanes.as_mut_ptr().cast(), sum);
        }
        let vector_sum: i32 = lanes.into_iter().sum();
        vector_sum
            + weights[offset..]
                .iter()
                .zip(&input[offset..])
                .map(|(w, x)| i32::from(*w) * i32::from(*x))
                .sum::<i32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_kernels_match_scalar_on_boundaries() {
        let mut friend = [0i16; 256];
        let mut enemy = [0i16; 256];
        let mut weights = [0i16; 256];
        for index in 0..256 {
            friend[index] = [i16::MIN, -1, 0, 1, 126, 127, 128, i16::MAX][index % 8];
            enemy[index] = friend[255 - index];
            weights[index] = (index as i16).wrapping_mul(997);
        }
        assert_eq!(clamp_pair(&friend, &enemy), scalar::clamp_pair(&friend, &enemy));

        let mut active = friend;
        let mut expected = friend;
        add_assign(&mut active, &weights);
        scalar::add_assign(&mut expected, &weights);
        assert_eq!(active, expected);
        sub_assign(&mut active, &weights);
        scalar::sub_assign(&mut expected, &weights);
        assert_eq!(active, expected);

        let input = clamp_pair(&friend, &enemy);
        let signed = (0..512).map(|index| (index as i8).wrapping_mul(53)).collect::<Vec<_>>();
        assert_eq!(dot(&signed, &input), scalar::dot(&signed, &input));
    }

    #[test]
    fn route_follows_the_running_cpu_not_the_compile_target() {
        #[cfg(target_arch = "x86_64")]
        let expected = if std::arch::is_x86_feature_detected!("avx2") { "avx2" } else { "scalar" };
        #[cfg(not(target_arch = "x86_64"))]
        let expected = "scalar";
        assert_eq!(active_route(), expected);

        // `-C target-feature=+avx2`でbuildしても、選択はCPU側の事実に従う。
        if cfg!(all(target_arch = "x86_64", target_feature = "avx2")) {
            assert_eq!(active_route(), "avx2", "avx2向けbuildを実行できているならavx2経路になる");
        }
    }
}

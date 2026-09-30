//! Late Move Reductionの縮小量テーブルを担う。
//!
//! `r = floor(ln(depth) * ln(index) * 100 / divisor)` をworkerの探索開始時に
//! 事前計算し、探索loopでは表引きだけを行う。PVノード・improving・履歴による
//! 補正は[`lmr_reduction`]が実行時に加える。

use crate::params::SearchParams;

/// depth/move indexの表の一辺。どちらもこの値-1へ飽和する。
const TABLE_SIZE: usize = 64;

/// 事前計算した縮小量の基礎値。
pub(super) struct LmrReductions {
    table: [[u32; TABLE_SIZE]; TABLE_SIZE],
}

impl LmrReductions {
    /// divisorが大きいほど縮小は浅くなる。0以下は1へ丸めて0除算を避ける。
    pub(super) fn new(divisor: i32) -> Self {
        let divisor = f64::from(divisor.max(1));
        let mut table = [[0u32; TABLE_SIZE]; TABLE_SIZE];
        for (depth, row) in table.iter_mut().enumerate().skip(1) {
            for (index, entry) in row.iter_mut().enumerate().skip(1) {
                let product = (depth as f64).ln() * (index as f64).ln() * 100.0;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    *entry = (product / divisor).floor() as u32;
                }
            }
        }
        Self { table }
    }

    /// 縮小量の基礎値。depthとindexは表の上限へ飽和する。
    pub(super) fn base(&self, depth: u32, index: usize) -> i32 {
        let depth = (depth as usize).min(TABLE_SIZE - 1);
        let index = index.min(TABLE_SIZE - 1);
        self.table[depth][index] as i32
    }
}

/// 表引きした縮小量の基礎値へ実行時の補正を加える。
///
/// PVノードは浅く、improvingでないノードは深く縮小する。履歴スコアによる
/// 補正は`lmr_history_divisor`で段数へ換算し、`±lmr_history_clamp`に収める。
/// 最終値は`0..=depth-2`へclampし、縮小後の子の深さが1を下回らないようにする。
pub(super) fn lmr_reduction(
    base: i32,
    pv_node: bool,
    improving: bool,
    history_score: i32,
    depth: u32,
    params: &SearchParams,
) -> i32 {
    let mut reduction = base;
    if pv_node {
        reduction -= 1;
    }
    if !improving {
        reduction += 1;
    }
    let clamp = params.lmr_history_clamp.max(0);
    reduction -= (history_score / params.lmr_history_divisor.max(1)).clamp(-clamp, clamp);
    reduction.clamp(0, depth as i32 - 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reductions_are_monotonic_in_depth_and_move_index() {
        let table = LmrReductions::new(236);
        for depth in 2..TABLE_SIZE as u32 {
            for index in 2..TABLE_SIZE {
                assert!(
                    table.base(depth, index) >= table.base(depth - 1, index),
                    "depth単調性: depth={depth} index={index}"
                );
                assert!(
                    table.base(depth, index) >= table.base(depth, index - 1),
                    "index単調性: depth={depth} index={index}"
                );
            }
        }
    }

    #[test]
    fn representative_reductions_match_the_formula() {
        let table = LmrReductions::new(236);
        // ln(3)*ln(4)*100/236 = 0.645... -> 0
        assert_eq!(table.base(3, 4), 0);
        // ln(20)*ln(30)*100/236 = 4.317... -> 4
        assert_eq!(table.base(20, 30), 4);
        // ln(8)*ln(16)*100/236 = 2.442... -> 2
        assert_eq!(table.base(8, 16), 2);
        assert_eq!(table.base(1, 63), 0, "depth 1の行はln(1)=0で常に0");
        assert_eq!(table.base(63, 1), 0, "index 1の列はln(1)=0で常に0");
    }

    #[test]
    fn out_of_table_arguments_saturate_at_the_table_edge() {
        let table = LmrReductions::new(236);
        assert_eq!(table.base(1_000, 500), table.base(63, 63));
        assert_eq!(table.base(63, 63), (f64::ln(63.0) * f64::ln(63.0) * 100.0 / 236.0) as i32);
    }

    #[test]
    fn a_larger_divisor_reduces_less() {
        let aggressive = LmrReductions::new(100);
        let conservative = LmrReductions::new(600);
        for depth in [4, 8, 16, 32] {
            for index in [4, 8, 16, 32] {
                assert!(aggressive.base(depth, index) >= conservative.base(depth, index));
            }
        }
        assert!(aggressive.base(16, 16) > conservative.base(16, 16));
    }

    #[test]
    fn lmr_adjustments_shift_the_base_by_at_most_one_each() {
        let lmr = SearchParams {
            lmr_history_divisor: 8_192,
            lmr_history_clamp: 1,
            ..SearchParams::default()
        };
        // PVノードは-1、improvingでないと+1、履歴は±1。
        assert_eq!(lmr_reduction(2, false, true, 0, 8, &lmr), 2);
        assert_eq!(lmr_reduction(2, true, true, 0, 8, &lmr), 1);
        assert_eq!(lmr_reduction(2, false, false, 0, 8, &lmr), 3);
        assert_eq!(lmr_reduction(2, false, true, 8_192, 8, &lmr), 1);
        assert_eq!(lmr_reduction(2, false, true, -8_192, 8, &lmr), 3);
        assert_eq!(
            lmr_reduction(2, false, true, 16_384, 8, &lmr),
            1,
            "履歴の補正は±1でclampされる"
        );
        assert_eq!(
            lmr_reduction(2, false, true, -16_384, 8, &lmr),
            3,
            "履歴の補正は±1でclampされる"
        );
    }

    #[test]
    fn lmr_reduction_clamps_into_the_valid_depth_window() {
        let lmr = SearchParams {
            lmr_history_divisor: 8_192,
            lmr_history_clamp: 1,
            ..SearchParams::default()
        };
        assert_eq!(
            lmr_reduction(0, true, true, 8_192, 8, &lmr),
            0,
            "負の補正でも0未満にはならない"
        );
        assert_eq!(lmr_reduction(10, false, false, -16_384, 6, &lmr), 4, "depth-2でclampされる");
        assert_eq!(lmr_reduction(3, false, false, 0, 2, &lmr), 0, "depth 2では縮小しない");
    }
}

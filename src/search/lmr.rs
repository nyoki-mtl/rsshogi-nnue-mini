//! Late Move Reductionの縮小量テーブルを担う。
//!
//! `r = ln(depth) * ln(index) * 100 / divisor`を1/1024段の単位でworkerの探索開始時に
//! 事前計算し、探索loopでは表引きだけを行う。nodeと手の性質による補正は
//! [`lmr_reduction`]が同じ1/1024段の単位で実行時に加える。

use crate::params::SearchParams;

/// 深さの表の上限。従来と同じ63へ飽和する。
const DEPTH_SIZE: usize = 64;
/// 将棋の手生成上限より広く取り、持ち駒の多い局面でも着手順63で飽和させない。
const MOVE_INDEX_SIZE: usize = 1024;

/// 縮小量の1段を表す単位。補正を段より細かく積み、最後に段へ丸める。
pub(super) const LMR_UNIT: i32 = 1024;

/// 事前計算した縮小量の基礎値(1/1024段)。
pub(super) struct LmrReductions {
    table: Box<[[u32; MOVE_INDEX_SIZE]]>,
}

impl LmrReductions {
    /// divisorが大きいほど縮小は浅くなる。0以下は1へ丸めて0除算を避ける。
    pub(super) fn new(divisor: i32) -> Self {
        let divisor = f64::from(divisor.max(1));
        let mut table = vec![[0u32; MOVE_INDEX_SIZE]; DEPTH_SIZE].into_boxed_slice();
        let move_logs: [f64; MOVE_INDEX_SIZE] =
            std::array::from_fn(|index| if index == 0 { 0.0 } else { (index as f64).ln() });
        for (depth, row) in table.iter_mut().enumerate().skip(1) {
            let depth_log = (depth as f64).ln();
            for (index, entry) in row.iter_mut().enumerate().skip(1) {
                let product = depth_log * move_logs[index] * 100.0;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    *entry = (product * f64::from(LMR_UNIT) / divisor).floor() as u32;
                }
            }
        }
        Self { table }
    }

    /// 縮小量の基礎値(1/1024段)。depthとindexは表の上限へ飽和する。
    pub(super) fn base_fine(&self, depth: u32, index: usize) -> i32 {
        let depth = (depth as usize).min(DEPTH_SIZE - 1);
        let index = index.min(MOVE_INDEX_SIZE - 1);
        self.table[depth][index] as i32
    }

    /// 縮小量の基礎値を段へ切り捨てた値。
    pub(super) fn base(&self, depth: u32, index: usize) -> i32 {
        self.base_fine(depth, index) / LMR_UNIT
    }
}

/// 縮小量の補正に使うnodeと手の性質。
#[derive(Clone, Copy, Default)]
pub(super) struct LmrInputs {
    /// 今PV上にあるか、以前PV上で読まれたnode。
    pub(super) tt_pv: bool,
    pub(super) improving: bool,
    /// fail highが見込まれるnode。
    pub(super) cut_node: bool,
    /// TT moveが捕獲で、静かな手が局面を変える見込みが薄いnode。
    pub(super) tt_capture: bool,
    /// このnodeの子で既に起きたβカットの回数。
    pub(super) child_cutoffs: u32,
    pub(super) history_score: i32,
    /// 実際に静的評価へ加えた補正の絶対値。大きい局面では縮小を控える。
    pub(super) correction_abs: i32,
    /// このnodeの探索窓の幅。zero-windowでは追加補正しない。
    pub(super) window_width: i32,
}

/// 表引きした基礎値(1/1024段)へ実行時の補正を加え、1/1024段のまま返す。
///
/// PV上で読まれたnodeは浅く、improvingでないnode・cut node・TT moveが捕獲のnode・
/// 子でβカットが続いたnodeは深く縮小する。履歴スコアは`lmr_history_divisor`で
/// 1段に換算し、`±lmr_history_clamp`段に収める。0未満にはしない。
pub(super) fn lmr_reduction(base: i32, inputs: LmrInputs, params: &SearchParams) -> i32 {
    let mut reduction = base;
    if inputs.tt_pv {
        reduction -= params.lmr_tt_pv;
    }
    if !inputs.improving {
        reduction += params.lmr_not_improving;
    }
    if inputs.cut_node {
        reduction += params.lmr_cut_node;
    }
    if inputs.tt_capture {
        reduction += params.lmr_tt_capture;
    }
    if inputs.child_cutoffs > 1 {
        reduction += params.lmr_child_cutoffs;
    }
    let clamp = params.lmr_history_clamp.max(0) * LMR_UNIT;
    let history = i64::from(inputs.history_score) * i64::from(LMR_UNIT)
        / i64::from(params.lmr_history_divisor.max(1));
    reduction -= (history as i32).clamp(-clamp, clamp);
    // 既存の補正値を使い、128点で最大1段だけ縮小を控える。
    reduction -= inputs.correction_abs.clamp(0, 128) * LMR_UNIT / 128;
    // 通常のaspiration窓まで、最大半段だけ広い窓の探索を保護する。
    let window_span = params.aspiration_window.max(1);
    reduction -=
        inputs.window_width.saturating_sub(1).clamp(0, window_span) * (LMR_UNIT / 2) / window_span;
    reduction.max(0)
}

/// 1/1024段の縮小量を段へ切り捨て、縮小後の子の深さが1を下回らないよう`depth-2`で抑える。
pub(super) fn reduction_plies(reduction: i32, depth: u32) -> i32 {
    (reduction / LMR_UNIT).clamp(0, (depth as i32 - 2).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reductions_are_monotonic_in_depth_and_move_index() {
        let table = LmrReductions::new(236);
        for depth in 2..DEPTH_SIZE as u32 {
            for index in 2..MOVE_INDEX_SIZE {
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
        assert_eq!(table.base(1_000, 10_000), table.base(63, MOVE_INDEX_SIZE - 1));
        assert_eq!(table.base(63, 63), (f64::ln(63.0) * f64::ln(63.0) * 100.0 / 236.0) as i32);
    }

    #[test]
    fn high_branching_nodes_keep_increasing_reductions_after_move_63() {
        let table = LmrReductions::new(236);
        assert!(table.base_fine(12, 256) > table.base_fine(12, 63));
        for depth in 1..64 {
            for index in 1..64 {
                let expected = ((f64::from(depth).ln() * (index as f64).ln() * 100.0)
                    * f64::from(LMR_UNIT)
                    / 236.0)
                    .floor() as i32;
                assert_eq!(table.base_fine(depth, index), expected);
            }
        }
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
    fn lmr_adjustments_shift_the_base_in_fine_units() {
        let params = SearchParams {
            lmr_history_divisor: 8_192,
            lmr_history_clamp: 1,
            ..SearchParams::default()
        };
        let base = 2 * LMR_UNIT;
        let neutral = LmrInputs { improving: true, ..LmrInputs::default() };
        assert_eq!(lmr_reduction(base, neutral, &params), base);
        assert_eq!(
            lmr_reduction(base, LmrInputs { tt_pv: true, ..neutral }, &params),
            base - params.lmr_tt_pv
        );
        assert_eq!(
            lmr_reduction(base, LmrInputs { improving: false, ..neutral }, &params),
            base + params.lmr_not_improving
        );
        assert_eq!(
            lmr_reduction(base, LmrInputs { cut_node: true, ..neutral }, &params),
            base + params.lmr_cut_node
        );
        assert_eq!(
            lmr_reduction(base, LmrInputs { child_cutoffs: 1, ..neutral }, &params),
            base,
            "βカット1回では補正しない"
        );
        assert_eq!(
            lmr_reduction(base, LmrInputs { history_score: 4_096, ..neutral }, &params),
            base - LMR_UNIT / 2,
            "履歴は段より細かく効く"
        );
        assert_eq!(
            lmr_reduction(base, LmrInputs { history_score: -16_384, ..neutral }, &params),
            base + LMR_UNIT,
            "履歴の補正は±1段でclampされる"
        );
        assert_eq!(
            lmr_reduction(0, LmrInputs { history_score: 16_384, ..neutral }, &params),
            0,
            "負の補正でも0未満にはならない"
        );
    }

    #[test]
    fn a_large_evaluation_correction_reduces_lmr_by_at_most_one_ply() {
        let params = SearchParams::default();
        let neutral = LmrInputs { improving: true, ..LmrInputs::default() };
        let base = 3 * LMR_UNIT;
        assert_eq!(lmr_reduction(base, neutral, &params), base);
        assert_eq!(
            lmr_reduction(base, LmrInputs { correction_abs: 64, ..neutral }, &params),
            base - LMR_UNIT / 2
        );
        assert_eq!(
            lmr_reduction(base, LmrInputs { correction_abs: 1000, ..neutral }, &params),
            base - LMR_UNIT
        );
    }

    #[test]
    fn a_scout_window_does_not_change_lmr_and_wide_windows_get_at_most_half_a_ply() {
        let params = SearchParams { aspiration_window: 100, ..SearchParams::default() };
        let neutral = LmrInputs { improving: true, ..LmrInputs::default() };
        let base = 3 * LMR_UNIT;
        for width in [0, 1] {
            assert_eq!(
                lmr_reduction(base, LmrInputs { window_width: width, ..neutral }, &params),
                base
            );
        }
        for width in [101, 10_000] {
            assert_eq!(
                lmr_reduction(base, LmrInputs { window_width: width, ..neutral }, &params),
                base - LMR_UNIT / 2
            );
        }
    }

    #[test]
    fn reduction_plies_floor_and_clamp_into_the_valid_depth_window() {
        assert_eq!(reduction_plies(LMR_UNIT * 3 - 1, 8), 2);
        assert_eq!(reduction_plies(LMR_UNIT * 10, 6), 4, "depth-2でclampされる");
        assert_eq!(reduction_plies(LMR_UNIT * 3, 2), 0, "depth 2では縮小しない");
    }
}

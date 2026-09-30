//! 主探索の枝刈り・延長・縮小を試すかどうかの判定。
//!
//! どれも局面を動かさない純粋な関数で、適用と再探索は`negamax`側が行う。

use crate::params::SearchParams;
use crate::tt::{Bound, TtEntry};

use super::{CaptureSee, MATE_TT_THRESHOLD};

/// IIR: TT moveが無く十分深いノードは1浅く探索する。
pub(super) const fn iir_depth(depth: u32, has_tt_move: bool, iir_min_depth: i32) -> u32 {
    if !has_tt_move && depth as i32 >= iir_min_depth { depth - 1 } else { depth }
}

/// Reverse futility: 静的評価がβをdepth比例のmargin以上上回る浅いノードは
/// 探索せず評価値で打ち切る。improvingなら`rfp_improving_depth`分marginを緩和する。
pub(super) fn should_reverse_futility_prune(
    depth: u32,
    pv_node: bool,
    improving: bool,
    static_eval: i32,
    beta: i32,
    params: &SearchParams,
) -> bool {
    let relaxed = u32::from(improving) * params.rfp_improving_depth.max(0) as u32;
    let margin_depth = depth.saturating_sub(relaxed);
    depth as i32 <= params.rfp_max_depth
        && !pv_node
        && static_eval - params.reverse_futility_margin * margin_depth as i32 >= beta
}

/// Futility: 静的評価がαへdepth比例のmargin分届かない浅いノードでは、
/// 最初の手を除く静かな非王手手を刈る(適用はムーブループ側)。
pub(super) fn should_futility_prune_quiets(
    depth: u32,
    in_check: bool,
    static_eval: Option<i32>,
    alpha: i32,
    params: &SearchParams,
) -> bool {
    depth as i32 <= params.futility_max_depth
        && !in_check
        && static_eval.is_some_and(|eval| eval + params.futility_margin * depth as i32 <= alpha)
}

/// SEE枝刈り: 浅いnon-PVノードでは、交換値が`-margin * depth^2`を下回る捕獲を読まない。
///
/// `see`はMovePickerが取り出し時に計算済みの値で、捕獲以外は`None`。
/// 王手中は回避手を減らせないので発火しない。
pub(super) fn should_prune_capture_by_see(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    see: CaptureSee,
    params: &SearchParams,
) -> bool {
    if pv_node || in_check || depth as i32 > params.see_prune_max_depth {
        return false;
    }
    let threshold = -params.see_prune_margin * (depth * depth) as i32;
    see.is_some_and(|exchange| exchange < threshold)
}

/// history枝刈り: 浅いnon-PVノードでは、履歴が`-margin * depth`を下回る静かな手を読まない。
///
/// 履歴は±`HISTORY_MAX`(16_384)へ飽和するので、閾値は「繰り返し失敗した手」だけを捉える。
pub(super) fn should_prune_by_history(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    history_score: i32,
    params: &SearchParams,
) -> bool {
    !pv_node
        && !in_check
        && depth as i32 <= params.history_prune_max_depth
        && history_score < -params.history_prune_margin * depth as i32
}

/// LMPの上限手数 `(lmp_base + depth^2) / (2 - improving)`。improvingなら倍許す。
pub(super) fn late_move_prune_limit(depth: u32, improving: bool, params: &SearchParams) -> usize {
    ((params.lmp_base.max(0) as u32 + depth * depth) / (2 - u32::from(improving))) as usize
}

/// 静かな打ち駒のLMP上限。盤上の静かな手の上限を`drop_lmp_divisor`で割り、最低1手は残す。
///
/// 除数が0なら「打ち駒を刈らない」を表し、上限なし(`None`)を返す。
pub(super) fn late_move_prune_drop_limit(
    depth: u32,
    improving: bool,
    params: &SearchParams,
) -> Option<usize> {
    let divisor = params.drop_lmp_divisor;
    if divisor <= 0 {
        return None;
    }
    Some((late_move_prune_limit(depth, improving, params) / divisor as usize).max(1))
}

pub(super) fn should_prune_late_quiet_drop(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    improving: bool,
    eligible_drops_seen: usize,
    params: &SearchParams,
) -> bool {
    !pv_node
        && !in_check
        && late_move_prune_drop_limit(depth, improving, params)
            .is_some_and(|limit| eligible_drops_seen >= limit)
}

/// LMPの対象になる盤上の静かな手のうち、上限を超えた手を刈るか。
pub(super) fn should_prune_late_quiet(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    improving: bool,
    eligible_quiets_seen: usize,
    params: &SearchParams,
) -> bool {
    !pv_node && !in_check && eligible_quiets_seen >= late_move_prune_limit(depth, improving, params)
}

pub(super) fn should_try_null_move(
    zero_window: bool,
    in_check: bool,
    static_eval: i32,
    beta: i32,
    plies_from_null: u16,
) -> bool {
    zero_window
        && !in_check
        && beta > -MATE_TT_THRESHOLD
        && plies_from_null > 0
        && static_eval >= beta
}

/// null moveの動的縮小量 `R = base + depth/depth_divisor + min((static_eval - beta) / eval_divisor, 3)`。
///
/// βを大きく上回るほど深く縮小する。前提により`static_eval >= beta`だが、
/// 負側もclampして守る。
pub(super) fn null_move_reduction_amount(
    depth: u32,
    static_eval: i32,
    beta: i32,
    params: &SearchParams,
) -> u32 {
    let eval_term =
        ((static_eval - beta) / params.null_move_eval_divisor.max(1)).clamp(0, 3) as u32;
    params.null_move_base_reduction.max(0) as u32
        + depth / params.null_move_depth_divisor.max(1) as u32
        + eval_term
}

/// singular extensionの判定を試すnodeか。
///
/// 基準にするTTの値は下限(Lower/Exact)で、現在のdepthに近い探索から得たものに限る。
/// 詰みの値は延長しなくても結論が変わらないので除く。
/// 延長の連鎖で探索が伸び続けないよう、plyがiterationのdepthの2倍に達したら試さない。
pub(super) fn should_try_singular(
    depth: u32,
    ply: u32,
    root_depth: u32,
    entry: &TtEntry,
    min_depth: i32,
) -> bool {
    depth as i32 >= min_depth
        && ply > 0
        && ply < root_depth.saturating_mul(2)
        && matches!(entry.bound, Bound::Lower | Bound::Exact)
        && entry.depth + 3 >= depth
        && entry.score.abs() < MATE_TT_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::super::MATE;
    use super::*;

    #[test]
    fn null_move_guard_requires_every_conservative_precondition() {
        assert!(should_try_null_move(true, false, 20, 20, 1));
        assert!(!should_try_null_move(false, false, 20, 20, 1));
        assert!(!should_try_null_move(true, true, 20, 20, 1));
        assert!(!should_try_null_move(true, false, 19, 20, 1));
        assert!(!should_try_null_move(true, false, 20, 20, 0));
        assert!(!should_try_null_move(true, false, 0, -MATE_TT_THRESHOLD, 1));
        assert!(should_try_null_move(true, false, 0, -MATE_TT_THRESHOLD + 1, 1));
    }

    #[test]
    fn null_move_reduction_grows_with_depth_and_eval_margin() {
        // R = 3 + depth/3 + min((eval-beta)/200, 3)
        let params = SearchParams {
            null_move_eval_divisor: 200,
            null_move_base_reduction: 3,
            null_move_depth_divisor: 3,
            ..SearchParams::default()
        };
        let zero_divisor = SearchParams { null_move_eval_divisor: 0, ..params };
        assert_eq!(null_move_reduction_amount(4, 0, 0, &params), 4);
        assert_eq!(null_move_reduction_amount(12, 0, 0, &params), 7);
        assert_eq!(null_move_reduction_amount(12, 700, 0, &params), 10);
        assert_eq!(null_move_reduction_amount(12, 5_000, 0, &params), 10, "eval項は3でclampされる");
        assert_eq!(
            null_move_reduction_amount(6, -100, 0, &params),
            5,
            "負のeval差は0へclampされる"
        );
        assert_eq!(
            null_move_reduction_amount(6, 100, 0, &zero_divisor),
            8,
            "divisor 0は1へ丸めて0除算を避ける"
        );
    }

    #[test]
    fn see_pruning_skips_only_badly_losing_captures_in_shallow_non_pv_nodes() {
        let params = SearchParams {
            see_prune_margin: 80,
            see_prune_max_depth: 6,
            ..SearchParams::default()
        };
        // 閾値は -margin * depth^2。depth 2なら-320。
        assert!(should_prune_capture_by_see(2, false, false, Some(-321), &params));
        assert!(!should_prune_capture_by_see(2, false, false, Some(-320), &params));
        assert!(
            !should_prune_capture_by_see(2, true, false, Some(-10_000), &params),
            "PVノードでは刈らない"
        );
        assert!(
            !should_prune_capture_by_see(2, false, true, Some(-10_000), &params),
            "王手中は刈らない"
        );
        assert!(
            !should_prune_capture_by_see(7, false, false, Some(-10_000), &params),
            "depth 7では発火しない"
        );
        assert!(
            !should_prune_capture_by_see(2, false, false, None, &params),
            "SEEの無い手(捕獲以外)は対象外"
        );
    }

    #[test]
    fn history_pruning_requires_a_strongly_negative_history_in_a_shallow_non_pv_node() {
        let params = SearchParams {
            history_prune_margin: 2_000,
            history_prune_max_depth: 4,
            ..SearchParams::default()
        };
        assert!(should_prune_by_history(1, false, false, -2_001, &params));
        assert!(!should_prune_by_history(1, false, false, -2_000, &params));
        assert!(should_prune_by_history(4, false, false, -8_001, &params));
        assert!(!should_prune_by_history(4, false, false, -8_000, &params));
        assert!(
            !should_prune_by_history(5, false, false, -100_000, &params),
            "depth 5では発火しない"
        );
        assert!(
            !should_prune_by_history(1, true, false, -100_000, &params),
            "PVノードでは刈らない"
        );
        assert!(!should_prune_by_history(1, false, true, -100_000, &params), "王手中は刈らない");
        assert!(!should_prune_by_history(1, false, false, 0, &params), "履歴の無い手は刈らない");
    }

    #[test]
    fn lmp_limit_grows_quadratically_and_doubles_when_improving() {
        let base3 = SearchParams { lmp_base: 3, ..SearchParams::default() };
        // (3 + d^2) / (2 - improving)
        assert_eq!(late_move_prune_limit(1, false, &base3), 2);
        assert_eq!(late_move_prune_limit(1, true, &base3), 4);
        assert_eq!(late_move_prune_limit(3, false, &base3), 6);
        assert_eq!(late_move_prune_limit(4, false, &base3), 9);
        assert_eq!(late_move_prune_limit(4, true, &base3), 19);
        assert_eq!(late_move_prune_limit(8, false, &base3), 33, "深いノードにも上限がかかる");
    }

    #[test]
    fn quiet_drops_get_a_tighter_lmp_limit_than_board_quiets() {
        let base3 = SearchParams { lmp_base: 3, ..SearchParams::default() };
        let drop0 = SearchParams { lmp_base: 3, drop_lmp_divisor: 0, ..SearchParams::default() };
        let drop2 = SearchParams { lmp_base: 3, drop_lmp_divisor: 2, ..SearchParams::default() };
        let drop6 = SearchParams { lmp_base: 3, drop_lmp_divisor: 6, ..SearchParams::default() };
        // 盤上の静かな手の上限をdivisorで割り、最低1手は残す。
        assert_eq!(late_move_prune_drop_limit(1, false, &drop2), Some(1));
        assert_eq!(late_move_prune_drop_limit(4, false, &drop2), Some(4));
        assert_eq!(late_move_prune_drop_limit(4, true, &drop2), Some(9));
        assert_eq!(late_move_prune_drop_limit(8, false, &drop2), Some(16));
        assert_eq!(late_move_prune_drop_limit(1, false, &drop6), Some(1), "上限は1手を下回らない");
        assert_eq!(
            late_move_prune_drop_limit(4, false, &drop0),
            None,
            "divisor 0は打ち駒を刈らない"
        );
        assert!(
            late_move_prune_drop_limit(6, false, &drop2).expect("有効なdivisor")
                < late_move_prune_limit(6, false, &base3),
            "打ち駒の上限は盤上の静かな手より厳しい"
        );
    }

    #[test]
    fn quiet_drop_pruning_keeps_the_pv_and_check_evasion_paths() {
        let drop0 = SearchParams { lmp_base: 3, drop_lmp_divisor: 0, ..SearchParams::default() };
        let drop2 = SearchParams { lmp_base: 3, drop_lmp_divisor: 2, ..SearchParams::default() };
        assert!(should_prune_late_quiet_drop(4, false, false, false, 4, &drop2));
        assert!(!should_prune_late_quiet_drop(4, false, false, false, 3, &drop2));
        assert!(
            !should_prune_late_quiet_drop(4, true, false, false, 99, &drop2),
            "PVノードでは刈らない"
        );
        assert!(
            !should_prune_late_quiet_drop(4, false, true, false, 99, &drop2),
            "王手中は刈らない"
        );
        assert!(
            !should_prune_late_quiet_drop(4, false, false, false, 99, &drop0),
            "divisor 0は打ち駒枝刈りを無効化する"
        );
    }

    #[test]
    fn lmp_requires_a_non_pv_unchecked_eligible_late_quiet() {
        let base3 = SearchParams { lmp_base: 3, ..SearchParams::default() };
        assert!(should_prune_late_quiet(1, false, false, false, 2, &base3));
        assert!(!should_prune_late_quiet(1, false, false, false, 1, &base3));
        assert!(!should_prune_late_quiet(1, true, false, false, 2, &base3), "PVノードでは刈らない");
        assert!(!should_prune_late_quiet(1, false, true, false, 2, &base3), "王手中は刈らない");
        assert!(!should_prune_late_quiet(1, false, false, true, 2, &base3), "improvingは上限が倍");
        assert!(should_prune_late_quiet(1, false, false, true, 4, &base3));
    }

    #[test]
    fn reverse_futility_covers_depth_8_and_relaxes_when_improving() {
        let params = SearchParams {
            reverse_futility_margin: 100,
            rfp_max_depth: 8,
            rfp_improving_depth: 1,
            ..SearchParams::default()
        };
        assert!(should_reverse_futility_prune(8, false, false, 800, 0, &params));
        assert!(!should_reverse_futility_prune(8, false, false, 799, 0, &params));
        assert!(
            !should_reverse_futility_prune(9, false, false, 10_000, 0, &params),
            "depth 9では発火しない"
        );
        assert!(
            !should_reverse_futility_prune(8, true, false, 10_000, 0, &params),
            "PVノードでは発火しない"
        );
        assert!(
            should_reverse_futility_prune(8, false, true, 700, 0, &params),
            "improvingはmarginが1 depth分緩む"
        );
        assert!(!should_reverse_futility_prune(8, false, true, 699, 0, &params));
    }

    #[test]
    fn futility_covers_depth_4_with_a_depth_scaled_margin() {
        let params =
            SearchParams { futility_margin: 100, futility_max_depth: 4, ..SearchParams::default() };
        assert!(should_futility_prune_quiets(4, false, Some(-400), 0, &params));
        assert!(!should_futility_prune_quiets(4, false, Some(-399), 0, &params));
        assert!(
            !should_futility_prune_quiets(5, false, Some(-10_000), 0, &params),
            "depth 5では発火しない"
        );
        assert!(
            !should_futility_prune_quiets(4, true, Some(-400), 0, &params),
            "王手中は発火しない"
        );
        assert!(!should_futility_prune_quiets(4, false, None, 0, &params));
        assert!(should_futility_prune_quiets(1, false, Some(-100), 0, &params));
    }

    #[test]
    fn singular_needs_a_near_lower_bound_away_from_the_mate_band() {
        let entry = TtEntry {
            key: 1,
            depth: 7,
            score: 100,
            bound: Bound::Lower,
            best_move: None,
            static_eval: None,
        };
        assert!(should_try_singular(10, 3, 12, &entry, 7));
        assert!(!should_try_singular(6, 3, 12, &entry, 7), "浅いnodeでは試さない");
        assert!(!should_try_singular(10, 0, 12, &entry, 7), "rootでは試さない");
        assert!(
            !should_try_singular(10, 24, 12, &entry, 7),
            "iterationのdepthの2倍より深いplyでは試さない"
        );
        assert!(!should_try_singular(11, 3, 12, &entry, 7), "基準のdepthが遠い");
        let upper = TtEntry { bound: Bound::Upper, ..entry };
        assert!(!should_try_singular(10, 3, 12, &upper, 7), "上限の値は基準にしない");
        let mate = TtEntry { score: MATE - 10, ..entry };
        assert!(!should_try_singular(10, 3, 12, &mate, 7), "詰みの値は延長しない");
    }

    #[test]
    fn iir_reduces_only_deep_nodes_without_a_tt_move() {
        assert_eq!(iir_depth(4, false, 4), 3);
        assert_eq!(iir_depth(4, true, 4), 4);
        assert_eq!(iir_depth(3, false, 4), 3, "depth 3以下はそのまま");
        assert_eq!(iir_depth(12, false, 4), 11);
    }
}

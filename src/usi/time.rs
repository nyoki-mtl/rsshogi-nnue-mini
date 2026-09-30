//! `go`の時間指定を探索の予算と締切へ換算する。

use std::sync::Arc;
use std::time::Duration;

use rsshogi::board::Position;
use rsshogi::types::Color;
use rsshogi_usi::GoParams;

use crate::params::SearchParams;
use crate::position::MAX_SEARCH_DEPTH;
use crate::search::{SearchDeadline, SearchLimits};

const MAX_TIME_BUDGET: Duration = Duration::from_secs(365 * 24 * 60 * 60);
/// GUIとの往復や秒読み判定のずれへ残す猶予の既定値。
///
/// 中継対局で余裕が足りないと即座に時間切れ負けになる一方、余裕を多く取る損失は思考時間の数%に留まる。
/// 非対称なので既定は安全側に置き、遅延のない環境では運用者が下げる。
pub(super) const DEFAULT_MOVE_OVERHEAD_MS: u64 = 500;
pub(super) const MAX_MOVE_OVERHEAD_MS: u64 = 5_000;
/// 予算のうち、安全余裕として引ける最大の割合の逆数。
///
/// 全額引くと短い秒読みや残り時間僅少の終盤で予算が潰れるため、思考時間は必ず予算の半分を残す。
const MOVE_OVERHEAD_BUDGET_DIVISOR: u64 = 2;
/// 一手で使ってよい残り時間の割合の逆数。延長で持ち時間を使い込まないための上限。
const MAX_REMAINING_SHARE_DIVISOR: u64 = 4;

pub(super) fn limits_from_go(
    position: &Position,
    params: &GoParams,
    overhead_ms: u64,
    search: &SearchParams,
) -> SearchLimits {
    let has_resource_limit = params.nodes.is_some()
        || params.movetime.is_some()
        || params.btime.is_some()
        || params.wtime.is_some()
        || params.binc.is_some()
        || params.winc.is_some()
        || params.byoyomi.is_some();
    let max_depth = params
        .depth
        .unwrap_or(if params.infinite || params.ponder || has_resource_limit {
            MAX_SEARCH_DEPTH
        } else {
            4
        })
        .clamp(1, MAX_SEARCH_DEPTH);

    let to_duration = |milliseconds: u64| Duration::from_millis(milliseconds).min(MAX_TIME_BUDGET);
    let allocation_ms =
        if params.infinite { None } else { time_budget_ms(position, params, overhead_ms) };
    // 使い切ってよい予算をsoft stopで削らず、延長もしない。
    // `movetime`はその時間を使えという指示であり、持ち時間のない秒読みは残しても次の手へ回せない。
    let spendable = is_spendable_budget(position, params);
    let deadline = allocation_ms.map(|allocation_ms| {
        let budget = to_duration(allocation_ms);
        let (soft, hard) = if spendable {
            (budget, budget)
        } else {
            let maximum = maximum_budget_ms(position, params, allocation_ms, overhead_ms, search)
                .map_or(budget, to_duration)
                .max(budget);
            // この割合を過ぎたら新しいiterationを始めない。探索側が局面の安定度で伸縮する。
            let soft_fraction = f64::from(search.time_soft_permille) / 1_000.0;
            (budget.mul_f64(soft_fraction), maximum)
        };
        Arc::new(SearchDeadline::new(Some(hard), Some(soft)))
    });

    SearchLimits {
        max_depth,
        max_nodes: params.nodes.map(|nodes| nodes.max(1)),
        deadline,
        searchmoves: params.searchmoves.clone(),
        max_moves_to_draw: 0,
    }
}

/// 手番側の残り時間と増秒。
fn own_clock(position: &Position, params: &GoParams) -> (Option<u64>, u64) {
    match position.turn() {
        Color::BLACK => (params.btime, params.binc.unwrap_or(0)),
        Color::WHITE => (params.wtime, params.winc.unwrap_or(0)),
    }
}

/// この`go`の予算を残しても次の手へ回せないかどうか。
///
/// `movetime`はその時間を使えという指示で、持ち時間のない秒読みは各手で使い切りになる。
/// どちらもiterationを一つ余分に試したほうがよく、soft stopで削る意味がない。
fn is_spendable_budget(position: &Position, params: &GoParams) -> bool {
    let (remaining, _) = own_clock(position, params);
    params.movetime.is_some()
        || (remaining.is_none_or(|remaining| remaining == 0)
            && params.byoyomi.is_some_and(|byoyomi| byoyomi > 0))
}

/// 予算からGUI・通信の遅延分を引く。
///
/// 短い予算で余裕を引きすぎないよう、引く量は予算の一定割合を超えない。
fn apply_move_overhead(budget: u64, overhead_ms: u64) -> u64 {
    let reserve = overhead_ms.min(budget / MOVE_OVERHEAD_BUDGET_DIVISOR);
    budget.saturating_sub(reserve).max(1)
}

fn time_budget_ms(position: &Position, params: &GoParams, overhead_ms: u64) -> Option<u64> {
    if let Some(movetime) = params.movetime {
        return Some(apply_move_overhead(movetime, overhead_ms));
    }

    let (remaining, increment) = own_clock(position, params);
    let byoyomi = params.byoyomi.unwrap_or(0);

    match remaining {
        Some(remaining) => {
            let moves = params.effective_movestogo().unwrap_or(30).max(1);
            let budget = remaining
                .checked_div(moves)
                .unwrap_or(remaining)
                .saturating_add(increment)
                .saturating_add(byoyomi);
            Some(apply_move_overhead(budget.min(spendable_now(remaining, byoyomi)), overhead_ms))
        }
        // 残り時間が来ていない`go`は銀行の残高が不明であり、0とは違う。
        // ここでclampすると秒読み・増秒だけの対局が毎手の即指しになる。
        None if byoyomi > 0 || increment > 0 => {
            Some(apply_move_overhead(byoyomi.saturating_add(increment), overhead_ms))
        }
        None => None,
    }
}

/// 最善手が揺れる局面で延長してよいhard stopの位置。持ち時間がある場合だけ配分より長くなる。
fn maximum_budget_ms(
    position: &Position,
    params: &GoParams,
    allocation_ms: u64,
    overhead_ms: u64,
    search: &SearchParams,
) -> Option<u64> {
    let remaining = own_clock(position, params).0?;
    let byoyomi = params.byoyomi.unwrap_or(0);
    let share = (remaining / MAX_REMAINING_SHARE_DIVISOR)
        .saturating_add(byoyomi)
        .min(spendable_now(remaining, byoyomi));
    let share = apply_move_overhead(share, overhead_ms);
    // 最善手が揺れる局面で、配分の何倍まで使ってよいか。
    let ratio_percent = u64::try_from(search.time_max_ratio_percent).unwrap_or(100);
    let extended = allocation_ms.saturating_mul(ratio_percent) / 100;
    Some(extended.min(share).max(allocation_ms))
}

/// この一手で使い切れる上限。
///
/// 増秒は着手を終えてから加算されるため、着手前の残高には数えない。
/// `btime 0`・`binc 2000`で2秒使えると誤ると、実際には0秒しかなく時間切れ負けになる。
fn spendable_now(remaining: u64, byoyomi: u64) -> u64 {
    remaining.saturating_add(byoyomi)
}

#[cfg(test)]
mod tests {
    use rsshogi::board;

    use super::*;

    fn limits(position: &Position, params: &GoParams) -> SearchLimits {
        limits_from_go(position, params, DEFAULT_MOVE_OVERHEAD_MS, &SearchParams::default())
    }

    fn budget_ms(params: &GoParams) -> Option<u64> {
        time_budget_ms(&board::hirate_position(), params, DEFAULT_MOVE_OVERHEAD_MS)
    }

    fn white_to_move() -> Position {
        board::position_from_sfen(
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL w - 2",
        )
        .expect("a valid SFEN with white to move")
    }

    #[test]
    fn depth_only_has_no_deadline() {
        let position = board::hirate_position();
        let params = GoParams { depth: Some(3), ..GoParams::default() };
        let limits = limits(&position, &params);
        assert_eq!(limits.max_depth, 3);
        assert!(limits.deadline.is_none());
    }

    #[test]
    fn go_depth_is_clamped_to_the_finite_search_bound() {
        let position = board::hirate_position();
        let params = GoParams { depth: Some(u32::MAX), ..GoParams::default() };
        let limits = limits(&position, &params);
        assert_eq!(limits.max_depth, MAX_SEARCH_DEPTH);
    }

    #[test]
    fn zero_node_request_keeps_the_mandatory_fallback_node() {
        let position = board::hirate_position();
        let params = GoParams { nodes: Some(0), ..GoParams::default() };
        let limits = limits(&position, &params);
        assert_eq!(limits.max_nodes, Some(1));
    }

    #[test]
    fn movetime_creates_a_deadline() {
        let position = board::hirate_position();
        let params = GoParams { movetime: Some(100), ..GoParams::default() };
        let limits = limits(&position, &params);
        assert!(limits.deadline.is_some());
    }

    #[test]
    fn extreme_time_values_are_saturated_before_building_a_deadline() {
        let position = board::hirate_position();
        let movetime = GoParams { movetime: Some(u64::MAX), ..GoParams::default() };
        assert!(limits(&position, &movetime).deadline.is_some());

        let combined = GoParams {
            btime: Some(u64::MAX),
            binc: Some(u64::MAX),
            byoyomi: Some(u64::MAX),
            ..GoParams::default()
        };
        assert_eq!(budget_ms(&combined), Some(u64::MAX - DEFAULT_MOVE_OVERHEAD_MS));
        assert!(limits(&position, &combined).deadline.is_some());
    }

    #[test]
    fn the_move_overhead_never_consumes_a_short_budget() {
        assert_eq!(apply_move_overhead(10_000, 500), 9_500);
        assert_eq!(apply_move_overhead(1_000, 500), 500);
        // 予算の半分を超えて引かないので、短い予算でも思考時間が残る。
        assert_eq!(apply_move_overhead(800, 500), 400);
        assert_eq!(apply_move_overhead(100, 500), 50);
        assert_eq!(apply_move_overhead(1, 500), 1);
        assert_eq!(apply_move_overhead(0, 500), 1);
        // 遅延のない環境では控除そのものを無効にできる。
        assert_eq!(apply_move_overhead(10_000, 0), 10_000);
    }

    #[test]
    fn byoyomi_keeps_a_latency_reserve() {
        let params = GoParams { byoyomi: Some(10_000), ..GoParams::default() };
        assert_eq!(budget_ms(&params), Some(9_500));
    }

    #[test]
    fn the_budget_never_exceeds_what_this_move_can_spend() {
        // 増秒は着手後に加算されるため、残り時間0で増秒だけがある局面は即指しにする。
        let increment_only = GoParams { btime: Some(0), binc: Some(2_000), ..GoParams::default() };
        assert_eq!(budget_ms(&increment_only), Some(1));

        // 残り時間があれば、秒読みと合わせた上限までは使ってよい。
        let with_byoyomi = GoParams {
            btime: Some(1_000),
            wtime: Some(1_000),
            byoyomi: Some(10_000),
            ..GoParams::default()
        };
        assert_eq!(budget_ms(&with_byoyomi), Some(9_533));

        // 残り時間が来ていない`go`は残高不明であり、0ではない。clampしない。
        let byoyomi_only = GoParams { byoyomi: Some(1_000), ..GoParams::default() };
        assert_eq!(budget_ms(&byoyomi_only), Some(500));

        // 配分が残高を上回るときは、floorではなく残高そのもので頭打ちになる。
        let short_of_increment =
            GoParams { btime: Some(100), binc: Some(5_000), ..GoParams::default() };
        assert_eq!(budget_ms(&short_of_increment), Some(50));
    }

    #[test]
    fn the_budget_follows_the_side_to_move() {
        let params = GoParams {
            btime: Some(60_000),
            wtime: Some(6_000),
            binc: Some(10_000),
            winc: Some(0),
            ..GoParams::default()
        };

        // 先手番は自分のbtimeとbincだけを見る。60000/30 + 10000 - 500。
        assert_eq!(
            time_budget_ms(&board::hirate_position(), &params, DEFAULT_MOVE_OVERHEAD_MS),
            Some(11_500)
        );
        // 後手番はwtimeとwincだけを見る。6000/30 - 500は半分までしか引けない。
        assert_eq!(time_budget_ms(&white_to_move(), &params, DEFAULT_MOVE_OVERHEAD_MS), Some(100));
    }

    #[test]
    fn the_last_move_of_a_period_keeps_the_overhead() {
        let params = GoParams {
            btime: Some(5_000),
            wtime: Some(5_000),
            movestogo: Some(1),
            ..GoParams::default()
        };
        assert_eq!(budget_ms(&params), Some(4_500));
    }

    #[test]
    fn a_main_time_search_gets_a_soft_deadline_before_the_hard_one() {
        let position = board::hirate_position();
        let params = GoParams { btime: Some(60_000), wtime: Some(60_000), ..GoParams::default() };
        let limits = limits(&position, &params);
        let deadline = limits.deadline.expect("timed search must have a deadline");
        assert!(
            deadline.soft_budget() < deadline.hard_budget(),
            "the soft deadline must come first"
        );
    }

    #[test]
    fn a_spendable_budget_is_not_cut_short_by_the_soft_deadline() {
        let position = board::hirate_position();
        let budgets = |params: &GoParams| {
            let deadline = limits(&position, params).deadline.expect("a timed search");
            (deadline.soft_budget(), deadline.hard_budget())
        };

        // `movetime`はその時間を使えという指示なので、途中で打ち切らない。
        let movetime = GoParams { movetime: Some(10_000), ..GoParams::default() };
        let (soft, hard) = budgets(&movetime);
        assert_eq!(soft, hard);

        // 持ち時間のない秒読みも各手で使い切りなので、残しても次へ回せない。
        let byoyomi = GoParams { byoyomi: Some(10_000), ..GoParams::default() };
        let (soft, hard) = budgets(&byoyomi);
        assert_eq!(soft, hard);

        // GUIが0の持ち時間を明示しても、手番側は純秒読みなので使い切る。
        let explicit_zero = GoParams {
            btime: Some(0),
            wtime: Some(0),
            byoyomi: Some(10_000),
            ..GoParams::default()
        };
        let (soft, hard) = budgets(&explicit_zero);
        assert_eq!(soft, hard);

        // 相手側に持ち時間が残っていても、手番側が0なら純秒読みとして扱う。
        let only_opponent_has_main_time = GoParams {
            btime: Some(0),
            wtime: Some(60_000),
            byoyomi: Some(10_000),
            ..GoParams::default()
        };
        let (soft, hard) = budgets(&only_opponent_has_main_time);
        assert_eq!(soft, hard);

        // 持ち時間があれば秒読みが付いていても温存する価値がある。
        let with_main_time = GoParams {
            btime: Some(60_000),
            wtime: Some(60_000),
            byoyomi: Some(10_000),
            ..GoParams::default()
        };
        let (soft, hard) = budgets(&with_main_time);
        assert!(soft < hard);
    }

    #[test]
    fn main_time_allows_extending_up_to_a_share_of_the_remaining_time() {
        let position = board::hirate_position();
        let hard_ms = |params: &GoParams| {
            let deadline = limits_from_go(&position, params, 100, &SearchParams::default())
                .deadline
                .expect("a timed search");
            deadline.hard_budget().expect("a hard budget").as_millis()
        };

        // 20秒+0.2秒の初手: 配分20000/30+200-100=766、延長は3倍の2298まで。
        let fischer = GoParams {
            btime: Some(20_000),
            wtime: Some(20_000),
            binc: Some(200),
            winc: Some(200),
            ..GoParams::default()
        };
        assert_eq!(hard_ms(&fischer), 2_298);

        // 秒読みで配分が大きいときは、残りの1/4と秒読みの和が延長の上限になる。
        // 配分4000/30+1000-100=1033、3倍は3099、上限は1000+1000-100=1900。
        let with_byoyomi = GoParams {
            btime: Some(4_000),
            wtime: Some(4_000),
            byoyomi: Some(1_000),
            ..GoParams::default()
        };
        assert_eq!(hard_ms(&with_byoyomi), 1_900);

        // 延長の上限は配分を下回らない。
        let tiny = GoParams {
            btime: Some(100),
            wtime: Some(100),
            binc: Some(5_000),
            ..GoParams::default()
        };
        assert_eq!(hard_ms(&tiny), 50);
    }

    #[test]
    fn an_infinite_search_has_neither_deadline() {
        let position = board::hirate_position();
        let params = GoParams { infinite: true, ..GoParams::default() };
        let limits = limits(&position, &params);
        assert!(limits.deadline.is_none());
    }

    #[test]
    fn increment_only_time_control_is_resource_limited() {
        let position = board::hirate_position();
        let params = GoParams { binc: Some(100), winc: Some(100), ..GoParams::default() };
        let limits = limits(&position, &params);
        assert_eq!(limits.max_depth, MAX_SEARCH_DEPTH);
        assert!(limits.deadline.is_some());
    }
}

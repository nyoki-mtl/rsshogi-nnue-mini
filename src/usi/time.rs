//! `go`の時間指定を探索の予算と締切へ換算する。

use std::sync::Arc;
use std::time::Duration;

use rsshogi::board::Position;
use rsshogi::types::Color;
use rsshogi_usi::GoParams;

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
/// この割合を過ぎたら新しいiterationを始めない。
const SOFT_DEADLINE_FRACTION: f64 = 0.6;

pub(super) fn limits_from_go(
    position: &Position,
    params: &GoParams,
    overhead_ms: u64,
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

    let budget = if params.infinite {
        None
    } else {
        time_budget_ms(position, params, overhead_ms)
            .map(|milliseconds| Duration::from_millis(milliseconds).min(MAX_TIME_BUDGET))
    };
    // 使い切ってよい予算をsoft stopで削らない。
    // `movetime`はその時間を使えという指示であり、持ち時間のない秒読みは残しても次の手へ回せない。
    let soft_fraction =
        if is_spendable_budget(position, params) { 1.0 } else { SOFT_DEADLINE_FRACTION };
    let soft_budget = budget.map(|budget| budget.mul_f64(soft_fraction));
    let deadline = budget.map(|budget| Arc::new(SearchDeadline::new(Some(budget), soft_budget)));

    SearchLimits {
        max_depth,
        max_nodes: params.nodes.map(|nodes| nodes.max(1)),
        deadline,
        searchmoves: params.searchmoves.clone(),
        max_moves_to_draw: 0,
    }
}

/// この`go`の予算を残しても次の手へ回せないかどうか。
///
/// `movetime`はその時間を使えという指示で、持ち時間のない秒読みは各手で使い切りになる。
/// どちらもiterationを一つ余分に試したほうがよく、soft stopで削る意味がない。
fn is_spendable_budget(position: &Position, params: &GoParams) -> bool {
    let remaining = match position.turn() {
        Color::BLACK => params.btime,
        Color::WHITE => params.wtime,
    };
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

    let remaining = match position.turn() {
        Color::BLACK => params.btime,
        Color::WHITE => params.wtime,
    };
    let increment = match position.turn() {
        Color::BLACK => params.binc.unwrap_or(0),
        Color::WHITE => params.winc.unwrap_or(0),
    };
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
        let limits = limits_from_go(&position, &params, DEFAULT_MOVE_OVERHEAD_MS);
        assert_eq!(limits.max_depth, 3);
        assert!(limits.deadline.is_none());
    }

    #[test]
    fn go_depth_is_clamped_to_the_finite_search_bound() {
        let position = board::hirate_position();
        let params = GoParams { depth: Some(u32::MAX), ..GoParams::default() };
        let limits = limits_from_go(&position, &params, DEFAULT_MOVE_OVERHEAD_MS);
        assert_eq!(limits.max_depth, MAX_SEARCH_DEPTH);
    }

    #[test]
    fn zero_node_request_keeps_the_mandatory_fallback_node() {
        let position = board::hirate_position();
        let params = GoParams { nodes: Some(0), ..GoParams::default() };
        let limits = limits_from_go(&position, &params, DEFAULT_MOVE_OVERHEAD_MS);
        assert_eq!(limits.max_nodes, Some(1));
    }

    #[test]
    fn movetime_creates_a_deadline() {
        let position = board::hirate_position();
        let params = GoParams { movetime: Some(100), ..GoParams::default() };
        let limits = limits_from_go(&position, &params, DEFAULT_MOVE_OVERHEAD_MS);
        assert!(limits.deadline.is_some());
    }

    #[test]
    fn extreme_time_values_are_saturated_before_building_a_deadline() {
        let position = board::hirate_position();
        let movetime = GoParams { movetime: Some(u64::MAX), ..GoParams::default() };
        assert!(limits_from_go(&position, &movetime, DEFAULT_MOVE_OVERHEAD_MS).deadline.is_some());

        let combined = GoParams {
            btime: Some(u64::MAX),
            binc: Some(u64::MAX),
            byoyomi: Some(u64::MAX),
            ..GoParams::default()
        };
        assert_eq!(budget_ms(&combined), Some(u64::MAX - DEFAULT_MOVE_OVERHEAD_MS));
        assert!(limits_from_go(&position, &combined, DEFAULT_MOVE_OVERHEAD_MS).deadline.is_some());
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
        let limits = limits_from_go(&position, &params, DEFAULT_MOVE_OVERHEAD_MS);
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
            let deadline = limits_from_go(&position, params, DEFAULT_MOVE_OVERHEAD_MS)
                .deadline
                .expect("a timed search");
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
    fn an_infinite_search_has_neither_deadline() {
        let position = board::hirate_position();
        let params = GoParams { infinite: true, ..GoParams::default() };
        let limits = limits_from_go(&position, &params, DEFAULT_MOVE_OVERHEAD_MS);
        assert!(limits.deadline.is_none());
    }

    #[test]
    fn ponderhit_restarts_the_budget_from_that_moment() {
        let deadline =
            SearchDeadline::new(Some(Duration::from_millis(400)), Some(Duration::from_millis(200)));
        std::thread::sleep(Duration::from_millis(120));
        let before = deadline.remaining().expect("a budgeted deadline");
        deadline.restart();
        let after = deadline.remaining().expect("a budgeted deadline");

        assert!(
            before < Duration::from_millis(300),
            "ponder中に予算が減っていない前提が崩れている"
        );
        assert!(after > before, "ponderhitで予算を計り直していない");
        assert!(after > Duration::from_millis(350), "計り直した予算が元の予算に足りていない");
    }

    #[test]
    fn increment_only_time_control_is_resource_limited() {
        let position = board::hirate_position();
        let params = GoParams { binc: Some(100), winc: Some(100), ..GoParams::default() };
        let limits = limits_from_go(&position, &params, DEFAULT_MOVE_OVERHEAD_MS);
        assert_eq!(limits.max_depth, 64);
        assert!(limits.deadline.is_some());
    }
}

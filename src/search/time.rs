//! 単調時計、起点を引き直せる探索締切、および反復深化の時間伸縮率を提供する。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rsshogi::types::Move32;

use super::params::SearchParams;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use std::time::Instant;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Instant(f64);

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn mini_now_ms() -> f64;
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl Instant {
    pub(crate) fn now() -> Self {
        // The embedding supplies performance.now(), a monotonic millisecond clock.
        Self(unsafe { mini_now_ms() })
    }

    pub(crate) fn elapsed(self) -> Duration {
        let elapsed_ms = Self::now().0 - self.0;
        // An invalid host clock must expire the budget rather than panic or keep
        // a bounded search alive forever. Finite backward jumps still clamp to zero.
        if !elapsed_ms.is_finite() {
            return Duration::MAX;
        }
        Duration::try_from_secs_f64((elapsed_ms / 1_000.0).max(0.0)).unwrap_or(Duration::MAX)
    }
}

/// 予算を保持したまま、起点だけをあとから引き直せる締切。
///
/// `go ponder`の時点では相手の手番なので、予算を数え始めてはいけない。
/// `ponderhit`で[`SearchDeadline::restart`]を呼ぶと、その時刻から予算を計り直す。
/// 探索中のworkerが毎node読むため、共有部分はatomicにしてlockを持たない。
///
/// soft budgetがhard budgetより短いときだけ、探索側が局面の安定度に応じてsoft budgetを伸縮できる。
/// 等しいときは使い切る予算なので伸縮しない。
#[derive(Debug)]
pub(crate) struct SearchDeadline {
    origin: Instant,
    hard_budget: Option<Duration>,
    soft_budget: Option<Duration>,
    anchor_micros: AtomicU64,
    hard_micros: AtomicU64,
    soft_micros: AtomicU64,
}

/// 締切なしを表す`*_micros`の値。
const NO_DEADLINE: u64 = u64::MAX;

impl SearchDeadline {
    pub(crate) fn new(hard_budget: Option<Duration>, soft_budget: Option<Duration>) -> Self {
        let deadline = Self {
            origin: Instant::now(),
            hard_budget,
            soft_budget,
            anchor_micros: AtomicU64::new(0),
            hard_micros: AtomicU64::new(NO_DEADLINE),
            soft_micros: AtomicU64::new(NO_DEADLINE),
        };
        deadline.anchor_at(0);
        deadline
    }

    /// いまを起点として予算を計り直す。
    pub(crate) fn restart(&self) {
        self.anchor_at(self.elapsed_micros());
    }

    fn anchor_at(&self, base_micros: u64) {
        self.anchor_micros.store(base_micros, Ordering::Relaxed);
        self.hard_micros.store(offset(base_micros, self.hard_budget), Ordering::Relaxed);
        self.soft_micros.store(offset(base_micros, self.soft_budget), Ordering::Relaxed);
    }

    fn elapsed_micros(&self) -> u64 {
        self.origin.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
    }

    pub(super) fn hard_expired(&self) -> bool {
        self.expired_at(&self.hard_micros, self.elapsed_micros())
    }

    /// soft budgetを`scale`倍した位置を過ぎたか。伸縮できない締切では`scale`を無視する。
    pub(super) fn scaled_soft_expired(&self, scale: f64) -> bool {
        self.scaled_soft_expired_at(scale, self.elapsed_micros())
    }

    fn scaled_soft_expired_at(&self, scale: f64, now_micros: u64) -> bool {
        let (Some(soft), Some(hard)) = (self.soft_budget, self.hard_budget) else {
            return self.expired_at(&self.soft_micros, now_micros);
        };
        if soft >= hard {
            return self.expired_at(&self.soft_micros, now_micros);
        }
        let scaled = soft.mul_f64(scale.max(0.0)).min(hard);
        let limit = offset(self.anchor_micros.load(Ordering::Relaxed), Some(scaled));
        now_micros >= limit
    }

    fn expired_at(&self, limit: &AtomicU64, now_micros: u64) -> bool {
        let limit = limit.load(Ordering::Relaxed);
        limit != NO_DEADLINE && now_micros >= limit
    }

    #[cfg(test)]
    pub(crate) fn hard_budget(&self) -> Option<Duration> {
        self.hard_budget
    }

    #[cfg(test)]
    pub(crate) fn soft_budget(&self) -> Option<Duration> {
        self.soft_budget
    }
}

fn offset(base_micros: u64, budget: Option<Duration>) -> u64 {
    match budget {
        Some(budget) => {
            let budget = u64::try_from(budget.as_micros()).unwrap_or(u64::MAX);
            base_micros.saturating_add(budget).min(NO_DEADLINE - 1)
        }
        None => NO_DEADLINE,
    }
}

/// 最善手が変わった回数を、iterationごとにこの割合で減衰させる。
const CHANGE_DECAY: f64 = 0.5;
const FALLING_MIN: f64 = 0.8;
const FALLING_MAX: f64 = 1.5;
const STABILITY_MIN: f64 = 0.6;
const SCALE_MIN: f64 = 0.4;
const SCALE_MAX: f64 = 2.5;

/// 反復深化の結果から、次のiterationを始めるかどうかの時間の伸縮率を決める。
///
/// 最善手が揺れている局面や評価値が下がっている局面では、浅い結論を急がずに時間を足す。
/// 同じ最善手が続く局面では、次のiterationで結論が変わりにくいので早めに指す。
#[derive(Debug)]
pub(super) struct IterationTiming {
    /// 減衰させた変化回数1回あたりに足す伸縮率。
    change_weight: f64,
    /// 評価値がこの幅だけ下がるごとに、伸縮率を1倍分足す。
    falling_span: f64,
    /// 同じ最善手が続いたiteration数1回あたりに引く伸縮率。
    stability_step: f64,
    previous_best: Option<Move32>,
    /// 過去iterationの評価値の指数平均。評価値の下落を測る基準にする。
    score_average: Option<f64>,
    changes: f64,
    stable_iterations: u32,
}

impl IterationTiming {
    pub(super) fn new(params: &SearchParams) -> Self {
        Self {
            change_weight: f64::from(params.time_change_weight_permille) / 1_000.0,
            falling_span: f64::from(params.time_falling_span.max(1)),
            stability_step: f64::from(params.time_stability_step_permille) / 1_000.0,
            previous_best: None,
            score_average: None,
            changes: 0.0,
            stable_iterations: 0,
        }
    }

    /// 完了したiterationの結果を記録し、soft budgetへ掛ける伸縮率を返す。
    ///
    /// 合法手が1つだけなら考えても結論は変わらないので、次のiterationを始めない率を返す。
    pub(super) fn record(&mut self, best_move: Move32, score: i32, root_moves: usize) -> f64 {
        if root_moves <= 1 {
            return 0.0;
        }
        let changed = self.previous_best.is_some_and(|previous| previous != best_move);
        self.changes = self.changes * CHANGE_DECAY + if changed { 1.0 } else { 0.0 };
        self.stable_iterations = if changed { 0 } else { self.stable_iterations + 1 };
        self.previous_best = Some(best_move);

        let score = f64::from(score);
        let falling = match self.score_average {
            Some(average) => {
                (1.0 + (average - score) / self.falling_span).clamp(FALLING_MIN, FALLING_MAX)
            }
            None => 1.0,
        };
        self.score_average =
            Some(self.score_average.map_or(score, |average| (average + score) / 2.0));

        let instability = 1.0 + self.change_weight * self.changes;
        let stability =
            (1.0 - self.stability_step * f64::from(self.stable_iterations)).max(STABILITY_MIN);
        (instability * falling * stability).clamp(SCALE_MIN, SCALE_MAX)
    }
}

#[cfg(test)]
mod tests {
    use rsshogi::board::hirate_position;

    use super::super::legal_moves;
    use super::*;

    fn two_moves() -> (Move32, Move32) {
        let moves = legal_moves(&hirate_position());
        (moves[0], moves[1])
    }

    #[test]
    fn reanchoring_resets_soft_and_hard_deadlines_from_the_anchor_instant() {
        let deadline =
            SearchDeadline::new(Some(Duration::from_millis(400)), Some(Duration::from_millis(200)));
        assert!(!deadline.expired_at(&deadline.soft_micros, 199_999));
        assert!(deadline.expired_at(&deadline.soft_micros, 200_000));
        assert!(deadline.expired_at(&deadline.hard_micros, 400_000));

        // Simulate ponderhit arriving at t = 350ms: both budgets shift to [350ms + soft/hard].
        deadline.anchor_at(350_000);
        assert!(!deadline.expired_at(&deadline.soft_micros, 549_999));
        assert!(deadline.expired_at(&deadline.soft_micros, 550_000));
        assert!(!deadline.expired_at(&deadline.hard_micros, 749_999));
        assert!(deadline.expired_at(&deadline.hard_micros, 750_000));
        assert!(deadline.scaled_soft_expired_at(1.5, 650_000));
        assert!(!deadline.scaled_soft_expired_at(1.5, 649_999));
    }

    #[test]
    fn a_single_legal_move_stops_after_the_first_iteration() {
        let (first, _) = two_moves();
        assert_eq!(IterationTiming::new(&SearchParams::default()).record(first, 0, 1), 0.0);
    }

    #[test]
    fn a_stable_best_move_shrinks_the_budget() {
        let (first, _) = two_moves();
        let mut timing = IterationTiming::new(&SearchParams::default());
        let mut scale = timing.record(first, 50, 30);
        for _ in 0..8 {
            scale = timing.record(first, 50, 30);
        }
        assert!(scale < 1.0, "安定した局面で時間を削っていない: {scale}");
    }

    #[test]
    fn a_changing_best_move_extends_the_budget() {
        let (first, second) = two_moves();
        let mut timing = IterationTiming::new(&SearchParams::default());
        timing.record(first, 50, 30);
        timing.record(second, 50, 30);
        let scale = timing.record(first, 50, 30);
        assert!(scale > 1.0, "最善手の揺れで時間を足していない: {scale}");
    }

    #[test]
    fn a_falling_score_extends_the_budget_within_its_bound() {
        let (first, _) = two_moves();
        let mut timing = IterationTiming::new(&SearchParams::default());
        timing.record(first, 200, 30);
        let scale = timing.record(first, -2_000, 30);
        assert!(scale > 1.0);
        assert!(scale <= SCALE_MAX);
    }
}

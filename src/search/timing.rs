//! 反復深化の結果から、次のiterationを始めるかどうかの時間の伸縮率を決める。
//!
//! 最善手が揺れている局面や評価値が下がっている局面では、浅い結論を急がずに時間を足す。
//! 同じ最善手が続く局面では、次のiterationで結論が変わりにくいので早めに指す。

use rsshogi::types::Move32;

use crate::params::SearchParams;

/// 最善手が変わった回数を、iterationごとにこの割合で減衰させる。
const CHANGE_DECAY: f64 = 0.5;
const FALLING_MIN: f64 = 0.8;
const FALLING_MAX: f64 = 1.5;
const STABILITY_MIN: f64 = 0.6;
const SCALE_MIN: f64 = 0.4;
const SCALE_MAX: f64 = 2.5;

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

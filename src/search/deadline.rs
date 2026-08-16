//! 起点を引き直せる探索の締切を提供する。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 予算を保持したまま、起点だけをあとから引き直せる締切。
///
/// `go ponder`の時点では相手の手番なので、予算を数え始めてはいけない。
/// `ponderhit`で[`SearchDeadline::restart`]を呼ぶと、その時刻から予算を計り直す。
/// 探索中のworkerが毎node読むため、共有部分はatomicにしてlockを持たない。
#[derive(Debug)]
pub struct SearchDeadline {
    origin: Instant,
    hard_budget: Option<Duration>,
    soft_budget: Option<Duration>,
    hard_micros: AtomicU64,
    soft_micros: AtomicU64,
}

/// 締切なしを表す`*_micros`の値。
const NO_DEADLINE: u64 = u64::MAX;

impl SearchDeadline {
    pub fn new(hard_budget: Option<Duration>, soft_budget: Option<Duration>) -> Self {
        let deadline = Self {
            origin: Instant::now(),
            hard_budget,
            soft_budget,
            hard_micros: AtomicU64::new(NO_DEADLINE),
            soft_micros: AtomicU64::new(NO_DEADLINE),
        };
        deadline.anchor(0);
        deadline
    }

    /// いまを起点として予算を計り直す。
    pub fn restart(&self) {
        self.anchor(self.elapsed_micros());
    }

    fn anchor(&self, base_micros: u64) {
        self.hard_micros.store(offset(base_micros, self.hard_budget), Ordering::Relaxed);
        self.soft_micros.store(offset(base_micros, self.soft_budget), Ordering::Relaxed);
    }

    fn elapsed_micros(&self) -> u64 {
        self.origin.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
    }

    pub(super) fn hard_expired(&self) -> bool {
        self.expired(&self.hard_micros)
    }

    pub(super) fn soft_expired(&self) -> bool {
        self.expired(&self.soft_micros)
    }

    fn expired(&self, limit: &AtomicU64) -> bool {
        let limit = limit.load(Ordering::Relaxed);
        limit != NO_DEADLINE && self.elapsed_micros() >= limit
    }

    #[cfg(test)]
    pub fn hard_budget(&self) -> Option<Duration> {
        self.hard_budget
    }

    #[cfg(test)]
    pub fn soft_budget(&self) -> Option<Duration> {
        self.soft_budget
    }

    /// 現在の起点から見た残り予算。testだけが使う。
    #[cfg(test)]
    pub fn remaining(&self) -> Option<Duration> {
        let limit = self.hard_micros.load(Ordering::Relaxed);
        (limit != NO_DEADLINE)
            .then(|| Duration::from_micros(limit.saturating_sub(self.elapsed_micros())))
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

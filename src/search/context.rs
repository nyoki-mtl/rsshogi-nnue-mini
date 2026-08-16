//! workerごとの探索状態と置換表keyの構成を担う。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rsshogi::board::Position;
use rsshogi::types::{Move32, RepetitionState};

use crate::eval::Evaluator;
use crate::params::SearchParams;
use crate::tt::TranspositionTable;

use super::SearchLimits;

/// TT keyのfingerprintで遡る手数の上限。
const REPETITION_HISTORY_PLIES: usize = 16;
const FNV_OFFSET_BASIS: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

pub(crate) struct SearchContext {
    pub(super) evaluator: Evaluator,
    pub(super) params: SearchParams,
    pub(super) limits: SearchLimits,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) pondering: Arc<AtomicBool>,
    pub(super) nodes: Arc<AtomicU64>,
    pub(super) table: Arc<TranspositionTable>,
    pub(super) history: HashMap<Move32, i32>,
    pub(super) killers: Vec<[Option<Move32>; 2]>,
}

impl SearchContext {
    pub(super) fn enter_node(&mut self) -> Option<()> {
        if self.cancel.load(Ordering::Relaxed) || self.hard_deadline_expired() {
            return None;
        }
        if let Some(limit) = self.limits.max_nodes {
            self.nodes
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |nodes| {
                    (nodes < limit).then_some(nodes + 1)
                })
                .ok()?;
        } else {
            self.nodes.fetch_add(1, Ordering::Relaxed);
        }
        Some(())
    }

    pub(super) fn should_stop(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
            || self.limits.max_nodes.is_some_and(|limit| self.node_count() >= limit)
            || self.hard_deadline_expired()
    }

    /// ponder中は締切を保留する。`ponderhit`まで自分の持ち時間は減らない。
    fn hard_deadline_expired(&self) -> bool {
        !self.pondering.load(Ordering::Acquire)
            && self.limits.deadline.as_ref().is_some_and(|deadline| deadline.hard_expired())
    }

    /// ponder中は保留し、softな締切を過ぎていれば新しいiterationを始めない。
    pub(super) fn should_skip_new_iteration(&self) -> bool {
        !self.pondering.load(Ordering::Acquire)
            && self.limits.deadline.as_ref().is_some_and(|deadline| deadline.soft_expired())
    }

    pub(super) fn node_count(&self) -> u64 {
        self.nodes.load(Ordering::Relaxed)
    }

    pub(super) fn record_quiet_cutoff(&mut self, mv: Move32, depth: u32, ply: u32) {
        let bonus = i32::try_from(depth.saturating_mul(depth)).unwrap_or(i32::MAX).min(4_096);
        let value = self.history.entry(mv).or_default();
        *value = value.saturating_add(bonus).min(100_000);

        if let Some(killers) = self.killers.get_mut(ply as usize)
            && killers[0] != Some(mv)
        {
            killers[1] = killers[0];
            killers[0] = Some(mv);
        }
    }
}

/// 局面に、千日手判定の結果を左右する状態だけを足したTT key。
///
/// `rsshogi`の探索用千日手判定は`min(ply, plies_from_null, 16)`手までしか遡らない。
/// そのため、遡れる幅、連続王手の長さ、すでに数えた同一局面の回数がkeyに必要になる。
/// 一方で窓の中身、つまりどの経路でこの局面へ来たかはkeyへ入れない。
/// 入れると手順前後で合流した同一局面が別entryになり、TTがtranspositionを共有しなくなる。
/// 代わりに、祖先集合が違う二つの経路が同じentryを共有し得るという通常のGHIを受け入れる。
pub(super) fn tt_key(position: &Position, ply: u32, max_moves_to_draw: u32) -> u64 {
    let scan_window =
        (ply as usize).min(usize::from(position.plies_from_null())).min(REPETITION_HISTORY_PLIES);
    let mut fingerprint = FNV_OFFSET_BASIS;
    mix_tt_component(&mut fingerprint, scan_window as u64);
    mix_tt_component(
        &mut fingerprint,
        u64::from_ne_bytes(i64::from(position.repetition_times()).to_ne_bytes()),
    );
    mix_tt_component(&mut fingerprint, repetition_state_tag(position.repetition_type()));
    for checks in position.continuous_checks() {
        mix_tt_component(&mut fingerprint, u64::from(checks));
    }
    if max_moves_to_draw > 0 {
        mix_tt_component(&mut fingerprint, u64::from(max_moves_to_draw));
        mix_tt_component(&mut fingerprint, u64::from(position.game_ply()));
    }
    position.key().low_u64() ^ fingerprint.rotate_left(1)
}

fn mix_tt_component(fingerprint: &mut u64, component: u64) {
    *fingerprint ^= component;
    *fingerprint = fingerprint.wrapping_mul(FNV_PRIME);
}

const fn repetition_state_tag(state: RepetitionState) -> u64 {
    match state {
        RepetitionState::None => 0,
        RepetitionState::Win => 1,
        RepetitionState::Lose => 2,
        RepetitionState::Draw => 3,
        RepetitionState::Superior => 4,
        RepetitionState::Inferior => 5,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    use rsshogi::board;

    use super::super::SearchDeadline;
    use super::super::test_support::test_context;
    use super::*;

    #[test]
    fn max_moves_tt_key_includes_game_ply() {
        let early = board::position_from_sfen(
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 99",
        )
        .expect("valid position");
        let late = board::position_from_sfen(
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 101",
        )
        .expect("valid position");
        assert_eq!(tt_key(&early, 0, 0), tt_key(&late, 0, 0));
        assert_ne!(tt_key(&early, 0, 100), tt_key(&late, 0, 100));
        assert_ne!(tt_key(&early, 0, 100), tt_key(&early, 0, 200));
    }

    #[test]
    fn tt_key_shares_a_transposition_reached_by_a_different_move_order() {
        board::init();
        let mut direct = board::hirate_position();
        let mut transposed = board::hirate_position();
        for text in ["7g7f", "3c3d", "2g2f"] {
            let mv = board::move_from_usi(&direct, text).expect("legal move");
            direct.apply_move32(mv);
        }
        for text in ["2g2f", "3c3d", "7g7f"] {
            let mv = board::move_from_usi(&transposed, text).expect("legal move");
            transposed.apply_move32(mv);
        }

        assert_eq!(direct.key(), transposed.key(), "the two orders must transpose");
        assert_eq!(
            tt_key(&direct, 3, 0),
            tt_key(&transposed, 3, 0),
            "手順前後で合流した同一局面はTT entryを共有する"
        );
    }

    #[test]
    fn tt_key_separates_equal_boards_with_different_repetition_histories() {
        let cycle = ["2h3h", "8b7b", "3h2h", "7b8b"];
        let mut history_rich = board::hirate_position();
        for _ in 0..2 {
            for text in cycle {
                let mv = board::move_from_usi(&history_rich, text).expect("legal cycle move");
                history_rich.apply_move32(mv);
            }
        }
        let mut history_fresh =
            board::position_from_sfen(&history_rich.to_sfen(None)).expect("valid reparsed SFEN");

        assert_eq!(history_rich.key(), history_fresh.key());
        assert_eq!(history_rich.repetition_state(), RepetitionState::None);
        assert_eq!(history_fresh.repetition_state(), RepetitionState::None);
        assert_ne!(tt_key(&history_rich, 0, 0), tt_key(&history_fresh, 0, 0));
        assert_ne!(tt_key(&history_rich, 8, 0), tt_key(&history_fresh, 8, 0));
        assert_ne!(tt_key(&history_rich, 0, 0), tt_key(&history_rich, 8, 0));

        for text in cycle {
            let rich_move = board::move_from_usi(&history_rich, text).expect("legal rich move");
            history_rich.apply_move32(rich_move);
            let fresh_move = board::move_from_usi(&history_fresh, text).expect("legal fresh move");
            history_fresh.apply_move32(fresh_move);
        }
        assert_eq!(history_rich.repetition_state(), RepetitionState::Draw);
        assert_eq!(history_fresh.repetition_state(), RepetitionState::None);
    }

    #[test]
    fn shared_node_admission_does_not_overshoot_under_contention() {
        let nodes = Arc::new(AtomicU64::new(0));
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let nodes = Arc::clone(&nodes);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut context = test_context(nodes, Some(1));
                    barrier.wait();
                    context.enter_node().is_some()
                })
            })
            .collect::<Vec<_>>();

        let admitted = handles
            .into_iter()
            .map(|handle| handle.join().expect("node-admission worker"))
            .filter(|admitted| *admitted)
            .count();
        assert_eq!(admitted, 1);
        assert_eq!(nodes.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn ponder_suppresses_only_the_deadline() {
        let pondering = Arc::new(AtomicBool::new(true));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.limits.deadline = Some(Arc::new(SearchDeadline::new(Some(Duration::ZERO), None)));
        context.pondering = Arc::clone(&pondering);

        assert!(context.enter_node().is_some(), "pondering must suppress the time deadline");
        pondering.store(false, Ordering::Release);
        assert!(context.enter_node().is_none(), "ponderhit must reactivate the same deadline");
    }
}

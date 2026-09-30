//! workerごとの探索状態と置換表keyの構成を担う。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rsshogi::board::Position;
use rsshogi::types::{Color, Move32, RepetitionState};

use crate::eval::Evaluator;
use crate::nnue::MAX_NNUE_EVAL;
use crate::params::SearchParams;
use crate::position::MAX_SEARCH_PLY;
use crate::tt::TranspositionTable;

use super::history::{CONTINUATION_PLIES, HistoryTables};
use super::lmr::LmrReductions;
use super::ordering::OrderingBuffer;
use super::{MAX_QPLY, SearchLimits};

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
    pub(super) history: HistoryTables,
    pub(super) killers: Vec<[Option<Move32>; 2]>,
    /// paramsの`lmr_divisor`から探索開始時に事前計算した縮小テーブル。
    pub(super) lmr: LmrReductions,
    /// plyごとに再利用する並べ替えバッファ。毎ノードのVec確保を避ける。
    pub(super) ordering: Vec<OrderingBuffer>,
    /// plyごとに「そこで指した手のpiece-to index」。null moveは`None`。
    /// continuation historyがこれを遡って直前の手を引く。
    pub(super) continuation: Vec<Option<usize>>,
    /// plyごとに、singularの判定探索で読まない手。判定探索の間だけ設定する。
    pub(super) excluded: Vec<Option<Move32>>,
    /// 実行中のiterationのdepth。延長が際限なく続かないよう、延長できるplyを制限する。
    pub(super) root_depth: u32,
}

/// 全workerが共有する停止・ponder状態、node数、置換表。
#[derive(Clone)]
pub(super) struct SharedSearch {
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) pondering: Arc<AtomicBool>,
    pub(super) nodes: Arc<AtomicU64>,
    pub(super) table: Arc<TranspositionTable>,
}

impl SearchContext {
    pub(super) fn new(
        evaluator: Evaluator,
        params: SearchParams,
        limits: SearchLimits,
        shared: SharedSearch,
        history: HistoryTables,
    ) -> Self {
        let SharedSearch { cancel, pondering, nodes, table } = shared;
        let killer_count = limits.max_depth as usize + MAX_QPLY as usize + 4;
        let per_ply = MAX_SEARCH_PLY as usize + 2;
        Self {
            evaluator,
            lmr: LmrReductions::new(params.lmr_divisor),
            params,
            limits,
            cancel,
            pondering,
            nodes,
            table,
            history,
            killers: vec![[None; 2]; killer_count],
            ordering: std::iter::repeat_with(Default::default).take(per_ply).collect(),
            continuation: vec![None; per_ply],
            excluded: vec![None; per_ply],
            root_depth: 0,
        }
    }

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

    /// ponder中は保留し、`scale`倍したsoftな締切を過ぎていれば新しいiterationを始めない。
    pub(super) fn should_skip_new_iteration(&self, scale: f64) -> bool {
        !self.pondering.load(Ordering::Acquire)
            && self
                .limits
                .deadline
                .as_ref()
                .is_some_and(|deadline| deadline.scaled_soft_expired(scale))
    }

    pub(super) fn node_count(&self) -> u64 {
        self.nodes.load(Ordering::Relaxed)
    }

    /// 生の静的評価へcorrection historyの補正を足す。
    ///
    /// 補正後も通常評価の範囲を守り、詰みscoreとTTの距離補正へ混入させない。
    /// TTへ保存するのは補正前の値で、補正は枝刈りとstand patの判定にだけ効かせる。
    pub(super) fn corrected_eval(&self, position: &Position, raw: i32) -> i32 {
        let correction =
            self.history.corrections.correction(position, self.params.correction_apply_max);
        (raw + correction).clamp(-MAX_NNUE_EVAL, MAX_NNUE_EVAL)
    }

    /// plyのpoolから並べ替えバッファを借りる。pool外のplyには空を渡す。
    pub(super) fn take_ordering_buffer(&mut self, ply: u32) -> OrderingBuffer {
        self.ordering.get_mut(ply as usize).map(std::mem::take).unwrap_or_default()
    }

    /// 借りたバッファをplyのpoolへ返す。探索中断で返らない分は次に再確保される。
    pub(super) fn recycle_ordering_buffer(&mut self, ply: u32, buffer: OrderingBuffer) {
        if let Some(slot) = self.ordering.get_mut(ply as usize) {
            *slot = buffer;
        }
    }

    /// このplyで指した手を記録する。子ノードのcontinuation historyが読む。
    pub(super) fn set_continuation(&mut self, ply: u32, entry: Option<usize>) {
        if let Some(slot) = self.continuation.get_mut(ply as usize) {
            *slot = entry;
        }
    }

    /// 直前と2手前のpiece-to index。遡れない範囲は`None`。
    pub(super) fn previous_continuations(&self, ply: u32) -> [Option<usize>; CONTINUATION_PLIES] {
        std::array::from_fn(|offset| {
            (ply as usize)
                .checked_sub(offset + 1)
                .and_then(|index| self.continuation.get(index).copied().flatten())
        })
    }

    /// 静かな手のβカットを履歴テーブルとkillerへ記録する。
    ///
    /// `tried_quiets`は同じノードで先に試してalphaを上げられなかった静かな手で、
    /// カットした手と同じ大きさのペナルティを受ける。
    pub(super) fn record_quiet_cutoff(
        &mut self,
        stm: Color,
        mv: Move32,
        tried_quiets: &[Move32],
        depth: u32,
        ply: u32,
    ) {
        self.history.record_quiet_cutoff(stm, mv, tried_quiets, depth);
        let previous = self.previous_continuations(ply);
        self.history.record_continuation_cutoff(&previous, mv, tried_quiets, depth);

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
/// このnodeで判定済みの同一局面の回数と種別、連続王手の長さをkeyへ混ぜる。
/// 探索plyは混ぜない。混ぜるとrootが進んだ次の`go`で同じ局面を引けなくなる。
/// どの経路でこの局面へ来たかもkeyへ入れない。
/// 入れると手順前後で合流した同一局面が別entryになり、TTがtranspositionを共有しなくなる。
/// 代わりに、祖先集合が違う二つの経路が同じentryを共有し得るという通常のGHIを受け入れる。
pub(super) fn tt_key(position: &Position, max_moves_to_draw: u32) -> u64 {
    let mut fingerprint = FNV_OFFSET_BASIS;
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
        assert_eq!(tt_key(&early, 0), tt_key(&late, 0));
        assert_ne!(tt_key(&early, 100), tt_key(&late, 100));
        assert_ne!(tt_key(&early, 100), tt_key(&early, 200));
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
            tt_key(&direct, 0),
            tt_key(&transposed, 0),
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
        assert_ne!(tt_key(&history_rich, 0), tt_key(&history_fresh, 0));

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

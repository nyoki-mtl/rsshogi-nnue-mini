//! workerごとの探索状態と置換表keyの構成を担う。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rsshogi::board::Position;
use rsshogi::types::{Bitboard, Color, Move32, RepetitionState};

use crate::eval::Evaluator;
use crate::nnue::MAX_NNUE_EVAL;
use crate::params::SearchParams;
use crate::position::MAX_SEARCH_PLY;
use crate::tt::TranspositionTable;

use super::history::{CONTINUATION_PLIES, HistoryTables, piece_to_index};
use super::lmr::LmrReductions;
use super::ordering::OrderingBuffer;
use super::{MAX_QPLY, SearchLimits};

/// 時間で止める探索で、共有のnode数へまとめて加算し、締切を確かめる間隔(node数)。
///
/// 全workerが毎nodeで同じatomicへ加算し時計を読むと、Threads>1ではcache lineの
/// 取り合いが、1 threadでも時計の読み出しが無視できない費用になる。
/// 止まるのが最大でこの数のnodeだけ遅れる。
const NODE_BATCH: u64 = 256;

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
    /// plyごとの捕獲文脈つきpiece-to index。null moveは`None`。
    /// continuation historyがこれを遡って直前の手を引く。
    pub(super) continuation: Vec<Option<usize>>,
    /// plyごとに、singularの判定探索で読まない手。判定探索の間だけ設定する。
    pub(super) excluded: Vec<Option<Move32>>,
    /// 実行中のiterationのdepth。延長が際限なく続かないよう、延長できるplyを制限する。
    pub(super) root_depth: u32,
    /// null cutを検証しているnodeのply。同じnodeで再びnullを試さない。
    pub(super) null_verification_ply: Option<u32>,
    /// plyごとのβカット数。親nodeが子を読む間に兄弟で起きたカットを数える。
    cutoffs: Vec<u32>,
    /// plyごとに、子を縮小探索している間だけその縮小量を置く。子が事後に深さを調整する。
    reductions: Vec<u32>,
    /// 共有のnode数へまだ加算していない、このworkerのnode数。
    pending_nodes: u64,
    /// このworkerが受理されたnodeの累計。rootの手ごとの探索量に使う。
    local_nodes: u64,
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
    /// 確保領域を残し、次のgoの意味上の状態だけを初期化する。
    pub(super) fn prepare(
        &mut self,
        evaluator: &Evaluator,
        params: SearchParams,
        limits: SearchLimits,
        shared: SharedSearch,
    ) {
        self.evaluator.prepare(evaluator);
        if self.params.lmr_divisor != params.lmr_divisor {
            self.lmr = LmrReductions::new(params.lmr_divisor);
        }
        self.params = params;
        self.killers.resize(limits.max_depth as usize + MAX_QPLY as usize + 4, [None; 2]);
        self.killers.fill([None; 2]);
        for buffer in &mut self.ordering {
            buffer.clear();
        }
        self.continuation.fill(None);
        self.excluded.fill(None);
        self.cutoffs.fill(0);
        self.reductions.fill(0);
        self.root_depth = 0;
        self.null_verification_ply = None;
        self.pending_nodes = 0;
        self.local_nodes = 0;
        self.limits = limits;
        let SharedSearch { cancel, pondering, nodes, table } = shared;
        self.cancel = cancel;
        self.pondering = pondering;
        self.nodes = nodes;
        self.table = table;
    }

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
            null_verification_ply: None,
            cutoffs: vec![0; per_ply + 2],
            reductions: vec![0; per_ply],
            pending_nodes: 0,
            local_nodes: 0,
        }
    }

    pub(super) fn enter_node(&mut self) -> Option<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(limit) = self.limits.max_nodes {
            // node数で止める探索は上限を正確に守るため、1 nodeずつ共有の数へ加算する。
            if self.hard_deadline_expired() {
                return None;
            }
            self.nodes
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |nodes| {
                    (nodes < limit).then_some(nodes + 1)
                })
                .ok()?;
            self.local_nodes += 1;
            return Some(());
        }
        // まとめの最初のnodeでだけ締切を確かめ、まとめが埋まったら共有の数へ加算する。
        if self.pending_nodes == 0 && self.hard_deadline_expired() {
            return None;
        }
        self.pending_nodes += 1;
        if self.pending_nodes == NODE_BATCH {
            self.flush_nodes();
        }
        self.local_nodes += 1;
        Some(())
    }

    /// まだ共有のnode数へ加算していない分を加算する。worker終了時にも呼ぶ。
    pub(super) fn flush_nodes(&mut self) {
        if self.pending_nodes > 0 {
            self.nodes.fetch_add(self.pending_nodes, Ordering::Relaxed);
            self.pending_nodes = 0;
        }
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

    /// 全workerのnode数。このworkerの未加算分を含む(他workerの未加算分は含まない)。
    pub(super) fn node_count(&self) -> u64 {
        self.nodes.load(Ordering::Relaxed) + self.pending_nodes
    }

    /// 他workerの進行や、共有counterへ反映するタイミングに依存しない探索量。
    pub(super) fn local_node_count(&self) -> u64 {
        self.local_nodes
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

    /// 直前と2手前の捕獲文脈つきpiece-to index。遡れない範囲は`None`。
    pub(super) fn previous_continuations(&self, ply: u32) -> [Option<usize>; CONTINUATION_PLIES] {
        std::array::from_fn(|offset| {
            (ply as usize)
                .checked_sub(offset + 1)
                .and_then(|index| self.continuation.get(index).copied().flatten())
        })
    }

    /// plyで子へ掛けている縮小量を置く。縮小探索が終われば0へ戻す。
    pub(super) fn set_reduction(&mut self, ply: u32, reduction: u32) {
        if let Some(slot) = self.reductions.get_mut(ply as usize) {
            *slot = reduction;
        }
    }

    /// plyで子へ掛けている縮小量。
    pub(super) fn reduction(&self, ply: u32) -> u32 {
        self.reductions.get(ply as usize).copied().unwrap_or(0)
    }

    /// plyのβカット数を0へ戻す。
    pub(super) fn reset_cutoffs(&mut self, ply: u32) {
        if let Some(slot) = self.cutoffs.get_mut(ply as usize) {
            *slot = 0;
        }
    }

    /// plyで起きたβカットを数える。
    pub(super) fn add_cutoff(&mut self, ply: u32) {
        if let Some(slot) = self.cutoffs.get_mut(ply as usize) {
            *slot += 1;
        }
    }

    /// plyで数えたβカット数。
    pub(super) fn cutoffs(&self, ply: u32) -> u32 {
        self.cutoffs.get(ply as usize).copied().unwrap_or(0)
    }

    /// 縮小探索がαを超えた静かな手を、直前の手に続く手としてcontinuation historyで褒める。
    pub(super) fn record_continuation_bonus(&mut self, ply: u32, mv: Move32, bonus: i32) {
        let previous = self.previous_continuations(ply);
        self.history.continuations.record(&previous, piece_to_index(mv), bonus);
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
        threats: Bitboard,
    ) {
        self.history.record_quiet_cutoff(stm, mv, tried_quiets, depth, threats);
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
    fn prepare_resets_transient_state_and_updates_snapshots_without_reallocating() {
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.killers[0] = [Some(rsshogi::types::MOVE_NULL); 2];
        context.continuation[0] = Some(1);
        context.excluded[0] = Some(rsshogi::types::MOVE_NULL);
        context.cutoffs[0] = 9;
        context.reductions[0] = 3;
        context.root_depth = 12;
        context.null_verification_ply = Some(2);
        context.pending_nodes = 7;
        context.local_nodes = 99;
        let ordering = context.ordering.as_ptr();
        let continuation = context.continuation.as_ptr();
        let before = context.lmr.base_fine(10, 20);
        let mut params = context.params;
        params.lmr_divisor *= 2;
        let eval = crate::eval::EvalParams { pawn: 123, ..Default::default() };
        let shared = SharedSearch {
            cancel: Arc::new(AtomicBool::new(false)),
            pondering: Arc::new(AtomicBool::new(true)),
            nodes: Arc::new(AtomicU64::new(4)),
            table: Arc::new(TranspositionTable::new(1)),
        };
        context.prepare(
            &Evaluator::material(eval),
            params,
            super::super::test_support::depth_limits(2),
            shared.clone(),
        );
        assert_eq!(context.ordering.as_ptr(), ordering);
        assert_eq!(context.continuation.as_ptr(), continuation);
        assert!(context.killers.iter().all(|entry| *entry == [None; 2]));
        assert!(context.continuation.iter().all(Option::is_none));
        assert!(context.excluded.iter().all(Option::is_none));
        assert!(context.cutoffs.iter().all(|value| *value == 0));
        assert!(context.reductions.iter().all(|value| *value == 0));
        assert_eq!(context.root_depth, 0);
        assert_eq!(context.null_verification_ply, None);
        assert_eq!((context.pending_nodes, context.local_nodes), (0, 0));
        assert_eq!(context.params, params);
        assert_eq!(context.evaluator.params(), eval);
        assert!(context.lmr.base_fine(10, 20) < before);
        assert!(Arc::ptr_eq(&context.cancel, &shared.cancel));
        assert!(Arc::ptr_eq(&context.pondering, &shared.pondering));
        assert!(Arc::ptr_eq(&context.nodes, &shared.nodes));
        assert!(Arc::ptr_eq(&context.table, &shared.table));
    }

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
    fn local_nodes_ignore_other_workers_and_count_only_admitted_nodes() {
        for limit in [None, Some(NODE_BATCH + 2)] {
            let nodes = Arc::new(AtomicU64::new(0));
            let mut context = test_context(Arc::clone(&nodes), limit);
            for _ in 0..NODE_BATCH + 1 {
                context.enter_node().expect("node admitted");
            }
            assert_eq!(context.local_node_count(), NODE_BATCH + 1);
            // 別workerの進行を模す。自workerの累計には混ざらない。
            nodes.fetch_add(100, Ordering::Relaxed);
            context.flush_nodes();
            assert_eq!(context.node_count(), NODE_BATCH + 101);
            assert_eq!(context.local_node_count(), NODE_BATCH + 1);
            if limit.is_some() {
                assert!(context.enter_node().is_none(), "shared budget exhausted");
            } else {
                context.cancel.store(true, Ordering::Relaxed);
                assert!(context.enter_node().is_none(), "cancelled node not admitted");
            }
            assert_eq!(context.local_node_count(), NODE_BATCH + 1);
        }
    }

    #[test]
    fn ponder_suppresses_only_the_deadline() {
        let pondering = Arc::new(AtomicBool::new(true));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.limits.deadline = Some(Arc::new(SearchDeadline::new(Some(Duration::ZERO), None)));
        context.pondering = Arc::clone(&pondering);

        assert!(context.enter_node().is_some(), "pondering must suppress the time deadline");
        pondering.store(false, Ordering::Release);
        // 締切はnodeのまとめごとに確かめるので、遅くとも次のまとめの最初で止まる。
        let stopped = (0..=NODE_BATCH).any(|_| context.enter_node().is_none());
        assert!(stopped, "ponderhit must reactivate the same deadline");
    }
}

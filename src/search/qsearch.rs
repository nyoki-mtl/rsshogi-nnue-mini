//! 静止探索と、その候補生成・SEEに基づく判定を担う。

use rsshogi::board::{CapturePlusPro, Legal, Move32List, Position, generate_moves_move32};
use rsshogi::mate::solve_mate_in_one;
use rsshogi::types::{Move32, Move32Metadata};

use crate::eval::{EvalParams, Evaluator};
use crate::position::MAX_SEARCH_PLY;
use crate::tt::{Bound, TtEntry};

use super::context::{SearchContext, tt_key};
use super::history::CONTINUATION_PLIES;
use super::ordering::{MovePicker, OrderedMove, OrderingInputs, is_legal_generated};
use super::score::{bound_after_search, declaration_score, score_to_tt, terminal_score, tt_cutoff};
use super::{CaptureSee, MATE, MAX_QPLY, legal_moves};

pub(super) fn qsearch(
    position: &mut Position,
    mut alpha: i32,
    beta: i32,
    ply: u32,
    qply: u32,
    context: &mut SearchContext,
) -> Option<i32> {
    context.enter_node()?;
    if let Some(score) = terminal_score(position, ply, context.limits.max_moves_to_draw) {
        return Some(score);
    }
    if let Some(score) = declaration_score(position, ply) {
        return Some(score);
    }
    if ply >= MAX_SEARCH_PLY {
        return Some(structural_leaf_score(position, ply, &mut context.evaluator));
    }
    // TTを読み書きするのは静止探索の入口だけ。再帰した先の局面は保存しない。
    let tt = QsearchTt {
        key: (qply == 0).then(|| tt_key(position, context.limits.max_moves_to_draw)),
        ply,
    };
    // 深いentryほど信頼できるので、depthによるprobeの制限は設けない。
    // 保存側はdepth 0のままなので、主探索のentryを上書きすることはない。
    let tt_entry = tt.key.and_then(|key| context.table.probe(key));
    let tt_move = tt_entry.and_then(|entry| entry.best_move);
    if let Some(entry) = tt_entry
        && let Some(score) = tt_cutoff(&entry, ply, &mut alpha, beta)
    {
        return Some(score);
    }
    let search_alpha = alpha;
    let in_check = position.is_in_check();
    if !in_check && let Some(mate_move) = solve_mate_in_one(position) {
        let score = MATE - ply as i32 - 1;
        tt.store(context, score, Bound::Exact, Some(mate_move), None);
        return Some(score);
    }
    // 王手されていなければ、静止探索が見るのは捕獲と歩の成りだけなので、
    // 静かな手まで生成しない。候補が一つでもあれば合法手があるので詰みでもない。
    // 生成はheapを使わないstack上のリストへ行う。
    let mut move_list = Move32List::new();
    if in_check {
        generate_moves_move32::<Legal>(position, &mut move_list);
    } else {
        qsearch_candidates(position, &mut move_list);
    }
    let moves = move_list.as_slice();
    if moves.is_empty() && (in_check || position.is_mated()) {
        let score = -MATE + ply as i32;
        tt.store(context, score, Bound::Exact, None, None);
        return Some(score);
    }
    // TTへ書き戻すのは生の値、stand patとdelta刈りに使うのは補正後の値。
    let (raw_stand_pat, stand_pat) = if in_check {
        (None, None)
    } else {
        // TT entryに静的評価が残っていれば、高価な再評価を省いてそれを使う。
        let raw = tt_entry
            .and_then(|entry| entry.static_eval)
            .unwrap_or_else(|| context.evaluator.evaluate(position));
        let stand_pat = context.corrected_eval(position, raw);
        if stand_pat >= beta {
            tt.store(context, stand_pat, Bound::Lower, None, Some(raw));
            return Some(stand_pat);
        }
        alpha = alpha.max(stand_pat);
        (Some(raw), Some(stand_pat))
    };
    if (qply >= MAX_QPLY && !in_check) || moves.is_empty() {
        tt.store(context, alpha, bound_after_search(alpha, search_alpha), None, raw_stand_pat);
        return Some(alpha);
    }
    let ordering_buffer = context.take_ordering_buffer(ply);
    let inputs = OrderingInputs {
        eval_params: context.evaluator.params(),
        search_params: context.params,
        killers: [None; 2],
        // qsearchは静かな手を並べないのでcontinuationは引かない。
        previous: [None; CONTINUATION_PLIES],
    };
    let mut picker =
        MovePicker::new(position, moves, tt_move, inputs, &context.history, ordering_buffer);

    let mut best_move = None;
    let mut pruned_move = false;
    while let Some(ordered) = picker.next(position, &context.history) {
        let OrderedMove { mv, metadata, see, .. } = ordered;
        // 王手判定は並べ替えでは払わず、枝刈り判定の直前で遅延計算する。
        // 王手回避中はSEE刈りが除外され、delta刈りもstand_patが無く成立しない
        // ため、判定自体を省略できる。
        let gives_check = !in_check && position.gives_check_move32(mv);
        if should_prune_qsearch_capture_by_see(
            metadata,
            in_check || gives_check,
            see,
            context.params.qsearch_see_margin,
        ) {
            pruned_move = true;
            continue;
        }
        let gain = immediate_material_gain(metadata, context.evaluator.params());
        if !gives_check
            && stand_pat
                .is_some_and(|score| score + gain + context.params.qsearch_delta_margin < alpha)
        {
            pruned_move = true;
            continue;
        }
        position.apply_move32(mv);
        context.evaluator.advance(position, mv);
        let child = qsearch(position, -beta, -alpha, ply + 1, qply + 1, context).map(|v| -v);
        position.undo_move32(mv).expect("a searched move must be undoable");
        context.evaluator.undo();
        let score = child?;

        if score >= beta {
            context.recycle_ordering_buffer(ply, picker.into_buffer());
            tt.store(context, score, Bound::Lower, Some(mv), raw_stand_pat);
            return Some(score);
        }
        if score > alpha {
            best_move = Some(mv);
        }
        alpha = alpha.max(score);
    }
    context.recycle_ordering_buffer(ply, picker.into_buffer());

    // 枝刈りした静止探索がcutoffしなかった値は、読まなかった手の分だけ真値とずれ得る。
    if !pruned_move {
        let bound = bound_after_search(alpha, search_alpha);
        tt.store(context, alpha, bound, best_move, raw_stand_pat);
    }
    Some(alpha)
}

/// 静止探索の入口で使うTTの保存先。`key`が無いnodeは保存しない。
struct QsearchTt {
    key: Option<u64>,
    ply: u32,
}

impl QsearchTt {
    fn store(
        &self,
        context: &SearchContext,
        score: i32,
        bound: Bound,
        best_move: Option<Move32>,
        static_eval: Option<i32>,
    ) {
        if let Some(key) = self.key {
            context.table.store(TtEntry {
                key,
                depth: 0,
                score: score_to_tt(score, self.ply),
                bound,
                best_move,
                static_eval,
            });
        }
    }
}

pub(super) fn structural_leaf_score(
    position: &Position,
    ply: u32,
    evaluator: &mut Evaluator,
) -> i32 {
    if legal_moves(position).is_empty() { -MATE + ply as i32 } else { evaluator.evaluate(position) }
}

fn immediate_material_gain(metadata: Move32Metadata, params: EvalParams) -> i32 {
    let captured = metadata.captured_piece_type();
    params.piece_value(captured)
        + params.piece_value(captured.demote())
        + if metadata.is_promotion() { params.promotion_bonus } else { 0 }
}

/// 静止探索で読む手を、静かな手を生成せずに直接作る。
///
/// `CapturePlusPro`は捕獲と歩の成りを返す。生成結果はpseudo-legalなので、
/// 自玉の安全だけをここで確かめる。捕獲でも歩の成りでもない成り（銀成りなど）は
/// この時点で候補から落ちる。王手されていない局面でだけ使う。
fn qsearch_candidates(position: &Position, out: &mut Move32List) {
    let mut list = Move32List::new();
    generate_moves_move32::<CapturePlusPro>(position, &mut list);
    for mv in list.as_slice().iter().copied() {
        if is_legal_generated(position, mv) {
            out.push(mv);
        }
    }
}

fn should_prune_qsearch_capture_by_see(
    metadata: Move32Metadata,
    tactical_exclusion: bool,
    see: CaptureSee,
    margin: i32,
) -> bool {
    if tactical_exclusion || !metadata.is_capture() {
        return false;
    }
    see.is_some_and(|score| score < -margin)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rsshogi::board;

    use crate::see::static_exchange_eval;
    use crate::tt::TranspositionTable;

    use super::super::INF;
    use super::super::score::score_from_tt;
    use super::super::test_support::test_context;
    use super::*;

    #[test]
    fn corrected_evaluations_stay_outside_the_mate_band() {
        use crate::nnue::MAX_NNUE_EVAL;

        use super::super::negamax::negamax;
        use super::super::score::mate_distance;

        for sign in [-1, 1] {
            for apply_max in [67, 256] {
                let mut position = board::position_from_sfen("k8/9/9/9/9/9/9/9/8K b - 1")
                    .expect("quiet position without a mate or declaration");
                let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
                context.params.correction_apply_max = apply_max;
                for _ in 0..200 {
                    context.history.corrections.record(&position, 16, sign * 200);
                }
                assert!(sign * context.history.corrections.correction(&position, apply_max) > 0);
                let raw = sign * MAX_NNUE_EVAL;
                let key = tt_key(&position, 0);
                // A loose upper bound supplies a boundary static eval without an
                // external model or a TT cutoff hiding the correction path.
                let seed = TtEntry {
                    key,
                    depth: 0,
                    score: INF,
                    bound: Bound::Upper,
                    best_move: None,
                    static_eval: Some(raw),
                };
                context.table.store(seed);
                let beta = if sign > 0 { 0 } else { INF };
                let score = qsearch(&mut position, -INF, beta, 1, 0, &mut context)
                    .expect("qsearch completes");
                assert_eq!(score, raw);
                assert_eq!(mate_distance(score), None);
                let stored = context.table.probe(key).expect("qsearch stores its bound");
                assert_eq!(stored.bound, if sign > 0 { Bound::Lower } else { Bound::Exact });
                assert_eq!(stored.static_eval, Some(raw));
                assert_eq!(stored.score, raw);
                assert_eq!(score_from_tt(stored.score, 5), raw);

                context.table.clear();
                context.table.store(seed);
                // Reverse futility returns the corrected static evaluation directly.
                let beta = raw - 200;
                let score = negamax(&mut position, 1, beta - 1, beta, 1, [None; 2], &mut context)
                    .expect("main search completes");
                assert_eq!(score, raw);
                assert_eq!(mate_distance(score), None);
            }
        }
    }

    #[test]
    fn qsearch_see_pruning_skips_only_completed_losing_exchanges() {
        let position = board::position_from_sfen("k8/9/4p4/4s2p1/4R2P1/9/9/9/8K b - 1")
            .expect("valid SEE-pruning position");
        let losing = board::move_from_usi(&position, "5e5d").expect("legal losing capture");
        let profitable = board::move_from_usi(&position, "2e2d").expect("legal profitable capture");

        assert!(should_prune_qsearch_capture_by_see(
            position.move32_metadata(losing),
            false,
            static_exchange_eval(&position, losing, EvalParams::default()),
            0,
        ));
        assert!(!should_prune_qsearch_capture_by_see(
            position.move32_metadata(profitable),
            false,
            static_exchange_eval(&position, profitable, EvalParams::default()),
            0,
        ));
        let promotion_position = board::position_from_sfen("k8/9/4P4/9/9/9/9/9/8K b - 1")
            .expect("valid quiet-promotion position");
        let quiet_promotion =
            board::move_from_usi(&promotion_position, "5c5b+").expect("legal quiet promotion");
        assert!(!should_prune_qsearch_capture_by_see(
            promotion_position.move32_metadata(quiet_promotion),
            false,
            None,
            0,
        ));
    }

    #[test]
    fn qsearch_see_pruning_keeps_checking_captures_and_evasions() {
        let position = board::position_from_sfen("4k4/9/5g3/4s4/4R4/9/9/9/8K b - 1")
            .expect("valid checking-capture position");
        let losing_check =
            board::move_from_usi(&position, "5e5d").expect("legal losing checking capture");
        assert!(position.gives_check_move32(losing_check));

        assert!(!should_prune_qsearch_capture_by_see(
            position.move32_metadata(losing_check),
            true,
            static_exchange_eval(&position, losing_check, EvalParams::default()),
            0,
        ));
    }

    #[test]
    fn delta_gain_counts_the_captured_board_piece_and_demoted_hand_piece() {
        let rook_position = board::position_from_sfen("4k4/9/9/4r4/4P4/9/9/9/4K4 b - 1")
            .expect("valid rook-capture position");
        let rook_capture =
            board::move_from_usi(&rook_position, "5e5d").expect("legal rook capture");
        assert_eq!(
            immediate_material_gain(
                rook_position.move32_metadata(rook_capture),
                EvalParams::default()
            ),
            2_000
        );

        let promoted_position = board::position_from_sfen("4k4/9/9/4+p4/4R4/9/9/9/4K4 b - 1")
            .expect("valid promoted-victim position");
        let promoted_capture =
            board::move_from_usi(&promoted_position, "5e5d").expect("legal promoted capture");
        assert_eq!(
            immediate_material_gain(
                promoted_position.move32_metadata(promoted_capture),
                EvalParams::default()
            ),
            550
        );
    }

    #[test]
    fn qsearch_delta_pruning_keeps_a_materially_winning_rook_capture() {
        let mut position = board::position_from_sfen("4k4/9/9/4r4/4P4/9/9/9/4K4 b - 1")
            .expect("valid delta-pruning position");
        let nodes = Arc::new(AtomicU64::new(0));
        let mut context = test_context(nodes, None);

        let score =
            qsearch(&mut position, 500, INF, 0, 0, &mut context).expect("qsearch should finish");

        assert_eq!(score, 1_100);
    }

    #[test]
    fn qsearch_does_not_cut_off_while_in_check() {
        let mut position = board::position_from_sfen("4k4/9/9/9/9/9/9/9/4R3K w - 1")
            .expect("valid checked position");
        assert!(position.is_in_check());
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);

        let score = qsearch(&mut position, -INF, INF, 0, MAX_QPLY, &mut context)
            .expect("qsearch should finish");

        assert!(score > -INF, "a checked node must search legal evasions");
        assert!(context.node_count() > 1, "the horizon check must not return immediately");
    }

    #[test]
    fn structural_ply_limit_stops_qsearch_after_one_bounded_leaf() {
        let mut position = board::hirate_position();
        let nodes = Arc::new(AtomicU64::new(0));
        let mut context = test_context(Arc::clone(&nodes), None);

        assert_eq!(qsearch(&mut position, -INF, INF, MAX_SEARCH_PLY, 0, &mut context), Some(0));
        assert_eq!(nodes.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn qsearch_uses_an_exact_tt_entry() {
        let mut position = board::hirate_position();
        let table = Arc::new(TranspositionTable::new(1));
        table.store(TtEntry {
            key: tt_key(&position, 0),
            depth: 0,
            score: score_to_tt(123, 0),
            bound: Bound::Exact,
            best_move: None,
            static_eval: None,
        });
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.table = table;

        assert_eq!(qsearch(&mut position, -INF, INF, 0, 0, &mut context), Some(123));
        assert_eq!(context.node_count(), 1);
        assert_eq!(
            qsearch(&mut position, -INF, INF, 0, 1, &mut context),
            Some(0),
            "a recursive qsearch node must not reuse a root-qsearch horizon entry"
        );
    }

    #[test]
    fn qsearch_uses_a_deeper_main_search_tt_entry() {
        let mut position = board::hirate_position();
        let table = Arc::new(TranspositionTable::new(1));
        let main_entry = TtEntry {
            key: tt_key(&position, 0),
            depth: 1,
            score: score_to_tt(123, 0),
            bound: Bound::Exact,
            best_move: None,
            static_eval: None,
        };
        table.store(main_entry);
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.table = Arc::clone(&table);

        assert_eq!(
            qsearch(&mut position, -INF, INF, 0, 0, &mut context),
            Some(123),
            "深い主探索のentryは静止探索でもカットに使える"
        );
        assert_eq!(context.node_count(), 1);
        assert_eq!(
            table.probe(main_entry.key),
            Some(main_entry),
            "保存はdepth 0のままなので壊さない"
        );
    }

    #[test]
    fn qsearch_reuses_a_stored_static_eval_instead_of_evaluating() {
        let mut position = board::hirate_position();
        let table = Arc::new(TranspositionTable::new(1));
        // boundは使えない(Upperかつscoreが窓の下)が、static_evalだけ残っているentry。
        table.store(TtEntry {
            key: tt_key(&position, 0),
            depth: 0,
            score: score_to_tt(-5_000, 0),
            bound: Bound::Upper,
            best_move: None,
            static_eval: Some(777),
        });
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.table = table;

        // material評価なら平手は0。777が返るのはTTのstatic_evalを使った証拠。
        assert_eq!(qsearch(&mut position, -INF, INF, 0, 0, &mut context), Some(777));
    }

    #[test]
    fn selective_qsearch_result_is_not_stored_in_tt() {
        let mut position = board::position_from_sfen("k8/9/4p4/4s2p1/4R2P1/9/9/9/8K b - 1")
            .expect("valid selective qsearch position");
        let key = tt_key(&position, 0);
        let table = Arc::new(TranspositionTable::new(1));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.table = Arc::clone(&table);

        assert!(qsearch(&mut position, -INF, INF, 0, 0, &mut context).is_some());
        assert!(table.probe(key).is_none());
    }

    #[test]
    fn qsearch_stores_an_exact_quiet_position() {
        board::init();
        let mut position = board::hirate_position();
        let key = tt_key(&position, 0);
        let table = Arc::new(TranspositionTable::new(1));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.table = Arc::clone(&table);

        let score =
            qsearch(&mut position, -INF, INF, 0, 0, &mut context).expect("qsearch should finish");
        let stored = table.probe(key).expect("qsearch should store its result");
        assert_eq!(stored.depth, 0);
        assert_eq!(stored.bound, Bound::Exact);
        assert_eq!(score_from_tt(stored.score, 0), score);
    }

    #[test]
    fn qsearch_finds_a_quiet_mate_in_one() {
        let mut position = board::position_from_sfen(
            "lnsG5/4g4/prpp1p1pp/1p4p1k/4+B4/2P1P3P/P+b1PSP1L1/4K2SL/2G2G1r1 b SP3nl3p 73",
        )
        .expect("valid mate position");
        let mate_move = solve_mate_in_one(&position).expect("mate in one");
        let metadata = position.move32_metadata(mate_move);
        assert!(!metadata.is_capture() && !metadata.is_promotion());
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);

        let score = qsearch(&mut position, MATE - 2, INF, 0, 0, &mut context)
            .expect("qsearch should finish");
        assert_eq!(score, MATE - 1);
    }

    #[test]
    fn qsearch_does_not_expand_a_non_mating_quiet_check() {
        let position = board::position_from_sfen("4k4/9/9/9/9/9/9/4R4/4K4 b - 1")
            .expect("valid quiet-check position");
        let quiet_check = legal_moves(&position)
            .into_iter()
            .find(|mv| {
                let metadata = position.move32_metadata(*mv);
                position.gives_check_move32(*mv)
                    && !metadata.is_capture()
                    && !metadata.is_promotion()
            })
            .expect("position should have a quiet check");

        assert!(solve_mate_in_one(&position).is_none());
        let mut candidates = Move32List::new();
        qsearch_candidates(&position, &mut candidates);
        assert!(!candidates.as_slice().contains(&quiet_check));
    }

    #[test]
    fn qsearch_candidates_are_the_legal_captures_and_pawn_promotions() {
        for sfen in [
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1",
            "l6nl/5+P1gk/2np1S3/p1p4Pp/3P2Sp1/1PPb2P1P/P5GS1/R8/LN4bKL w GR5pnsg 1",
            "k8/4s4/4P4/9/9/9/9/9/4K4 b - 1",
            "4k4/9/4g4/4s4/4P4/9/9/9/4K4 b - 1",
        ] {
            let position = board::position_from_sfen(sfen).expect("valid SFEN");
            if position.is_in_check() {
                continue;
            }
            let mut list = Move32List::new();
            qsearch_candidates(&position, &mut list);
            let mut candidates = list.as_slice().to_vec();
            candidates.sort_by_key(|mv| mv.to_usi());

            let mut expected = super::super::search_moves(&position)
                .into_iter()
                .filter(|mv| {
                    let metadata = position.move32_metadata(*mv);
                    metadata.is_capture()
                        || (metadata.is_promotion()
                            && position.moved_piece_before(*mv).piece_type()
                                == rsshogi::types::PieceType::PAWN)
                })
                .collect::<Vec<_>>();
            expected.sort_by_key(|mv| mv.to_usi());

            assert_eq!(candidates, expected, "{sfen}");
        }
    }
}

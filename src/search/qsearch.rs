//! 静止探索と、その候補生成・SEEに基づく判定を担う。

use rsshogi::board::{CapturePlusProAll, Move32List, Position, generate_moves_move32};
use rsshogi::mate::solve_mate_in_one;
use rsshogi::types::{Move32, Move32Metadata};

use crate::eval::{EvalParams, Evaluator};
use crate::position::MAX_SEARCH_PLY;
use crate::tt::{Bound, TtEntry};

use super::context::{SearchContext, tt_key};
use super::ordering::{OrderedMove, order_moves};
use super::score::{
    bound_after_search, declaration_score, score_from_tt, score_to_tt, terminal_score,
};
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
        return Some(structural_leaf_score(position, ply, &context.evaluator));
    }
    let key = if qply == 0 { tt_key(position, ply, context.limits.max_moves_to_draw) } else { 0 };
    let tt_entry =
        (qply == 0).then(|| context.table.probe(key).filter(|entry| entry.depth == 0)).flatten();
    let tt_move = tt_entry.and_then(|entry| entry.best_move);
    if let Some(entry) = tt_entry {
        let score = score_from_tt(entry.score, ply);
        match entry.bound {
            Bound::Exact => return Some(score),
            Bound::Lower if score >= beta => return Some(score),
            Bound::Upper if score <= alpha => return Some(score),
            Bound::Lower => alpha = alpha.max(score),
            Bound::Upper => {}
        }
    }
    let search_alpha = alpha;
    let in_check = position.is_in_check();
    if !in_check && let Some(mate_move) = solve_mate_in_one(position) {
        let score = MATE - ply as i32 - 1;
        store_qsearch_entry(
            context,
            qply,
            TtEntry {
                key,
                depth: 0,
                score: score_to_tt(score, ply),
                bound: Bound::Exact,
                best_move: Some(mate_move),
            },
        );
        return Some(score);
    }
    // 王手されていなければ、静止探索が見るのは捕獲と歩の成りだけなので、
    // 静かな手まで生成しない。候補が一つでもあれば合法手があるので詰みでもない。
    let moves = if in_check { legal_moves(position) } else { qsearch_candidates(position) };
    if moves.is_empty() && (in_check || position.is_mated()) {
        let score = -MATE + ply as i32;
        store_qsearch_entry(
            context,
            qply,
            TtEntry {
                key,
                depth: 0,
                score: score_to_tt(score, ply),
                bound: Bound::Exact,
                best_move: None,
            },
        );
        return Some(score);
    }
    let stand_pat = if !in_check {
        let stand_pat = context.evaluator.evaluate(position);
        if stand_pat >= beta {
            store_qsearch_entry(
                context,
                qply,
                TtEntry {
                    key,
                    depth: 0,
                    score: score_to_tt(stand_pat, ply),
                    bound: Bound::Lower,
                    best_move: None,
                },
            );
            return Some(stand_pat);
        }
        alpha = alpha.max(stand_pat);
        Some(stand_pat)
    } else {
        None
    };
    if qply >= MAX_QPLY && !in_check {
        store_qsearch_entry(
            context,
            qply,
            TtEntry {
                key,
                depth: 0,
                score: score_to_tt(alpha, ply),
                bound: bound_after_search(alpha, search_alpha),
                best_move: None,
            },
        );
        return Some(alpha);
    }

    if moves.is_empty() {
        let score = alpha;
        store_qsearch_entry(
            context,
            qply,
            TtEntry {
                key,
                depth: 0,
                score: score_to_tt(score, ply),
                bound: bound_after_search(score, search_alpha),
                best_move: None,
            },
        );
        return Some(score);
    }
    let moves = order_moves(
        position,
        &moves,
        context.evaluator.params(),
        tt_move,
        [None; 2],
        &context.history,
        context.params,
    );

    let mut best_move = None;
    let mut pruned_move = false;
    for ordered in moves {
        let OrderedMove { mv, metadata, gives_check, see, .. } = ordered;
        if should_prune_qsearch_capture_by_see(metadata, in_check || gives_check, see) {
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
            store_qsearch_entry(
                context,
                qply,
                TtEntry {
                    key,
                    depth: 0,
                    score: score_to_tt(score, ply),
                    bound: Bound::Lower,
                    best_move: Some(mv),
                },
            );
            return Some(score);
        }
        if score > alpha {
            best_move = Some(mv);
        }
        alpha = alpha.max(score);
    }

    if !pruned_move {
        store_qsearch_entry(
            context,
            qply,
            TtEntry {
                key,
                depth: 0,
                score: score_to_tt(alpha, ply),
                bound: bound_after_search(alpha, search_alpha),
                best_move,
            },
        );
    }
    Some(alpha)
}

pub(super) fn structural_leaf_score(position: &Position, ply: u32, evaluator: &Evaluator) -> i32 {
    if legal_moves(position).is_empty() { -MATE + ply as i32 } else { evaluator.evaluate(position) }
}

fn immediate_material_gain(metadata: Move32Metadata, params: EvalParams) -> i32 {
    let captured = metadata.captured_piece_type();
    params.piece_value(captured)
        + params.piece_value(captured.demote())
        + if metadata.is_promotion() { params.promotion_bonus } else { 0 }
}

fn store_qsearch_entry(context: &SearchContext, qply: u32, entry: TtEntry) {
    if qply == 0 {
        context.table.store(entry);
    }
}

/// 静止探索で読む手を、静かな手を生成せずに直接作る。
///
/// `CapturePlusProAll`は捕獲と歩の成りを返す。生成結果はpseudo-legalなので、
/// 合法性はここで確かめる。捕獲でも歩の成りでもない成り（銀成りなど）は
/// この時点で候補から落ちる。
fn qsearch_candidates(position: &Position) -> Vec<Move32> {
    let mut list = Move32List::new();
    generate_moves_move32::<CapturePlusProAll>(position, &mut list);
    list.as_slice().iter().copied().filter(|mv| position.is_legal_move32(*mv)).collect()
}

fn should_prune_qsearch_capture_by_see(
    metadata: Move32Metadata,
    tactical_exclusion: bool,
    see: CaptureSee,
) -> bool {
    if tactical_exclusion || !metadata.is_capture() {
        return false;
    }
    see.is_some_and(|score| score < 0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rsshogi::board;

    use crate::see::static_exchange_eval;
    use crate::tt::TranspositionTable;

    use super::super::INF;
    use super::super::test_support::test_context;
    use super::*;

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
        ));
        assert!(!should_prune_qsearch_capture_by_see(
            position.move32_metadata(profitable),
            false,
            static_exchange_eval(&position, profitable, EvalParams::default()),
        ));
        let promotion_position = board::position_from_sfen("k8/9/4P4/9/9/9/9/9/8K b - 1")
            .expect("valid quiet-promotion position");
        let quiet_promotion =
            board::move_from_usi(&promotion_position, "5c5b+").expect("legal quiet promotion");
        assert!(!should_prune_qsearch_capture_by_see(
            promotion_position.move32_metadata(quiet_promotion),
            false,
            None,
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
            key: tt_key(&position, 0, 0),
            depth: 0,
            score: score_to_tt(123, 0),
            bound: Bound::Exact,
            best_move: None,
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
    fn qsearch_ignores_a_main_search_tt_entry() {
        let mut position = board::hirate_position();
        let table = Arc::new(TranspositionTable::new(1));
        let main_entry = TtEntry {
            key: tt_key(&position, 0, 0),
            depth: 1,
            score: score_to_tt(123, 0),
            bound: Bound::Exact,
            best_move: None,
        };
        table.store(main_entry);
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.table = Arc::clone(&table);

        assert_eq!(qsearch(&mut position, -INF, INF, 0, 0, &mut context), Some(0));
        assert_eq!(table.probe(main_entry.key), Some(main_entry));
    }

    #[test]
    fn selective_qsearch_result_is_not_stored_in_tt() {
        let mut position = board::position_from_sfen("k8/9/4p4/4s2p1/4R2P1/9/9/9/8K b - 1")
            .expect("valid selective qsearch position");
        let key = tt_key(&position, 0, 0);
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
        let key = tt_key(&position, 0, 0);
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
        assert!(!qsearch_candidates(&position).contains(&quiet_check));
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
            let mut candidates = qsearch_candidates(&position);
            candidates.sort_by_key(|mv| mv.to_usi());

            let mut expected = legal_moves(&position)
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

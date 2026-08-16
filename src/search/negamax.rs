//! 主探索のnegamaxと、そこに属する枝刈りの判定を担う。

use rsshogi::board::Position;

use crate::position::MAX_SEARCH_PLY;
use crate::tt::{Bound, TtEntry};

use super::context::{SearchContext, tt_key};
use super::ordering::{OrderedMove, order_moves};
use super::qsearch::{qsearch, structural_leaf_score};
use super::score::{
    bound_after_search, declaration_score, score_from_tt, score_to_tt, terminal_score,
};
use super::{INF, MATE, MATE_TT_THRESHOLD, legal_moves};

pub(super) fn negamax(
    position: &mut Position,
    depth: u32,
    mut alpha: i32,
    beta: i32,
    ply: u32,
    static_eval_history: [Option<i32>; 2],
    context: &mut SearchContext,
) -> Option<i32> {
    let pv_node = beta > alpha.saturating_add(1);
    let zero_window = beta == alpha.saturating_add(1);
    if depth == 0 {
        return qsearch(position, alpha, beta, ply, 0, context);
    }
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

    let key = tt_key(position, ply, context.limits.max_moves_to_draw);
    let tt_entry = context.table.probe(key);
    let tt_move = tt_entry.and_then(|entry| entry.best_move);
    if let Some(entry) = tt_entry
        && entry.depth >= depth
    {
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

    let moves = legal_moves(position);
    if moves.is_empty() {
        return Some(-MATE + ply as i32);
    }
    let in_check = position.is_in_check();
    let static_eval = (!in_check).then(|| context.evaluator.evaluate(position));
    if should_try_null_move(
        depth,
        zero_window,
        in_check,
        static_eval,
        beta,
        position.plies_from_null(),
        context.params.null_move_reduction,
    ) && position.try_apply_search_null_move().is_ok()
    {
        let child_depth = depth - 1 - context.params.null_move_reduction;
        let child = negamax(position, child_depth, -beta, -beta + 1, ply + 1, [None; 2], context)
            .map(|v| -v);
        position.undo_search_null_move().expect("a search null move must be undoable");
        let score = child?;
        if score >= beta {
            context.table.store(TtEntry {
                key,
                depth,
                score: score_to_tt(beta, ply),
                bound: Bound::Lower,
                best_move: None,
            });
            return Some(beta);
        }
    }
    if depth <= 2
        && !pv_node
        && let Some(static_eval) = static_eval
        && static_eval - context.params.reverse_futility_margin * depth as i32 >= beta
    {
        return Some(static_eval);
    }
    let killers = context.killers.get(ply as usize).copied().unwrap_or([None; 2]);
    let moves = order_moves(
        position,
        &moves,
        context.evaluator.params(),
        tt_move,
        killers,
        &context.history,
        context.params,
    );

    let futility = depth == 1
        && static_eval.is_some_and(|score| score + context.params.futility_margin <= alpha)
        && !in_check;
    let mut best_score = -INF;
    let mut best_move = None;
    let mut eligible_quiets_seen = 0;
    let mut pruned_move = false;
    let mut reduced_move_without_research = false;
    let child_static_eval_history = [static_eval_history[1], static_eval];

    for (index, ordered) in moves.into_iter().enumerate() {
        let OrderedMove { mv, metadata, gives_check, quiet, .. } = ordered;
        if futility && quiet && !gives_check && index > 0 {
            pruned_move = true;
            continue;
        }
        let lmp_eligible = quiet && !metadata.is_drop() && !gives_check;
        let prune_by_lmp = should_prune_late_quiet(
            depth,
            pv_node,
            in_check,
            static_eval,
            static_eval_history[0],
            eligible_quiets_seen,
            lmp_eligible,
            context.params.lmp_quiet_limits,
        );
        if lmp_eligible {
            eligible_quiets_seen += 1;
        }
        if prune_by_lmp {
            pruned_move = true;
            continue;
        }
        position.apply_move32(mv);
        context.evaluator.advance(position, mv);
        let mut reduced_without_research = false;
        let child = if index == 0 {
            negamax(position, depth - 1, -beta, -alpha, ply + 1, child_static_eval_history, context)
                .map(|v| -v)
        } else {
            let reduced = depth >= context.params.lmr_min_depth
                && depth > context.params.lmr_reduction
                && index >= context.params.lmr_move_index
                && quiet
                && !in_check
                && !gives_check;
            let mut scout = if reduced {
                negamax(
                    position,
                    depth - 1 - context.params.lmr_reduction,
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                    child_static_eval_history,
                    context,
                )
                .map(|v| -v)
            } else {
                negamax(
                    position,
                    depth - 1,
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                    child_static_eval_history,
                    context,
                )
                .map(|v| -v)
            };
            if reduced && scout.is_some_and(|score| score > alpha) {
                scout = negamax(
                    position,
                    depth - 1,
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                    child_static_eval_history,
                    context,
                )
                .map(|v| -v);
            } else if reduced {
                reduced_without_research = true;
            }
            match scout {
                Some(score) if score > alpha && score < beta => negamax(
                    position,
                    depth - 1,
                    -beta,
                    -alpha,
                    ply + 1,
                    child_static_eval_history,
                    context,
                )
                .map(|v| -v),
                other => other,
            }
        };
        position.undo_move32(mv).expect("a searched move must be undoable");
        context.evaluator.undo();
        let score = child?;
        reduced_move_without_research |= reduced_without_research;
        if score > best_score {
            best_score = score;
            best_move = Some(mv);
        }

        if score >= beta {
            if quiet {
                context.record_quiet_cutoff(mv, depth, ply);
            }
            context.table.store(TtEntry {
                key,
                depth,
                score: score_to_tt(score, ply),
                bound: Bound::Lower,
                best_move: Some(mv),
            });
            return Some(score);
        }
        alpha = alpha.max(score);
    }

    if !pruned_move && !reduced_move_without_research {
        let bound = bound_after_search(alpha, search_alpha);
        context.table.store(TtEntry {
            key,
            depth,
            score: score_to_tt(alpha, ply),
            bound,
            best_move,
        });
    }
    Some(alpha)
}

#[allow(clippy::too_many_arguments)]
fn should_prune_late_quiet(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    static_eval: Option<i32>,
    same_side_eval: Option<i32>,
    eligible_quiets_seen: usize,
    eligible: bool,
    quiet_limits: [usize; 3],
) -> bool {
    let Some(limit) = depth.checked_sub(1).and_then(|index| quiet_limits.get(index as usize))
    else {
        return false;
    };
    eligible
        && !pv_node
        && !in_check
        && static_eval.zip(same_side_eval).is_some_and(|(current, previous)| current <= previous)
        && eligible_quiets_seen >= *limit
}

fn should_try_null_move(
    depth: u32,
    zero_window: bool,
    in_check: bool,
    static_eval: Option<i32>,
    beta: i32,
    plies_from_null: u16,
    reduction: u32,
) -> bool {
    depth >= reduction + 2
        && zero_window
        && !in_check
        && beta > -MATE_TT_THRESHOLD
        && plies_from_null > 0
        && static_eval.is_some_and(|score| score >= beta)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rsshogi::board;

    use crate::eval::EvalParams;
    use crate::params::SearchParams;
    use crate::tt::TranspositionTable;

    use super::super::test_support::test_context;
    use super::*;

    #[test]
    fn null_move_guard_requires_every_conservative_precondition() {
        assert!(should_try_null_move(4, true, false, Some(20), 20, 1, 2));
        assert!(!should_try_null_move(3, true, false, Some(20), 20, 1, 2));
        assert!(!should_try_null_move(4, false, false, Some(20), 20, 1, 2));
        assert!(!should_try_null_move(4, true, true, Some(20), 20, 1, 2));
        assert!(!should_try_null_move(4, true, false, Some(19), 20, 1, 2));
        assert!(!should_try_null_move(4, true, false, None, 20, 1, 2));
        assert!(!should_try_null_move(4, true, false, Some(20), 20, 0, 2));
        assert!(!should_try_null_move(4, true, false, Some(0), -MATE_TT_THRESHOLD, 1, 2));
        assert!(should_try_null_move(4, true, false, Some(0), -MATE_TT_THRESHOLD + 1, 1, 2));
    }

    #[test]
    fn search_null_move_roundtrip_preserves_position_and_flips_evaluation() {
        let mut position = board::hirate_position();
        let first = board::move_from_usi(&position, "7g7f").expect("legal first move");
        position.apply_move32(first);
        let before_sfen = position.to_sfen(None);
        let before_key = position.key();
        let before_turn = position.turn();
        let before_game_ply = position.game_ply();
        let before_plies_from_null = position.plies_from_null();
        let before_repetition = position.repetition_state();
        let before_eval = crate::eval::evaluate_material(&position, EvalParams::default());

        position.try_apply_search_null_move().expect("search null move");
        assert_eq!(position.turn(), before_turn.flip());
        assert_eq!(position.game_ply(), before_game_ply);
        assert_eq!(position.plies_from_null(), 0);
        assert_eq!(crate::eval::evaluate_material(&position, EvalParams::default()), -before_eval);

        position.undo_search_null_move().expect("undo search null move");
        assert_eq!(position.to_sfen(None), before_sfen);
        assert_eq!(position.key(), before_key);
        assert_eq!(position.turn(), before_turn);
        assert_eq!(position.game_ply(), before_game_ply);
        assert_eq!(position.plies_from_null(), before_plies_from_null);
        assert_eq!(position.repetition_state(), before_repetition);
    }

    #[test]
    fn null_move_fail_high_stores_a_move_less_lower_bound() {
        let mut position = board::position_from_sfen("k8/9/9/9/4R4/9/9/9/8K w - 1")
            .expect("valid null-cutoff position");
        let white_move = board::move_from_usi(&position, "9a8a").expect("legal real move");
        position.apply_move32(white_move);
        let key = tt_key(&position, 1, 0);
        let table = Arc::new(TranspositionTable::new(1));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.limits.max_depth = 4;
        context.table = Arc::clone(&table);

        assert_eq!(negamax(&mut position, 4, -1, 0, 1, [None; 2], &mut context), Some(0));
        let stored = table.probe(key).expect("null cutoff should store a bound");
        assert_eq!(stored.bound, Bound::Lower);
        assert_eq!(stored.best_move, None);
    }

    #[test]
    fn lmp_requires_a_shallow_non_pv_non_improving_quiet() {
        let limits = SearchParams::default().lmp_quiet_limits;
        assert!(should_prune_late_quiet(1, false, false, Some(10), Some(10), 8, true, limits));
        assert!(!should_prune_late_quiet(4, false, false, Some(10), Some(10), 18, true, limits));
        assert!(!should_prune_late_quiet(1, true, false, Some(10), Some(10), 8, true, limits));
        assert!(!should_prune_late_quiet(1, false, true, Some(10), Some(10), 8, true, limits));
        assert!(!should_prune_late_quiet(1, false, false, None, Some(10), 8, true, limits));
        assert!(!should_prune_late_quiet(1, false, false, Some(10), None, 8, true, limits));
        assert!(!should_prune_late_quiet(1, false, false, Some(11), Some(10), 8, true, limits));
        assert!(!should_prune_late_quiet(1, false, false, Some(10), Some(10), 8, false, limits));
    }

    #[test]
    fn lmp_thresholds_count_only_preceding_eligible_quiets() {
        let limits = SearchParams::default().lmp_quiet_limits;
        for (depth, limit) in (1..=3).zip(limits) {
            assert!(!should_prune_late_quiet(
                depth,
                false,
                false,
                Some(0),
                Some(0),
                limit - 1,
                true,
                limits,
            ));
            assert!(should_prune_late_quiet(
                depth,
                false,
                false,
                Some(0),
                Some(0),
                limit,
                true,
                limits,
            ));
        }
    }

    #[test]
    fn selective_lmp_result_is_not_stored_as_an_exact_tt_entry() {
        let mut position = board::hirate_position();
        for usi in ["7g7f", "3c3d"] {
            let mv = board::move_from_usi(&position, usi).expect("legal setup move");
            position.apply_move32(mv);
        }
        let key = tt_key(&position, 1, 0);
        let table = Arc::new(TranspositionTable::new(1));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.limits.max_depth = 2;
        context.table = Arc::clone(&table);

        assert_eq!(
            negamax(&mut position, 2, 500, 501, 1, [Some(1_000), None], &mut context),
            Some(500),
        );
        assert!(table.probe(key).is_none());
    }

    #[test]
    fn direct_depth_zero_negamax_counts_the_frontier_once() {
        let mut position = board::hirate_position();
        let nodes = Arc::new(AtomicU64::new(0));
        let mut context = test_context(Arc::clone(&nodes), None);

        assert!(negamax(&mut position, 0, -INF, INF, 0, [None; 2], &mut context).is_some());
        assert_eq!(nodes.load(Ordering::Relaxed), 1);
    }
}

//! root局面のiteration一回分の探索と、読み筋・fallbackの構成を担う。

use rsshogi::board::Position;
use rsshogi::types::{MOVE_WIN, Move32};

use crate::tt::{Bound, TtEntry};

use super::context::{SearchContext, tt_key};
use super::negamax::search_child;
use super::score::{declaration_score, score_to_tt, terminal_score};
use super::{MATE, MAX_PV_LENGTH, legal_moves};

/// 置換表を辿って読み筋を組み立てる。
///
/// 置換表は上書きされるため、これは探索が実際に読んだ手順の再構成であって保証ではない。
/// 合法でない手、終局、長さ上限のいずれかで打ち切る。
pub(super) fn collect_pv(
    position: &mut Position,
    first: Move32,
    context: &SearchContext,
) -> Vec<Move32> {
    let mut pv = vec![first];
    if first == MOVE_WIN || !position.is_legal_move32(first) {
        return pv;
    }
    position.apply_move32(first);
    let mut applied = 1usize;
    while pv.len() < MAX_PV_LENGTH {
        let ply = applied as u32;
        if terminal_score(position, ply, context.limits.max_moves_to_draw).is_some() {
            break;
        }
        let key = tt_key(position, context.limits.max_moves_to_draw);
        let Some(mv) = context.table.probe(key).and_then(|entry| entry.best_move) else {
            break;
        };
        if mv == MOVE_WIN || !position.is_legal_move32(mv) {
            break;
        }
        pv.push(mv);
        position.apply_move32(mv);
        applied += 1;
    }
    for mv in pv[..applied].iter().rev() {
        position.undo_move32(*mv).expect("a PV move must be undoable");
    }
    pv
}

pub(super) fn fallback_root_choice(
    position: &mut Position,
    moves: &[Move32],
    context: &mut SearchContext,
) -> (Move32, i32) {
    let mut best_move = moves[0];
    let mut best_score = 0;
    let mut evaluated_any = false;
    for mv in moves.iter().copied() {
        if evaluated_any && context.enter_node().is_none() {
            break;
        }
        position.apply_move32(mv);
        context.evaluator.advance(position, mv);
        let child_score = terminal_score(position, 1, context.limits.max_moves_to_draw)
            .or_else(|| declaration_score(position, 1))
            .or_else(|| legal_moves(position).is_empty().then_some(-MATE + 1))
            .unwrap_or_else(|| context.evaluator.evaluate(position));
        context.evaluator.undo();
        position.undo_move32(mv).expect("a fallback root move must be undoable");
        let score = -child_score;
        if !evaluated_any || score > best_score {
            best_move = mv;
            best_score = score;
        }
        evaluated_any = true;
    }
    (best_move, best_score)
}

/// rootのiterationの結果。
///
/// `complete`が偽なら、途中で打ち切られたが`alpha`を更新できた手が残っている。
pub(super) struct RootOutcome {
    pub(super) best_move: Move32,
    pub(super) score: i32,
    pub(super) complete: bool,
}

pub(super) fn search_root(
    position: &mut Position,
    moves: &mut [Move32],
    depth: u32,
    mut alpha: i32,
    beta: i32,
    context: &mut SearchContext,
) -> Option<RootOutcome> {
    let original_alpha = alpha;
    let mut best_move = moves[0];
    let key = tt_key(position, context.limits.max_moves_to_draw);
    if let Some(tt_move) = context.table.probe(key).and_then(|entry| entry.best_move)
        && let Some(index) = moves.iter().position(|mv| *mv == tt_move)
    {
        moves.swap(0, index);
    }

    let partial = |alpha: i32, best_move: Move32| {
        (alpha > original_alpha).then_some(RootOutcome { best_move, score: alpha, complete: false })
    };

    for (index, mv) in moves.iter().copied().enumerate() {
        if context.should_stop() {
            return partial(alpha, best_move);
        }
        position.apply_move32(mv);
        context.evaluator.advance(position, mv);
        let child_depth = depth.saturating_sub(1);
        let child = if index == 0 {
            search_child(position, child_depth, alpha, beta, 1, [None; 2], context)
        } else {
            match search_child(position, child_depth, alpha, alpha + 1, 1, [None; 2], context) {
                Some(score) if score > alpha && score < beta => {
                    search_child(position, child_depth, alpha, beta, 1, [None; 2], context)
                }
                other => other,
            }
        };
        position.undo_move32(mv).expect("a searched move must be undoable");
        context.evaluator.undo();
        let Some(score) = child else {
            return partial(alpha, best_move);
        };

        if score > alpha {
            alpha = score;
            best_move = mv;
        }
        if score >= beta {
            context.table.store(TtEntry {
                key,
                depth,
                score: score_to_tt(score, 0),
                bound: Bound::Lower,
                best_move: Some(best_move),
                static_eval: None,
            });
            return Some(RootOutcome { best_move, score, complete: true });
        }
    }

    context.table.store(TtEntry {
        key,
        depth,
        score: score_to_tt(alpha, 0),
        bound: if alpha > original_alpha { Bound::Exact } else { Bound::Upper },
        best_move: Some(best_move),
        static_eval: None,
    });
    Some(RootOutcome { best_move, score: alpha, complete: true })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rsshogi::board;

    use super::super::INF;
    use super::super::test_support::test_context;
    use super::*;

    #[test]
    fn an_aborted_root_iteration_keeps_the_move_that_improved_alpha() {
        let mut position = board::hirate_position();
        let mut moves = legal_moves(&position);

        // まず打ち切りなしで一回分のnode数を測り、その途中で止まる予算を選ぶ。
        let counted = Arc::new(AtomicU64::new(0));
        let mut context = test_context(Arc::clone(&counted), None);
        context.evaluator.initialize(&position);
        search_root(&mut position, &mut moves, 2, -INF, INF, &mut context).expect("full iteration");
        let full = counted.load(Ordering::Relaxed);
        assert!(full > 4, "iterationが短すぎて打ち切りを試せない");

        let nodes = Arc::new(AtomicU64::new(0));
        let mut context = test_context(Arc::clone(&nodes), Some(full / 2));
        context.evaluator.initialize(&position);
        let outcome = search_root(&mut position, &mut moves, 2, -INF, INF, &mut context)
            .expect("the budget must cover at least one completed root move");

        assert!(!outcome.complete, "the node budget must cut this iteration short");
        assert!(moves.contains(&outcome.best_move));
        assert!(position.is_legal_move32(outcome.best_move));
    }
}

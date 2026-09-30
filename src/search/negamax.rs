//! 主探索のnegamaxと、そこに属する枝刈りの判定を担う。

use rsshogi::board::{Captures, Legal, Move32List, Position, generate_moves_move32};

use crate::position::MAX_SEARCH_PLY;
use crate::see::static_exchange_eval;
use crate::tt::{Bound, TtEntry};

use super::context::{SearchContext, tt_key};
use super::history::{TriedQuiets, piece_to_index};
use super::lmr::lmr_reduction;
use super::ordering::{MovePicker, OrderedMove, OrderingInputs, is_legal_generated};
use super::pruning::{
    iir_depth, null_move_reduction_amount, should_futility_prune_quiets, should_prune_by_history,
    should_prune_capture_by_see, should_prune_late_quiet, should_prune_late_quiet_drop,
    should_reverse_futility_prune, should_try_null_move, should_try_singular,
};
use super::qsearch::{qsearch, structural_leaf_score};
use super::score::{
    bound_after_search, declaration_score, score_from_tt, score_to_tt, terminal_score, tt_cutoff,
};
use super::{INF, MATE, MATE_TT_THRESHOLD};

/// 子局面を探索し、手番側から見た値へ符号を戻す。
pub(super) fn search_child(
    position: &mut Position,
    depth: u32,
    alpha: i32,
    beta: i32,
    ply: u32,
    static_eval_history: [Option<i32>; 2],
    context: &mut SearchContext,
) -> Option<i32> {
    negamax(position, depth, -beta, -alpha, ply, static_eval_history, context).map(|v| -v)
}

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
        return Some(structural_leaf_score(position, ply, &mut context.evaluator));
    }

    // singularの判定探索では、TT moveを除いた残りの手だけを読む。
    // このnodeの結果はTT moveを含む本来の値ではないので、TTでは打ち切らず保存もしない。
    let excluded = context.excluded.get(ply as usize).copied().flatten();
    let key = tt_key(position, context.limits.max_moves_to_draw);
    let tt_entry = context.table.probe(key);
    let tt_move = tt_entry.and_then(|entry| entry.best_move);
    // PV nodeではTTで打ち切らない。打ち切るとPVが途中で切れ、
    // 以前の窓で得た値が千日手の判定などを経ずにPV上へ戻ってくる。
    if let Some(entry) = tt_entry
        && entry.depth >= depth
        && !pv_node
        && excluded.is_none()
        && let Some(score) = tt_cutoff(&entry, ply, &mut alpha, beta)
    {
        return Some(score);
    }
    let search_alpha = alpha;
    // IIR: TT moveの無い深いノードは良い並べ替えを欠いて高くつくので、
    // 1浅く探索して次のvisitへTT moveを残す。
    let depth = if excluded.is_some() {
        depth
    } else {
        iir_depth(depth, tt_move.is_some(), context.params.iir_min_depth)
    };

    // TT moveは別の局面のentryと衝突し得るので、合法なときだけ使う。
    let tt_move = tt_move.filter(|&mv| position.is_legal_move32(mv));
    let in_check = position.is_in_check();
    // 王手されていないnodeでは、手を段階ごとに必要になった時点で生成する(MovePicker)。
    // 王手回避と、除外手つきの判定探索では全合法手を先に生成する。
    // 合法手はheapを使わないstack上のリストへ直接生成する。
    let staged = !in_check && excluded.is_none();
    let mut move_list = Move32List::new();
    if !staged {
        generate_moves_move32::<Legal>(position, &mut move_list);
        if move_list.as_slice().is_empty() {
            return Some(-MATE + ply as i32);
        }
    }
    // TT entryに静的評価が残っていれば、高価な再評価を省いてそれを使う。
    // TTへ書き戻すのはこの生の値で、補正は枝刈り判定にだけ効かせる。
    let raw_static_eval = if in_check {
        None
    } else {
        tt_entry
            .and_then(|entry| entry.static_eval)
            .or_else(|| Some(context.evaluator.evaluate(position)))
    };
    // correction history: 同じ特徴を持つ局面で観測した「探索結果 - 静的評価」を足し戻す。
    let static_eval = raw_static_eval.map(|raw| context.corrected_eval(position, raw));
    // improvingは「2手前の同手番より評価が良い」。どちらか欠けていれば
    // improving扱いにしない(LMP・RFPの判定と共通)。
    let improving = static_eval
        .zip(static_eval_history[0])
        .is_some_and(|(current, previous)| current > previous);
    let child_static_eval_history = [static_eval_history[1], static_eval];
    if excluded.is_none()
        && let Some(eval) = static_eval
        && should_reverse_futility_prune(depth, pv_node, improving, eval, beta, &context.params)
    {
        return Some(eval);
    }
    if excluded.is_none()
        && let Some(eval) = static_eval
        && should_try_null_move(zero_window, in_check, eval, beta, position.plies_from_null())
        && position.try_apply_search_null_move().is_ok()
    {
        context.set_continuation(ply, None);
        let reduction = null_move_reduction_amount(depth, eval, beta, &context.params);
        let child_depth = depth.saturating_sub(1 + reduction);
        let child =
            search_child(position, child_depth, beta - 1, beta, ply + 1, [None; 2], context);
        position.undo_search_null_move().expect("a search null move must be undoable");
        let score = child?;
        if score >= beta {
            // 深いノードのnull cutは千日手・受け無し形の誤検出が高くつくので、
            // nullなしの検証探索が同じくβを超えることを確かめてからカットする。
            // 検証は同一ノードのdepth - reduction再帰で、深さが真に減るので停止する。
            let verified = if depth as i32 >= context.params.null_move_verify_depth {
                negamax(
                    position,
                    depth.saturating_sub(reduction),
                    beta - 1,
                    beta,
                    ply,
                    static_eval_history,
                    context,
                )?
            } else {
                score
            };
            if verified >= beta {
                context.table.store(TtEntry {
                    key,
                    depth,
                    score: score_to_tt(beta, ply),
                    bound: Bound::Lower,
                    best_move: None,
                    static_eval: raw_static_eval,
                });
                return Some(beta);
            }
        }
    }
    // ProbCut: βを大きく超える捕獲が浅い探索でも超えるなら、このnodeもβを超えると見なす。
    // 候補はSEEで上乗せ分を賄える捕獲に限り、静止探索で当たりを付けてから浅く確かめる。
    if !pv_node
        && !in_check
        && excluded.is_none()
        && depth >= context.params.probcut_min_depth as u32
        && beta.abs() < MATE_TT_THRESHOLD
        && let Some(eval) = static_eval
    {
        let probcut_beta = beta + context.params.probcut_margin;
        let tt_denies = tt_entry.is_some_and(|entry| {
            matches!(entry.bound, Bound::Upper | Bound::Exact)
                && entry.depth + 3 >= depth
                && score_from_tt(entry.score, ply) < probcut_beta
        });
        if !tt_denies {
            let eval_params = context.evaluator.params();
            // ProbCutは王手されていないnodeでだけ試すので、候補は合法な捕獲だけ生成する。
            let mut captures = Move32List::new();
            generate_moves_move32::<Captures>(position, &mut captures);
            for mv in captures.as_slice().iter().copied() {
                if !is_legal_generated(position, mv)
                    || !static_exchange_eval(position, mv, eval_params)
                        .is_some_and(|see| see >= probcut_beta - eval)
                {
                    continue;
                }
                position.apply_move32(mv);
                context.evaluator.advance(position, mv);
                context.set_continuation(ply, Some(piece_to_index(mv)));
                let mut probe =
                    qsearch(position, -probcut_beta, -probcut_beta + 1, ply + 1, 0, context)
                        .map(|v| -v);
                if probe.is_some_and(|score| score >= probcut_beta) {
                    probe = search_child(
                        position,
                        depth + 1 - context.params.probcut_min_depth as u32,
                        probcut_beta - 1,
                        probcut_beta,
                        ply + 1,
                        child_static_eval_history,
                        context,
                    );
                }
                position.undo_move32(mv).expect("a searched move must be undoable");
                context.evaluator.undo();
                let score = probe?;
                if score >= probcut_beta {
                    context.table.store(TtEntry {
                        key,
                        depth: depth + 2 - context.params.probcut_min_depth as u32,
                        score: score_to_tt(score, ply),
                        bound: Bound::Lower,
                        best_move: Some(mv),
                        static_eval: raw_static_eval,
                    });
                    return Some(score);
                }
            }
        }
    }
    // singular extension: TT moveを除いた手がどれもTTの値から十分下回るなら、
    // TT moveだけが局面を支えている。その手を1手深く読む。
    // 除いた手でもβを超えるなら、複数の手がβを超えるのでこのnodeを打ち切る(multi-cut)。
    let mut singular_extension = false;
    if excluded.is_none()
        && let Some(entry) = tt_entry
        && let Some(tt_mv) = tt_move
        && should_try_singular(
            depth,
            ply,
            context.root_depth,
            &entry,
            context.params.singular_min_depth,
        )
    {
        let singular_beta =
            score_from_tt(entry.score, ply) - context.params.singular_margin * depth as i32;
        context.excluded[ply as usize] = Some(tt_mv);
        let value = negamax(
            position,
            (depth - 1) / 2,
            singular_beta - 1,
            singular_beta,
            ply,
            static_eval_history,
            context,
        );
        context.excluded[ply as usize] = None;
        let value = value?;
        if value < singular_beta {
            singular_extension = true;
        } else if singular_beta >= beta && !pv_node {
            return Some(singular_beta);
        }
    }
    let inputs = OrderingInputs {
        eval_params: context.evaluator.params(),
        search_params: context.params,
        killers: context.killers.get(ply as usize).copied().unwrap_or([None; 2]),
        previous: context.previous_continuations(ply),
    };
    let ordering_buffer = context.take_ordering_buffer(ply);
    let mut picker = if staged {
        MovePicker::staged(tt_move, inputs, ordering_buffer)
    } else {
        MovePicker::new(
            position,
            move_list.as_slice(),
            tt_move,
            inputs,
            &context.history,
            ordering_buffer,
        )
    };

    let futility =
        should_futility_prune_quiets(depth, in_check, static_eval, alpha, &context.params);
    let mut best_score = -INF;
    let mut best_move = None;
    let mut best_is_quiet = false;
    let mut eligible_quiets_seen = 0;
    let mut eligible_drops_seen = 0;
    let mut pruned_move = false;
    let mut reduced_move_without_research = false;
    // このノードで試したがalphaを上げられなかった静かな手。βカット時にペナルティを受ける。
    let mut tried_quiets = TriedQuiets::new();

    let mut yielded = 0usize;
    while let Some(ordered) = picker.next(position, &context.history) {
        if Some(ordered.mv) == excluded {
            continue;
        }
        let index = yielded;
        yielded += 1;
        let OrderedMove { mv, metadata, see, quiet, history_score } = ordered;
        // 捕獲のSEEはMovePickerが取り出し時に払っているので、ここでの判定は
        // 追加計算を伴わない。大きく損な捕獲は浅いnon-PVノードでは読まない。
        if index > 0 && should_prune_capture_by_see(depth, pv_node, in_check, see, &context.params)
        {
            pruned_move = true;
            continue;
        }
        // 王手判定は並べ替えでは払わず、枝刈り判定に使う静かな手についてだけ
        // ここで遅延計算する。捕獲・成りの王手判定はこのノードでは使わない。
        let gives_check = quiet && position.gives_check_move32(mv);
        if futility && quiet && !gives_check && index > 0 {
            pruned_move = true;
            continue;
        }
        if index > 0
            && quiet
            && !gives_check
            && should_prune_by_history(depth, pv_node, in_check, history_score, &context.params)
        {
            pruned_move = true;
            continue;
        }
        // 打ち駒は盤上の静かな手とは別に数え、より厳しい上限で刈る。
        // 中盤の分岐の大半は静かな打ち駒で、その多くは読む価値が無い。
        let quiet_drop = quiet && metadata.is_drop() && !gives_check;
        let lmp_eligible = quiet && !metadata.is_drop() && !gives_check;
        let prune_by_lmp = if quiet_drop {
            should_prune_late_quiet_drop(
                depth,
                pv_node,
                in_check,
                improving,
                eligible_drops_seen,
                &context.params,
            )
        } else {
            lmp_eligible
                && should_prune_late_quiet(
                    depth,
                    pv_node,
                    in_check,
                    improving,
                    eligible_quiets_seen,
                    &context.params,
                )
        };
        if lmp_eligible {
            eligible_quiets_seen += 1;
        }
        if quiet_drop {
            eligible_drops_seen += 1;
        }
        if prune_by_lmp {
            pruned_move = true;
            continue;
        }
        let child_depth = depth - 1 + u32::from(singular_extension && Some(mv) == tt_move);
        position.apply_move32(mv);
        context.evaluator.advance(position, mv);
        context.set_continuation(ply, Some(piece_to_index(mv)));
        let mut reduced_without_research = false;
        let child = if index == 0 {
            search_child(
                position,
                child_depth,
                alpha,
                beta,
                ply + 1,
                child_static_eval_history,
                context,
            )
        } else {
            // 対数LMR: 遅い静かな手ほど、深い局面ほど大きく縮小する。
            // 王手になる静かな手は、応手が限られて結論が早く出やすいので1段浅く縮める。
            let reduction = if quiet
                && !in_check
                && depth >= 2
                && index >= context.params.lmr_min_index as usize + usize::from(pv_node)
            {
                (lmr_reduction(
                    context.lmr.base(depth, index),
                    pv_node,
                    improving,
                    history_score,
                    depth,
                    &context.params,
                ) - i32::from(gives_check) * context.params.check_lmr_offset)
                    .max(0)
            } else if see.is_some_and(|exchange| exchange < 0)
                && !in_check
                && depth >= 3
                && index >= context.params.lmr_min_index as usize
            {
                // SEEで損と分かっている捕獲も、遅い手なら静かな手より1段浅く縮める。
                // 縮めた探索がalphaを超えれば、通常どおり全深さで読み直す。
                (context.lmr.base(depth, index) - context.params.bad_capture_lmr_offset)
                    .clamp(1, depth as i32 - 2)
            } else {
                0
            };
            let reduced = reduction > 0;
            let mut scout = search_child(
                position,
                child_depth - reduction as u32,
                alpha,
                alpha + 1,
                ply + 1,
                child_static_eval_history,
                context,
            );
            if reduced && scout.is_some_and(|score| score > alpha) {
                scout = search_child(
                    position,
                    child_depth,
                    alpha,
                    alpha + 1,
                    ply + 1,
                    child_static_eval_history,
                    context,
                );
            } else if reduced {
                reduced_without_research = true;
            }
            match scout {
                Some(score) if score > alpha && score < beta => search_child(
                    position,
                    child_depth,
                    alpha,
                    beta,
                    ply + 1,
                    child_static_eval_history,
                    context,
                ),
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
            best_is_quiet = quiet;
        }

        if score >= beta {
            if excluded.is_none()
                && let Some(raw) = raw_static_eval
                && should_record_correction(in_check, Bound::Lower, score, raw, quiet)
            {
                context.history.corrections.record(position, depth, score - raw);
            }
            if quiet {
                context.record_quiet_cutoff(
                    position.turn(),
                    mv,
                    tried_quiets.as_slice(),
                    depth,
                    ply,
                );
            }
            context.recycle_ordering_buffer(ply, picker.into_buffer());
            if excluded.is_none() {
                context.table.store(TtEntry {
                    key,
                    depth,
                    score: score_to_tt(score, ply),
                    bound: Bound::Lower,
                    best_move: Some(mv),
                    static_eval: raw_static_eval,
                });
            }
            return Some(score);
        }
        if quiet && score <= alpha {
            tried_quiets.push(mv);
        }
        alpha = alpha.max(score);
    }
    context.recycle_ordering_buffer(ply, picker.into_buffer());
    // 段階生成で一手も出なかったnodeは、王手されていないが合法手がない。
    if staged && yielded == 0 {
        return Some(-MATE + ply as i32);
    }

    if excluded.is_some() {
        return Some(alpha);
    }
    // 枝刈り・再探索なしの縮小があっても常に保存する。ただしそのようなノードの
    // 窓内の結果は真値とずれ得るので、Exactは主張せずUpperへ丸める。
    let selective = pruned_move || reduced_move_without_research;
    let bound = stored_bound(alpha, search_alpha, selective);
    if let Some(raw) = raw_static_eval
        && should_record_correction(in_check, bound, alpha, raw, best_is_quiet)
    {
        context.history.corrections.record(position, depth, alpha - raw);
    }
    context.table.store(TtEntry {
        key,
        depth,
        score: score_to_tt(alpha, ply),
        bound,
        best_move,
        static_eval: raw_static_eval,
    });
    Some(alpha)
}

/// correction historyへ観測を記録してよいノードかの判定。
///
/// 王手中は静的評価が無く、捕獲・成りが最善のノードは静的評価との差が
/// 交換の結果に支配されて系統誤差の観測にならない。詰みスコア帯も除く。
/// boundは「静的評価が誤っていた向き」と一致するものだけを信じる。
fn should_record_correction(
    in_check: bool,
    bound: Bound,
    score: i32,
    raw_eval: i32,
    best_is_quiet: bool,
) -> bool {
    if in_check || !best_is_quiet || score.abs() >= MATE_TT_THRESHOLD {
        return false;
    }
    match bound {
        Bound::Exact => true,
        Bound::Lower => score > raw_eval,
        Bound::Upper => score < raw_eval,
    }
}

/// 探索し終えたノードをTTへ保存するときのbound。
///
/// 窓内で真にalphaを上げたノードだけがExactを主張できる。枝刈りや
/// 再探索なしの縮小で手を省いたノード(`selective`)の窓内の値は
/// 真値を保証しないため、安全側のUpperへ丸める。fail-lowは常にUpper。
fn stored_bound(alpha: i32, search_alpha: i32, selective: bool) -> Bound {
    if selective { Bound::Upper } else { bound_after_search(alpha, search_alpha) }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rsshogi::board;

    use crate::eval::EvalParams;
    use crate::tt::TranspositionTable;

    use super::super::test_support::test_context;
    use super::*;

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
        let key = tt_key(&position, 0);
        let table = Arc::new(TranspositionTable::new(1));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.limits.max_depth = 4;
        context.table = Arc::clone(&table);

        // βを静的評価ちょうどに置くと、reverse futilityのmarginには届かず
        // null moveの前提(eval >= beta)だけが成り立つ。
        let eval = crate::eval::evaluate_material(&position, EvalParams::default());
        assert_eq!(
            negamax(&mut position, 4, eval - 1, eval, 1, [None; 2], &mut context),
            Some(eval)
        );
        let stored = table.probe(key).expect("null cutoff should store a bound");
        assert_eq!(stored.bound, Bound::Lower);
        assert_eq!(stored.best_move, None);
    }

    #[test]
    fn correction_is_recorded_only_when_the_bound_agrees_with_the_error() {
        // Exactは常に信じる。
        assert!(should_record_correction(false, Bound::Exact, 100, 0, true));
        assert!(should_record_correction(false, Bound::Exact, -100, 0, true));
        // Lowerは「静的評価より高い」観測だけ、Upperは「低い」観測だけ。
        assert!(should_record_correction(false, Bound::Lower, 100, 0, true));
        assert!(!should_record_correction(false, Bound::Lower, -100, 0, true));
        assert!(should_record_correction(false, Bound::Upper, -100, 0, true));
        assert!(!should_record_correction(false, Bound::Upper, 100, 0, true));
        // 王手中・捕獲が最善・詰みスコア帯は観測しない。
        assert!(!should_record_correction(true, Bound::Exact, 100, 0, true));
        assert!(!should_record_correction(false, Bound::Exact, 100, 0, false));
        assert!(!should_record_correction(false, Bound::Exact, MATE_TT_THRESHOLD, 0, true));
        assert!(!should_record_correction(false, Bound::Exact, -MATE_TT_THRESHOLD, 0, true));
    }

    #[test]
    fn selective_lmp_result_is_not_stored_as_an_exact_tt_entry() {
        let mut position = board::hirate_position();
        for usi in ["7g7f", "3c3d"] {
            let mv = board::move_from_usi(&position, usi).expect("legal setup move");
            position.apply_move32(mv);
        }
        let key = tt_key(&position, 0);
        let table = Arc::new(TranspositionTable::new(1));
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.limits.max_depth = 2;
        context.table = Arc::clone(&table);

        assert_eq!(
            negamax(&mut position, 2, 500, 501, 1, [Some(1_000), None], &mut context),
            Some(500),
        );
        let stored = table.probe(key).expect("枝刈りが起きたノードもTTへは保存される");
        assert_ne!(stored.bound, Bound::Exact, "枝刈りが起きたノードはExactを主張できない");
    }

    #[test]
    fn stored_bound_demotes_a_selective_in_window_result_to_upper() {
        // 窓内で真にalphaを上げたノードだけがExact。
        assert_eq!(stored_bound(10, 5, false), Bound::Exact);
        assert_eq!(stored_bound(10, 5, true), Bound::Upper, "枝刈り・未再探索縮小はUpperへ丸める");
        assert_eq!(stored_bound(5, 5, false), Bound::Upper, "fail-lowは常にUpper");
        assert_eq!(stored_bound(5, 5, true), Bound::Upper);
    }

    #[test]
    fn negamax_reuses_a_stored_static_eval_instead_of_evaluating() {
        let mut position = board::hirate_position();
        let table = Arc::new(TranspositionTable::new(1));
        // depthが浅くbound cutは起きないが、static_evalだけ残っているentry。
        table.store(TtEntry {
            key: tt_key(&position, 0),
            depth: 0,
            score: score_to_tt(0, 1),
            bound: Bound::Upper,
            best_move: None,
            static_eval: Some(10_000),
        });
        let mut context = test_context(Arc::new(AtomicU64::new(0)), None);
        context.limits.max_depth = 2;
        context.table = table;

        // material評価なら平手は0でreverse futilityは発火しない。
        // 10_000が返るのはTTのstatic_evalを使った証拠。
        assert_eq!(negamax(&mut position, 2, 0, 1, 1, [None; 2], &mut context), Some(10_000));
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

//! 主探索のnegamaxと、そこに属する枝刈りの判定を担う。

use rsshogi::board::{Move32List, Position, generate_legal_all_move32};

use crate::position::MAX_SEARCH_PLY;
use crate::tt::{Bound, TtEntry};

use super::context::{SearchContext, tt_key};
use super::history::{TriedQuiets, piece_to_index};
use super::ordering::{MovePicker, OrderedMove};
use super::qsearch::{qsearch, structural_leaf_score};
use super::score::{
    bound_after_search, declaration_score, score_from_tt, score_to_tt, terminal_score,
};
use super::{CaptureSee, INF, MATE, MATE_TT_THRESHOLD};

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
    // IIR: TT moveの無い深いノードは良い並べ替えを欠いて高くつくので、
    // 1浅く探索して次のvisitへTT moveを残す。
    let depth = iir_depth(depth, tt_move.is_some());

    // 合法手はheapを使わないstack上のリストへ直接生成する。
    let mut move_list = Move32List::new();
    generate_legal_all_move32(position, &mut move_list);
    if move_list.as_slice().is_empty() {
        return Some(-MATE + ply as i32);
    }
    let in_check = position.is_in_check();
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
    let static_eval = raw_static_eval.map(|raw| {
        raw + context.history.corrections.correction(position, context.params.correction_apply_max)
    });
    // improvingは「2手前の同手番より評価が良い」。どちらか欠けていれば
    // improving扱いにしない(LMP・RFPの判定と共通)。
    let improving = static_eval
        .zip(static_eval_history[0])
        .is_some_and(|(current, previous)| current > previous);
    if let Some(eval) = static_eval
        && should_reverse_futility_prune(
            depth,
            pv_node,
            improving,
            eval,
            beta,
            context.params.reverse_futility_margin,
        )
    {
        return Some(eval);
    }
    if let Some(eval) = static_eval
        && should_try_null_move(zero_window, in_check, eval, beta, position.plies_from_null())
        && position.try_apply_search_null_move().is_ok()
    {
        context.set_continuation(ply, None);
        let reduction =
            null_move_reduction_amount(depth, eval, beta, context.params.null_move_eval_divisor);
        let child_depth = depth.saturating_sub(1 + reduction);
        let child = negamax(position, child_depth, -beta, -beta + 1, ply + 1, [None; 2], context)
            .map(|v| -v);
        position.undo_search_null_move().expect("a search null move must be undoable");
        let score = child?;
        if score >= beta {
            // 深いノードのnull cutは千日手・受け無し形の誤検出が高くつくので、
            // nullなしの検証探索が同じくβを超えることを確かめてからカットする。
            // 検証は同一ノードのdepth - reduction再帰で、深さが真に減るので停止する。
            let verified = if depth >= 12 {
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
    let killers = context.killers.get(ply as usize).copied().unwrap_or([None; 2]);
    let ordering_buffer = context.take_ordering_buffer(ply);
    let previous_continuations = context.previous_continuations(ply);
    let mut picker = MovePicker::new(
        position,
        move_list.as_slice(),
        context.evaluator.params(),
        tt_move,
        killers,
        &context.history,
        &previous_continuations,
        context.params,
        ordering_buffer,
    );

    let futility = should_futility_prune_quiets(
        depth,
        in_check,
        static_eval,
        alpha,
        context.params.futility_margin,
    );
    let mut best_score = -INF;
    let mut best_move = None;
    let mut best_is_quiet = false;
    let mut eligible_quiets_seen = 0;
    let mut eligible_drops_seen = 0;
    let mut pruned_move = false;
    let mut reduced_move_without_research = false;
    let child_static_eval_history = [static_eval_history[1], static_eval];
    // このノードで試したがalphaを上げられなかった静かな手。βカット時にペナルティを受ける。
    let mut tried_quiets = TriedQuiets::new();

    let mut yielded = 0usize;
    while let Some(ordered) = picker.next(position) {
        let index = yielded;
        yielded += 1;
        let OrderedMove { mv, metadata, see, quiet, history_score } = ordered;
        // 捕獲のSEEはMovePickerが取り出し時に払っているので、ここでの判定は
        // 追加計算を伴わない。大きく損な捕獲は浅いnon-PVノードでは読まない。
        if index > 0
            && should_prune_capture_by_see(
                depth,
                pv_node,
                in_check,
                see,
                context.params.see_prune_margin,
            )
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
            && should_prune_by_history(
                depth,
                pv_node,
                in_check,
                history_score,
                context.params.history_prune_margin,
            )
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
                context.params.drop_lmp_divisor,
            )
        } else {
            should_prune_late_quiet(
                depth,
                pv_node,
                in_check,
                improving,
                eligible_quiets_seen,
                lmp_eligible,
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
        position.apply_move32(mv);
        context.evaluator.advance(position, mv);
        context.set_continuation(ply, Some(piece_to_index(mv)));
        let mut reduced_without_research = false;
        let child = if index == 0 {
            negamax(position, depth - 1, -beta, -alpha, ply + 1, child_static_eval_history, context)
                .map(|v| -v)
        } else {
            // 対数LMR: 遅い静かな手ほど、深い局面ほど大きく縮小する。
            let reduction = if quiet
                && !in_check
                && !gives_check
                && depth >= 2
                && index >= 2 + usize::from(pv_node)
            {
                lmr_reduction(
                    context.lmr.base(depth, index),
                    pv_node,
                    improving,
                    history_score,
                    depth,
                )
            } else {
                0
            };
            let reduced = reduction > 0;
            let mut scout = if reduced {
                negamax(
                    position,
                    depth - 1 - reduction as u32,
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
            best_is_quiet = quiet;
        }

        if score >= beta {
            if let Some(raw) = raw_static_eval
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
            context.table.store(TtEntry {
                key,
                depth,
                score: score_to_tt(score, ply),
                bound: Bound::Lower,
                best_move: Some(mv),
                static_eval: raw_static_eval,
            });
            return Some(score);
        }
        if quiet && score <= alpha {
            tried_quiets.push(mv);
        }
        alpha = alpha.max(score);
    }
    context.recycle_ordering_buffer(ply, picker.into_buffer());

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
        bound: stored_bound(alpha, search_alpha, selective),
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

/// IIR: TT moveが無く十分深いノードは1浅く探索する。
const fn iir_depth(depth: u32, has_tt_move: bool) -> u32 {
    if !has_tt_move && depth >= 4 { depth - 1 } else { depth }
}

/// Reverse futility: 静的評価がβをdepth比例のmargin以上上回る浅いノードは
/// 探索せず評価値で打ち切る。improvingなら1 depth分marginを緩和する。
fn should_reverse_futility_prune(
    depth: u32,
    pv_node: bool,
    improving: bool,
    static_eval: i32,
    beta: i32,
    margin_per_depth: i32,
) -> bool {
    let margin_depth = depth.saturating_sub(u32::from(improving));
    depth <= 8 && !pv_node && static_eval - margin_per_depth * margin_depth as i32 >= beta
}

/// Futility: 静的評価がαへdepth比例のmargin分届かない浅いノードでは、
/// 最初の手を除く静かな非王手手を刈る(適用はムーブループ側)。
fn should_futility_prune_quiets(
    depth: u32,
    in_check: bool,
    static_eval: Option<i32>,
    alpha: i32,
    margin_per_depth: i32,
) -> bool {
    depth <= 4
        && !in_check
        && static_eval.is_some_and(|eval| eval + margin_per_depth * depth as i32 <= alpha)
}

/// SEE枝刈り: 浅いnon-PVノードでは、交換値が`-margin * depth^2`を下回る捕獲を読まない。
///
/// `see`はMovePickerが取り出し時に計算済みの値で、捕獲以外は`None`。
/// 王手中は回避手を減らせないので発火しない。
fn should_prune_capture_by_see(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    see: CaptureSee,
    margin: i32,
) -> bool {
    if pv_node || in_check || depth > 6 {
        return false;
    }
    let threshold = -margin * (depth * depth) as i32;
    see.is_some_and(|exchange| exchange < threshold)
}

/// history枝刈り: 浅いnon-PVノードでは、履歴が`-margin * depth`を下回る静かな手を読まない。
///
/// 履歴は±`HISTORY_MAX`(16_384)へ飽和するので、閾値は「繰り返し失敗した手」だけを捉える。
fn should_prune_by_history(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    history_score: i32,
    margin: i32,
) -> bool {
    !pv_node && !in_check && depth <= 4 && history_score < -margin * depth as i32
}

/// LMPの上限手数 `(3 + depth^2) / (2 - improving)`。improvingなら倍許す。
fn late_move_prune_limit(depth: u32, improving: bool) -> usize {
    ((3 + depth * depth) / (2 - u32::from(improving))) as usize
}

/// 静かな打ち駒のLMP上限。盤上の静かな手の上限を`divisor`で割り、最低1手は残す。
///
/// `divisor == 0`は「打ち駒を刈らない」を表し、上限なし(`None`)を返す。
fn late_move_prune_drop_limit(depth: u32, improving: bool, divisor: i32) -> Option<usize> {
    if divisor <= 0 {
        return None;
    }
    Some((late_move_prune_limit(depth, improving) / divisor as usize).max(1))
}

fn should_prune_late_quiet_drop(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    improving: bool,
    eligible_drops_seen: usize,
    divisor: i32,
) -> bool {
    !pv_node
        && !in_check
        && late_move_prune_drop_limit(depth, improving, divisor)
            .is_some_and(|limit| eligible_drops_seen >= limit)
}

fn should_prune_late_quiet(
    depth: u32,
    pv_node: bool,
    in_check: bool,
    improving: bool,
    eligible_quiets_seen: usize,
    eligible: bool,
) -> bool {
    eligible
        && !pv_node
        && !in_check
        && eligible_quiets_seen >= late_move_prune_limit(depth, improving)
}

/// 表引きした縮小量の基礎値へ実行時の補正を加える。
///
/// PVノードは浅く、improvingでないノードは深く縮小する。履歴スコアは
/// ±16_384なので`/ 8_192`のclampは実質±1の補正になる。最終値は
/// `0..=depth-2`へclampし、縮小後の子の深さが1を下回らないようにする。
fn lmr_reduction(base: i32, pv_node: bool, improving: bool, history_score: i32, depth: u32) -> i32 {
    let mut reduction = base;
    if pv_node {
        reduction -= 1;
    }
    if !improving {
        reduction += 1;
    }
    reduction -= (history_score / 8_192).clamp(-1, 1);
    reduction.clamp(0, depth as i32 - 2)
}

fn should_try_null_move(
    zero_window: bool,
    in_check: bool,
    static_eval: i32,
    beta: i32,
    plies_from_null: u16,
) -> bool {
    zero_window
        && !in_check
        && beta > -MATE_TT_THRESHOLD
        && plies_from_null > 0
        && static_eval >= beta
}

/// null moveの動的縮小量 `R = 3 + depth/3 + min((static_eval - beta) / divisor, 3)`。
///
/// βを大きく上回るほど深く縮小する。前提により`static_eval >= beta`だが、
/// 負側もclampして守る。
fn null_move_reduction_amount(depth: u32, static_eval: i32, beta: i32, eval_divisor: i32) -> u32 {
    let eval_term = ((static_eval - beta) / eval_divisor.max(1)).clamp(0, 3) as u32;
    3 + depth / 3 + eval_term
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
    fn null_move_guard_requires_every_conservative_precondition() {
        assert!(should_try_null_move(true, false, 20, 20, 1));
        assert!(!should_try_null_move(false, false, 20, 20, 1));
        assert!(!should_try_null_move(true, true, 20, 20, 1));
        assert!(!should_try_null_move(true, false, 19, 20, 1));
        assert!(!should_try_null_move(true, false, 20, 20, 0));
        assert!(!should_try_null_move(true, false, 0, -MATE_TT_THRESHOLD, 1));
        assert!(should_try_null_move(true, false, 0, -MATE_TT_THRESHOLD + 1, 1));
    }

    #[test]
    fn null_move_reduction_grows_with_depth_and_eval_margin() {
        // R = 3 + depth/3 + min((eval-beta)/200, 3)
        assert_eq!(null_move_reduction_amount(4, 0, 0, 200), 4);
        assert_eq!(null_move_reduction_amount(12, 0, 0, 200), 7);
        assert_eq!(null_move_reduction_amount(12, 700, 0, 200), 10);
        assert_eq!(null_move_reduction_amount(12, 5_000, 0, 200), 10, "eval項は3でclampされる");
        assert_eq!(null_move_reduction_amount(6, -100, 0, 200), 5, "負のeval差は0へclampされる");
        assert_eq!(
            null_move_reduction_amount(6, 100, 0, 0),
            8,
            "divisor 0は1へ丸めて0除算を避ける"
        );
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
    fn see_pruning_skips_only_badly_losing_captures_in_shallow_non_pv_nodes() {
        let margin = 80;
        // 閾値は -margin * depth^2。depth 2なら-320。
        assert!(should_prune_capture_by_see(2, false, false, Some(-321), margin));
        assert!(!should_prune_capture_by_see(2, false, false, Some(-320), margin));
        assert!(
            !should_prune_capture_by_see(2, true, false, Some(-10_000), margin),
            "PVノードでは刈らない"
        );
        assert!(
            !should_prune_capture_by_see(2, false, true, Some(-10_000), margin),
            "王手中は刈らない"
        );
        assert!(
            !should_prune_capture_by_see(7, false, false, Some(-10_000), margin),
            "depth 7では発火しない"
        );
        assert!(
            !should_prune_capture_by_see(2, false, false, None, margin),
            "SEEの無い手(捕獲以外)は対象外"
        );
    }

    #[test]
    fn history_pruning_requires_a_strongly_negative_history_in_a_shallow_non_pv_node() {
        let margin = 2_000;
        assert!(should_prune_by_history(1, false, false, -2_001, margin));
        assert!(!should_prune_by_history(1, false, false, -2_000, margin));
        assert!(should_prune_by_history(4, false, false, -8_001, margin));
        assert!(!should_prune_by_history(4, false, false, -8_000, margin));
        assert!(
            !should_prune_by_history(5, false, false, -100_000, margin),
            "depth 5では発火しない"
        );
        assert!(!should_prune_by_history(1, true, false, -100_000, margin), "PVノードでは刈らない");
        assert!(!should_prune_by_history(1, false, true, -100_000, margin), "王手中は刈らない");
        assert!(!should_prune_by_history(1, false, false, 0, margin), "履歴の無い手は刈らない");
    }

    #[test]
    fn lmp_limit_grows_quadratically_and_doubles_when_improving() {
        // (3 + d^2) / (2 - improving)
        assert_eq!(late_move_prune_limit(1, false), 2);
        assert_eq!(late_move_prune_limit(1, true), 4);
        assert_eq!(late_move_prune_limit(3, false), 6);
        assert_eq!(late_move_prune_limit(4, false), 9);
        assert_eq!(late_move_prune_limit(4, true), 19);
        assert_eq!(late_move_prune_limit(8, false), 33, "深いノードにも上限がかかる");
    }

    #[test]
    fn quiet_drops_get_a_tighter_lmp_limit_than_board_quiets() {
        // 盤上の静かな手の上限をdivisorで割り、最低1手は残す。
        assert_eq!(late_move_prune_drop_limit(1, false, 2), Some(1));
        assert_eq!(late_move_prune_drop_limit(4, false, 2), Some(4));
        assert_eq!(late_move_prune_drop_limit(4, true, 2), Some(9));
        assert_eq!(late_move_prune_drop_limit(8, false, 2), Some(16));
        assert_eq!(late_move_prune_drop_limit(1, false, 6), Some(1), "上限は1手を下回らない");
        assert_eq!(late_move_prune_drop_limit(4, false, 0), None, "divisor 0は打ち駒を刈らない");
        assert!(
            late_move_prune_drop_limit(6, false, 2).expect("有効なdivisor")
                < late_move_prune_limit(6, false),
            "打ち駒の上限は盤上の静かな手より厳しい"
        );
    }

    #[test]
    fn quiet_drop_pruning_keeps_the_pv_and_check_evasion_paths() {
        assert!(should_prune_late_quiet_drop(4, false, false, false, 4, 2));
        assert!(!should_prune_late_quiet_drop(4, false, false, false, 3, 2));
        assert!(
            !should_prune_late_quiet_drop(4, true, false, false, 99, 2),
            "PVノードでは刈らない"
        );
        assert!(!should_prune_late_quiet_drop(4, false, true, false, 99, 2), "王手中は刈らない");
        assert!(
            !should_prune_late_quiet_drop(4, false, false, false, 99, 0),
            "divisor 0は打ち駒枝刈りを無効化する"
        );
    }

    #[test]
    fn lmp_requires_a_non_pv_unchecked_eligible_late_quiet() {
        assert!(should_prune_late_quiet(1, false, false, false, 2, true));
        assert!(!should_prune_late_quiet(1, false, false, false, 1, true,));
        assert!(!should_prune_late_quiet(1, true, false, false, 2, true), "PVノードでは刈らない");
        assert!(!should_prune_late_quiet(1, false, true, false, 2, true), "王手中は刈らない");
        assert!(!should_prune_late_quiet(1, false, false, true, 2, true), "improvingは上限が倍");
        assert!(should_prune_late_quiet(1, false, false, true, 4, true));
        assert!(
            !should_prune_late_quiet(1, false, false, false, 2, false),
            "対象外の手は数えるだけ"
        );
    }

    #[test]
    fn reverse_futility_covers_depth_8_and_relaxes_when_improving() {
        let margin = 100;
        assert!(should_reverse_futility_prune(8, false, false, 800, 0, margin));
        assert!(!should_reverse_futility_prune(8, false, false, 799, 0, margin));
        assert!(
            !should_reverse_futility_prune(9, false, false, 10_000, 0, margin),
            "depth 9では発火しない"
        );
        assert!(
            !should_reverse_futility_prune(8, true, false, 10_000, 0, margin),
            "PVノードでは発火しない"
        );
        assert!(
            should_reverse_futility_prune(8, false, true, 700, 0, margin),
            "improvingはmarginが1 depth分緩む"
        );
        assert!(!should_reverse_futility_prune(8, false, true, 699, 0, margin));
    }

    #[test]
    fn futility_covers_depth_4_with_a_depth_scaled_margin() {
        let margin = 100;
        assert!(should_futility_prune_quiets(4, false, Some(-400), 0, margin));
        assert!(!should_futility_prune_quiets(4, false, Some(-399), 0, margin));
        assert!(
            !should_futility_prune_quiets(5, false, Some(-10_000), 0, margin),
            "depth 5では発火しない"
        );
        assert!(
            !should_futility_prune_quiets(4, true, Some(-400), 0, margin),
            "王手中は発火しない"
        );
        assert!(!should_futility_prune_quiets(4, false, None, 0, margin));
        assert!(should_futility_prune_quiets(1, false, Some(-100), 0, margin));
    }

    #[test]
    fn iir_reduces_only_deep_nodes_without_a_tt_move() {
        assert_eq!(iir_depth(4, false), 3);
        assert_eq!(iir_depth(4, true), 4);
        assert_eq!(iir_depth(3, false), 3, "depth 3以下はそのまま");
        assert_eq!(iir_depth(12, false), 11);
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
            key: tt_key(&position, 1, 0),
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
    fn lmr_adjustments_shift_the_base_by_at_most_one_each() {
        // PVノードは-1、improvingでないと+1、履歴は±1。
        assert_eq!(lmr_reduction(2, false, true, 0, 8), 2);
        assert_eq!(lmr_reduction(2, true, true, 0, 8), 1);
        assert_eq!(lmr_reduction(2, false, false, 0, 8), 3);
        assert_eq!(lmr_reduction(2, false, true, 8_192, 8), 1);
        assert_eq!(lmr_reduction(2, false, true, -8_192, 8), 3);
        assert_eq!(lmr_reduction(2, false, true, 16_384, 8), 1, "履歴の補正は±1でclampされる");
        assert_eq!(lmr_reduction(2, false, true, -16_384, 8), 3, "履歴の補正は±1でclampされる");
    }

    #[test]
    fn lmr_reduction_clamps_into_the_valid_depth_window() {
        assert_eq!(lmr_reduction(0, true, true, 8_192, 8), 0, "負の補正でも0未満にはならない");
        assert_eq!(lmr_reduction(10, false, false, -16_384, 6), 4, "depth-2でclampされる");
        assert_eq!(lmr_reduction(3, false, false, 0, 2), 0, "depth 2では縮小しない");
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

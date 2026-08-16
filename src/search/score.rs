//! 終局・宣言・千日手の点数付けと、TTに出し入れする詰みscoreの変換を担う。

use rsshogi::board::Position;
use rsshogi::types::{MOVE_WIN, RepetitionState};

use crate::tt::Bound;

use super::{MATE, MATE_TT_THRESHOLD, REPETITION_SUPERIOR};

/// 探索でalphaを更新できたかどうかで、TTへ格納するboundを決める。
pub(super) fn bound_after_search(alpha: i32, search_alpha: i32) -> Bound {
    if alpha > search_alpha { Bound::Exact } else { Bound::Upper }
}

pub(super) fn terminal_score(position: &Position, ply: u32, max_moves_to_draw: u32) -> Option<i32> {
    let state = if ply == 0 {
        position.repetition_state()
    } else {
        position.repetition_state_with_ply(ply as usize)
    };
    let repetition = match state {
        RepetitionState::None => None,
        RepetitionState::Win => Some(MATE - ply as i32),
        RepetitionState::Lose => Some(-MATE + ply as i32),
        RepetitionState::Draw => Some(0),
        RepetitionState::Superior => Some(REPETITION_SUPERIOR),
        RepetitionState::Inferior => Some(-REPETITION_SUPERIOR),
    };
    repetition.or_else(|| {
        (max_moves_to_draw > 0 && u32::from(position.game_ply()) > max_moves_to_draw).then_some(0)
    })
}

/// rootでは対局が継続する優等・劣等を終局扱いにしない。
pub(super) fn root_terminal_score(position: &Position, max_moves_to_draw: u32) -> Option<i32> {
    let repetition = match position.repetition_state() {
        RepetitionState::None | RepetitionState::Superior | RepetitionState::Inferior => None,
        RepetitionState::Win => Some(MATE),
        RepetitionState::Lose => Some(-MATE),
        RepetitionState::Draw => Some(0),
    };
    repetition.or_else(|| {
        (max_moves_to_draw > 0 && u32::from(position.game_ply()) > max_moves_to_draw).then_some(0)
    })
}

pub(super) fn declaration_score(position: &Position, ply: u32) -> Option<i32> {
    (position.declaration_win_move() == MOVE_WIN).then_some(MATE - ply as i32)
}

pub(super) fn score_to_tt(score: i32, ply: u32) -> i32 {
    if score >= MATE_TT_THRESHOLD {
        score.saturating_add(ply as i32)
    } else if score <= -MATE_TT_THRESHOLD {
        score.saturating_sub(ply as i32)
    } else {
        score
    }
}

pub(super) fn score_from_tt(score: i32, ply: u32) -> i32 {
    if score >= MATE_TT_THRESHOLD {
        score.saturating_sub(ply as i32)
    } else if score <= -MATE_TT_THRESHOLD {
        score.saturating_add(ply as i32)
    } else {
        score
    }
}

pub(crate) fn mate_distance(score: i32) -> Option<i32> {
    if score.abs() < MATE_TT_THRESHOLD {
        return None;
    }
    let distance = (MATE - score.abs()).max(0);
    Some(if score > 0 { distance } else { -distance })
}

#[cfg(test)]
mod tests {
    use rsshogi::board;

    use crate::nnue::MAX_NNUE_EVAL;

    use super::super::SearchLimits;
    use super::super::test_support::run_result;
    use super::*;

    #[test]
    fn declaration_score_recognizes_supported_point_rule() {
        let mut point = board::position_from_sfen(
            "K+N5+L1/G+L+P+B1+R+P2/3+P2G2/9/2+p+n5/3s2+ss1/3+p+p1+s1+r/7g+n/6g+nk b 2L8Pb4p 1",
        )
        .expect("valid point-rule position");
        point.set_entering_king_rule(rsshogi::types::EnteringKingRule::Point27);
        assert_eq!(declaration_score(&point, 3), Some(MATE - 3));
    }

    #[test]
    fn repetition_superiority_is_decisive_but_not_mate() {
        assert_eq!(mate_distance(REPETITION_SUPERIOR), None);
        assert_eq!(score_to_tt(REPETITION_SUPERIOR, 7), REPETITION_SUPERIOR);
    }

    #[test]
    fn root_searches_an_existing_inferior_repetition_state() {
        let mut position = board::position_from_sfen(
            "lr6l/4g1pkp/2ns1s3/4pp1SP/PPBP2Pp1/2gpPP3/2B2S3/2K6/L6RL b 4P2g3np 1",
        )
        .expect("valid inferior-repetition position");
        for text in ["P*2c", "2b2a", "2c2b+", "2a2b"] {
            let mv = board::move_from_usi(&position, text).expect("legal repetition move");
            position.apply_move32(mv);
        }

        assert_eq!(position.repetition_state(), RepetitionState::Inferior);
        assert_eq!(terminal_score(&position, 0, 0), Some(-REPETITION_SUPERIOR));
        assert_eq!(root_terminal_score(&position, 0), None);

        let result = run_result(
            position.clone_for_search(),
            SearchLimits {
                max_depth: 1,
                max_nodes: None,
                deadline: None,
                searchmoves: Vec::new(),
                max_moves_to_draw: 0,
            },
            1,
        );
        assert_eq!(result.depth, 1);
        assert!(result.nodes > 0);
        assert!(result.best_move.is_some_and(|mv| position.is_legal_move32(mv)));
    }

    #[test]
    fn normal_repetition_is_scored_as_draw() {
        let mut position = board::hirate_position();
        for _ in 0..3 {
            for text in ["2h3h", "8b7b", "3h2h", "7b8b"] {
                let mv = board::move_from_usi(&position, text).expect("legal repetition move");
                position.apply_move32(mv);
            }
        }

        assert_eq!(position.repetition_state(), RepetitionState::Draw);
        assert_eq!(terminal_score(&position, 5, 0), Some(0));
        assert_eq!(root_terminal_score(&position, 0), Some(0));
    }

    #[test]
    fn a_repetition_created_inside_the_search_line_is_terminal() {
        let mut position = board::hirate_position();
        for text in ["2h3h", "8b7b", "3h2h", "7b8b"] {
            let mv = board::move_from_usi(&position, text).expect("legal repetition move");
            position.apply_move32(mv);
        }

        assert_eq!(position.repetition_state(), RepetitionState::None);
        assert_eq!(terminal_score(&position, 3, 0), None);
        assert_eq!(terminal_score(&position, 4, 0), Some(0));
    }

    #[test]
    fn perpetual_check_is_scored_as_loss_for_the_checking_side() {
        let mut position = board::position_from_sfen(
            "lr2+R2G1/6g1k/p3ppppp/2p1b4/9/2P+b4P/P+p3PPP1/4G1SK1/LN3+p1NL b 2SNLgsn3p 1",
        )
        .expect("valid perpetual-check position");
        for _ in 0..3 {
            for text in ["2a1a", "1b2b", "1a2a", "2b1b"] {
                let mv = board::move_from_usi(&position, text).expect("legal checking sequence");
                position.apply_move32(mv);
            }
        }

        assert_eq!(position.repetition_state(), RepetitionState::Lose);
        assert_eq!(terminal_score(&position, 5, 0), Some(-MATE + 5));
    }

    #[test]
    fn perpetual_check_created_inside_the_search_line_is_terminal() {
        let mut position = board::position_from_sfen(
            "lr2+R2G1/6g1k/p3ppppp/2p1b4/9/2P+b4P/P+p3PPP1/4G1SK1/LN3+p1NL b 2SNLgsn3p 1",
        )
        .expect("valid perpetual-check position");
        for text in ["2a1a", "1b2b", "1a2a", "2b1b"] {
            let mv = board::move_from_usi(&position, text).expect("legal checking move");
            position.apply_move32(mv);
        }

        assert_eq!(position.repetition_state(), RepetitionState::None);
        assert_eq!(terminal_score(&position, 4, 0), Some(-MATE + 4));
    }

    #[test]
    fn mate_scores_roundtrip_at_a_different_ply() {
        let stored = score_to_tt(MATE - 7, 7);
        assert_eq!(score_from_tt(stored, 3), MATE - 3);
        let stored_loss = score_to_tt(-MATE + 7, 7);
        assert_eq!(score_from_tt(stored_loss, 3), -MATE + 3);
    }

    #[test]
    fn maximum_static_eval_is_not_normalized_as_mate() {
        assert_eq!(score_to_tt(MAX_NNUE_EVAL, 17), MAX_NNUE_EVAL);
        assert_eq!(score_from_tt(-MAX_NNUE_EVAL, 17), -MAX_NNUE_EVAL);
    }

    #[test]
    fn mate_distance_uses_signed_ply_distance() {
        assert_eq!(mate_distance(MATE - 3), Some(3));
        assert_eq!(mate_distance(-MATE + 4), Some(-4));
        assert_eq!(mate_distance(MAX_NNUE_EVAL), None);
    }

    #[test]
    fn tt_lower_bound_without_a_searched_improvement_stays_upper() {
        // TTのlower boundでalphaが持ち上がっても、探索がそれを超えられなければUpperのまま。
        assert_eq!(bound_after_search(40, 40), Bound::Upper);
        assert_eq!(bound_after_search(41, 40), Bound::Exact);
    }
}

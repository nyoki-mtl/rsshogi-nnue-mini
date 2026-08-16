use rsshogi::board::Position;
use rsshogi::types::{Bitboard, Color, Move32, PieceType, Square};

use crate::eval::EvalParams;

/// 盤上の駒は最大40枚なので、取り合いの列はこれ以上長くならない。
const MAX_EXCHANGE_PLIES: usize = 40;

/// 取り合いをswap列として畳み込む、局面を進めないstatic exchange evaluation。
///
/// 各手番で最も安い攻撃駒を選び、occupancyから外しては攻撃駒を求め直すことで
/// x-rayを追う。後ろから畳み込むので、どちらの側も不利な取り返しを降りられる。
/// 返す値はroot手番から見た駒得で、盤上の駒を失うことと持ち駒が増えることの
/// 両方を数える。捕獲でない手には`None`を返す。
///
/// 合法な取り返しを全探索する評価と比べて、近似が二つある。
///
/// - pinは開始局面の`blockers_for_king`でだけ見る。取り合いが進んで駒が減った
///   結果として生じたり解けたりするpinは追わない。
/// - 生成器を使わないので、王手がかかっている局面でも取り合いだけを見る。
///
/// 玉での取り返しだけは、相手に攻撃駒が残っている間は行わない。
/// 合法手生成を使わない実装で、これを外すと玉が自殺する読み筋を評価してしまう。
pub fn static_exchange_eval(position: &Position, mv: Move32, params: EvalParams) -> Option<i32> {
    let metadata = position.move32_metadata(mv);
    if !metadata.is_capture() {
        return None;
    }
    let target = mv.to_sq();
    let from = mv.from_sq();
    if from.is_none() {
        // 打ちは捕獲にならないため、ここへは来ない。
        return None;
    }

    let mover = position.turn();
    let mut occupied = position.pieces();
    occupied.clear(from);

    let mut gains = [0i32; MAX_EXCHANGE_PLIES];
    gains[0] = capture_gain(metadata.captured_piece_type(), params)
        + promotion_gain(
            position.moved_piece_before(mv).piece_type(),
            metadata.is_promotion(),
            params,
        );
    // 取り返されたときに相手が得る、いま対象マスに立っている駒の価値。
    let mut standing = capture_gain(position.moved_piece_after(mv).piece_type(), params);

    let mut side = mover.flip();
    let mut depth = 0usize;
    while depth + 1 < MAX_EXCHANGE_PLIES {
        let attackers = position.attackers_to(target, occupied) & occupied;
        let Some((square, piece_type)) =
            least_valuable_attacker(position, attackers, target, side, params)
        else {
            break;
        };
        if piece_type == PieceType::KING
            && !(attackers & position.pieces_by_color(side.flip())).is_empty()
        {
            // 玉の移動先ではpinされた駒も利きとして数える。玉を取る手が先に成立するため。
            break;
        }

        let promotes = promotes(piece_type, square, target, side);
        depth += 1;
        gains[depth] = standing + promotion_gain(piece_type, promotes, params) - gains[depth - 1];
        standing = capture_gain(if promotes { piece_type.promote() } else { piece_type }, params);
        occupied.clear(square);
        side = side.flip();
    }

    while depth > 0 {
        gains[depth - 1] = -(-gains[depth - 1]).max(gains[depth]);
        depth -= 1;
    }
    Some(gains[0])
}

/// 駒を取ったときの駒得。盤上から消える分と持ち駒に増える分の両方を数える。
fn capture_gain(victim: PieceType, params: EvalParams) -> i32 {
    params.piece_value(victim) + params.piece_value(victim.demote())
}

/// 成ったときに増える盤上の価値。持ち駒側は成りを持ち越さないので一度だけ数える。
fn promotion_gain(piece_type: PieceType, promotes: bool, params: EvalParams) -> i32 {
    if promotes {
        params.piece_value(piece_type.promote()) - params.piece_value(piece_type)
    } else {
        0
    }
}

/// 取り返しが成れるかどうか。成れるなら必ず得なので、成るものとして数える。
fn promotes(piece_type: PieceType, from: Square, to: Square, side: Color) -> bool {
    if !piece_type.is_promotable() {
        return false;
    }
    let zone = Bitboard::promotion_zone(side);
    zone.test(from) || zone.test(to)
}

/// `attackers`のうち`side`の駒で最も安いものを選ぶ。玉は非玉がないときだけ選ぶ。
///
/// 攻撃駒は多くても十数枚なので、駒種ごとのbitboardを引くより素直に走査する。
/// 値の順序が[`EvalParams`]に追随するので、調整した駒価値がそのまま効く。
fn least_valuable_attacker(
    position: &Position,
    attackers: Bitboard,
    target: Square,
    side: Color,
    params: EvalParams,
) -> Option<(Square, PieceType)> {
    let mut candidates = attackers & position.pieces_by_color(side);
    let blockers = position.blockers_for_king(side);
    let king = position.square(side, PieceType::KING);
    let mut best: Option<(Square, PieceType, i32)> = None;
    while let Some(square) = candidates.pop_lsb() {
        // pinされた駒は、玉との直線上へ動くときだけ取り返せる。
        if blockers.test(square)
            && king.is_on_board()
            && !Bitboard::is_aligned(square, target, king)
        {
            continue;
        }
        let piece_type = position.piece_on(square).piece_type();
        let value = params.piece_value(piece_type);
        if best.is_none_or(|(_, current_piece_type, current)| {
            (piece_type == PieceType::KING, value)
                < (current_piece_type == PieceType::KING, current)
        }) {
            best = Some((square, piece_type, value));
        }
    }
    best.map(|(square, piece_type, _)| (square, piece_type))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsshogi::board;
    use rsshogi::board::{Move32List, generate_legal_all_move32};

    use crate::eval::evaluate_material_unclamped;

    /// 差分テスト用のオラクル。合法な取り返しを全探索し、双方が不利な継続を
    /// 降りられるものとして取り合いの結果を求める。
    fn exact_exchange_eval(position: &Position, mv: Move32, params: EvalParams) -> i32 {
        assert!(position.is_legal_move32(mv), "the oracle requires a legal move");
        assert!(position.move32_metadata(mv).is_capture(), "the oracle requires a capture");
        let target = mv.to_sq();
        let mut exchange = position.clone_for_search();
        let gain = apply_material_delta(&mut exchange, mv, params);
        gain - best_recapture_gain(&mut exchange, target, params)
    }

    fn best_recapture_gain(position: &mut Position, target: Square, params: EvalParams) -> i32 {
        let mut moves = Move32List::new();
        generate_legal_all_move32(position, &mut moves);
        let mut best = 0;
        for mv in moves.as_slice().iter().copied() {
            if mv.to_sq() != target || !position.move32_metadata(mv).is_capture() {
                continue;
            }
            let gain = apply_material_delta(position, mv, params);
            let continuation = best_recapture_gain(position, target, params);
            position.undo_move32(mv).expect("a generated legal recapture must be undoable");
            best = best.max(gain - continuation);
        }
        best
    }

    fn apply_material_delta(position: &mut Position, mv: Move32, params: EvalParams) -> i32 {
        let before = evaluate_material_unclamped(position, params);
        position.apply_move32(mv);
        let after = -evaluate_material_unclamped(position, params);
        after - before
    }

    fn see(sfen: &str, usi: &str) -> i32 {
        let position = board::position_from_sfen(sfen).expect("valid SFEN");
        let before_sfen = position.to_sfen(None);
        let before_key = position.key();
        let mv = board::move_from_usi(&position, usi).expect("valid move text");
        let result = static_exchange_eval(&position, mv, EvalParams::default())
            .expect("a legal capture must be evaluated");
        assert_eq!(position.to_sfen(None), before_sfen, "SEEは局面を進めない");
        assert_eq!(position.key(), before_key);
        result
    }

    fn exact(sfen: &str, usi: &str) -> i32 {
        let position = board::position_from_sfen(sfen).expect("valid SFEN");
        let mv = board::move_from_usi(&position, usi).expect("valid move text");
        exact_exchange_eval(&position, mv, EvalParams::default())
    }

    #[test]
    fn rejects_a_quiet_move_and_a_drop() {
        let position = board::hirate_position();
        let quiet = board::move_from_usi(&position, "7g7f").expect("legal quiet move");
        assert_eq!(static_exchange_eval(&position, quiet, EvalParams::default()), None);

        let drop_position =
            board::position_from_sfen("4k4/9/9/9/9/9/9/9/4K4 b P 1").expect("valid SFEN");
        let drop = board::move_from_usi(&drop_position, "P*5e").expect("legal drop");
        assert_eq!(static_exchange_eval(&drop_position, drop, EvalParams::default()), None);
    }

    #[test]
    fn scores_an_unguarded_capture() {
        assert_eq!(see("4k4/9/9/4s4/4P4/9/9/9/4K4 b - 1", "5e5d"), 900);
    }

    #[test]
    fn includes_a_legal_recapture() {
        assert_eq!(see("4k4/9/4g4/4s4/4P4/9/9/9/4K4 b - 1", "5e5d"), 700);
    }

    #[test]
    fn scores_from_the_white_root_movers_perspective() {
        assert_eq!(see("k8/9/9/9/4p4/4S4/9/9/K8 w - 1", "5e5f"), 900);
    }

    #[test]
    fn scores_a_promoted_victim_as_board_loss_plus_demoted_hand_gain() {
        assert_eq!(see("4k4/9/9/4+p4/4R4/9/9/9/4K4 b - 1", "5e5d"), 550);
    }

    #[test]
    fn includes_the_promotion_delta_once() {
        let sfen = "k8/4s4/4P4/9/9/9/9/9/4K4 b - 1";
        assert_eq!(see(sfen, "5c5b+"), 1_250);
        assert_eq!(see(sfen, "5c5b"), 900);
    }

    #[test]
    fn sees_a_lance_xray_after_the_first_capturer_moves() {
        assert_eq!(see("k8/9/4g4/4p4/4R4/9/9/9/4L3K b - 1", "5e5d"), -700);
    }

    #[test]
    fn excludes_a_pinned_recapturer() {
        let pinned = "4k4/4g4/5p3/5S3/9/9/9/9/4R3K b - 1";
        let unpinned = "k8/4g4/5p3/5S3/9/9/9/9/4R3K b - 1";

        assert_eq!(see(pinned, "4d4c"), 200);
        assert_eq!(see(unpinned, "4d4c"), -700);
    }

    #[test]
    fn permits_only_a_safe_king_recapture() {
        assert_eq!(see("9/9/4k4/4p4/4R4/9/9/9/8K b - 1", "5e5d"), -1_800);
        assert_eq!(see("9/9/4k4/4p4/4RG3/9/9/9/8K b - 1", "5e5d"), 200);
    }

    #[test]
    fn a_side_may_decline_a_losing_recapture() {
        assert_eq!(see("k8/9/4r4/4s4/4PG3/9/9/9/8K b - 1", "5e5d"), 900);
    }

    #[test]
    fn uses_a_non_king_recapturer_before_the_king() {
        let sfen = "9/9/5s3/4pk3/3B5/4L4/9/9/8K b - 1";
        assert_eq!(see(sfen, "6e5d"), exact(sfen, "6e5d"));
    }

    #[test]
    fn counts_a_pinned_defender_against_a_king_recapture() {
        let sfen = "4r4/9/9/5k3/5p3/4G4/4K1N2/9/9 b - 1";
        assert_eq!(see(sfen, "3g4e"), exact(sfen, "3g4e"));
    }

    #[test]
    fn agrees_with_the_exact_evaluation_where_pins_do_not_matter() {
        let cases = [
            ("4k4/9/9/4s4/4P4/9/9/9/4K4 b - 1", "5e5d"),
            ("4k4/9/4g4/4s4/4P4/9/9/9/4K4 b - 1", "5e5d"),
            ("k8/9/9/9/4p4/4S4/9/9/K8 w - 1", "5e5f"),
            ("4k4/9/9/4+p4/4R4/9/9/9/4K4 b - 1", "5e5d"),
            ("k8/4s4/4P4/9/9/9/9/9/4K4 b - 1", "5c5b+"),
            ("k8/4s4/4P4/9/9/9/9/9/4K4 b - 1", "5c5b"),
            ("k8/9/4g4/4p4/4R4/9/9/9/4L3K b - 1", "5e5d"),
            ("9/9/4k4/4p4/4R4/9/9/9/8K b - 1", "5e5d"),
            ("9/9/4k4/4p4/4RG3/9/9/9/8K b - 1", "5e5d"),
            ("k8/9/4r4/4s4/4PG3/9/9/9/8K b - 1", "5e5d"),
            ("k8/9/3gg4/4s4/4P4/9/9/9/8K b - 1", "5e5d"),
            ("4k4/4g4/5p3/5S3/9/9/9/9/4R3K b - 1", "4d4c"),
            ("k8/4g4/5p3/5S3/9/9/9/9/4R3K b - 1", "4d4c"),
        ];

        for (sfen, usi) in cases {
            assert_eq!(see(sfen, usi), exact(sfen, usi), "{sfen} の {usi} で一致しない");
        }
    }
}

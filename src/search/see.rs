use crate::position::SearchPositionExt;
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
pub(super) fn static_exchange_eval(
    position: &Position,
    mv: Move32,
    params: EvalParams,
) -> Option<i32> {
    let metadata = position.move_facts(mv);
    if !metadata.is_capture() {
        return None;
    }
    let target = mv.to_sq();
    let from = mv.from_sq();
    if from.is_none() {
        // 打ちは捕獲にならないため、ここへは来ない。
        return None;
    }

    let mut occupied = position.pieces();
    occupied.clear(from);

    let gain = capture_gain(metadata.captured_piece_type(), params)
        + promotion_gain(
            position.moved_piece_before(mv).piece_type(),
            metadata.is_promotion(),
            params,
        );
    // 取り返されたときに相手が得る、いま対象マスに立っている駒の価値。
    let standing = capture_gain(position.moved_piece_after(mv).piece_type(), params);
    Some(exchange_value(position, target, occupied, gain, standing, params))
}

/// 捕獲`mv`のSEEが`threshold`以上かを、閾値で早期打ち切りしながら判定する。
/// 捕獲でない手には常に偽を返す。結果は`static_exchange_eval(...) >= Some(threshold)`と完全一致する。
pub(super) fn static_exchange_ge(
    position: &Position,
    mv: Move32,
    threshold: i32,
    params: EvalParams,
) -> bool {
    let metadata = position.move_facts(mv);
    if !metadata.is_capture() {
        return false;
    }
    let from = mv.from_sq();
    if from.is_none() {
        return false;
    }
    let target = mv.to_sq();
    let mut occupied = position.pieces();
    occupied.clear(from);
    let gain = capture_gain(metadata.captured_piece_type(), params)
        + promotion_gain(
            position.moved_piece_before(mv).piece_type(),
            metadata.is_promotion(),
            params,
        );
    let standing = capture_gain(position.moved_piece_after(mv).piece_type(), params);
    let result = exchange_ge(position, target, occupied, gain, standing, threshold, params);
    debug_assert_eq!(
        result,
        static_exchange_eval(position, mv, params).is_some_and(|see| see >= threshold)
    );
    result
}

/// Evaluate a legal noncapture, including a drop or promotion, from the mover's
/// perspective. Captures retain the separate public SEE contract above.
/// The same initial-pin and local-exchange approximations apply.
#[cfg_attr(not(any(test, debug_assertions)), allow(dead_code))]
pub(super) fn quiet_static_exchange_eval(
    position: &Position,
    mv: Move32,
    params: EvalParams,
) -> Option<i32> {
    let metadata = position.move_facts(mv);
    if metadata.is_capture() {
        return None;
    }
    let mut occupied = position.pieces();
    if !mv.from_sq().is_none() {
        occupied.clear(mv.from_sq());
    }
    occupied.set(mv.to_sq());
    let gain = promotion_gain(
        position.moved_piece_before(mv).piece_type(),
        metadata.is_promotion(),
        params,
    );
    let standing = capture_gain(position.moved_piece_after(mv).piece_type(), params);
    Some(exchange_value(position, mv.to_sq(), occupied, gain, standing, params))
}

/// 非捕獲`mv`のSEEが`threshold`以上かを、閾値で早期打ち切りしながら判定する。
/// 捕獲には偽を返す。結果は`quiet_static_exchange_eval(...) >= Some(threshold)`と完全一致する。
pub(super) fn quiet_static_exchange_ge(
    position: &Position,
    mv: Move32,
    threshold: i32,
    params: EvalParams,
) -> bool {
    let metadata = position.move_facts(mv);
    if metadata.is_capture() {
        return false;
    }
    let mut occupied = position.pieces();
    if !mv.from_sq().is_none() {
        occupied.clear(mv.from_sq());
    }
    let target = mv.to_sq();
    occupied.set(target);
    let gain = promotion_gain(
        position.moved_piece_before(mv).piece_type(),
        metadata.is_promotion(),
        params,
    );
    let standing = capture_gain(position.moved_piece_after(mv).piece_type(), params);
    let result = exchange_ge(position, target, occupied, gain, standing, threshold, params);
    debug_assert_eq!(
        result,
        quiet_static_exchange_eval(position, mv, params).is_some_and(|see| see >= threshold)
    );
    result
}

fn exchange_ge(
    position: &Position,
    target: Square,
    mut occupied: Bitboard,
    gain: i32,
    mut standing: i32,
    threshold: i32,
    params: EvalParams,
) -> bool {
    let mut bound = i64::from(gain) - i64::from(threshold);
    if bound < 0 {
        return false;
    }
    let max_promo = i64::from(params.promotion_bonus.max(0));
    let mut parity = true;
    let mut side = position.turn().flip();
    let mut depth = 0usize;
    while depth + 1 < MAX_EXCHANGE_PLIES {
        if i64::from(standing) + max_promo <= bound {
            return parity;
        }
        let mut attackers = position.attackers_to(target, occupied) & occupied;
        attackers.clear(target);
        let Some((square, piece_type)) =
            least_valuable_attacker(position, attackers, target, side, params)
        else {
            return parity;
        };
        if piece_type == PieceType::KING
            && !(attackers & position.pieces_by_color(side.flip())).is_empty()
        {
            return parity;
        }
        let promotes = promotes(piece_type, square, target, side);
        let capture = i64::from(standing) + i64::from(promotion_gain(piece_type, promotes, params));
        if capture <= bound {
            return parity;
        }
        bound = capture - bound - 1;
        parity = !parity;
        standing = capture_gain(if promotes { piece_type.promote() } else { piece_type }, params);
        occupied.clear(square);
        side = side.flip();
        depth += 1;
    }
    parity
}

fn exchange_value(
    position: &Position,
    target: Square,
    mut occupied: Bitboard,
    gain: i32,
    mut standing: i32,
    params: EvalParams,
) -> i32 {
    let mut gains = [0i32; MAX_EXCHANGE_PLIES];
    gains[0] = gain;

    let mut side = position.turn().flip();
    let mut depth = 0usize;
    while depth + 1 < MAX_EXCHANGE_PLIES {
        let mut attackers = position.attackers_to(target, occupied) & occupied;
        // The standing piece is an occupant, never a recapture candidate. Piece
        // masks still describe the initial board, including an old capture victim.
        attackers.clear(target);
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
    gains[0]
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
    if candidates.is_empty() {
        return None;
    }
    let blockers = position.blockers_for_king(side);
    let king =
        if !(candidates & blockers).is_empty() { position.king_square(side) } else { Square::NONE };
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
            if mv.to_sq() != target || !position.move_facts(mv).is_capture() {
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

    fn quiet_see(sfen: &str, usi: &str) -> i32 {
        let position = board::position_from_sfen(sfen).expect("valid SFEN");
        let before_sfen = position.to_sfen(None);
        let before_key = position.key();
        let mv = board::move_from_usi(&position, usi).expect("valid move text");
        let result = quiet_static_exchange_eval(&position, mv, EvalParams::default())
            .expect("a noncapture must be evaluated");
        assert_eq!(result, exact_exchange_eval(&position, mv, EvalParams::default()));
        assert_eq!(position.to_sfen(None), before_sfen);
        assert_eq!(position.key(), before_key);
        result
    }

    #[test]
    fn scores_hanging_and_safe_quiet_moves() {
        assert_eq!(quiet_see("k8/9/9/4p4/9/4R4/9/9/8K b - 1", "5f5e"), -2_000);
        assert_eq!(quiet_see("k8/9/9/9/9/4R4/9/9/8K b - 1", "5f5e"), 0);
    }

    #[test]
    fn clears_the_quiet_moves_origin_for_xrays() {
        assert_eq!(quiet_see("k8/9/2b6/3S5/9/9/9/9/8K b - 1", "6d5e"), -900);
    }

    #[test]
    fn scores_defended_and_hanging_drops() {
        assert_eq!(quiet_see("k8/9/9/4s4/9/5G3/9/9/8K b P 1", "P*5e"), 0);
        assert_eq!(quiet_see("k8/9/9/4s4/9/9/9/9/8K b P 1", "P*5e"), -200);
        assert_eq!(quiet_see("k8/9/9/4p4/9/5G3/9/9/8K b S 1", "S*5e"), -700);
    }

    #[test]
    fn includes_noncapture_promotion_gain_and_promoted_piece_loss() {
        assert_eq!(quiet_see("k8/9/9/4P4/9/9/9/9/8K b - 1", "5d5c+"), 350);
        assert_eq!(quiet_see("k8/4s4/9/4P4/9/9/9/9/8K b - 1", "5d5c+"), -200);
        assert_eq!(quiet_see("k8/9/9/9/9/4p4/9/9/8K w - 1", "5f5g+"), 350);
    }

    #[test]
    fn quiet_see_rejects_captures_without_changing_capture_see() {
        let position =
            board::position_from_sfen("4k4/9/9/4s4/4P4/9/9/9/4K4 b - 1").expect("valid SFEN");
        let mv = board::move_from_usi(&position, "5e5d").expect("legal capture");
        assert_eq!(quiet_static_exchange_eval(&position, mv, EvalParams::default()), None);
        assert_eq!(static_exchange_eval(&position, mv, EvalParams::default()), Some(900));
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
    fn capture_see_matches_expected_scores_and_exact_oracle() {
        let cases: &[(&str, &str, Option<i32>)] = &[
            // Unguarded capture & legal recapture
            ("4k4/9/9/4s4/4P4/9/9/9/4K4 b - 1", "5e5d", Some(900)),
            ("4k4/9/4g4/4s4/4P4/9/9/9/4K4 b - 1", "5e5d", Some(700)),
            ("k8/9/3gg4/4s4/4P4/9/9/9/8K b - 1", "5e5d", None),
            // White root mover perspective
            ("k8/9/9/9/4p4/4S4/9/9/K8 w - 1", "5e5f", Some(900)),
            // Promoted victim = board loss + demoted hand gain
            ("4k4/9/9/4+p4/4R4/9/9/9/4K4 b - 1", "5e5d", Some(550)),
            // Promotion delta counted once
            ("k8/4s4/4P4/9/9/9/9/9/4K4 b - 1", "5c5b+", Some(1_250)),
            ("k8/4s4/4P4/9/9/9/9/9/4K4 b - 1", "5c5b", Some(900)),
            // Lance x-ray after first capturer moves
            ("k8/9/4g4/4p4/4R4/9/9/9/4L3K b - 1", "5e5d", Some(-700)),
            // Initial-pin exclusion vs unpinned recapturer
            ("4k4/4g4/5p3/5S3/9/9/9/9/4R3K b - 1", "4d4c", Some(200)),
            ("k8/4g4/5p3/5S3/9/9/9/9/4R3K b - 1", "4d4c", Some(-700)),
            // Safe vs unsafe king recapture, and non-king recapturer ordering
            ("9/9/4k4/4p4/4R4/9/9/9/8K b - 1", "5e5d", Some(-1_800)),
            ("9/9/4k4/4p4/4RG3/9/9/9/8K b - 1", "5e5d", Some(200)),
            ("9/9/5s3/4pk3/3B5/4L4/9/9/8K b - 1", "6e5d", None),
            ("4r4/9/9/5k3/5p3/4G4/4K1N2/9/9 b - 1", "3g4e", None),
            // Declining a losing recapture
            ("k8/9/4r4/4s4/4PG3/9/9/9/8K b - 1", "5e5d", Some(900)),
        ];

        for &(sfen, usi, expected) in cases {
            let actual = see(sfen, usi);
            if let Some(expected) = expected {
                assert_eq!(actual, expected, "{sfen} の {usi} の期待値が一致しない");
            }
            assert_eq!(actual, exact(sfen, usi), "{sfen} の {usi} でexact評価と一致しない");
        }
    }

    #[test]
    fn thresholded_see_matches_full_swap_value_across_random_walks_and_extreme_bounds() {
        board::init();
        let thresholds = [
            i32::MIN,
            -5_000,
            -2_000,
            -900,
            -700,
            -550,
            -350,
            -200,
            -1,
            0,
            1,
            200,
            350,
            550,
            700,
            900,
            1_250,
            2_000,
            i32::MAX,
        ];
        let param_sets = [
            EvalParams::default(),
            EvalParams { bishop: 1_400, rook: 500, promotion_bonus: 0, ..EvalParams::default() },
        ];
        let mut position = board::hirate_position();
        let mut state = 0x517c_c1b7_2722_0a95_u64;
        for _ in 0..250 {
            let mut moves = Move32List::new();
            generate_legal_all_move32(&position, &mut moves);
            if moves.as_slice().is_empty() {
                position = board::hirate_position();
                continue;
            }
            for &params in &param_sets {
                for &mv in moves.as_slice() {
                    let cap = static_exchange_eval(&position, mv, params);
                    let quiet = quiet_static_exchange_eval(&position, mv, params);
                    for &threshold in &thresholds {
                        assert_eq!(
                            static_exchange_ge(&position, mv, threshold, params),
                            cap.is_some_and(|v| v >= threshold),
                        );
                        assert_eq!(
                            quiet_static_exchange_ge(&position, mv, threshold, params),
                            quiet.is_some_and(|v| v >= threshold),
                        );
                    }
                    if let Some(v) = cap {
                        assert!(static_exchange_ge(&position, mv, v, params));
                        assert!(!static_exchange_ge(&position, mv, v + 1, params));
                    }
                    if let Some(v) = quiet {
                        assert!(quiet_static_exchange_ge(&position, mv, v, params));
                        assert!(!quiet_static_exchange_ge(&position, mv, v + 1, params));
                    }
                }
            }
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let chosen = moves.as_slice()[(state as usize) % moves.as_slice().len()];
            position.apply_move32(chosen);
        }
    }
}

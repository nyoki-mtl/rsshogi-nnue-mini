//! 手の並べ替えと、そのための遅延評価を担う。
//!
//! 以前は全合法手にSEEと王手判定を前計算していたが、βカットで大半の手が
//! 探索されないノードでは無駄が大きい。ここでは安価なscore(捕獲価値・killer・
//! 履歴)だけで一度並べ、SEEは捕獲を取り出す時点で遅延計算する。負け捕獲は
//! 後回しの山へ移し、全手の後に返す。王手判定は並べ替えでは行わず、
//! 枝刈り判定に必要な手についてだけ探索側が遅延計算する。

use std::cmp::Reverse;

use rsshogi::board::Position;
use rsshogi::types::{Move32, Move32Metadata};

use crate::eval::EvalParams;
use crate::params::SearchParams;
use crate::see::static_exchange_eval;

use super::CaptureSee;
use super::history::{CONTINUATION_PLIES, HistoryTables, piece_to_index};

/// continuation historyの寄与を重み(256で等倍)で縮める。0なら寄与しない。
fn scaled_continuation(score: i32, weight: i32) -> i32 {
    score * weight.clamp(0, 1_024) / 256
}

/// 取り出し済みの一手と、その手について確定した分類。
///
/// `gives_check`は持たない。必要な手だけ探索側が遅延計算する。
#[derive(Clone, Copy)]
pub(super) struct OrderedMove {
    pub(super) mv: Move32,
    pub(super) metadata: Move32Metadata,
    pub(super) see: CaptureSee,
    pub(super) quiet: bool,
    /// 静かな手の履歴スコア。LMRの補正が再計算せずに使う。quiet以外は0。
    pub(super) history_score: i32,
}

/// 初期スコア付けを終えた一手。SEEは取り出し時まで未計算。
#[derive(Clone, Copy)]
struct ScoredMove {
    score: i32,
    mv: Move32,
    metadata: Move32Metadata,
    quiet: bool,
    history_score: i32,
    see: CaptureSee,
    principal: bool,
}

impl ScoredMove {
    fn into_ordered(self) -> OrderedMove {
        OrderedMove {
            mv: self.mv,
            metadata: self.metadata,
            see: self.see,
            quiet: self.quiet,
            history_score: self.history_score,
        }
    }
}

/// plyごとに再利用する並べ替えバッファ。毎ノードのVec確保を避ける。
#[derive(Default)]
pub(super) struct OrderingBuffer {
    entries: Vec<ScoredMove>,
    deferred: Vec<ScoredMove>,
}

/// 遅延評価つきの手の供給器。
///
/// 生成時に安価なscoreで降順に並べ、`next`が先頭から返す。捕獲は取り出す
/// 時点で初めてSEEを払い、負け捕獲(TT moveを除く)は山へ回して最後に返す。
pub(super) struct MovePicker {
    buffer: OrderingBuffer,
    index: usize,
    deferred_index: usize,
    deferred_sorted: bool,
    capture_class_bonus: i32,
    eval_params: EvalParams,
}

impl MovePicker {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        position: &Position,
        moves: &[Move32],
        params: EvalParams,
        tt_move: Option<Move32>,
        killers: [Option<Move32>; 2],
        history: &HistoryTables,
        previous: &[Option<usize>; CONTINUATION_PLIES],
        search_params: SearchParams,
        mut buffer: OrderingBuffer,
    ) -> Self {
        let capture_class_bonus = capture_class_bonus(params, search_params.check_ordering_bonus);
        let stm = position.turn();
        buffer.entries.clear();
        buffer.deferred.clear();
        for mv in moves.iter().copied() {
            let metadata = position.move32_metadata(mv);
            let quiet = !metadata.is_capture() && !metadata.is_promotion();
            let capture = params.piece_value(metadata.captured_piece_type());
            let promotion = if metadata.is_promotion() { params.promotion_bonus } else { 0 };
            // 王手bonusは捕獲・成りに限定する。静かな手の並べ替えのためだけに
            // 全手の王手判定を払うのをやめた(経緯はtask 0040 decisions.md)。
            let check = if !quiet && position.gives_check_move32(mv) {
                search_params.check_ordering_bonus
            } else {
                0
            };
            let principal = Some(mv) == tt_move;
            // 静かな手の履歴は「手そのもの」と「直前の手からの続き」の和。
            // continuationの重みが0なら和は従来通りmain historyだけになる。
            let history_score = if quiet {
                history.quiet_score(stm, mv)
                    + scaled_continuation(
                        history.continuations.score(previous, piece_to_index(mv)),
                        search_params.continuation_weight,
                    )
            } else {
                0
            };
            // 静かな手の層はkiller(80_000/70_000)、履歴の順。履歴は±16_384に
            // 収まるので、killerの層(70_000)へは届かない。
            let quiet_rank = if !quiet {
                0
            } else if Some(mv) == killers[0] {
                80_000
            } else if Some(mv) == killers[1] {
                70_000
            } else {
                history_score
            };
            // 捕獲はまだSEEを払わず、勝ち捕獲として楽観的に上の層へ置く。
            let exchange_class = if metadata.is_capture() { capture_class_bonus } else { 0 };
            let score = if principal { 1_000_000 } else { 0 }
                + capture * 16
                + exchange_class
                + promotion
                + check
                + quiet_rank;
            buffer.entries.push(ScoredMove {
                score,
                mv,
                metadata,
                quiet,
                history_score,
                see: None,
                principal,
            });
        }
        // 安価なscoreで一度だけ並べる。遅延選択(選択ソート)も試したが、
        // ALLノードのO(n^2)走査が支配的で全体ソートより遅かった(task 0040)。
        buffer.entries.sort_by_key(|entry| Reverse(entry.score));
        Self {
            buffer,
            index: 0,
            deferred_index: 0,
            deferred_sorted: false,
            capture_class_bonus,
            eval_params: params,
        }
    }

    pub(super) fn next(&mut self, position: &Position) -> Option<OrderedMove> {
        while self.index < self.buffer.entries.len() {
            let mut entry = self.buffer.entries[self.index];
            self.index += 1;
            if entry.metadata.is_capture() {
                // 捕獲はここで初めてSEEを払う。βカットで届かない手には払わない。
                let see = static_exchange_eval(position, entry.mv, self.eval_params);
                entry.see = see;
                if !entry.principal
                    && let Some(exchange) = see
                    && exchange < 0
                {
                    // 負け捕獲は山へ。旧形式のscore(-class + see)へ置き直して
                    // 山の中の相対順序を保つ。TT moveは負け捕獲でも先頭のまま。
                    entry.score = entry.score - 2 * self.capture_class_bonus + exchange;
                    self.buffer.deferred.push(entry);
                    continue;
                }
            }
            return Some(entry.into_ordered());
        }
        if !self.deferred_sorted {
            self.buffer.deferred.sort_by_key(|entry| Reverse(entry.score));
            self.deferred_sorted = true;
        }
        let entry = self.buffer.deferred.get(self.deferred_index).copied()?;
        self.deferred_index += 1;
        Some(entry.into_ordered())
    }

    /// バッファをplyのpoolへ返すために取り出す。
    pub(super) fn into_buffer(self) -> OrderingBuffer {
        self.buffer
    }
}

fn capture_class_bonus(params: EvalParams, check_ordering_bonus: i32) -> i32 {
    let largest_base = [
        params.pawn,
        params.lance,
        params.knight,
        params.silver,
        params.gold,
        params.bishop,
        params.rook,
    ]
    .into_iter()
    .max()
    .expect("the material table has piece values");
    (largest_base + params.promotion_bonus) * 16 + params.promotion_bonus + check_ordering_bonus + 1
}

#[cfg(test)]
mod tests {
    use rsshogi::board;
    use rsshogi::types::Color;

    use super::*;

    fn collect_order(
        position: &Position,
        moves: &[Move32],
        tt_move: Option<Move32>,
        killers: [Option<Move32>; 2],
        history: &HistoryTables,
    ) -> Vec<OrderedMove> {
        let mut picker = MovePicker::new(
            position,
            moves,
            EvalParams::default(),
            tt_move,
            killers,
            history,
            &[None; CONTINUATION_PLIES],
            SearchParams::default(),
            OrderingBuffer::default(),
        );
        let mut ordered = Vec::new();
        while let Some(entry) = picker.next(position) {
            ordered.push(entry);
        }
        ordered
    }

    #[test]
    fn capture_ordering_prefers_the_better_exchange() {
        let position = board::position_from_sfen("k8/9/4p4/4s2p1/4R2P1/9/9/9/8K b - 1")
            .expect("valid ordering position");
        let losing_high_victim =
            board::move_from_usi(&position, "5e5d").expect("legal losing capture");
        let profitable_low_victim =
            board::move_from_usi(&position, "2e2d").expect("legal profitable capture");
        let moves = [losing_high_victim, profitable_low_victim];
        // killerと履歴を負け捕獲に与えても、捕獲の並べ替えには影響しない。
        let mut history = HistoryTables::new();
        for _ in 0..64 {
            history.record_quiet_cutoff(Color::BLACK, losing_high_victim, &[], 13);
        }

        let ordered =
            collect_order(&position, &moves, None, [Some(losing_high_victim), None], &history);

        assert_eq!(
            ordered.iter().map(|ordered| ordered.mv).collect::<Vec<_>>(),
            [profitable_low_victim, losing_high_victim]
        );
        assert!(ordered[0].see.is_some_and(|see| see >= 0));
        assert!(ordered[1].see.is_some_and(|see| see < 0), "負け捕獲もSEEを添えて返る");
    }

    #[test]
    fn the_full_order_keeps_the_tt_killer_capture_history_layers() {
        board::init();
        // 5e飛は5d銀(負け捕獲)と2d歩(勝ち捕獲)を取れ、静かな手も多数ある。
        let position = board::position_from_sfen("k8/9/4p4/4s2p1/4R2P1/9/9/9/8K b - 1")
            .expect("valid layered position");
        let moves = super::super::legal_moves(&position);
        let mv = |text: &str| board::move_from_usi(&position, text).expect("legal move");
        let losing_capture = mv("5e5d");
        let winning_capture = mv("2e2d");
        let killer = mv("1i1h");
        let boosted = mv("5e4e");
        let mut history = HistoryTables::new();
        history.record_quiet_cutoff(Color::BLACK, boosted, &[], 13);

        let ordered =
            collect_order(&position, &moves, Some(losing_capture), [Some(killer), None], &history);
        let order = ordered.iter().map(|ordered| ordered.mv).collect::<Vec<_>>();

        // TT move(負け捕獲でも先頭) > killer > 勝ち捕獲 > 履歴の良い静かな手 > 他の静かな手。
        assert_eq!(order[0], losing_capture, "TT moveは負け捕獲でも先頭に立つ");
        assert_eq!(order[1], killer);
        assert_eq!(order[2], winning_capture);
        assert_eq!(order[3], boosted);
        assert_eq!(ordered.len(), moves.len(), "遅延評価でも全ての手が一度ずつ返る");
    }

    #[test]
    fn a_losing_capture_without_tt_status_is_deferred_to_the_tail() {
        let position = board::position_from_sfen("k8/9/4p4/4s2p1/4R2P1/9/9/9/8K b - 1")
            .expect("valid deferral position");
        let moves = super::super::legal_moves(&position);
        let losing_capture = board::move_from_usi(&position, "5e5d").expect("legal losing capture");
        let history = HistoryTables::new();

        let ordered = collect_order(&position, &moves, None, [None; 2], &history);

        assert_eq!(
            ordered.last().map(|ordered| ordered.mv),
            Some(losing_capture),
            "負け捕獲は全ての手の後に返る"
        );
        assert_eq!(ordered.len(), moves.len());
    }

    #[test]
    fn capture_classes_stay_separate_when_bishop_is_worth_more_than_rook() {
        let params =
            EvalParams { bishop: 1_400, rook: 500, promotion_bonus: 0, ..EvalParams::default() };
        let check_bonus = SearchParams::default().check_ordering_bonus;
        let largest_unknown_capture = params.bishop * 16 + check_bonus;

        assert!(capture_class_bonus(params, check_bonus) > largest_unknown_capture);
    }
}

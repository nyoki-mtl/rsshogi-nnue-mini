//! 手の並べ替えと、そのための遅延評価を担う。
//!
//! βカットで大半の手が探索されないノードでは、全合法手の生成と、SEEや王手判定の
//! 前計算は無駄が大きい。主探索では手を段階ごとに生成し、安価なscore(捕獲価値・
//! killer・履歴)だけで並べ、SEEは捕獲を取り出す時点で遅延計算する。負け捕獲は
//! 後回しの山へ移し、全手の後に返す。静かな手の王手判定は並べ替えでは行わず、
//! 枝刈り判定に必要な手についてだけ探索側が遅延計算する。

use std::cmp::Reverse;

use rsshogi::board::{Captures, Move32List, MoveGenType, Position, Quiets, generate_moves_move32};
use rsshogi::types::{Move32, Move32Metadata, PieceType};

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
    /// 全手モードで、捕獲の層より下のscoreの手。上の層を使い切るまで並べない。
    late: Vec<ScoredMove>,
    deferred: Vec<ScoredMove>,
}

/// 手の並べ替えに使う、nodeごとに決まる材料。
#[derive(Clone, Copy)]
pub(super) struct OrderingInputs {
    pub(super) eval_params: EvalParams,
    pub(super) search_params: SearchParams,
    pub(super) killers: [Option<Move32>; 2],
    pub(super) previous: [Option<usize>; CONTINUATION_PLIES],
}

/// 次に供給する手の種類。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// 生成済みの全手を一度に並べて返す(静止探索・王手回避・除外手つきの探索)。
    All,
    TtMove,
    Killers,
    Captures,
    Quiets,
    Deferred,
}

/// 遅延評価つきの手の供給器。
///
/// 捕獲は取り出す時点で初めてSEEを払い、負け捕獲(TT moveを除く)は山へ回して最後に返す。
///
/// 全手モードは生成済みの手を安価なscoreで降順に並べて先頭から返す。捕獲の層より下の手
/// (主に静かな手)は上の層を使い切った時点で並べる。上の層のscoreは下の層より必ず
/// 大きいので、順序は全体を一度に安定ソートした場合と同じになる。
///
/// 段階モードは王手されていない主探索のnodeで使い、TT move、killer、捕獲、静かな手の順に
/// 必要になった段階で生成する。TT move、killer、捕獲でβを超えるnodeでは、
/// 静かな手の生成も並べ替えも払わない。
pub(super) struct MovePicker {
    buffer: OrderingBuffer,
    stage: Stage,
    index: usize,
    late_merged: bool,
    deferred_index: usize,
    deferred_sorted: bool,
    capture_class_bonus: i32,
    inputs: OrderingInputs,
    tt_move: Option<Move32>,
    /// 段階モードでkillerとして返した手。後の段階では飛ばす。
    yielded_killers: [Option<Move32>; 2],
}

impl MovePicker {
    /// 生成済みの`moves`を一度に並べる全手モード。
    pub(super) fn new(
        position: &Position,
        moves: &[Move32],
        tt_move: Option<Move32>,
        inputs: OrderingInputs,
        history: &HistoryTables,
        buffer: OrderingBuffer,
    ) -> Self {
        let mut picker = Self::empty(Stage::All, tt_move, inputs, buffer);
        for mv in moves.iter().copied() {
            let entry = picker.score(position, mv, history);
            let layer = if entry.score >= picker.capture_class_bonus {
                &mut picker.buffer.entries
            } else {
                &mut picker.buffer.late
            };
            layer.push(entry);
        }
        // 安価なscoreで一度だけ並べる。遅延選択(選択ソート)はALLノードの
        // O(n^2)走査が支配的になり、全体ソートより遅い。
        picker.buffer.entries.sort_by_key(|entry| Reverse(entry.score));
        picker
    }

    /// 必要になった段階で生成する段階モード。`tt_move`は合法手でなければならない。
    pub(super) fn staged(
        tt_move: Option<Move32>,
        inputs: OrderingInputs,
        buffer: OrderingBuffer,
    ) -> Self {
        Self::empty(Stage::TtMove, tt_move, inputs, buffer)
    }

    fn empty(
        stage: Stage,
        tt_move: Option<Move32>,
        inputs: OrderingInputs,
        mut buffer: OrderingBuffer,
    ) -> Self {
        buffer.entries.clear();
        buffer.late.clear();
        buffer.deferred.clear();
        Self {
            buffer,
            stage,
            index: 0,
            late_merged: false,
            deferred_index: 0,
            deferred_sorted: false,
            capture_class_bonus: capture_class_bonus(
                inputs.eval_params,
                inputs.search_params.check_ordering_bonus,
            ),
            inputs,
            tt_move,
            yielded_killers: [None; 2],
        }
    }

    /// 静かな手の履歴は「手そのもの」と「直前の手からの続き」の和。
    /// continuationの重みが0なら和はmain historyだけになる。
    fn history_score(&self, position: &Position, mv: Move32, history: &HistoryTables) -> i32 {
        history.quiet_score(position.turn(), mv)
            + scaled_continuation(
                history.continuations.score(&self.inputs.previous, piece_to_index(mv)),
                self.inputs.search_params.continuation_weight,
            )
    }

    fn score(&self, position: &Position, mv: Move32, history: &HistoryTables) -> ScoredMove {
        let params = self.inputs.eval_params;
        let killers = self.inputs.killers;
        let metadata = position.move32_metadata(mv);
        let quiet = !metadata.is_capture() && !metadata.is_promotion();
        let capture = params.piece_value(metadata.captured_piece_type());
        let promotion = if metadata.is_promotion() { params.promotion_bonus } else { 0 };
        // 王手bonusは捕獲・成りに限定する。静かな手の並べ替えのためだけに
        // 全手の王手判定を払うと、得られる並べ替えの改善より高くつく。
        let check = if !quiet && position.gives_check_move32(mv) {
            self.inputs.search_params.check_ordering_bonus
        } else {
            0
        };
        let principal = Some(mv) == self.tt_move;
        let history_score = if quiet { self.history_score(position, mv, history) } else { 0 };
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
        let exchange_class = if metadata.is_capture() { self.capture_class_bonus } else { 0 };
        let score = if principal { 1_000_000 } else { 0 }
            + capture * 16
            + exchange_class
            + promotion
            + check
            + quiet_rank;
        ScoredMove { score, mv, metadata, quiet, history_score, see: None, principal }
    }

    /// 段階モードで、TT moveと返し済みのkillerを除いた`T`の合法手を並べて`entries`へ置く。
    fn fill_stage<T: MoveGenType>(&mut self, position: &Position, history: &HistoryTables) {
        let mut list = Move32List::new();
        generate_moves_move32::<T>(position, &mut list);
        self.buffer.entries.clear();
        self.index = 0;
        for mv in list.as_slice().iter().copied() {
            if Some(mv) == self.tt_move
                || self.yielded_killers.contains(&Some(mv))
                || !is_legal_generated(position, mv)
            {
                continue;
            }
            let entry = self.score(position, mv, history);
            self.buffer.entries.push(entry);
        }
        self.buffer.entries.sort_by_key(|entry| Reverse(entry.score));
    }

    pub(super) fn next(
        &mut self,
        position: &Position,
        history: &HistoryTables,
    ) -> Option<OrderedMove> {
        loop {
            match self.stage {
                Stage::TtMove => {
                    self.stage = Stage::Killers;
                    if let Some(mv) = self.tt_move {
                        // 最初の手ではSEEも履歴scoreも枝刈り・縮小に使わないので計算しない。
                        let metadata = position.move32_metadata(mv);
                        return Some(OrderedMove {
                            mv,
                            metadata,
                            see: None,
                            quiet: !metadata.is_capture() && !metadata.is_promotion(),
                            history_score: 0,
                        });
                    }
                }
                Stage::Killers => {
                    // killerは別の局面で記録した手なので、合法でこの局面でも静かな手の
                    // ときだけ返す。それ以外は捕獲や静かな手の段階で通常どおり並ぶ。
                    let slot = self.index;
                    if slot == self.inputs.killers.len() {
                        self.stage = Stage::Captures;
                        self.fill_stage::<Captures>(position, history);
                        continue;
                    }
                    self.index += 1;
                    let Some(mv) = self.inputs.killers[slot] else { continue };
                    if Some(mv) == self.tt_move
                        || self.yielded_killers.contains(&Some(mv))
                        || !position.is_legal_move32(mv)
                    {
                        continue;
                    }
                    let metadata = position.move32_metadata(mv);
                    if metadata.is_capture() || metadata.is_promotion() {
                        continue;
                    }
                    self.yielded_killers[slot] = Some(mv);
                    return Some(OrderedMove {
                        mv,
                        metadata,
                        see: None,
                        quiet: true,
                        history_score: self.history_score(position, mv, history),
                    });
                }
                Stage::All | Stage::Captures | Stage::Quiets => {
                    if self.index == self.buffer.entries.len() {
                        match self.stage {
                            Stage::Captures => {
                                self.stage = Stage::Quiets;
                                self.fill_stage::<Quiets>(position, history);
                            }
                            Stage::All if !self.late_merged => {
                                self.buffer.late.sort_by_key(|entry| Reverse(entry.score));
                                let OrderingBuffer { entries, late, .. } = &mut self.buffer;
                                entries.append(late);
                                self.late_merged = true;
                            }
                            _ => self.stage = Stage::Deferred,
                        }
                        continue;
                    }
                    let mut entry = self.buffer.entries[self.index];
                    self.index += 1;
                    if entry.metadata.is_capture() {
                        // 捕獲はここで初めてSEEを払う。βカットで届かない手には払わない。
                        let see = static_exchange_eval(position, entry.mv, self.inputs.eval_params);
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
                Stage::Deferred => {
                    if !self.deferred_sorted {
                        self.buffer.deferred.sort_by_key(|entry| Reverse(entry.score));
                        self.deferred_sorted = true;
                    }
                    let entry = self.buffer.deferred.get(self.deferred_index).copied()?;
                    self.deferred_index += 1;
                    return Some(entry.into_ordered());
                }
            }
        }
    }

    /// バッファをplyのpoolへ返すために取り出す。
    pub(super) fn into_buffer(self) -> OrderingBuffer {
        self.buffer
    }
}

/// 王手されていない局面で、擬似合法手の生成器が出した手の合法性。
///
/// 生成器の手は擬似合法なので、自玉が取られないかだけを確かめる。駒打ちで自玉が
/// 取られることはないが、打ち歩詰めは生成器が検査しないので、歩の駒打ちだけは
/// 打ち歩詰めを含む判定を通す。
pub(super) fn is_legal_generated(position: &Position, mv: Move32) -> bool {
    if mv.is_drop() {
        mv.dropped_piece() != Some(PieceType::PAWN) || position.is_legal_move32(mv)
    } else {
        position.is_legal_after_pseudo_move32(mv)
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

    fn test_inputs(killers: [Option<Move32>; 2]) -> OrderingInputs {
        OrderingInputs {
            eval_params: EvalParams::default(),
            search_params: SearchParams::default(),
            killers,
            previous: [None; CONTINUATION_PLIES],
        }
    }

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
            tt_move,
            test_inputs(killers),
            history,
            OrderingBuffer::default(),
        );
        let mut ordered = Vec::new();
        while let Some(entry) = picker.next(position, history) {
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
    fn the_staged_picker_yields_every_legal_move_once_in_layer_order() {
        board::init();
        // 乱数の手順で進めた局面のうち、王手されていない局面で段階生成と探索の対象の手を比べる。
        let mut position = board::hirate_position();
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut checked = 0;
        for _ in 0..400 {
            let moves = super::super::search_moves(&position);
            if moves.is_empty() {
                break;
            }
            if !position.is_in_check() {
                checked += 1;
                let tt_move = moves.get(moves.len() / 2).copied();
                // killerの一方は合法な静かな手、他方はこの局面で指せない手にする。
                let quiet = moves.iter().copied().find(|&mv| {
                    let metadata = position.move32_metadata(mv);
                    Some(mv) != tt_move && !metadata.is_capture() && !metadata.is_promotion()
                });
                let killers = [quiet, Some(Move32::MOVE_NULL)];
                let history = HistoryTables::new();
                let mut picker =
                    MovePicker::staged(tt_move, test_inputs(killers), OrderingBuffer::default());
                let mut yielded = Vec::new();
                while let Some(entry) = picker.next(&position, &history) {
                    yielded.push(entry.mv);
                }
                assert_eq!(yielded.first().copied(), tt_move, "TT moveが先頭");
                if let Some(quiet) = quiet {
                    assert_eq!(yielded.get(1).copied(), Some(quiet), "合法な静かなkillerが次");
                }
                let mut expected = moves.clone();
                expected.sort_by_key(|mv| mv.raw());
                yielded.sort_by_key(|mv| mv.raw());
                assert_eq!(yielded, expected, "{}", position.to_sfen(None));
            }
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            position.apply_move32(moves[(state % moves.len() as u64) as usize]);
        }
        assert!(checked > 100);
    }

    #[test]
    fn the_staged_picker_does_not_yield_a_mating_pawn_drop() {
        board::init();
        // 1bへの歩打ちは金に守られ、玉の逃げ道は飛車が塞ぐので打ち歩詰めになる。
        let position = board::position_from_sfen("8k/9/8G/9/9/9/9/9/K6R1 b P 1")
            .expect("valid pawn-drop-mate position");
        let mate_drop = Move32::from_usi("P*1b").expect("drop notation");
        let history = HistoryTables::new();
        let mut picker =
            MovePicker::staged(None, test_inputs([None; 2]), OrderingBuffer::default());
        let mut yielded = Vec::new();
        while let Some(entry) = picker.next(&position, &history) {
            yielded.push(entry.mv);
        }
        assert!(yielded.iter().all(|mv| mv.to_usi() != mate_drop.to_usi()));
        assert!(yielded.iter().any(|mv| mv.to_usi() == "P*1d"), "他の歩打ちは返る");
        assert_eq!(yielded.len(), super::super::search_moves(&position).len());
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

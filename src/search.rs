//! 探索モジュールの入口。workerの起動と反復深化の運転、公開型の再輸出を担う。

mod context;
mod coordinator;
mod history;
mod lmr;
mod negamax;
mod ordering;
mod params;
mod pruning;
mod qsearch;
mod root;
mod score;
mod see;
#[cfg(test)]
mod test_support;
mod time;
mod tt;

#[cfg(test)]
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc::Sender};
use std::thread;
use std::time::Duration;

use rsshogi::board::{Move32List, Position, generate_legal_all_move32};
use rsshogi::types::{MOVE_WIN, Move32};

use crate::eval::{Evaluator, MAX_NNUE_EVAL};
#[cfg(test)]
use crate::position::MAX_SEARCH_DEPTH;

use context::{SearchContext, stats};
pub(crate) use coordinator::SearchCoordinator;
pub(crate) use params::SearchParams;
#[cfg(feature = "tuning")]
pub(crate) use params::tunable_manifest;
use root::{collect_pv, fallback_root_choice, search_root};
pub(crate) use score::mate_distance;
use score::root_terminal_score;
pub(crate) use time::SearchDeadline;
use time::{Instant, IterationTiming};
pub(crate) use tt::TranspositionTable;

const INF: i32 = 32_767;
const MATE: i32 = 32_000;
const MAX_QPLY: u32 = 12;
const MATE_TT_THRESHOLD: i32 = MAX_NNUE_EVAL + 1;
const REPETITION_SUPERIOR: i32 = MATE_TT_THRESHOLD - 1;
/// 公開する読み筋の上限。置換表を辿るだけなので、長さは正しさではなく可読性の問題。
const MAX_PV_LENGTH: usize = 32;
/// 前のiterationの評価値を中心とした初期のaspiration窓。
fn aspiration_window(center: i32, delta: i32) -> (i32, i32) {
    (center.saturating_sub(delta).max(-INF), center.saturating_add(delta).min(INF))
}

/// 評価の変動が大きいiterationは初期窓を広げ、幅を既存値の1〜3倍に収める。
fn adaptive_aspiration_delta(base: i32, score_change: i32) -> i32 {
    let base = base.max(1);
    base.saturating_add(score_change.clamp(0, base.saturating_mul(2))).min(INF)
}

/// 窓を外した側だけを、外れた値から`delta`離れた位置まで広げる。
///
/// 反対側は動かさない。外れた値の近くに真の値があることが多いので、
/// 全窓へ飛ぶより狭い窓で読み直した方が安い。
fn widen_aspiration_window(alpha: i32, beta: i32, score: i32, delta: i32) -> (i32, i32) {
    if score <= alpha {
        (score.saturating_sub(delta).max(-INF), beta)
    } else {
        (alpha, score.saturating_add(delta).min(INF))
    }
}

/// 一手についてのSEEの判定。捕獲でない手には判定がない。
type CaptureSee = Option<i32>;

#[derive(Debug, Clone)]
pub(crate) struct SearchLimits {
    pub(crate) max_depth: u32,
    pub(crate) max_nodes: Option<u64>,
    /// hard stopと、新しいiterationを始めないsoft stopをまとめた締切。
    pub(crate) deadline: Option<Arc<SearchDeadline>>,
    pub(crate) searchmoves: Vec<String>,
    pub(crate) max_moves_to_draw: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchIteration {
    pub(crate) depth: u32,
    pub(crate) score: i32,
    pub(crate) nodes: u64,
    pub(crate) elapsed: Duration,
    pub(crate) hashfull: u32,
    /// rootからの読み筋。先頭は必ずそのiterationの最善手。
    pub(crate) pv: Vec<Move32>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchResult {
    pub(crate) best_move: Option<Move32>,
    pub(crate) ponder_move: Option<Move32>,
    pub(crate) score: i32,
    pub(crate) depth: u32,
    pub(crate) nodes: u64,
    pub(crate) elapsed: Duration,
    pub(crate) hashfull: u32,
    /// rootからの読み筋。`best_move`がないときは空。
    pub(crate) pv: Vec<Move32>,
}

impl SearchResult {
    /// 指せる手を出せないときのfail-closedな結果。投了の合図として使う。
    pub(crate) fn fail_closed(elapsed: Duration) -> Self {
        Self {
            best_move: None,
            ponder_move: None,
            score: 0,
            depth: 0,
            nodes: 0,
            elapsed,
            hashfull: 0,
            pv: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum SearchEvent {
    Info(SearchIteration),
    Done(SearchResult),
}

#[derive(Clone)]
pub(crate) struct SearchControl {
    cancel: Arc<AtomicBool>,
    pondering: Arc<AtomicBool>,
    #[cfg(test)]
    panic_after_helpers_spawned: bool,
    #[cfg(test)]
    fail_helper_spawn_at: Option<usize>,
    #[cfg(test)]
    panic_helper_worker: Option<usize>,
}

impl SearchControl {
    pub(crate) fn new(cancel: Arc<AtomicBool>, pondering: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            pondering,
            #[cfg(test)]
            panic_after_helpers_spawned: false,
            #[cfg(test)]
            fail_helper_spawn_at: None,
            #[cfg(test)]
            panic_helper_worker: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_panic_after_helpers_spawned(mut self, enabled: bool) -> Self {
        self.panic_after_helpers_spawned = enabled;
        self
    }

    #[cfg(test)]
    fn with_failed_helper_spawn(mut self, worker_id: usize) -> Self {
        self.fail_helper_spawn_at = Some(worker_id);
        self
    }

    #[cfg(test)]
    fn with_panicking_helper(mut self, worker_id: usize) -> Self {
        self.panic_helper_worker = Some(worker_id);
        self
    }
}

/// 一回の`go`で探索する内容。常駐coordinatorへ渡す。
pub(crate) struct SearchJob {
    pub(crate) position: Position,
    pub(crate) evaluator: Evaluator,
    pub(crate) params: SearchParams,
    pub(crate) limits: SearchLimits,
    pub(crate) control: SearchControl,
    pub(crate) events: Sender<SearchEvent>,
    pub(crate) threads: usize,
}

/// 結果を公開するメインworkerだけが持つ役割。
struct MainWorker<'a> {
    events: &'a Sender<SearchEvent>,
    /// 深さ1を完了したら立て、ヘルパーの探索を始めさせる。
    helpers_start: &'a AtomicBool,
}

/// helperがこのdepthのiterationを飛ばすか。
///
/// 全workerが同じdepthを同じ順で読むと、共有TTを同じ局面で埋め合うだけで探索が広がらない。
/// helperごとに飛ばすdepthをずらし、mainより先の深さの結果をTTへ早く置かせる。
/// mainは飛ばさない。浅いdepthはTT moveと履歴の土台なので全workerが読む。
fn helper_skips_depth(worker_id: usize, depth: u32) -> bool {
    worker_id > 0 && depth >= 3 && (depth as usize + worker_id).is_multiple_of(3)
}

fn wait_for_helper_start(start: &AtomicBool, cancel: &AtomicBool) {
    while !start.load(Ordering::Acquire) && !cancel.load(Ordering::Relaxed) {
        thread::yield_now();
    }
}

/// 一つのworkerの反復深化。`main`はメインworkerだけが持つ。
///
/// メインworkerは予約した1 nodeでrootの子を評価してfallbackを用意し、
/// iterationごとに`Info`を送り、softな締切で新しいiterationを止める。
fn search_worker(
    mut position: Position,
    mut context: SearchContext,
    worker_id: usize,
    main: Option<MainWorker<'_>>,
) -> (SearchResult, SearchContext) {
    let start = Instant::now();
    let root_terminal = root_terminal_score(&position, context.limits.max_moves_to_draw);
    let declaration_move = position.declaration_win_move();
    let mut root_moves = legal_moves(&position);
    let has_legal_move = !root_moves.is_empty();
    if !context.limits.searchmoves.is_empty() {
        root_moves.retain(|mv| context.limits.searchmoves.iter().any(|text| text == &mv.to_usi()));
    }

    // 探索せずに決まる結果。予約したnodeは使わないので返す。
    let immediate = if root_terminal.is_none() && declaration_move == MOVE_WIN {
        Some((Some(declaration_move), MATE, Vec::new()))
    } else if root_moves.is_empty() {
        let score = root_terminal.unwrap_or(if has_legal_move { 0 } else { -MATE });
        Some((None, score, Vec::new()))
    } else {
        root_terminal.map(|score| (Some(root_moves[0]), score, vec![root_moves[0]]))
    };
    if let Some((best_move, score, pv)) = immediate {
        if main.is_some() {
            context.nodes.fetch_sub(1, Ordering::Relaxed);
        }
        let result = SearchResult {
            best_move,
            ponder_move: None,
            score,
            depth: 0,
            nodes: context.node_count(),
            elapsed: start.elapsed(),
            hashfull: 0,
            pv,
        };
        context.flush_nodes();
        return (result, context);
    }

    let rotation = worker_id % root_moves.len();
    root_moves.rotate_left(rotation);
    context.evaluator.initialize(&position);
    let (fallback_move, fallback_score) = if main.is_some() {
        fallback_root_choice(&mut position, &root_moves, &mut context)
    } else {
        (root_moves[0], 0)
    };
    let mut completed_depth = 0;
    let mut best_move = fallback_move;
    let mut best_score = fallback_score;
    let mut aspiration_change = 0;
    let mut timing = IterationTiming::new(&context.params);
    let mut time_scale = 1.0;

    for depth in 1..=context.limits.max_depth.max(1) {
        if context.should_stop() {
            break;
        }
        context.root_depth = depth;
        // 残り予算で次のiterationが終わらない見込みなら、丸ごと捨てる探索を始めない。
        // 判断するのは結果を公開するworkerだけで、helperは最後までTTを埋める。
        if depth > 1 && main.is_some() && context.should_skip_new_iteration(time_scale) {
            break;
        }
        if helper_skips_depth(worker_id, depth) {
            continue;
        }
        if let Some(index) = root_moves.iter().position(|mv| *mv == best_move) {
            root_moves[..=index].rotate_right(1);
        }

        // 完了した探索がaspiration窓を外れたら、外れた側だけを広げて同じdepthを引き直す。
        let mut delta =
            adaptive_aspiration_delta(context.params.aspiration_window, aspiration_change);
        let (mut window_alpha, mut window_beta) =
            if depth >= 2 { aspiration_window(best_score, delta) } else { (-INF, INF) };
        let outcome = loop {
            let Some(outcome) = search_root(
                &mut position,
                &mut root_moves,
                depth,
                window_alpha,
                window_beta,
                &mut context,
            ) else {
                break None;
            };
            if !outcome.complete {
                break Some(outcome);
            }
            let failed_low = outcome.score <= window_alpha && window_alpha != -INF;
            let failed_high = outcome.score >= window_beta && window_beta != INF;
            if !failed_low && !failed_high {
                break Some(outcome);
            }
            // 外すたびに幅を倍にするので、何度外しても数回で全窓に届く。
            delta = delta.saturating_mul(2);
            (window_alpha, window_beta) =
                widen_aspiration_window(window_alpha, window_beta, outcome.score, delta);
            if main.is_some() {
                stats::bump(stats::Stat::AspirationFail);
            }
        };
        let Some(outcome) = outcome else {
            break;
        };

        let previous_score = best_score;
        best_move = outcome.best_move;
        best_score = outcome.score;
        // 打ち切られたiterationでも、alphaを更新できた手はその窓で前の最善手を上回っている。
        // depthは完了したiterationのものを保ち、途中経過をinfoとして公開もしない。
        if !outcome.complete {
            break;
        }
        if completed_depth > 0 {
            aspiration_change = (aspiration_change + (best_score - previous_score).abs()) / 2;
        }
        completed_depth = depth;
        if main.is_some() {
            stats::bump(stats::Stat::RootIterations);
        }
        time_scale = timing.record(best_move, best_score, root_moves.len());
        if let Some(main) = &main {
            if depth == 1 {
                main.helpers_start.store(true, Ordering::Release);
            }
            let _ = main.events.send(SearchEvent::Info(SearchIteration {
                depth,
                score: best_score,
                nodes: context.node_count(),
                elapsed: start.elapsed(),
                hashfull: context.table.hashfull(),
                pv: collect_pv(&mut position, best_move, &context),
            }));
        }
    }

    let pv = collect_pv(&mut position, best_move, &context);
    let result = SearchResult {
        best_move: Some(best_move),
        ponder_move: pv.get(1).copied(),
        score: best_score,
        depth: completed_depth,
        nodes: context.node_count(),
        elapsed: start.elapsed(),
        hashfull: 0,
        pv,
    };
    context.flush_nodes();
    (result, context)
}

fn legal_moves(position: &Position) -> Vec<Move32> {
    let mut list = Move32List::new();
    generate_legal_all_move32(position, &mut list);
    list.as_slice().to_vec()
}

/// 主探索と静止探索が扱う手（歩・角・飛の不成を除く合法手）。testの比較に使う。
#[cfg(test)]
fn search_moves(position: &Position) -> Vec<Move32> {
    let mut list = Move32List::new();
    rsshogi::board::generate_moves_move32::<rsshogi::board::Legal>(position, &mut list);
    list.as_slice().to_vec()
}

#[cfg(test)]
mod tests {
    use rsshogi::board;

    use super::test_support::{depth_limits, material_job, run_job, run_result};
    use super::*;

    #[test]
    fn helpers_skip_different_depths_and_the_main_worker_skips_none() {
        for depth in 1..20 {
            assert!(!helper_skips_depth(0, depth));
        }
        let skipped =
            |worker| (3..12).filter(|&depth| helper_skips_depth(worker, depth)).collect::<Vec<_>>();
        assert_ne!(skipped(1), skipped(2));
        assert!(!skipped(1).is_empty());
        assert!((1..3).all(|depth| !helper_skips_depth(1, depth)), "浅いdepthは飛ばさない");
    }

    #[test]
    fn the_initial_aspiration_window_brackets_the_previous_score() {
        assert_eq!(aspiration_window(120, 25), (95, 145));
        assert_eq!(aspiration_window(-INF, 25), (-INF, -INF + 25));
        assert_eq!(aspiration_window(INF, 25), (INF - 25, INF));
    }

    #[test]
    fn adaptive_aspiration_stays_bounded_and_widens_for_unstable_scores() {
        assert_eq!(adaptive_aspiration_delta(25, 0), 25);
        assert_eq!(adaptive_aspiration_delta(25, 10), 35);
        assert_eq!(adaptive_aspiration_delta(25, 50), 75);
        assert_eq!(adaptive_aspiration_delta(25, MATE * 2), 75);
        assert_eq!(adaptive_aspiration_delta(0, 0), 1);
        assert_eq!(adaptive_aspiration_delta(INF, MATE * 2), INF);
    }

    #[test]
    fn a_failed_aspiration_widens_only_the_failing_side() {
        assert_eq!(
            widen_aspiration_window(95, 145, 95, 50),
            (45, 145),
            "fail-lowはalphaだけ下げる"
        );
        assert_eq!(
            widen_aspiration_window(95, 145, 200, 50),
            (95, 250),
            "fail-highはbetaだけ上げる"
        );
        assert_eq!(widen_aspiration_window(-INF + 10, 0, -INF + 10, 100), (-INF, 0));
        assert_eq!(widen_aspiration_window(0, INF - 10, INF - 10, 100), (0, INF));
    }

    #[test]
    fn the_reported_pv_is_a_legal_line_that_starts_with_the_best_move() {
        let result = run_result(board::hirate_position(), depth_limits(5), 1);

        let best_move = result.best_move.expect("hirate has legal moves");
        assert_eq!(result.pv.first().copied(), Some(best_move));
        assert!(result.pv.len() > 1, "a depth 5 search should reconstruct more than one move");
        assert_eq!(result.ponder_move, result.pv.get(1).copied());

        let mut position = board::hirate_position();
        for mv in &result.pv {
            assert!(position.is_legal_move32(*mv), "PVの{}が合法でない", mv.to_usi());
            position.apply_move32(*mv);
        }
    }

    #[test]
    fn helper_spawn_failure_cancels_and_joins_already_started_helpers() {
        let cancel = Arc::new(AtomicBool::new(false));
        let retained_cancel = Arc::clone(&cancel);
        let (job, receiver) = material_job(
            board::hirate_position(),
            depth_limits(MAX_SEARCH_DEPTH),
            SearchControl::new(cancel, Arc::new(AtomicBool::new(false)))
                .with_failed_helper_spawn(2),
            4,
        );
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_job(job);
        }));

        assert!(result.is_err(), "spawn failure must propagate to the coordinator boundary");
        assert!(retained_cancel.load(Ordering::Relaxed));
        assert_eq!(Arc::strong_count(&retained_cancel), 1, "started helpers must be joined");
        assert!(receiver.recv().is_err(), "no partial result may be published");
    }

    #[test]
    fn helper_panic_is_joined_and_propagated_without_a_done_event() {
        let cancel = Arc::new(AtomicBool::new(false));
        let retained_cancel = Arc::clone(&cancel);
        let (job, receiver) = material_job(
            board::hirate_position(),
            depth_limits(2),
            SearchControl::new(cancel, Arc::new(AtomicBool::new(false))).with_panicking_helper(1),
            2,
        );
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_job(job);
        }));

        assert!(result.is_err(), "helper panic must propagate to the coordinator boundary");
        assert!(retained_cancel.load(Ordering::Relaxed));
        assert_eq!(Arc::strong_count(&retained_cancel), 1, "the panicking helper must be joined");
        assert!(receiver.try_iter().all(|event| matches!(event, SearchEvent::Info(_))));
    }

    #[test]
    fn max_moves_rule_does_not_turn_a_root_go_into_resignation() {
        board::init();
        let position = board::position_from_sfen(
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 101",
        )
        .expect("valid position");
        let original = position.clone();
        let result =
            run_result(position, SearchLimits { max_moves_to_draw: 100, ..depth_limits(1) }, 1);

        assert!(result.best_move.is_some_and(|mv| original.is_legal_move32(mv)));
        assert_eq!(result.score, 0);
    }

    #[test]
    fn root_terminal_rule_precedes_an_entering_king_declaration() {
        let mut position = board::position_from_sfen(
            "K+N5+L1/G+L+P+B1+R+P2/3+P2G2/9/2+p+n5/3s2+ss1/3+p+p1+s1+r/7g+n/6g+nk b 2L8Pb4p 101",
        )
        .expect("valid point-rule position");
        position.set_entering_king_rule(rsshogi::types::EnteringKingRule::Point27);
        assert_eq!(position.declaration_win_move(), MOVE_WIN);
        let original = position.clone();

        let result =
            run_result(position, SearchLimits { max_moves_to_draw: 100, ..depth_limits(1) }, 1);

        assert_eq!(result.depth, 0);
        assert_eq!(result.score, 0);
        assert_ne!(result.best_move, Some(MOVE_WIN));
        assert!(result.best_move.is_some_and(|mv| original.is_legal_move32(mv)));
    }

    #[test]
    fn searchmoves_restricts_root() {
        let result = run_result(
            board::hirate_position(),
            SearchLimits { searchmoves: vec!["7g7f".to_owned()], ..depth_limits(1) },
            1,
        );

        assert_eq!(result.best_move.expect("best move").to_usi(), "7g7f");
    }

    #[test]
    fn node_limited_fallback_pairs_the_move_with_its_static_score() {
        let position = board::position_from_sfen("4k4/9/9/9/4r4/4P4/9/9/4K4 b - 1")
            .expect("valid fallback position");
        let result =
            run_result(position, SearchLimits { max_nodes: Some(1), ..depth_limits(64) }, 1);

        assert_eq!(result.depth, 0);
        assert_eq!(result.best_move.expect("fallback move").to_usi(), "5f5e");
        assert_eq!(result.score, 1_100);
        assert_eq!(result.nodes, 1);
    }

    #[test]
    fn cancelled_search_bounds_root_fallback_to_the_reserved_node() {
        let position = board::position_from_sfen("4k4/9/9/9/4r4/4P4/9/9/4K4 b - 1")
            .expect("valid fallback position");
        let (job, receiver) = material_job(
            position,
            depth_limits(MAX_SEARCH_DEPTH),
            SearchControl::new(Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false))),
            1,
        );
        run_job(job);
        let result = receiver
            .into_iter()
            .find_map(|event| match event {
                SearchEvent::Done(result) => Some(result),
                SearchEvent::Info(_) => None,
            })
            .expect("cancelled search should return its fallback");

        assert_eq!(result.depth, 0);
        assert_eq!(result.nodes, 1);
        assert_eq!(result.best_move.expect("fallback move").to_usi(), "5f5e");
        assert_eq!(result.score, 1_100);
    }

    #[test]
    fn multi_thread_search_respects_the_shared_node_limit() {
        let result = run_result(
            board::hirate_position(),
            SearchLimits { max_nodes: Some(5), ..depth_limits(MAX_SEARCH_DEPTH) },
            4,
        );

        assert!((1..=5).contains(&result.nodes));
    }

    #[test]
    fn helper_workers_do_not_preempt_the_main_workers_first_iteration() {
        let result = run_result(
            board::hirate_position(),
            SearchLimits { max_nodes: Some(100), ..depth_limits(MAX_SEARCH_DEPTH) },
            4,
        );

        assert!(result.depth >= 1);
        assert!(result.nodes <= 100);
    }

    #[test]
    fn direct_search_limits_depth_before_allocating_search_state() {
        let result = run_result(
            board::hirate_position(),
            SearchLimits { max_nodes: Some(1), ..depth_limits(u32::MAX) },
            1,
        );

        assert_eq!(result.depth, 0);
        assert_eq!(result.nodes, 1);
    }

    #[test]
    fn unsupported_root_game_ply_fails_closed_without_applying_a_move() {
        let position = board::position_from_sfen(
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 65535",
        )
        .expect("syntactically valid high-ply position");
        let result = run_result(position, depth_limits(1), 1);

        assert_eq!(result.best_move, None);
        assert_eq!(result.nodes, 0);
    }

    #[test]
    fn capturable_opposing_king_fails_closed_without_applying_a_move() {
        let position = board::position_from_sfen("4k4/4R4/9/9/9/9/9/9/4K4 b - 1")
            .expect("syntactically valid invalid position");
        let result = run_result(position, depth_limits(1), 1);

        assert_eq!(result.best_move, None);
        assert_eq!(result.nodes, 0);
    }
}

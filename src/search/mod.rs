//! 探索モジュールの入口。workerの起動と反復深化の運転、公開型の再輸出を担う。

mod context;
mod deadline;
mod history;
mod lmr;
mod negamax;
mod ordering;
mod pruning;
mod qsearch;
mod root;
mod score;
#[cfg(test)]
pub(crate) mod test_support;
mod timing;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc::Sender};
use std::thread;
use std::time::{Duration, Instant};

use rsshogi::board::{Move32List, Position, generate_legal_all_move32};
use rsshogi::types::{MOVE_WIN, Move32};

use crate::eval::Evaluator;
use crate::nnue::MAX_NNUE_EVAL;
use crate::params::SearchParams;
use crate::position::{MAX_SEARCH_DEPTH, validate_evaluable_position};
use crate::tt::TranspositionTable;

use context::{SearchContext, SharedSearch};
pub(crate) use history::HistoryTables;
use root::{collect_pv, fallback_root_choice, search_root};
use score::root_terminal_score;
use timing::IterationTiming;

pub use deadline::SearchDeadline;
pub(crate) use score::mate_distance;

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
pub struct SearchLimits {
    pub max_depth: u32,
    pub max_nodes: Option<u64>,
    /// hard stopと、新しいiterationを始めないsoft stopをまとめた締切。
    pub deadline: Option<Arc<SearchDeadline>>,
    pub searchmoves: Vec<String>,
    pub max_moves_to_draw: u32,
}

#[derive(Debug, Clone)]
pub struct SearchIteration {
    pub depth: u32,
    pub score: i32,
    pub nodes: u64,
    pub elapsed: Duration,
    /// rootからの読み筋。先頭は必ずそのiterationの最善手。
    pub pv: Vec<Move32>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub best_move: Option<Move32>,
    pub ponder_move: Option<Move32>,
    pub score: i32,
    pub depth: u32,
    pub nodes: u64,
    pub elapsed: Duration,
    /// rootからの読み筋。`best_move`がないときは空。
    pub pv: Vec<Move32>,
}

impl SearchResult {
    /// 指せる手を出せないときのfail-closedな結果。投了の合図として使う。
    pub fn fail_closed(elapsed: Duration) -> Self {
        Self {
            best_move: None,
            ponder_move: None,
            score: 0,
            depth: 0,
            nodes: 0,
            elapsed,
            pv: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SearchEvent {
    Info(SearchIteration),
    Done(SearchResult),
}

#[derive(Clone)]
pub struct SearchControl {
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
    pub fn new(cancel: Arc<AtomicBool>, pondering: Arc<AtomicBool>) -> Self {
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
    pub fn with_panic_after_helpers_spawned(mut self, enabled: bool) -> Self {
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

/// 一回の`go`で探索する内容。常駐coordinatorから[`run`]へ渡す。
pub struct SearchJob {
    pub position: Position,
    pub evaluator: Evaluator,
    pub params: SearchParams,
    pub limits: SearchLimits,
    pub control: SearchControl,
    pub events: Sender<SearchEvent>,
    pub threads: usize,
}

/// 結果を公開するメインworkerだけが持つ役割。
struct MainWorker<'a> {
    events: &'a Sender<SearchEvent>,
    /// 深さ1を完了したら立て、ヘルパーの探索を始めさせる。
    helpers_start: &'a AtomicBool,
}

/// `histories`はworker slotごとの履歴テーブルで、呼び出し側が`go`をまたいで保持する。
/// worker数と長さが合わなければ作り直し、探索開始時に全slotを半減(aging)する。
/// panicで終わった探索は履歴を返さず、次の`go`がゼロから作り直す。
pub fn run(job: SearchJob, table: Arc<TranspositionTable>, histories: &mut Vec<HistoryTables>) {
    let SearchJob { position, evaluator, params, mut limits, control, events, threads } = job;
    let start = Instant::now();
    if validate_evaluable_position(&position).is_err() {
        let _ = events.send(SearchEvent::Done(SearchResult::fail_closed(start.elapsed())));
        return;
    }
    limits.max_depth = limits.max_depth.clamp(1, MAX_SEARCH_DEPTH);
    limits.max_nodes = limits.max_nodes.map(|nodes| nodes.max(1));
    table.new_search();
    // 履歴は`go`をまたいで持続する。worker数が変わったら作り直し、
    // 新しい探索の開始時に全slotを半減して古い傾向の重みを下げる。
    let worker_count = threads.max(1);
    if histories.len() != worker_count {
        *histories = std::iter::repeat_with(HistoryTables::new).take(worker_count).collect();
    }
    for history in histories.iter_mut() {
        history.age();
        history.configure(&params);
    }
    let mut history_slots = std::mem::take(histories);
    let helper_histories = history_slots.drain(1..).collect::<Vec<_>>();
    let main_history = history_slots.pop().expect("worker 0 always has a history slot");
    let shared = SharedSearch {
        cancel: control.cancel,
        pondering: control.pondering,
        // Reserve the first node for the reporting worker's evaluated root fallback.
        nodes: Arc::new(AtomicU64::new(1)),
        table,
    };
    let helpers_start = Arc::new(AtomicBool::new(false));
    let mut helpers = Vec::new();
    for (worker_id, helper_history) in (1..worker_count).zip(helper_histories) {
        let helper_position = position.clone();
        let helper_context = SearchContext::new(
            evaluator.clone(),
            params,
            limits.clone(),
            shared.clone(),
            helper_history,
        );
        let helper_start = Arc::clone(&helpers_start);
        #[cfg(test)]
        let panic_helper_worker = control.panic_helper_worker;
        let helper_work = move || {
            wait_for_helper_start(&helper_start, &helper_context.cancel);
            #[cfg(test)]
            assert_ne!(panic_helper_worker, Some(worker_id), "injected helper search panic");
            search_worker(helper_position, helper_context, worker_id, None).1
        };
        #[cfg(test)]
        let spawn_result = if control.fail_helper_spawn_at == Some(worker_id) {
            Err(std::io::Error::other("injected helper spawn failure"))
        } else {
            thread::Builder::new().spawn(helper_work)
        };
        #[cfg(not(test))]
        let spawn_result = thread::Builder::new().spawn(helper_work);
        match spawn_result {
            Ok(helper) => helpers.push(helper),
            Err(error) => {
                shared.cancel.store(true, Ordering::Relaxed);
                for helper in helpers {
                    let _ = helper.join();
                }
                panic!("failed to spawn search helper {worker_id}: {error}");
            }
        }
    }

    let main_context = SearchContext::new(evaluator, params, limits, shared.clone(), main_history);
    let result = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(test)]
        assert!(!control.panic_after_helpers_spawned, "injected main search worker panic");
        let main = MainWorker { events: &events, helpers_start: &helpers_start };
        search_worker(position, main_context, 0, Some(main))
    }));
    shared.cancel.store(true, Ordering::Relaxed);
    let mut helper_panic = None;
    let mut returned_helper_histories = Vec::with_capacity(worker_count.saturating_sub(1));
    for helper in helpers {
        match helper.join() {
            Ok(helper_history) => returned_helper_histories.push(helper_history),
            Err(payload) => {
                if helper_panic.is_none() {
                    helper_panic = Some(payload);
                }
            }
        }
    }
    match result {
        Err(payload) => resume_unwind(payload),
        Ok(_) if let Some(payload) = helper_panic => resume_unwind(payload),
        Ok((mut result, main_history)) => {
            histories.push(main_history);
            histories.append(&mut returned_helper_histories);
            result.nodes = shared.nodes.load(Ordering::Relaxed);
            let _ = events.send(SearchEvent::Done(result));
        }
    }
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
) -> (SearchResult, HistoryTables) {
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
            pv,
        };
        return (result, context.history);
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
        if let Some(index) = root_moves.iter().position(|mv| *mv == best_move) {
            root_moves.swap(0, index);
        }

        // 完了した探索がaspiration窓を外れたら、外れた側だけを広げて同じdepthを引き直す。
        let mut delta = context.params.aspiration_window.max(1);
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
        };
        let Some(outcome) = outcome else {
            break;
        };

        best_move = outcome.best_move;
        best_score = outcome.score;
        // 打ち切られたiterationでも、alphaを更新できた手はその窓で前の最善手を上回っている。
        // depthは完了したiterationのものを保ち、途中経過をinfoとして公開もしない。
        if !outcome.complete {
            break;
        }
        completed_depth = depth;
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
        pv,
    };
    (result, context.history)
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

    use super::test_support::{depth_limits, idle_control, material_job, run_result};
    use super::*;

    #[test]
    fn the_initial_aspiration_window_brackets_the_previous_score() {
        assert_eq!(aspiration_window(120, 25), (95, 145));
        assert_eq!(aspiration_window(-INF, 25), (-INF, -INF + 25));
        assert_eq!(aspiration_window(INF, 25), (INF - 25, INF));
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
            run(job, Arc::new(TranspositionTable::new(1)), &mut Vec::new());
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
            run(job, Arc::new(TranspositionTable::new(1)), &mut Vec::new());
        }));

        assert!(result.is_err(), "helper panic must propagate to the coordinator boundary");
        assert!(retained_cancel.load(Ordering::Relaxed));
        assert_eq!(Arc::strong_count(&retained_cancel), 1, "the panicking helper must be joined");
        assert!(receiver.try_iter().all(|event| matches!(event, SearchEvent::Info(_))));
    }

    #[test]
    fn depth_one_returns_a_legal_move() {
        board::init();
        let position = board::hirate_position();
        let original = position.clone();
        let result = run_result(position, depth_limits(1), 1);

        assert_eq!(result.depth, 1);
        assert!(original.is_legal_move32(result.best_move.expect("best move")));
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
        run(job, Arc::new(TranspositionTable::new(1)), &mut Vec::new());
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

    /// 深さ1では静かな手のβカットが起きないため、履歴は探索自体からは変化しない。
    /// これを使って、runをまたぐ持続とagingだけを観測する。
    fn run_depth_one(histories: &mut Vec<HistoryTables>, threads: usize) {
        let (job, receiver) =
            material_job(board::hirate_position(), depth_limits(1), idle_control(), threads);
        run(job, Arc::new(TranspositionTable::new(1)), histories);
        assert!(
            receiver.into_iter().any(|event| matches!(event, SearchEvent::Done(_))),
            "search should finish"
        );
    }

    #[test]
    fn histories_persist_across_runs_and_age_at_each_start() {
        board::init();
        let position = board::hirate_position();
        let mv = board::move_from_usi(&position, "7g7f").expect("legal quiet move");
        let stm = rsshogi::types::Color::BLACK;
        let mut histories = Vec::new();

        run_depth_one(&mut histories, 1);
        assert_eq!(histories.len(), 1, "runがworker slot分の履歴を作って返す");

        histories[0].record_quiet_cutoff(stm, mv, &[], 4);
        let before = histories[0].quiet_score(stm, mv);
        assert!(before > 0);

        run_depth_one(&mut histories, 1);
        assert_eq!(
            histories[0].quiet_score(stm, mv),
            before / 2,
            "2回目のgoで前回の履歴が半減されて残る"
        );
    }

    #[test]
    fn a_cleared_history_vec_restarts_from_zero() {
        board::init();
        let position = board::hirate_position();
        let mv = board::move_from_usi(&position, "7g7f").expect("legal quiet move");
        let stm = rsshogi::types::Color::BLACK;
        let mut histories = Vec::new();

        run_depth_one(&mut histories, 1);
        histories[0].record_quiet_cutoff(stm, mv, &[], 4);
        assert!(histories[0].quiet_score(stm, mv) > 0);

        // usinewgameはUSIセッション側がVecをclearする。次のgoはゼロから作り直す。
        histories.clear();
        run_depth_one(&mut histories, 1);
        assert_eq!(histories.len(), 1);
        assert_eq!(histories[0].quiet_score(stm, mv), 0);
    }

    #[test]
    fn history_slots_are_rebuilt_when_the_worker_count_changes() {
        board::init();
        let mut histories = Vec::new();

        run_depth_one(&mut histories, 3);
        assert_eq!(histories.len(), 3, "helperを含む全workerが自分のslotを返す");

        run_depth_one(&mut histories, 1);
        assert_eq!(histories.len(), 1, "worker数が減ったらVecを作り直す");
    }

    #[test]
    fn depth_two_returns_a_legal_ponder_move() {
        board::init();
        let position = board::hirate_position();
        let result = run_result(position.clone(), depth_limits(2), 1);

        let best_move = result.best_move.expect("best move");
        let mut child = position;
        child.apply_move32(best_move);
        assert!(child.is_legal_move32(result.ponder_move.expect("ponder move")));
    }
}

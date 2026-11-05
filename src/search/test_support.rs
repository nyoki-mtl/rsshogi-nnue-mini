//! 探索テストが共有する文脈生成と実行の補助を提供する。

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, mpsc};

use rsshogi::board::Position;

use crate::eval::{EvalParams, Evaluator};

use super::context::{SearchContext, SharedSearch};
use super::coordinator::SearchCoordinator;
use super::history::HistoryTables;
use super::params::SearchParams;
use super::tt::TranspositionTable;
use super::{SearchControl, SearchEvent, SearchJob, SearchLimits, SearchResult};

/// 深さだけを制限し、node数・時間・候補手・手数の制限を持たないlimits。
pub(super) fn depth_limits(max_depth: u32) -> SearchLimits {
    SearchLimits {
        max_depth,
        max_nodes: None,
        deadline: None,
        searchmoves: Vec::new(),
        max_moves_to_draw: 0,
    }
}

pub(super) fn test_context(nodes: Arc<AtomicU64>, max_nodes: Option<u64>) -> SearchContext {
    let mut context = SearchContext::new(
        Evaluator::material(EvalParams::default()),
        SearchParams::default(),
        SearchLimits { max_nodes, ..depth_limits(1) },
        SharedSearch {
            cancel: Arc::new(AtomicBool::new(false)),
            pondering: Arc::new(AtomicBool::new(false)),
            nodes,
            table: Arc::new(TranspositionTable::new(1)),
        },
        HistoryTables::new(),
    );
    context.root_depth = 64;
    context
}

/// 駒得評価と既定の探索パラメータで探索するjobと、そのeventの受信側。
pub(super) fn material_job(
    position: Position,
    limits: SearchLimits,
    control: SearchControl,
    threads: usize,
) -> (SearchJob, mpsc::Receiver<SearchEvent>) {
    let (events, receiver) = mpsc::channel();
    let job = SearchJob {
        position,
        evaluator: Evaluator::material(EvalParams::default()),
        params: SearchParams::default(),
        limits,
        control,
        events,
        threads,
    };
    (job, receiver)
}

pub(super) fn idle_control() -> SearchControl {
    SearchControl::new(Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)))
}

pub(super) fn run_job(job: SearchJob) {
    let mut coordinator = SearchCoordinator::new(job.threads, &job.control);
    coordinator.run(job, Arc::new(TranspositionTable::new(1)));
}

pub(super) fn run_result(position: Position, limits: SearchLimits, threads: usize) -> SearchResult {
    let (job, receiver) = material_job(position, limits, idle_control(), threads);
    run_job(job);
    receiver
        .into_iter()
        .find_map(|event| match event {
            SearchEvent::Done(result) => Some(result),
            SearchEvent::Info(_) => None,
        })
        .expect("search should finish")
}

//! 探索テストが共有する文脈生成と実行の補助を提供する。

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, mpsc};

use rsshogi::board::Position;

use crate::eval::{EvalParams, Evaluator};
use crate::params::SearchParams;
use crate::tt::TranspositionTable;

use super::context::SearchContext;
use super::history::HistoryTables;
use super::lmr::LmrReductions;
use super::{SearchControl, SearchEvent, SearchLimits, SearchResult, run};

pub(crate) fn test_context(nodes: Arc<AtomicU64>, max_nodes: Option<u64>) -> SearchContext {
    SearchContext {
        evaluator: Evaluator::material(EvalParams::default()),
        params: SearchParams::default(),
        limits: SearchLimits {
            max_depth: 1,
            max_nodes,
            deadline: None,
            searchmoves: Vec::new(),
            max_moves_to_draw: 0,
        },
        cancel: Arc::new(AtomicBool::new(false)),
        pondering: Arc::new(AtomicBool::new(false)),
        nodes,
        table: Arc::new(TranspositionTable::new(1)),
        history: HistoryTables::new(),
        killers: vec![[None; 2]; 20],
        lmr: LmrReductions::new(SearchParams::default().lmr_divisor),
        ordering: std::iter::repeat_with(Default::default)
            .take(crate::position::MAX_SEARCH_PLY as usize + 2)
            .collect(),
        continuation: vec![None; crate::position::MAX_SEARCH_PLY as usize + 2],
    }
}

pub(crate) fn run_result(position: Position, limits: SearchLimits, threads: usize) -> SearchResult {
    let (sender, receiver) = mpsc::channel();
    run(
        position,
        Evaluator::material(EvalParams::default()),
        SearchParams::default(),
        limits,
        SearchControl::new(Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false))),
        sender,
        Arc::new(TranspositionTable::new(1)),
        &mut Vec::new(),
        threads,
    );
    receiver
        .into_iter()
        .find_map(|event| match event {
            SearchEvent::Done(result) => Some(result),
            SearchEvent::Info(_) => None,
        })
        .expect("search should finish")
}

//! Synchronous embedding API. The host owns scheduling; only native helper workers spawn threads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rsshogi::board::{self, Position};
use rsshogi::types::EnteringKingRule;
use rsshogi_usi::PositionSpec;
use serde::{Deserialize, Serialize};

use crate::eval::{EvalParams, Evaluator};
use crate::nnue::AonlyNetwork;
use crate::params::SearchParams;
use crate::position::{MAX_SEARCH_DEPTH, replay};
use crate::search::{
    SearchControl, SearchDeadline, SearchEvent, SearchJob, SearchLimits, SearchWorkers,
};
use crate::tt::TranspositionTable;

/// Clone before running a blocking search to cancel it from another native thread.
/// A stop issued just before search entry also cancels that next search.
#[derive(Clone, Default)]
pub struct StopHandle {
    cancel: Arc<AtomicBool>,
    searching: Arc<AtomicBool>,
}

impl StopHandle {
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_searching(&self) -> bool {
        self.searching.load(Ordering::Acquire)
    }
}

struct SearchActivity(StopHandle);

impl Drop for SearchActivity {
    fn drop(&mut self) {
        self.0.cancel.store(false, Ordering::Relaxed);
        self.0.searching.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub depth: u32,
    pub nodes: Option<u64>,
    pub movetime_ms: Option<u64>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self { depth: MAX_SEARCH_DEPTH, nodes: None, movetime_ms: None }
    }
}

/// Scores use the engine's side-to-move scale, including its mate-score band.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchOutput {
    pub bestmove: String,
    pub ponder: Option<String>,
    pub score: i32,
    pub depth: u32,
    pub nodes: u64,
    pub elapsed_ms: u64,
    pub hashfull: u32,
    pub pv: Vec<String>,
}

/// Independent position, evaluator, transposition table and persistent worker contexts.
/// Methods are synchronous and require exclusive access. Run search on a host background queue.
pub struct Engine {
    position: Position,
    network: Option<Arc<AonlyNetwork>>,
    table: Arc<TranspositionTable>,
    workers: Option<SearchWorkers>,
    threads: usize,
    hash_mb: usize,
    stop: StopHandle,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Starts without an evaluator. Load model bytes or the optional embedded model first.
    pub fn new() -> Self {
        board::init();
        Self {
            position: initial_position(),
            network: None,
            table: Arc::new(TranspositionTable::new(16)),
            workers: None,
            threads: 1,
            hash_mb: 16,
            stop: StopHandle::default(),
        }
    }

    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    /// Model errors leave the previously loaded evaluator intact.
    pub fn load_model(&mut self, bytes: &[u8]) -> Result<String, String> {
        let network = AonlyNetwork::load_bytes(bytes)?;
        Ok(self.install(network))
    }

    pub fn load_embedded(&mut self) -> Result<String, String> {
        #[cfg(feature = "embedded-rsnn")]
        {
            Ok(self.install(AonlyNetwork::load_embedded()?))
        }
        #[cfg(not(feature = "embedded-rsnn"))]
        {
            Err("this build has no embedded model".to_owned())
        }
    }

    fn install(&mut self, network: AonlyNetwork) -> String {
        let digest = network.digest().to_hex();
        self.workers = None;
        self.network = Some(Arc::new(network));
        self.table.clear();
        digest
    }

    pub fn configure(
        &mut self,
        hash_mb: Option<usize>,
        threads: Option<usize>,
    ) -> Result<(), String> {
        let hash_mb = hash_mb.unwrap_or(self.hash_mb);
        let threads = threads.unwrap_or(self.threads);
        if !(1..=256).contains(&hash_mb) {
            return Err("hash_mb must be in 1..=256".to_owned());
        }
        if !(1..=16).contains(&threads) {
            return Err("threads must be in 1..=16".to_owned());
        }
        if cfg!(target_arch = "wasm32") && threads != 1 {
            return Err("WebAssembly supports threads=1 only".to_owned());
        }
        if hash_mb != self.hash_mb {
            let table = Arc::new(TranspositionTable::new(hash_mb));
            if let Some(workers) = &mut self.workers {
                workers.set_table(&table);
            }
            self.table = table;
            self.hash_mb = hash_mb;
        }
        if threads != self.threads {
            self.workers = None;
            self.table.clear();
            self.threads = threads;
        }
        Ok(())
    }

    /// Accepts `startpos` or the four raw SFEN fields. Move replay preserves repetition history.
    /// Invalid input does not alter the current position.
    pub fn set_position(&mut self, sfen: &str, moves: &[String]) -> Result<(), String> {
        let fields: Vec<_> = sfen.split_whitespace().collect();
        let spec = match fields.as_slice() {
            ["startpos"] => PositionSpec::StartPos,
            [board, side, hands, ply] => PositionSpec::Sfen {
                board: (*board).to_owned(),
                side_to_move: (*side).to_owned(),
                hands: (*hands).to_owned(),
                ply: (*ply).to_owned(),
            },
            _ => return Err("expected startpos or four SFEN fields".to_owned()),
        };
        let mut position = replay(&spec, moves)?;
        position.set_entering_king_rule(EnteringKingRule::Point27);
        self.position = position;
        Ok(())
    }

    pub fn new_game(&mut self) {
        self.position = initial_position();
        self.table.clear();
        if let Some(workers) = &mut self.workers {
            workers.clear();
        }
    }

    pub fn evaluate(&self) -> Result<i32, String> {
        let mut evaluator = self.evaluator()?;
        evaluator.initialize(&self.position);
        Ok(evaluator.evaluate(&self.position))
    }

    fn evaluator(&self) -> Result<Evaluator, String> {
        let network = self.network.as_ref().ok_or("load a model before evaluation or search")?;
        Ok(Evaluator::nnue(EvalParams::default(), Arc::clone(network)))
    }

    /// Runs the existing search driver on this calling thread, without a USI coordinator.
    /// On WebAssembly use a short movetime budget to yield control back to the host.
    pub fn search(&mut self, options: SearchOptions) -> Result<SearchOutput, String> {
        if !(1..=MAX_SEARCH_DEPTH).contains(&options.depth) {
            return Err(format!("depth must be in 1..={MAX_SEARCH_DEPTH}"));
        }
        if options.nodes == Some(0) || options.movetime_ms == Some(0) {
            return Err("nodes and movetime_ms must be positive when specified".to_owned());
        }
        let evaluator = self.evaluator()?;
        let control =
            SearchControl::new(Arc::clone(&self.stop.cancel), Arc::new(AtomicBool::new(false)));
        // Do not clear cancel here: a host may stop after admitting the request but before entry.
        self.stop.searching.store(true, Ordering::Release);
        let _activity = SearchActivity(self.stop.clone());
        let workers =
            self.workers.get_or_insert_with(|| SearchWorkers::new(self.threads, &control));
        let (events, receiver) = mpsc::channel();
        let budget = options.movetime_ms.map(Duration::from_millis);
        workers.run(
            SearchJob {
                position: self.position.clone(),
                evaluator,
                params: SearchParams::default(),
                limits: SearchLimits {
                    max_depth: options.depth,
                    max_nodes: options.nodes,
                    deadline: budget
                        .map(|time| Arc::new(SearchDeadline::new(Some(time), Some(time)))),
                    searchmoves: Vec::new(),
                    max_moves_to_draw: 0,
                },
                control,
                events,
                threads: self.threads,
            },
            Arc::clone(&self.table),
        );
        let result = receiver
            .into_iter()
            .find_map(|event| match event {
                SearchEvent::Done(result) => Some(result),
                SearchEvent::Info(_) => None,
            })
            .ok_or("search ended without a result")?;
        Ok(SearchOutput {
            bestmove: result.best_move.map_or_else(|| "resign".to_owned(), |m| m.to_usi()),
            ponder: result.ponder_move.map(|m| m.to_usi()),
            score: result.score,
            depth: result.depth,
            nodes: result.nodes,
            elapsed_ms: result.elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            hashfull: result.hashfull,
            pv: result.pv.iter().map(|m| m.to_usi()).collect(),
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        let mut engine = Self::new();
        engine.install(crate::nnue::synthetic_network(0x107));
        engine
    }
}

fn initial_position() -> Position {
    let mut position = board::hirate_position();
    position.set_entering_king_rule(EnteringKingRule::Point27);
    position
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_updates_without_losing_position_or_model() {
        let mut engine = Engine::fixture();
        let score = engine.evaluate().unwrap();
        assert!(engine.load_model(b"invalid").is_err());
        assert!(engine.set_position("startpos", &["7g7e".to_owned()]).is_err());
        assert!(engine.configure(Some(0), Some(2)).is_err());
        assert_eq!(engine.evaluate().unwrap(), score);
        assert_eq!(engine.threads, 1);
        assert!(Engine::new().evaluate().is_err());
    }

    #[test]
    fn same_driver_is_used_and_position_is_preserved() {
        let mut engine = Engine::fixture();
        engine.configure(Some(1), None).unwrap();
        let before = engine.position.key();
        let output = engine.search(SearchOptions { depth: 2, ..SearchOptions::default() }).unwrap();
        assert_eq!(output.depth, 2);
        assert_eq!(output.pv.first(), Some(&output.bestmove));
        assert!(board::move_from_usi(&engine.position, &output.bestmove).is_some());
        assert_eq!(engine.position.key(), before);
        engine.configure(Some(2), None).unwrap();
        engine.new_game();
        assert_eq!(engine.evaluate().unwrap(), Engine::fixture().evaluate().unwrap());
    }

    #[test]
    fn pending_stop_is_consumed_once_and_node_budget_is_observed() {
        let mut engine = Engine::fixture();
        let stop = engine.stop_handle();
        stop.stop();
        let stopped =
            engine.search(SearchOptions { depth: 2, ..SearchOptions::default() }).unwrap();
        assert_eq!(stopped.depth, 0);
        assert!(!stop.is_searching());
        let next =
            engine.search(SearchOptions { nodes: Some(100), ..SearchOptions::default() }).unwrap();
        assert!(next.nodes <= 100);
        assert!(next.nodes > stopped.nodes);
        assert!(!stop.is_searching());
    }

    #[test]
    fn native_search_can_be_stopped_from_another_thread() {
        let mut engine = Engine::fixture();
        let stop = engine.stop_handle();
        let handle = std::thread::spawn(move || engine.search(SearchOptions::default()).unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !stop.is_searching() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        stop.stop();
        let result = handle.join().unwrap();
        assert_ne!(result.bestmove, "resign");
        assert!(!stop.is_searching());
    }
}

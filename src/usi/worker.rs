//! 探索requestを常駐threadで実行するworker runtime。置換表の寿命も持つ。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use rsshogi::board::Position;

use crate::eval::Evaluator;
use crate::params::SearchParams;
use crate::search::{self, HistoryTables, SearchControl, SearchEvent, SearchLimits};
use crate::tt::TranspositionTable;

use super::DEFAULT_HASH_MB;

pub(super) struct SearchRequest {
    pub(super) position: Position,
    pub(super) evaluator: Evaluator,
    pub(super) search_params: SearchParams,
    pub(super) limits: SearchLimits,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) pondering: Arc<AtomicBool>,
    pub(super) events: mpsc::Sender<SearchEvent>,
    pub(super) threads: usize,
    #[cfg(test)]
    pub(super) panic_after_helpers_spawned: bool,
}

enum WorkerCommand {
    Run(Box<SearchRequest>),
    Clear(mpsc::SyncSender<()>),
    ResizeHash(usize, mpsc::SyncSender<()>),
    Shutdown,
}

pub(super) struct SearchWorkerRuntime {
    commands: mpsc::SyncSender<WorkerCommand>,
    handle: Option<JoinHandle<()>>,
}

impl SearchWorkerRuntime {
    pub(super) fn new() -> Self {
        let (commands, receiver) = mpsc::sync_channel(1);
        let handle = thread::spawn(move || {
            let mut table = Arc::new(TranspositionTable::new(DEFAULT_HASH_MB));
            // worker slotごとの履歴テーブル。対局中は`go`をまたいで持続し、
            // `usinewgame`(Clear)で破棄する。長さの調整は`search::run`が行う。
            let mut histories: Vec<HistoryTables> = Vec::new();
            while let Ok(command) = receiver.recv() {
                match command {
                    WorkerCommand::Run(request) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            #[cfg(test)]
                            let control = SearchControl::new(request.cancel, request.pondering)
                                .with_panic_after_helpers_spawned(
                                    request.panic_after_helpers_spawned,
                                );
                            #[cfg(not(test))]
                            let control = SearchControl::new(request.cancel, request.pondering);
                            search::run(
                                request.position,
                                request.evaluator,
                                request.search_params,
                                request.limits,
                                control,
                                request.events,
                                Arc::clone(&table),
                                &mut histories,
                                request.threads,
                            );
                        }));
                    }
                    WorkerCommand::Clear(acknowledge) => {
                        table.clear();
                        histories.clear();
                        let _ = acknowledge.send(());
                    }
                    WorkerCommand::ResizeHash(megabytes, acknowledge) => {
                        table = Arc::new(TranspositionTable::new(megabytes));
                        let _ = acknowledge.send(());
                    }
                    WorkerCommand::Shutdown => break,
                }
            }
        });
        Self { commands, handle: Some(handle) }
    }

    pub(super) fn start(&self, request: SearchRequest) {
        self.commands
            .send(WorkerCommand::Run(Box::new(request)))
            .expect("search worker must be alive");
    }

    pub(super) fn clear(&self) {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands.send(WorkerCommand::Clear(acknowledge)).expect("search worker must be alive");
        completed.recv().expect("search worker must acknowledge hash clear");
    }

    pub(super) fn resize_hash(&self, megabytes: usize) {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands
            .send(WorkerCommand::ResizeHash(megabytes, acknowledge))
            .expect("search worker must be alive");
        completed.recv().expect("search worker must acknowledge hash resize");
    }
}

impl Drop for SearchWorkerRuntime {
    fn drop(&mut self) {
        let _ = self.commands.send(WorkerCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

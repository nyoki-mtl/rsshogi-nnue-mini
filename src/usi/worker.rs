//! 探索requestを常駐threadで実行するworker runtime。置換表の寿命も持つ。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use crate::search::{self, HistoryTables, SearchJob};
use crate::tt::TranspositionTable;

use super::DEFAULT_HASH_MB;

enum WorkerCommand {
    Run(Box<SearchJob>),
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
                    WorkerCommand::Run(job) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            search::run(*job, Arc::clone(&table), &mut histories);
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

    pub(super) fn start(&self, job: SearchJob) {
        self.commands.send(WorkerCommand::Run(Box::new(job))).expect("search worker must be alive");
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

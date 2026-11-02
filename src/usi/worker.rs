//! 探索requestを常駐threadで実行するworker runtime。置換表の寿命も持つ。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use crate::search::{SearchJob, SearchWorkers};
use crate::tt::TranspositionTable;

use super::DEFAULT_HASH_MB;

enum WorkerCommand {
    Run(Box<SearchJob>),
    Clear(mpsc::SyncSender<()>),
    Reset(mpsc::SyncSender<()>),
    ResizeHash(usize, mpsc::SyncSender<()>),
    #[cfg(test)]
    ThreadIds(mpsc::SyncSender<Vec<thread::ThreadId>>),
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
            let mut workers: Option<SearchWorkers> = None;
            while let Ok(command) = receiver.recv() {
                match command {
                    WorkerCommand::Run(job) => {
                        // 異常時のチャネル切断はpoolの破棄と全threadのjoin後に通知する。
                        let events_guard = job.events.clone();
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            if workers.as_ref().is_none_or(|pool| pool.len() != job.threads.max(1))
                            {
                                // 古いpoolをjoinしてから新しいthreadを作る。
                                drop(workers.take());
                                workers = Some(SearchWorkers::new(job.threads, &job.control));
                            }
                            workers.as_mut().unwrap().run(*job, Arc::clone(&table));
                        }));
                        if result.is_err() {
                            // panicで途中になった状態は再利用しない。次のgoで再構築する。
                            workers = None;
                        }
                        drop(events_guard);
                    }
                    WorkerCommand::Clear(acknowledge) => {
                        table.clear();
                        if catch_unwind(AssertUnwindSafe(|| {
                            if let Some(pool) = &mut workers {
                                pool.clear();
                            }
                        }))
                        .is_err()
                        {
                            workers = None;
                        }
                        let _ = acknowledge.send(());
                    }
                    WorkerCommand::Reset(acknowledge) => {
                        // 評価packageの変更では旧network snapshotもここで解放する。
                        workers = None;
                        table.clear();
                        let _ = acknowledge.send(());
                    }
                    WorkerCommand::ResizeHash(megabytes, acknowledge) => {
                        table = Arc::new(TranspositionTable::new(megabytes));
                        if catch_unwind(AssertUnwindSafe(|| {
                            if let Some(pool) = &mut workers {
                                pool.set_table(&table);
                            }
                        }))
                        .is_err()
                        {
                            workers = None;
                        }
                        let _ = acknowledge.send(());
                    }
                    WorkerCommand::Shutdown => break,
                    #[cfg(test)]
                    WorkerCommand::ThreadIds(acknowledge) => {
                        let ids =
                            workers.as_ref().map(SearchWorkers::thread_ids).unwrap_or_default();
                        let _ = acknowledge.send(ids);
                    }
                }
            }
        });
        Self { commands, handle: Some(handle) }
    }

    pub(super) fn start(&self, job: SearchJob) {
        self.commands.send(WorkerCommand::Run(Box::new(job))).expect("search worker must be alive");
    }

    #[cfg(test)]
    pub(super) fn thread_ids(&self) -> Vec<thread::ThreadId> {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands.send(WorkerCommand::ThreadIds(acknowledge)).unwrap();
        completed.recv().unwrap()
    }

    pub(super) fn clear(&self) {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands.send(WorkerCommand::Clear(acknowledge)).expect("search worker must be alive");
        completed.recv().expect("search worker must acknowledge hash clear");
    }

    pub(super) fn reset(&self) {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands.send(WorkerCommand::Reset(acknowledge)).expect("search worker must be alive");
        completed.recv().expect("search worker must acknowledge reset");
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

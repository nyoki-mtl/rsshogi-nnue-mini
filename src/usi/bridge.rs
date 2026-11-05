//! USIセッションと探索コーディネーターを仲介する常駐ブリッジ。置換表の寿命も持つ。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use crate::search::{SearchCoordinator, SearchJob, TranspositionTable};

use super::DEFAULT_HASH_MB;

enum BridgeCommand {
    Run(Box<SearchJob>),
    Clear(mpsc::SyncSender<()>),
    Reset(mpsc::SyncSender<()>),
    ResizeHash(usize, mpsc::SyncSender<()>),
    #[cfg(test)]
    ThreadIds(mpsc::SyncSender<Vec<thread::ThreadId>>),
    Shutdown,
}

pub(super) struct UsiSearchBridge {
    commands: mpsc::SyncSender<BridgeCommand>,
    handle: Option<JoinHandle<()>>,
}

impl UsiSearchBridge {
    pub(super) fn new() -> Self {
        let (commands, receiver) = mpsc::sync_channel(1);
        let handle = thread::spawn(move || {
            let mut table = Arc::new(TranspositionTable::new(DEFAULT_HASH_MB));
            let mut coordinator: Option<SearchCoordinator> = None;
            while let Ok(command) = receiver.recv() {
                match command {
                    BridgeCommand::Run(job) => {
                        // 異常時のチャネル切断はcoordinatorの破棄と全threadのjoin後に通知する。
                        let events_guard = job.events.clone();
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            if coordinator
                                .as_ref()
                                .is_none_or(|current| current.len() != job.threads.max(1))
                            {
                                // 古いcoordinatorをjoinしてから新しいthreadを作る。
                                drop(coordinator.take());
                                coordinator =
                                    Some(SearchCoordinator::new(job.threads, &job.control));
                            }
                            coordinator.as_mut().unwrap().run(*job, Arc::clone(&table));
                        }));
                        if result.is_err() {
                            // panicで途中になった状態は再利用しない。次のgoで再構築する。
                            coordinator = None;
                        }
                        drop(events_guard);
                    }
                    BridgeCommand::Clear(acknowledge) => {
                        table.clear();
                        if catch_unwind(AssertUnwindSafe(|| {
                            if let Some(current) = &mut coordinator {
                                current.clear();
                            }
                        }))
                        .is_err()
                        {
                            coordinator = None;
                        }
                        let _ = acknowledge.send(());
                    }
                    BridgeCommand::Reset(acknowledge) => {
                        // 評価packageの変更では旧network snapshotもここで解放する。
                        coordinator = None;
                        table.clear();
                        let _ = acknowledge.send(());
                    }
                    BridgeCommand::ResizeHash(megabytes, acknowledge) => {
                        table = Arc::new(TranspositionTable::new(megabytes));
                        if catch_unwind(AssertUnwindSafe(|| {
                            if let Some(current) = &mut coordinator {
                                current.set_table(&table);
                            }
                        }))
                        .is_err()
                        {
                            coordinator = None;
                        }
                        let _ = acknowledge.send(());
                    }
                    BridgeCommand::Shutdown => break,
                    #[cfg(test)]
                    BridgeCommand::ThreadIds(acknowledge) => {
                        let ids = coordinator
                            .as_ref()
                            .map(SearchCoordinator::thread_ids)
                            .unwrap_or_default();
                        let _ = acknowledge.send(ids);
                    }
                }
            }
        });
        Self { commands, handle: Some(handle) }
    }

    pub(super) fn start(&self, job: SearchJob) {
        self.commands.send(BridgeCommand::Run(Box::new(job))).expect("search bridge must be alive");
    }

    #[cfg(test)]
    pub(super) fn thread_ids(&self) -> Vec<thread::ThreadId> {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands.send(BridgeCommand::ThreadIds(acknowledge)).unwrap();
        completed.recv().unwrap()
    }

    pub(super) fn clear(&self) {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands.send(BridgeCommand::Clear(acknowledge)).expect("search bridge must be alive");
        completed.recv().expect("search bridge must acknowledge hash clear");
    }

    pub(super) fn reset(&self) {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands.send(BridgeCommand::Reset(acknowledge)).expect("search bridge must be alive");
        completed.recv().expect("search bridge must acknowledge reset");
    }

    pub(super) fn resize_hash(&self, megabytes: usize) {
        let (acknowledge, completed) = mpsc::sync_channel(0);
        self.commands
            .send(BridgeCommand::ResizeHash(megabytes, acknowledge))
            .expect("search bridge must be alive");
        completed.recv().expect("search bridge must acknowledge hash resize");
    }
}

impl Drop for UsiSearchBridge {
    fn drop(&mut self) {
        let _ = self.commands.send(BridgeCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

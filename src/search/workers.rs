//! 常駐helperとworker専用の作業領域。探索方針はmod.rsのdriverが持つ。

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use rsshogi::board::Position;

use crate::clock::Instant;
use crate::eval::Evaluator;
use crate::params::SearchParams;
use crate::position::{MAX_SEARCH_DEPTH, validate_evaluable_position};
use crate::tt::TranspositionTable;

use super::context::{SearchContext, SharedSearch};
use super::{
    HistoryTables, MainWorker, SearchControl, SearchEvent, SearchJob, SearchLimits, SearchResult,
    search_worker, stats, wait_for_helper_start,
};

type Panic = Box<dyn Any + Send + 'static>;

#[derive(Default)]
struct WorkerState {
    context: Option<SearchContext>,
    // 初回だけ使う。テスト用adapterでは既存の履歴を受け渡す。
    history: Option<HistoryTables>,
    #[cfg(test)]
    preparations: Vec<(thread::ThreadId, usize, usize)>,
}

impl WorkerState {
    fn prepare(&mut self, input: &WorkerInput) -> SearchContext {
        let mut context = if let Some(mut context) = self.context.take() {
            context.prepare(
                &input.evaluator,
                input.params,
                input.limits.clone(),
                input.shared.clone(),
            );
            context
        } else {
            SearchContext::new(
                input.evaluator.clone(),
                input.params,
                input.limits.clone(),
                input.shared.clone(),
                self.history.take().unwrap_or_else(HistoryTables::new),
            )
        };
        context.history.age();
        context.history.configure(&input.params);
        #[cfg(test)]
        self.preparations.push((
            thread::current().id(),
            context.ordering.as_ptr() as usize,
            context.killers.as_ptr() as usize,
        ));
        context
    }

    fn clear(&mut self) {
        if let Some(context) = &mut self.context {
            context.history.clear();
        }
        if let Some(history) = &mut self.history {
            history.clear();
        }
    }

    #[cfg(test)]
    fn into_history(self) -> HistoryTables {
        self.context
            .map(|context| context.history)
            .or(self.history)
            .unwrap_or_else(HistoryTables::new)
    }
}

struct WorkerInput {
    position: Position,
    evaluator: Evaluator,
    params: SearchParams,
    limits: SearchLimits,
    shared: SharedSearch,
}

struct HelperJob {
    input: WorkerInput,
    start: Arc<AtomicBool>,
    completed: mpsc::Sender<Result<(), Panic>>,
    #[cfg(test)]
    panic: bool,
}

enum HelperCommand {
    Run(Box<HelperJob>),
    Clear(mpsc::SyncSender<()>),
    SetTable(Arc<TranspositionTable>, mpsc::SyncSender<()>),
    Shutdown,
}

struct PermanentHelper {
    commands: mpsc::SyncSender<HelperCommand>,
    handle: Option<JoinHandle<WorkerState>>,
}

impl PermanentHelper {
    #[cfg(test)]
    fn shutdown(&mut self) -> WorkerState {
        let _ = self.commands.send(HelperCommand::Shutdown);
        self.handle
            .take()
            .expect("helper is joined once")
            .join()
            .expect("idle helper must not panic")
    }
}

impl Drop for PermanentHelper {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let _ = self.commands.send(HelperCommand::Shutdown);
            let _ = self.handle.take().unwrap().join();
        }
    }
}

/// coordinator自身をworker 0とし、helperのOS threadと作業領域を同じ期間保持する。
pub(crate) struct SearchWorkers {
    main: WorkerState,
    helpers: Vec<PermanentHelper>,
}

impl SearchWorkers {
    pub(crate) fn new(threads: usize, control: &SearchControl) -> Self {
        Self::with_histories(threads, Vec::new(), control)
    }

    fn with_histories(
        threads: usize,
        histories: Vec<HistoryTables>,
        control: &SearchControl,
    ) -> Self {
        let count = threads.max(1);
        let mut histories =
            if histories.len() == count { histories } else { Vec::new() }.into_iter();
        let mut workers = Self {
            main: WorkerState { history: histories.next(), ..WorkerState::default() },
            helpers: Vec::new(),
        };
        for worker_id in 1..count {
            let (commands, receiver) = mpsc::sync_channel(1);
            let mut state = WorkerState { history: histories.next(), ..WorkerState::default() };
            let work = move || {
                while let Ok(command) = receiver.recv() {
                    match command {
                        HelperCommand::Run(job) => {
                            let HelperJob {
                                input,
                                start,
                                completed,
                                #[cfg(test)]
                                panic,
                            } = *job;
                            let cancel = Arc::clone(&input.shared.cancel);
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                let context = state.prepare(&input);
                                wait_for_helper_start(&start, &input.shared.cancel);
                                #[cfg(test)]
                                assert!(!panic, "injected helper search panic");
                                let (_, context) =
                                    search_worker(input.position, context, worker_id, None);
                                state.context = Some(context);
                            }));
                            let failed = result.is_err();
                            if failed {
                                cancel.store(true, Ordering::Relaxed);
                            }
                            let _ = completed.send(result);
                            if failed {
                                break;
                            }
                        }
                        HelperCommand::Clear(ack) => {
                            state.clear();
                            let _ = ack.send(());
                        }
                        HelperCommand::SetTable(table, ack) => {
                            if let Some(context) = &mut state.context {
                                context.table = table;
                            }
                            let _ = ack.send(());
                        }
                        HelperCommand::Shutdown => break,
                    }
                }
                state
            };
            #[cfg(test)]
            let spawned = if control.fail_helper_spawn_at == Some(worker_id) {
                Err(std::io::Error::other("injected helper spawn failure"))
            } else {
                thread::Builder::new().spawn(work)
            };
            #[cfg(not(test))]
            let spawned = thread::Builder::new().spawn(work);
            match spawned {
                Ok(handle) => {
                    workers.helpers.push(PermanentHelper { commands, handle: Some(handle) })
                }
                Err(error) => {
                    control.cancel.store(true, Ordering::Relaxed);
                    panic!("failed to spawn search helper {worker_id}: {error}");
                }
            }
        }
        workers
    }

    pub(crate) fn len(&self) -> usize {
        self.helpers.len() + 1
    }

    #[cfg(test)]
    pub(crate) fn thread_ids(&self) -> Vec<thread::ThreadId> {
        std::iter::once(thread::current().id())
            .chain(self.helpers.iter().map(|helper| helper.handle.as_ref().unwrap().thread().id()))
            .collect()
    }

    pub(crate) fn clear(&mut self) {
        self.main.clear();
        for helper in &self.helpers {
            let (ack, done) = mpsc::sync_channel(0);
            helper.commands.send(HelperCommand::Clear(ack)).expect("idle helper must be alive");
            done.recv().expect("helper must acknowledge clear");
        }
    }

    /// idle中のcontextも新TTへ付け替え、旧TTの大きな確保を残さない。
    pub(crate) fn set_table(&mut self, table: &Arc<TranspositionTable>) {
        if let Some(context) = &mut self.main.context {
            context.table = Arc::clone(table);
        }
        for helper in &self.helpers {
            let (ack, done) = mpsc::sync_channel(0);
            helper
                .commands
                .send(HelperCommand::SetTable(Arc::clone(table), ack))
                .expect("idle helper must be alive");
            done.recv().expect("helper must acknowledge table replacement");
        }
    }

    pub(crate) fn run(&mut self, job: SearchJob, table: Arc<TranspositionTable>) {
        let SearchJob { position, evaluator, params, mut limits, control, events, threads } = job;
        assert_eq!(self.len(), threads.max(1));
        let start = Instant::now();
        if validate_evaluable_position(&position).is_err() {
            let _ = events.send(SearchEvent::Done(SearchResult::fail_closed(start.elapsed())));
            return;
        }
        limits.max_depth = limits.max_depth.clamp(1, MAX_SEARCH_DEPTH);
        limits.max_nodes = limits.max_nodes.map(|nodes| nodes.max(1));
        table.new_search();
        let shared = SharedSearch {
            cancel: control.cancel,
            pondering: control.pondering,
            // mainの評価済みfallback用に1 nodeを予約する。
            nodes: Arc::new(AtomicU64::new(1)),
            table,
        };
        // dispatchやmainの準備がpanicしても、待機中のhelperを必ず解除する。
        let _cancel_on_drop = CancelOnDrop(Arc::clone(&shared.cancel));
        let helpers_start = Arc::new(AtomicBool::new(false));
        let mut completions = Vec::with_capacity(self.helpers.len());
        for (index, helper) in self.helpers.iter().enumerate() {
            let (completed, done) = mpsc::channel();
            helper
                .commands
                .send(HelperCommand::Run(Box::new(HelperJob {
                    input: WorkerInput {
                        position: position.clone(),
                        evaluator: evaluator.clone(),
                        params,
                        limits: limits.clone(),
                        shared: shared.clone(),
                    },
                    start: Arc::clone(&helpers_start),
                    completed,
                    #[cfg(test)]
                    panic: control.panic_helper_worker == Some(index + 1),
                })))
                .unwrap_or_else(|_| panic!("idle helper {} must be alive", index + 1));
            completions.push(done);
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            let input = WorkerInput { position, evaluator, params, limits, shared: shared.clone() };
            let context = self.main.prepare(&input);
            #[cfg(test)]
            assert!(!control.panic_after_helpers_spawned, "injected main search worker panic");
            let main = MainWorker { events: &events, helpers_start: &helpers_start };
            let (result, context) = search_worker(input.position, context, 0, Some(main));
            self.main.context = Some(context);
            result
        }));
        shared.cancel.store(true, Ordering::Relaxed);
        let mut helper_panic = None;
        for done in completions {
            if let Err(payload) = done.recv().expect("helper must report completion") {
                helper_panic.get_or_insert(payload);
            }
        }
        match result {
            Err(payload) => resume_unwind(payload),
            Ok(_) if let Some(payload) = helper_panic => resume_unwind(payload),
            Ok(mut result) => {
                result.nodes = shared.nodes.load(Ordering::Relaxed);
                result.hashfull = shared.table.hashfull();
                stats::dump();
                let _ = events.send(SearchEvent::Done(result));
            }
        }
    }

    #[cfg(test)]
    fn into_histories(mut self) -> Vec<HistoryTables> {
        let mut histories = vec![std::mem::take(&mut self.main).into_history()];
        for helper in &mut self.helpers {
            histories.push(helper.shutdown().into_history());
        }
        histories
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// 単発のテスト用adapter。USIはSearchWorkersを保持する。
#[cfg(test)]
pub(super) fn run(
    job: SearchJob,
    table: Arc<TranspositionTable>,
    histories: &mut Vec<HistoryTables>,
) {
    if validate_evaluable_position(&job.position).is_err() {
        let _ = job.events.send(SearchEvent::Done(SearchResult::fail_closed(Default::default())));
        return;
    }
    let mut workers =
        SearchWorkers::with_histories(job.threads, std::mem::take(histories), &job.control);
    workers.run(job, table);
    *histories = workers.into_histories();
}

#[cfg(test)]
mod tests {
    use rsshogi::board;
    use rsshogi::types::Bitboard;

    use super::super::test_support::{depth_limits, idle_control, material_job};
    use super::*;

    fn run_one(workers: &mut SearchWorkers, table: &Arc<TranspositionTable>) -> SearchResult {
        let (job, events) = material_job(
            board::hirate_position(),
            SearchLimits { max_nodes: Some(1), ..depth_limits(3) },
            idle_control(),
            workers.len(),
        );
        workers.run(job, Arc::clone(table));
        let results = events
            .into_iter()
            .filter_map(|event| match event {
                SearchEvent::Done(result) => Some(result),
                SearchEvent::Info(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        results.into_iter().next().unwrap()
    }

    #[test]
    fn permanent_workers_reuse_threads_buffers_and_age_their_own_histories() {
        let position = board::hirate_position();
        let mv = super::super::legal_moves(&position)[0];
        let mut histories = Vec::new();
        for _ in 0..3 {
            let mut history = HistoryTables::new();
            history.record_quiet_cutoff(position.turn(), mv, &[], 8, Bitboard::EMPTY);
            histories.push(history);
        }
        let before = histories[0].quiet_score(position.turn(), mv, Bitboard::EMPTY);
        let mut workers = SearchWorkers::with_histories(3, histories, &idle_control());
        let ids = workers
            .helpers
            .iter()
            .map(|h| h.handle.as_ref().unwrap().thread().id())
            .collect::<Vec<_>>();
        let table = Arc::new(TranspositionTable::new(1));
        for _ in 0..2 {
            let result = run_one(&mut workers, &table);
            assert_eq!(result.nodes, 1);
            assert!(position.is_legal_move32(result.best_move.unwrap()));
        }
        let mut states = vec![std::mem::take(&mut workers.main)];
        for helper in &mut workers.helpers {
            states.push(helper.shutdown());
        }
        for (index, state) in states.into_iter().enumerate() {
            assert_eq!(state.preparations.len(), 2);
            assert_eq!(
                state.preparations[0], state.preparations[1],
                "thread and allocations persist"
            );
            let expected = if index == 0 { thread::current().id() } else { ids[index - 1] };
            assert_eq!(state.preparations[0].0, expected, "preparation runs on the owning worker");
            assert_eq!(
                state.into_history().quiet_score(position.turn(), mv, Bitboard::EMPTY),
                before / 4
            );
        }
    }

    #[test]
    fn maintenance_keeps_threads_and_releases_the_old_table() {
        let mut workers = SearchWorkers::new(2, &idle_control());
        let table = Arc::new(TranspositionTable::new(1));
        run_one(&mut workers, &table);
        let old_table = Arc::downgrade(&table);
        drop(table);
        let next = Arc::new(TranspositionTable::new(2));
        let id = workers.helpers[0].handle.as_ref().unwrap().thread().id();
        workers.set_table(&next);
        assert!(old_table.upgrade().is_none(), "idle contexts must release the old TT");

        let position = board::hirate_position();
        let mv = super::super::legal_moves(&position)[0];
        workers.main.context.as_mut().unwrap().history.record_quiet_cutoff(
            position.turn(),
            mv,
            &[],
            8,
            Bitboard::EMPTY,
        );
        workers.clear();
        assert_eq!(
            workers.main.context.as_ref().unwrap().history.quiet_score(
                position.turn(),
                mv,
                Bitboard::EMPTY
            ),
            0
        );
        run_one(&mut workers, &next);
        assert_eq!(workers.helpers[0].handle.as_ref().unwrap().thread().id(), id);
    }
}

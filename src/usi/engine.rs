//! USI commandを解釈し、探索の開始・停止と結果の公開を司る。

use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rsshogi::board::{self, Position};
use rsshogi::types::EnteringKingRule;
use rsshogi_usi::{CheckmateResponse, GoParams, UsiCommand, UsiOption, parse_line};

use crate::eval::EvalParams;
use crate::eval::Evaluator;
use crate::nnue::{DEFAULT_FV_SCALE, MobileNetwork};
use crate::params::SearchParams;
#[cfg(feature = "tuning")]
use crate::params::{FV_SCALE_RANGE, tunable_manifest};
use crate::position::{MAX_ROOT_GAME_PLY, replay};
use crate::search::{SearchControl, SearchDeadline, SearchEvent, SearchJob, SearchResult};

#[cfg(feature = "tuning")]
use super::output::write_raw;
use super::output::{write_bestmove, write_command, write_search_info};
use super::time::{DEFAULT_MOVE_OVERHEAD_MS, MAX_MOVE_OVERHEAD_MS, limits_from_go};
use super::worker::SearchWorkerRuntime;
use super::{
    DEFAULT_ENTERING_KING_RULE, DEFAULT_EVAL_FILE, DEFAULT_HASH_MB, DEFAULT_THREADS, MAX_HASH_MB,
    MAX_THREADS,
};

pub(super) struct Engine {
    pub(super) position: Position,
    pub(super) eval_params: EvalParams,
    pub(super) search_params: SearchParams,
    pub(super) eval_file: String,
    pub(super) fv_scale: i32,
    pub(super) threads: usize,
    pub(super) entering_king_rule: EnteringKingRule,
    pub(super) max_moves_to_draw: u32,
    pub(super) move_overhead_ms: u64,
    pub(super) network: Option<Arc<MobileNetwork>>,
    pub(super) search: Option<SearchTask>,
    pub(super) worker: SearchWorkerRuntime,
    pub(super) quit: bool,
    #[cfg(test)]
    pub(super) material_test_mode: bool,
    #[cfg(test)]
    pub(super) panic_next_search: bool,
}

impl Engine {
    pub(super) fn new() -> Self {
        Self {
            position: position_with_rule(board::hirate_position(), DEFAULT_ENTERING_KING_RULE),
            eval_params: EvalParams::default(),
            search_params: SearchParams::default(),
            eval_file: DEFAULT_EVAL_FILE.to_owned(),
            fv_scale: DEFAULT_FV_SCALE,
            threads: DEFAULT_THREADS,
            entering_king_rule: DEFAULT_ENTERING_KING_RULE,
            max_moves_to_draw: 0,
            move_overhead_ms: DEFAULT_MOVE_OVERHEAD_MS,
            network: None,
            search: None,
            worker: SearchWorkerRuntime::new(),
            quit: false,
            #[cfg(test)]
            material_test_mode: true,
            #[cfg(test)]
            panic_next_search: false,
        }
    }

    pub(super) fn handle_line<W: Write>(&mut self, line: &str, writer: &mut W) -> io::Result<()> {
        if line.trim() == "usi_tunables" {
            #[cfg(feature = "tuning")]
            {
                write_raw(
                    writer,
                    &format!(
                        "info string shogiarena_tunables_json {}",
                        tunable_manifest(DEFAULT_FV_SCALE)
                    ),
                )?;
                write_raw(writer, "usi_tunablesok")?;
            }
            return Ok(());
        }

        let command = match parse_line(line) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("USI parse error: {error}");
                return Ok(());
            }
        };
        self.handle_command(command, writer)
    }

    fn handle_command<W: Write>(&mut self, command: UsiCommand, writer: &mut W) -> io::Result<()> {
        match command {
            UsiCommand::Usi => {
                write_command(writer, &UsiCommand::id_name("rsshogi-nnue-mini"))?;
                write_command(writer, &UsiCommand::id_author("rsshogi contributors"))?;
                for option in usi_options() {
                    write_command(writer, &UsiCommand::Option(option))?;
                }
                write_command(writer, &UsiCommand::usiok())?;
            }
            UsiCommand::SetOption { name, value } => {
                self.handle_setoption(&name, value.as_deref(), writer)?;
            }
            UsiCommand::IsReady => {
                if self.prepare_evaluator(writer)? {
                    write_command(writer, &UsiCommand::readyok())?;
                }
            }
            UsiCommand::UsiNewGame => {
                self.stop_search(false, writer)?;
                self.worker.clear();
                self.position =
                    position_with_rule(board::hirate_position(), self.entering_king_rule);
            }
            UsiCommand::Position { spec, moves } => {
                self.stop_search(false, writer)?;
                match replay(&spec, &moves) {
                    Ok(position) => {
                        self.position = position_with_rule(position, self.entering_king_rule);
                    }
                    Err(error) => write_command(
                        writer,
                        &UsiCommand::info_string(format!("position error: {error}")),
                    )?,
                }
            }
            UsiCommand::Go(params) => {
                self.stop_search(false, writer)?;
                if params.mate.is_some() {
                    write_command(
                        writer,
                        &UsiCommand::Checkmate(CheckmateResponse::NotImplemented),
                    )?;
                } else if !self.material_test_mode() && self.network.is_none() {
                    write_command(
                        writer,
                        &UsiCommand::info_string("NNUE is not loaded; run isready"),
                    )?;
                    write_bestmove(writer, &SearchResult::fail_closed(Duration::ZERO))?;
                } else {
                    self.start_search(params);
                }
            }
            UsiCommand::Stop => self.stop_search(true, writer)?,
            UsiCommand::PonderHit => self.handle_ponderhit(writer)?,
            UsiCommand::Extension { name, .. } if name == "ponderhit" => {
                self.handle_ponderhit(writer)?;
            }
            UsiCommand::GameOver(_) => self.stop_search(false, writer)?,
            UsiCommand::Quit => {
                self.stop_search(false, writer)?;
                self.quit = true;
            }
            UsiCommand::Extension { .. }
            | UsiCommand::Id { .. }
            | UsiCommand::Option(_)
            | UsiCommand::UsiOk
            | UsiCommand::ReadyOk
            | UsiCommand::BestMove(_)
            | UsiCommand::Info(_)
            | UsiCommand::Checkmate(_) => {}
        }
        Ok(())
    }

    fn handle_setoption<W: Write>(
        &mut self,
        name: &str,
        value: Option<&str>,
        writer: &mut W,
    ) -> io::Result<()> {
        match name {
            "Clear Hash" => {
                self.stop_search(false, writer)?;
                self.worker.clear();
            }
            // `go ponder` itself is the runtime authority. This standard
            // option only lets a GUI announce whether it will use that path.
            "USI_Ponder" => match value {
                Some("true" | "false") => {}
                _ => write_command(
                    writer,
                    &UsiCommand::info_string("USI_Ponder requires true or false"),
                )?,
            },
            "EvalPackage" => match value.filter(|path| !path.is_empty()) {
                Some(path) => {
                    self.stop_search(false, writer)?;
                    self.eval_file = path.to_owned();
                    self.network = None;
                    self.worker.clear();
                }
                None => {
                    write_command(writer, &UsiCommand::info_string("EvalPackage requires a path"))?
                }
            },
            #[cfg(feature = "tuning")]
            "FV_SCALE" => match parse_in_range(value, FV_SCALE_RANGE.0..=FV_SCALE_RANGE.1) {
                Some(scale) => {
                    self.stop_search(false, writer)?;
                    self.fv_scale = scale;
                    self.worker.clear();
                }
                _ => write_command(
                    writer,
                    &UsiCommand::info_string("FV_SCALE must be an integer in 1..=128"),
                )?,
            },
            "USI_Hash" => match parse_in_range(value, 1..=MAX_HASH_MB) {
                Some(megabytes) => {
                    self.stop_search(false, writer)?;
                    self.worker.resize_hash(megabytes);
                }
                _ => write_command(
                    writer,
                    &UsiCommand::info_string(format!(
                        "USI_Hash must be an integer in 1..={MAX_HASH_MB}"
                    )),
                )?,
            },
            "Threads" => match parse_in_range(value, 1..=MAX_THREADS) {
                Some(threads) => {
                    self.stop_search(false, writer)?;
                    self.threads = threads;
                }
                _ => write_command(
                    writer,
                    &UsiCommand::info_string(format!(
                        "Threads must be an integer in 1..={MAX_THREADS}"
                    )),
                )?,
            },
            // 進行中の探索の締切は`go`の時点で確定しているため、ここでcancelしない。
            "MoveOverhead" => match parse_in_range(value, 0..=MAX_MOVE_OVERHEAD_MS) {
                Some(overhead) => {
                    self.move_overhead_ms = overhead;
                }
                _ => write_command(
                    writer,
                    &UsiCommand::info_string(format!(
                        "MoveOverhead must be an integer in 0..={MAX_MOVE_OVERHEAD_MS}"
                    )),
                )?,
            },
            "EnteringKingRule" => match value.and_then(parse_entering_king_rule) {
                Some(rule) => {
                    self.stop_search(false, writer)?;
                    self.entering_king_rule = rule;
                    self.position.set_entering_king_rule(rule);
                    self.worker.clear();
                }
                None => {
                    write_command(writer, &UsiCommand::info_string("unsupported EnteringKingRule"))?
                }
            },
            "MaxMovesToDraw" => match parse_in_range(value, 0..=MAX_ROOT_GAME_PLY) {
                Some(max_moves) => {
                    self.stop_search(false, writer)?;
                    self.max_moves_to_draw = max_moves;
                    self.worker.clear();
                }
                _ => write_command(
                    writer,
                    &UsiCommand::info_string(format!(
                        "MaxMovesToDraw must be an integer in 0..={MAX_ROOT_GAME_PLY}"
                    )),
                )?,
            },
            _ => {
                self.stop_search(false, writer)?;
                #[cfg(feature = "tuning")]
                match self.search_params.set_option(name, value) {
                    Ok(true) => self.worker.clear(),
                    Ok(false) => {}
                    Err(error) => write_command(writer, &UsiCommand::info_string(error))?,
                }
            }
        }
        Ok(())
    }

    fn start_search(&mut self, params: GoParams) {
        let mut limits =
            limits_from_go(&self.position, &params, self.move_overhead_ms, &self.search_params);
        limits.max_moves_to_draw = self.max_moves_to_draw;
        let ponder = params.ponder;
        let position = self.position.clone();
        let evaluator = match self.network.as_ref() {
            Some(network) => Evaluator::nnue(self.eval_params, Arc::clone(network), self.fv_scale),
            None => {
                debug_assert!(self.material_test_mode());
                Evaluator::material(self.eval_params)
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let pondering = Arc::new(AtomicBool::new(params.ponder));
        let (events, receiver) = mpsc::channel();
        let deadline = limits.deadline.clone();
        let control = SearchControl::new(Arc::clone(&cancel), Arc::clone(&pondering));
        #[cfg(test)]
        let control =
            control.with_panic_after_helpers_spawned(std::mem::take(&mut self.panic_next_search));
        self.worker.start(SearchJob {
            position,
            evaluator,
            params: self.search_params,
            limits,
            control,
            events,
            threads: self.threads,
        });
        self.search = Some(SearchTask {
            cancel,
            pondering,
            receiver,
            defer_bestmove: ponder || params.infinite,
            completed: None,
            deadline,
        });
    }

    fn prepare_evaluator<W: Write>(&mut self, writer: &mut W) -> io::Result<bool> {
        if self.material_test_mode() {
            self.network = None;
            return Ok(true);
        }
        if self.network.is_some() {
            return Ok(true);
        }

        #[cfg(feature = "embedded-rsnn")]
        let loaded = if self.eval_file == "@default" {
            MobileNetwork::load_embedded()
        } else {
            MobileNetwork::load(std::path::Path::new(&self.eval_file), None)
        };
        #[cfg(not(feature = "embedded-rsnn"))]
        let loaded = MobileNetwork::load(std::path::Path::new(&self.eval_file), None);

        match loaded {
            Ok(network) => {
                write_command(
                    writer,
                    &UsiCommand::info_string(format!(
                        "loaded mobile .rsnn: {} digest={}",
                        self.eval_file,
                        network.digest()
                    )),
                )?;
                self.network = Some(Arc::new(network));
                Ok(true)
            }
            Err(error) => {
                write_command(
                    writer,
                    &UsiCommand::info_string(format!("NNUE load error: {error}")),
                )?;
                self.network = None;
                Ok(false)
            }
        }
    }

    fn material_test_mode(&self) -> bool {
        #[cfg(test)]
        {
            self.material_test_mode
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    pub(super) fn poll_search<W: Write>(&mut self, writer: &mut W) -> io::Result<()> {
        loop {
            let event = match self.search.as_ref() {
                None => return Ok(()),
                Some(task) if task.completed.is_some() => return Ok(()),
                Some(task) => task.receiver.try_recv(),
            };

            match event {
                Ok(SearchEvent::Info(info)) => write_search_info(writer, &info)?,
                Ok(SearchEvent::Done(result)) => return self.finish_search(result, writer),
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    write_command(
                        writer,
                        &UsiCommand::info_string("search worker ended without a result"),
                    )?;
                    return self.finish_search(SearchResult::fail_closed(Duration::ZERO), writer);
                }
            }
        }
    }

    /// 完了した結果を、ponder中なら保留し、そうでなければbestmoveとして公開する。
    fn finish_search<W: Write>(&mut self, result: SearchResult, writer: &mut W) -> io::Result<()> {
        let defer_bestmove = self.search.as_ref().is_some_and(|task| task.defer_bestmove);
        if defer_bestmove {
            if let Some(task) = self.search.as_mut() {
                task.completed = Some(result);
            }
        } else if self.search.take().is_some() {
            write_bestmove(writer, &result)?;
        }
        Ok(())
    }

    pub(super) fn stop_search<W: Write>(
        &mut self,
        publish: bool,
        writer: &mut W,
    ) -> io::Result<()> {
        let Some(mut task) = self.search.take() else {
            return Ok(());
        };
        task.cancel.store(true, Ordering::Relaxed);

        let mut result = task.completed.take();
        while result.is_none() {
            match task.receiver.recv() {
                Ok(SearchEvent::Info(info)) if publish => write_search_info(writer, &info)?,
                Ok(SearchEvent::Info(_)) => {}
                Ok(SearchEvent::Done(done)) => result = Some(done),
                Err(_) if publish => {
                    write_command(
                        writer,
                        &UsiCommand::info_string("search worker ended without a result"),
                    )?;
                    result = Some(SearchResult::fail_closed(Duration::ZERO));
                }
                Err(_) => return Ok(()),
            }
        }
        if publish {
            match result {
                Some(result) => write_bestmove(writer, &result)?,
                None => write_bestmove(writer, &SearchResult::fail_closed(Duration::ZERO))?,
            }
        }
        Ok(())
    }

    fn handle_ponderhit<W: Write>(&mut self, writer: &mut W) -> io::Result<()> {
        let Some(task) = self.search.as_mut() else {
            return Ok(());
        };
        if !task.pondering.load(Ordering::Acquire) {
            return Ok(());
        }
        // ponder中は相手の手番なので、自分の予算はここから数え始める。
        // `go ponder`の時点で締切を固定すると、相手が長考しただけで思考時間を失う。
        if let Some(deadline) = task.deadline.as_ref() {
            deadline.restart();
        }
        task.pondering.store(false, Ordering::Release);
        task.defer_bestmove = false;

        let completed = task.completed.take();
        if let Some(result) = completed {
            self.search.take();
            write_bestmove(writer, &result)?;
        }
        Ok(())
    }
}

/// `usi`へ返すoption。この順にGUIへ表示される。
fn usi_options() -> Vec<UsiOption> {
    let mut options = Vec::new();
    #[cfg(feature = "tuning")]
    options.extend(SearchParams::usi_options());
    options.push(UsiOption::check("USI_Ponder", false));
    options.push(UsiOption::string("EvalPackage", DEFAULT_EVAL_FILE));
    #[cfg(feature = "tuning")]
    options.push(UsiOption::spin(
        "FV_SCALE",
        i64::from(DEFAULT_FV_SCALE),
        i64::from(FV_SCALE_RANGE.0),
        i64::from(FV_SCALE_RANGE.1),
    ));
    // 以下2つのoption名と値はやねうら王互換。GUIや運用scriptの設定を
    // 他engineと共通のまま使えるようにするため、独自名へは変えない。
    let default_rule = ENTERING_KING_RULES
        .iter()
        .find(|(_, rule)| *rule == DEFAULT_ENTERING_KING_RULE)
        .map(|(name, _)| *name)
        .expect("the default entering-king rule has a USI name");
    options.push(UsiOption::combo(
        "EnteringKingRule",
        default_rule,
        ENTERING_KING_RULES.map(|(name, _)| name),
    ));
    options.push(UsiOption::spin("MaxMovesToDraw", 0, 0, i64::from(MAX_ROOT_GAME_PLY)));
    options.push(UsiOption::spin("USI_Hash", DEFAULT_HASH_MB as i64, 1, MAX_HASH_MB as i64));
    options.push(UsiOption::spin("Threads", DEFAULT_THREADS as i64, 1, MAX_THREADS as i64));
    options.push(UsiOption::spin(
        "MoveOverhead",
        DEFAULT_MOVE_OVERHEAD_MS as i64,
        0,
        MAX_MOVE_OVERHEAD_MS as i64,
    ));
    #[cfg(feature = "tuning")]
    options.push(UsiOption::button("Clear Hash"));
    options
}

/// `setoption`の値を整数として読み、範囲外なら`None`を返す。
fn parse_in_range<T: FromStr + PartialOrd>(
    value: Option<&str>,
    range: RangeInclusive<T>,
) -> Option<T> {
    value.and_then(|text| text.parse().ok()).filter(|parsed| range.contains(parsed))
}

/// USIの入玉規則名と規則の対応。
const ENTERING_KING_RULES: [(&str, EnteringKingRule); 5] = [
    ("NoEnteringKing", EnteringKingRule::None),
    ("CSARule24", EnteringKingRule::Point24),
    ("CSARule24H", EnteringKingRule::Point24Handicap),
    ("CSARule27", EnteringKingRule::Point27),
    ("CSARule27H", EnteringKingRule::Point27Handicap),
];

fn parse_entering_king_rule(value: &str) -> Option<EnteringKingRule> {
    ENTERING_KING_RULES.iter().find(|(name, _)| *name == value).map(|(_, rule)| *rule)
}

fn position_with_rule(mut position: Position, rule: EnteringKingRule) -> Position {
    position.set_entering_king_rule(rule);
    position
}

pub(super) struct SearchTask {
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) pondering: Arc<AtomicBool>,
    pub(super) receiver: mpsc::Receiver<SearchEvent>,
    pub(super) defer_bestmove: bool,
    pub(super) completed: Option<SearchResult>,
    /// `ponderhit`で予算を計り直すために、workerと共有している締切。
    pub(super) deadline: Option<Arc<SearchDeadline>>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let Some(task) = self.search.take() else {
            return;
        };
        task.cancel.store(true, Ordering::Relaxed);
        while let Ok(event) = task.receiver.recv() {
            if matches!(event, SearchEvent::Done(_)) {
                break;
            }
        }
    }
}

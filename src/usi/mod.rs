//! USIセッションの入口。標準入力の読み取りとpollループを持つ。

mod engine;
mod input;
mod output;
mod time;
mod worker;

#[cfg(test)]
mod tests;

use std::io;
use std::sync::mpsc;
use std::time::Duration;

use rsshogi::board;
use rsshogi::types::EnteringKingRule;

use engine::Engine;
use input::{InputEvent, spawn_stdin_reader};

const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const DEFAULT_HASH_MB: usize = 16;
const DEFAULT_THREADS: usize = 1;
const MAX_THREADS: usize = 16;
const DEFAULT_ENTERING_KING_RULE: EnteringKingRule = EnteringKingRule::Point27;
const DEFAULT_EVAL_FILE: &str = "eval/nn.bin";

pub fn run() -> io::Result<()> {
    board::init();
    let input = spawn_stdin_reader();
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut engine = Engine::new();

    while !engine.quit {
        match input.recv_timeout(INPUT_POLL_INTERVAL) {
            Ok(InputEvent::Line(line)) => engine.handle_line(&line, &mut writer)?,
            Ok(InputEvent::Eof) => {
                engine.stop_search(false, &mut writer)?;
                break;
            }
            Ok(InputEvent::Error(error)) => {
                eprintln!("stdin error: {error}");
                engine.stop_search(false, &mut writer)?;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                engine.stop_search(false, &mut writer)?;
                break;
            }
        }
        engine.poll_search(&mut writer)?;
    }

    Ok(())
}

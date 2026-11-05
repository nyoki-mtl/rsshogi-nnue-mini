//! USIセッションの入口。標準入力の読み取りとpollループを持つ。

mod bridge;
mod go_time;
mod input;
mod output;
mod session;

#[cfg(test)]
mod tests;

use std::io;
use std::sync::mpsc;
use std::time::Duration;

use rsshogi::board;
use rsshogi::types::EnteringKingRule;

use input::{InputEvent, spawn_stdin_reader};
use session::UsiSession;

const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const DEFAULT_HASH_MB: usize = 16;
const MAX_HASH_MB: usize = 1_024;
const DEFAULT_THREADS: usize = 1;
const MAX_THREADS: usize = 16;
const DEFAULT_ENTERING_KING_RULE: EnteringKingRule = EnteringKingRule::Point27;
#[cfg(feature = "embedded-rsnn")]
const DEFAULT_EVAL_FILE: &str = "@default";
#[cfg(not(feature = "embedded-rsnn"))]
const DEFAULT_EVAL_FILE: &str = "eval/model.rsnn";

pub fn run() -> io::Result<()> {
    board::init();
    let input = spawn_stdin_reader();
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    let mut session = UsiSession::new();

    while !session.quit {
        match input.recv_timeout(INPUT_POLL_INTERVAL) {
            Ok(InputEvent::Line(line)) => session.handle_line(&line, &mut writer)?,
            Ok(InputEvent::Eof) => {
                session.stop_search(false, &mut writer)?;
                break;
            }
            Ok(InputEvent::Error(error)) => {
                eprintln!("stdin error: {error}");
                session.stop_search(false, &mut writer)?;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                session.stop_search(false, &mut writer)?;
                break;
            }
        }
        session.poll_search(&mut writer)?;
    }

    Ok(())
}

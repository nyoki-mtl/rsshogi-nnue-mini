//! `UsiSession`を介したUSIプロトコル・探索制御・worker寿命のテスト群。

mod lifecycle;
mod options;
mod search;

use std::thread;
use std::time::{Duration, Instant};

use super::session::UsiSession;

pub(super) fn wait_for_idle(session: &mut UsiSession, writer: &mut Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while session.search.is_some() {
        session.poll_search(writer).expect("poll should succeed");
        assert!(Instant::now() < deadline, "search did not become idle");
        thread::yield_now();
    }
}

pub(super) fn wait_for_completed_ponder(session: &mut UsiSession, writer: &mut Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while session.search.as_ref().is_none_or(|task| task.completed.is_none()) {
        session.poll_search(writer).expect("poll should succeed");
        assert!(Instant::now() < deadline, "ponder did not complete");
        thread::yield_now();
    }
}

pub(super) fn output(writer: &[u8]) -> &str {
    std::str::from_utf8(writer).expect("USI output must be UTF-8")
}

pub(super) fn bestmove_count(writer: &[u8]) -> usize {
    output(writer).lines().filter(|line| line.starts_with("bestmove ")).count()
}

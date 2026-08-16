//! 標準入力を専用threadで読み、行単位のeventへ変換する。

use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;

pub(super) enum InputEvent {
    Line(String),
    Eof,
    Error(String),
}

pub(super) fn spawn_stdin_reader() -> mpsc::Receiver<InputEvent> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if sender.send(InputEvent::Line(line)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(InputEvent::Error(error.to_string()));
                    return;
                }
            }
        }
        let _ = sender.send(InputEvent::Eof);
    });
    receiver
}

//! NNUE対応の小さなUSI将棋エンジン。
//!
//! USIのentry pointとホスト組み込み用の同期APIを公開する。
//! 内部moduleはcrate内から所有者のpathで参照する。

#![warn(unreachable_pub)]

pub mod api;
mod eval;
#[cfg(feature = "c-api")]
#[allow(
    unreachable_pub,
    reason = "C ABI symbols are exported via #[unsafe(no_mangle)] from a private Rust module"
)]
mod ffi;
mod position;
mod search;
pub mod usi;

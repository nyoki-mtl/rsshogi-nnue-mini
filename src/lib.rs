//! NNUE対応の小さなUSI将棋エンジン。
//!
//! USIのentry point、同期組み込みAPIと`.rsnn` package検証器を公開する。
//! 内部moduleはcrate内から所有者のpathで参照する。

pub use nnue::RsnnPackage;

pub mod api;
mod clock;
mod eval;
#[cfg(feature = "c-api")]
mod ffi;
mod nnue;
mod params;
mod position;
mod search;
mod see;
mod tt;
pub mod usi;

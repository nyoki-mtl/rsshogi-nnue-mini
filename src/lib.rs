//! NNUE対応の小さなUSI将棋エンジン。
//!
//! USIのentry pointと`.rsnn` package検証器を公開し、内部moduleはcrate内から
//! 所有者のpathで参照する。

pub use nnue::RsnnPackage;

mod eval;
mod nnue;
mod params;
mod position;
mod search;
mod see;
mod tt;
pub mod usi;

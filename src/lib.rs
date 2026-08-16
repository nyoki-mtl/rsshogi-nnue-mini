//! NNUE対応の小さなUSI将棋エンジン。
//!
//! 外部へ公開するのはUSIのentry pointだけで、内部moduleはcrate内から
//! 所有者のpathで参照する。

mod eval;
mod nnue;
mod params;
mod position;
mod search;
mod see;
mod tt;
pub mod usi;

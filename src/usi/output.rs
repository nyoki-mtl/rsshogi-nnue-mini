//! USI応答の書き出しと、値の変換helperを置く。

use std::io::{self, Write};
use std::time::Duration;

use rsshogi::types::MOVE_WIN;
use rsshogi_usi::{BestMove, InfoCommand, MateScore, UsiCommand, format_command};

use crate::search::{self, SearchIteration, SearchResult};

pub(super) fn write_search_info<W: Write>(
    writer: &mut W,
    info: &SearchIteration,
) -> io::Result<()> {
    write_info(writer, info.depth, info.score, info.nodes, info.elapsed, &info.pv)
}

pub(super) fn write_info<W: Write>(
    writer: &mut W,
    depth: u32,
    score: i32,
    nodes: u64,
    elapsed: Duration,
    pv: &[rsshogi::types::Move32],
) -> io::Result<()> {
    let elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX);
    let nps = nodes.saturating_mul(1_000) / elapsed_ms.max(1);
    let mut info =
        InfoCommand::new().with_depth(depth).with_nodes(nodes).with_time(elapsed_ms).with_nps(nps);
    // 距離0は整数の符号を保持できないため、勝敗だけを表すUSIの`+`/`-`へ変換する。
    info = match search::mate_distance(score) {
        Some(0) if score.is_negative() => info.with_score_mate(MateScore::unknown_lose()),
        Some(0) => info.with_score_mate(MateScore::unknown_win()),
        Some(distance) => info.with_score_mate(MateScore::ply(distance)),
        None => info.with_score_cp(score),
    };
    if !pv.is_empty() {
        info = info.with_pv(pv.iter().map(|mv| mv.to_usi()));
    }
    let command = UsiCommand::info(info);
    write_command(writer, &command)
}

pub(super) fn write_bestmove<W: Write>(writer: &mut W, result: &SearchResult) -> io::Result<()> {
    write_info(writer, result.depth, result.score, result.nodes, result.elapsed, &result.pv)?;
    let bestmove = match result.best_move {
        Some(mv) if mv == MOVE_WIN => BestMove::win(),
        Some(mv) => BestMove::move_to(mv.to_usi())
            .with_optional_ponder(result.ponder_move.map(|ponder| ponder.to_usi())),
        None => BestMove::resign(),
    };
    write_command(writer, &UsiCommand::bestmove(bestmove))
}

pub(super) fn write_command<W: Write>(writer: &mut W, command: &UsiCommand) -> io::Result<()> {
    write_raw(writer, &format_command(command))
}

pub(super) fn write_raw<W: Write>(writer: &mut W, line: &str) -> io::Result<()> {
    writeln!(writer, "{line}")?;
    writer.flush()
}

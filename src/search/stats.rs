//! 探索木の統計(`stats` feature)。既定のbuildでは何も数えず、呼び出しは空になる。
//!
//! 全workerで共有する累積カウンタで、探索が終わるたびに標準エラーへ出す。
//! 並べ替えの質、TTの効き、枝刈りと延長の発火率を、候補の比較の前に把握するために使う。

#[derive(Clone, Copy)]
pub(super) enum Stat {
    MainNodes,
    QsearchNodes,
    TtHit,
    TtCut,
    BetaCut,
    BetaCutFirst,
    BetaCutIndexSum,
    StoreExact,
    StoreLower,
    StoreUpper,
    FailLowNodes,
    LmrReduced,
    LmrResearch,
    LmrResearchStillHigh,
    SingularTry,
    SingularExtend,
    SingularMultiCut,
    NullTry,
    NullCut,
    ProbCutTry,
    ProbCutCut,
    RfpCut,
    RootIterations,
    RootBestChanged,
    RootBestFromLate,
    AspirationFail,
    CutNodesWithTt,
    CutByTt,
    CutByKiller,
    CutByCapture,
    CutByQuiet,
    CutQuietIndexSum,
    NodesWithTtMove,
    LoopNodes,
}

#[cfg(feature = "stats")]
const NAMES: [&str; 34] = [
    "main_nodes",
    "qsearch_nodes",
    "tt_hit",
    "tt_cut",
    "beta_cut",
    "beta_cut_first",
    "beta_cut_index_sum",
    "store_exact",
    "store_lower",
    "store_upper",
    "fail_low_nodes",
    "lmr_reduced",
    "lmr_research",
    "lmr_research_still_high",
    "singular_try",
    "singular_extend",
    "singular_multicut",
    "null_try",
    "null_cut",
    "probcut_try",
    "probcut_cut",
    "rfp_cut",
    "root_iterations",
    "root_best_changed",
    "root_best_from_late",
    "aspiration_fail",
    "cut_nodes_with_tt",
    "cut_by_tt",
    "cut_by_killer",
    "cut_by_capture",
    "cut_by_quiet",
    "cut_quiet_index_sum",
    "nodes_with_tt_move",
    "loop_nodes",
];

#[cfg(feature = "stats")]
static COUNTERS: [std::sync::atomic::AtomicU64; 34] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 34];

#[inline]
pub(super) fn add(stat: Stat, amount: u64) {
    #[cfg(feature = "stats")]
    COUNTERS[stat as usize].fetch_add(amount, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(feature = "stats"))]
    let _ = (stat, amount);
}

#[inline]
pub(super) fn bump(stat: Stat) {
    add(stat, 1);
}

/// 累積値を一行で標準エラーへ出す。
pub(super) fn dump() {
    #[cfg(feature = "stats")]
    {
        let line = NAMES
            .iter()
            .zip(COUNTERS.iter())
            .map(|(name, counter)| {
                format!("{name}={}", counter.load(std::sync::atomic::Ordering::Relaxed))
            })
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!("stats {line}");
    }
}

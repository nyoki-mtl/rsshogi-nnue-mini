#[cfg(feature = "tuning")]
use rsshogi_usi::UsiOption;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchParams {
    pub(crate) aspiration_window: i32,
    pub(crate) reverse_futility_margin: i32,
    pub(crate) futility_margin: i32,
    /// null moveの動的縮小で`(static_eval - beta)`を縮小量へ換算する分母。
    pub(crate) null_move_eval_divisor: i32,
    /// 対数LMRの分母。大きいほど縮小が浅くなる。
    pub(crate) lmr_divisor: i32,
    pub(crate) qsearch_delta_margin: i32,
    pub(crate) check_ordering_bonus: i32,
    /// 主探索の捕獲SEE枝刈りで`depth^2`へ掛ける許容損失。
    pub(crate) see_prune_margin: i32,
    /// 静かな手のhistory枝刈りで`depth`へ掛ける負の履歴の閾値。
    pub(crate) history_prune_margin: i32,
    /// 静かな打ち駒のLMP上限を、盤上の静かな手の上限から割り引く除数。
    pub(crate) drop_lmp_divisor: i32,
    /// correction historyが静的評価へ足せる補正の上限。0で補正を無効化する。
    pub(crate) correction_apply_max: i32,
    /// continuation historyの並べ替えへの寄与の重み。256で等倍、0で無効。
    pub(crate) continuation_weight: i32,
    /// 新しいiterationを始めない位置の、予算に対する割合(千分率)。局面の安定度で伸縮する前の基準。
    pub(crate) time_soft_permille: i32,
    /// 最善手が揺れる局面で延長してよいhard stopの、配分に対する倍率(百分率)。
    pub(crate) time_max_ratio_percent: i32,
    /// 減衰させた最善手の変化回数1回あたりにsoft stopへ足す倍率(千分率)。
    pub(crate) time_change_weight_permille: i32,
    /// 評価値がこの幅だけ下がるごとに、soft stopを1倍分伸ばす。
    pub(crate) time_falling_span: i32,
    /// 同じ最善手が続いたiteration数1回あたりにsoft stopから引く倍率(千分率)。
    pub(crate) time_stability_step_permille: i32,
    /// TT moveの無いnodeを1浅く探索する最小のdepth。
    pub(crate) iir_min_depth: i32,
    /// reverse futilityを試す最大のdepth。
    pub(crate) rfp_max_depth: i32,
    /// 静かな手のfutility枝刈りを試す最大のdepth。
    pub(crate) futility_max_depth: i32,
    /// 捕獲のSEE枝刈りを試す最大のdepth。
    pub(crate) see_prune_max_depth: i32,
    /// history枝刈りを試す最大のdepth。
    pub(crate) history_prune_max_depth: i32,
    /// LMRの縮小量を履歴で1段ずらすのに要する履歴の大きさ。
    pub(crate) lmr_history_divisor: i32,
    /// null moveの縮小量の定数部分。
    pub(crate) null_move_base_reduction: i32,
    /// null cutを検証探索で確かめる最小のdepth。
    pub(crate) null_move_verify_depth: i32,
    /// LMPで読む静かな手の数`(base + depth^2) / (2 - improving)`の定数部分。
    pub(crate) lmp_base: i32,
    /// βカットで履歴へ足すbonusのdepth比例部分。
    pub(crate) history_bonus_per_depth: i32,
    /// βカットで履歴へ足すbonusから引く定数。
    pub(crate) history_bonus_offset: i32,
    /// βカットで履歴へ足すbonusの上限。
    pub(crate) history_bonus_max: i32,
    /// ProbCutで浅い探索が超えるべき値の、βへの上乗せ。
    pub(crate) probcut_margin: i32,
    /// singular extensionを試す最小のdepth。
    pub(crate) singular_min_depth: i32,
    /// singularと見なすためにTTの値から引くmarginのdepth比例係数。
    pub(crate) singular_margin: i32,
    /// LMRの縮小量を履歴でずらせる最大の段数。
    pub(crate) lmr_history_clamp: i32,
    /// ProbCutを試す最小のdepth。確認探索はこれより1少ない分だけ浅い。
    pub(crate) probcut_min_depth: i32,
    /// LMRを掛け始める手の添字(0始まり)。PV nodeは1つ後ろから。
    pub(crate) lmr_min_index: i32,
    /// SEEで損な捕獲の縮小を、表の基礎値から減らす段数。
    pub(crate) bad_capture_lmr_offset: i32,
    /// null moveの縮小量でdepthを割る除数。
    pub(crate) null_move_depth_divisor: i32,
    /// improvingのときreverse futilityのmarginを緩めるdepthの段数。
    pub(crate) rfp_improving_depth: i32,
    /// 静止探索で捕獲を刈る交換損の許容幅。SEEが-marginを下回る捕獲を読まない。
    pub(crate) qsearch_see_margin: i32,
    /// 王手になる静かな手の縮小を、静かな非王手から減らす段数。
    pub(crate) check_lmr_offset: i32,
    pub(crate) lmr_tt_pv: i32,
    pub(crate) lmr_cut_node: i32,
    pub(crate) lmr_tt_capture: i32,
    pub(crate) lmr_child_cutoffs: i32,
    pub(crate) lmr_deeper_margin: i32,
    pub(crate) lmr_shallower_margin: i32,
    pub(crate) lmr_research_bonus: i32,
    pub(crate) hindsight_margin: i32,
    /// βカットより先に試して失敗した静かな手へのペナルティの、bonusに対する百分率。
    pub(crate) history_malus_percent: i32,
    /// null moveを試す条件で、静的評価がβをこれだけ上回ることを求める。負なら下回っても試す。
    pub(crate) null_move_eval_margin: i32,
    /// improvingでないnodeの静かな手へ足す縮小量(1/1024段)。
    pub(crate) lmr_not_improving: i32,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            aspiration_window: 88,
            reverse_futility_margin: 132,
            futility_margin: 299,
            null_move_eval_divisor: 296,
            lmr_divisor: 190,
            qsearch_delta_margin: 196,
            check_ordering_bonus: 4175,
            see_prune_margin: 177,
            history_prune_margin: 2148,
            drop_lmp_divisor: 2,
            correction_apply_max: 45,
            continuation_weight: 229,
            time_soft_permille: 600,
            time_max_ratio_percent: 300,
            time_change_weight_permille: 600,
            time_falling_span: 300,
            time_stability_step_permille: 70,
            iir_min_depth: 2,
            rfp_max_depth: 12,
            futility_max_depth: 7,
            see_prune_max_depth: 5,
            history_prune_max_depth: 3,
            lmr_history_divisor: 7199,
            null_move_base_reduction: 4,
            null_move_verify_depth: 16,
            lmp_base: 4,
            history_bonus_per_depth: 169,
            history_bonus_offset: 53,
            history_bonus_max: 1675,
            probcut_margin: 194,
            singular_min_depth: 5,
            singular_margin: 2,
            lmr_history_clamp: 2,
            probcut_min_depth: 4,
            lmr_min_index: 2,
            bad_capture_lmr_offset: 3,
            null_move_depth_divisor: 3,
            rfp_improving_depth: 0,
            qsearch_see_margin: 1,
            check_lmr_offset: 0,
            lmr_tt_pv: 943,
            lmr_cut_node: 1688,
            lmr_tt_capture: 549,
            lmr_child_cutoffs: 807,
            lmr_deeper_margin: 26,
            lmr_shallower_margin: 10,
            lmr_research_bonus: 806,
            hindsight_margin: 162,
            history_malus_percent: 105,
            null_move_eval_margin: 5,
            lmr_not_improving: 945,
        }
    }
}

/// 調整用に公開する探索パラメータ。USI option、`setoption`、SPSA用manifestはこの表から作る。
#[cfg(feature = "tuning")]
struct Tunable {
    option: &'static str,
    /// manifestの`id`。SPSAの設定とrun履歴が参照するので、optionの名前から機械的には作らない。
    id: &'static str,
    field: fn(&mut SearchParams) -> &mut i32,
    min: i32,
    max: i32,
    /// SPSAの摂動幅の最終値。
    c_end: i32,
}

#[cfg(feature = "tuning")]
macro_rules! tunable {
    ($option:literal, $id:literal, $field:ident, $min:literal, $max:literal, $c_end:literal) => {
        Tunable {
            option: $option,
            id: $id,
            field: |params| &mut params.$field,
            min: $min,
            max: $max,
            c_end: $c_end,
        }
    };
}

#[cfg(feature = "tuning")]
#[rustfmt::skip]
const TUNABLES: [Tunable; 51] = [
    //       option, manifest id, field, min, max, c_end
    tunable!("SearchAspirationWindow",      "search_aspiration_window",        aspiration_window,            20,   400,   10),
    tunable!("SearchReverseFutilityMargin", "search_reverse_futility_margin",  reverse_futility_margin,      40,   400,   20),
    tunable!("SearchFutilityMargin",        "search_futility_margin",          futility_margin,              40,   500,   20),
    tunable!("SearchNullMoveEvalDivisor",   "search_null_move_eval_divisor",   null_move_eval_divisor,       50,   800,   20),
    tunable!("SearchLmrDivisor",            "search_lmr_divisor",              lmr_divisor,                  100,  600,   20),
    tunable!("SearchQsearchDeltaMargin",    "search_qsearch_delta_margin",     qsearch_delta_margin,         0,    400,   20),
    tunable!("SearchCheckBonus",            "search_check_bonus",              check_ordering_bonus,         500,  8000,  200),
    tunable!("SearchSeePruneMargin",        "search_see_prune_margin",         see_prune_margin,             10,   400,   15),
    tunable!("SearchHistoryPruneMargin",    "search_history_prune_margin",     history_prune_margin,         200,  8000,  300),
    tunable!("SearchDropLmpDivisor",        "search_drop_lmp_divisor",         drop_lmp_divisor,             0,    6,     1),
    tunable!("SearchCorrectionApplyMax",    "search_correction_apply_max",     correction_apply_max,         0,    256,   10),
    tunable!("SearchContinuationWeight",    "search_continuation_weight",      continuation_weight,          0,    1024,  40),
    tunable!("TimeSoftPermille",            "time_soft_permille",              time_soft_permille,           300,  1000,  40),
    tunable!("TimeMaxRatioPercent",         "time_max_ratio_percent",          time_max_ratio_percent,       100,  600,   25),
    tunable!("TimeChangeWeightPermille",    "time_change_weight_permille",     time_change_weight_permille,  0,    2000,  80),
    tunable!("TimeFallingSpan",             "time_falling_span",               time_falling_span,            50,   1000,  40),
    tunable!("TimeStabilityStepPermille",   "time_stability_step_permille",    time_stability_step_permille, 0,    200,   10),
    tunable!("SearchIirMinDepth",           "search_iir_min_depth",            iir_min_depth,                2,    10,    2),
    tunable!("SearchRfpMaxDepth",           "search_rfp_max_depth",            rfp_max_depth,                3,    16,    2),
    tunable!("SearchFutilityMaxDepth",      "search_futility_max_depth",       futility_max_depth,           1,    10,    2),
    tunable!("SearchSeePruneMaxDepth",      "search_see_prune_max_depth",      see_prune_max_depth,          2,    12,    2),
    tunable!("SearchHistoryPruneMaxDepth",  "search_history_prune_max_depth",  history_prune_max_depth,      1,    10,    2),
    tunable!("SearchLmrHistoryDivisor",     "search_lmr_history_divisor",      lmr_history_divisor,          2000, 20000, 800),
    tunable!("SearchNullMoveBaseReduction", "search_null_move_base_reduction", null_move_base_reduction,     1,    6,     2),
    tunable!("SearchNullMoveVerifyDepth",   "search_null_move_verify_depth",   null_move_verify_depth,       6,    24,    2),
    tunable!("SearchLmpBase",               "search_lmp_base",                 lmp_base,                     1,    12,    2),
    tunable!("SearchHistoryBonusPerDepth",  "search_history_bonus_per_depth",  history_bonus_per_depth,      40,   400,   20),
    tunable!("SearchHistoryBonusOffset",    "search_history_bonus_offset",     history_bonus_offset,         0,    400,   20),
    tunable!("SearchHistoryBonusMax",       "search_history_bonus_max",        history_bonus_max,            400,  6000,  200),
    tunable!("SearchProbcutMargin",         "search_probcut_margin",           probcut_margin,               50,   600,   20),
    tunable!("SearchSingularMinDepth",      "singular_min_depth",              singular_min_depth,           4,    14,    1),
    tunable!("SearchSingularMargin",        "singular_margin",                 singular_margin,              1,    8,     1),
    tunable!("SearchLmrHistoryClamp",       "lmr_history_clamp",               lmr_history_clamp,            0,    4,     1),
    tunable!("SearchProbcutMinDepth",       "probcut_min_depth",               probcut_min_depth,            3,    10,    1),
    tunable!("SearchLmrMinIndex",           "lmr_min_index",                   lmr_min_index,                1,    6,     1),
    tunable!("SearchBadCaptureLmrOffset",   "bad_capture_lmr_offset",          bad_capture_lmr_offset,       0,    3,     1),
    tunable!("SearchNullMoveDepthDivisor",  "null_move_depth_divisor",         null_move_depth_divisor,      2,    8,     1),
    tunable!("SearchRfpImprovingDepth",     "rfp_improving_depth",             rfp_improving_depth,          0,    3,     1),
    tunable!("SearchQsearchSeeMargin",      "qsearch_see_margin",              qsearch_see_margin,           0,    300,   20),
    tunable!("SearchCheckLmrOffset",        "check_lmr_offset",                check_lmr_offset,             0,    3,     1),
    tunable!("SearchLmrTtPv",               "search_lmr_tt_pv",                lmr_tt_pv,                    0,    3072,  128),
    tunable!("SearchLmrCutNode",            "search_lmr_cut_node",             lmr_cut_node,                 0,    4096,  192),
    tunable!("SearchLmrTtCapture",          "search_lmr_tt_capture",           lmr_tt_capture,               0,    2048,  128),
    tunable!("SearchLmrChildCutoffs",       "search_lmr_child_cutoffs",        lmr_child_cutoffs,            0,    3072,  128),
    tunable!("SearchLmrDeeperMargin",       "search_lmr_deeper_margin",        lmr_deeper_margin,            0,    300,   10),
    tunable!("SearchLmrShallowerMargin",    "search_lmr_shallower_margin",     lmr_shallower_margin,         0,    100,   4),
    tunable!("SearchLmrResearchBonus",      "search_lmr_research_bonus",       lmr_research_bonus,           0,    4000,  150),
    tunable!("SearchHindsightMargin",       "search_hindsight_margin",         hindsight_margin,             0,    600,   20),
    tunable!("SearchHistoryMalusPercent",   "search_history_malus_percent",    history_malus_percent,        25,   300,   10),
    tunable!("SearchNullMoveEvalMargin",    "search_null_move_eval_margin",    null_move_eval_margin,        -300, 300,   15),
    tunable!("SearchLmrNotImproving",       "search_lmr_not_improving",        lmr_not_improving,            0,    3072,  128),
];

#[cfg(feature = "tuning")]
impl SearchParams {
    fn default_value(tunable: &Tunable) -> i32 {
        *(tunable.field)(&mut Self::default())
    }

    pub(crate) fn usi_options() -> impl Iterator<Item = UsiOption> {
        TUNABLES.iter().map(|tunable| {
            UsiOption::spin(
                tunable.option,
                i64::from(Self::default_value(tunable)),
                i64::from(tunable.min),
                i64::from(tunable.max),
            )
        })
    }

    /// 表にあるoptionなら値を設定して`Ok(true)`、表に無い名前なら`Ok(false)`を返す。
    pub(crate) fn set_option(&mut self, name: &str, value: Option<&str>) -> Result<bool, String> {
        let Some(tunable) = TUNABLES.iter().find(|tunable| tunable.option == name) else {
            return Ok(false);
        };
        let (min, max) = (tunable.min, tunable.max);
        let text = value.ok_or_else(|| format!("{name} requires a value"))?;
        let parsed = text.parse::<i32>().map_err(|_| format!("invalid {name}: {text}"))?;
        if !(min..=max).contains(&parsed) {
            return Err(format!("{name} must be in {min}..={max}"));
        }
        *(tunable.field)(self) = parsed;
        Ok(true)
    }
}

/// ShogiArenaの`usi_tunables`へ返すmanifest。
#[cfg(feature = "tuning")]
pub(crate) fn tunable_manifest() -> String {
    let entry = |id: &str, option: &str, default: i32, min: i32, max: i32, c_end: i32| {
        format!(
            "{{\"id\":\"{id}\",\"option\":\"{option}\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":{default},\"min\":{min},\"max\":{max},\"schedule\":{{\"c_end\":{c_end},\"r_end\":0.002}}}}"
        )
    };
    let entries = TUNABLES
        .iter()
        .map(|tunable| {
            entry(
                tunable.id,
                tunable.option,
                SearchParams::default_value(tunable),
                tunable.min,
                tunable.max,
                tunable.c_end,
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{{\"schema_version\":\"shogiarena.usi_tunables.v1\",\"tunables\":[{}]}}",
        entries.join(",")
    )
}

#[cfg(all(test, feature = "tuning"))]
mod tests {
    use super::*;

    #[test]
    fn every_search_option_changes_its_backing_value() {
        for tunable in &TUNABLES {
            let default = SearchParams::default_value(tunable);
            assert!((tunable.min..=tunable.max).contains(&default), "{}", tunable.option);
            let value = if default < tunable.max { default + 1 } else { default - 1 };
            let mut params = SearchParams::default();
            assert!(
                params.set_option(tunable.option, Some(&value.to_string())).expect("valid option"),
                "{}",
                tunable.option
            );
            assert_ne!(params, SearchParams::default(), "{}", tunable.option);
        }
    }

    #[test]
    fn out_of_range_and_unknown_options_are_distinguished() {
        let mut params = SearchParams::default();
        assert_eq!(params.set_option("NoSuchOption", Some("1")), Ok(false));
        assert!(params.set_option("SearchLmpBase", Some("0")).is_err());
        assert!(params.set_option("SearchLmpBase", None).is_err());
        assert_eq!(params, SearchParams::default());
    }

    #[test]
    fn option_names_and_manifest_ids_are_unique() {
        for (index, tunable) in TUNABLES.iter().enumerate() {
            assert!(
                TUNABLES[index + 1..]
                    .iter()
                    .all(|other| other.option != tunable.option && other.id != tunable.id),
                "{}",
                tunable.option
            );
        }
    }
}

#[cfg(feature = "tuning")]
use rsshogi_usi::UsiOption;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchParams {
    pub aspiration_window: i32,
    pub reverse_futility_margin: i32,
    pub futility_margin: i32,
    /// null moveの動的縮小で`(static_eval - beta)`を縮小量へ換算する分母。
    pub null_move_eval_divisor: i32,
    /// 対数LMRの分母。大きいほど縮小が浅くなる。
    pub lmr_divisor: i32,
    pub qsearch_delta_margin: i32,
    pub check_ordering_bonus: i32,
    /// 主探索の捕獲SEE枝刈りで`depth^2`へ掛ける許容損失。
    pub see_prune_margin: i32,
    /// 静かな手のhistory枝刈りで`depth`へ掛ける負の履歴の閾値。
    pub history_prune_margin: i32,
    /// 静かな打ち駒のLMP上限を、盤上の静かな手の上限から割り引く除数。
    pub drop_lmp_divisor: i32,
    /// aspiration窓を段階的に広げる上限回数。0なら最初の失敗で全窓へ落とす。
    pub aspiration_widenings: i32,
    /// correction historyが静的評価へ足せる補正の上限。0で補正を無効化する。
    pub correction_apply_max: i32,
    /// continuation historyの並べ替えへの寄与の重み。256で等倍、0で無効。
    pub continuation_weight: i32,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            aspiration_window: 82,
            reverse_futility_margin: 137,
            futility_margin: 185,
            null_move_eval_divisor: 202,
            lmr_divisor: 223,
            qsearch_delta_margin: 118,
            check_ordering_bonus: 4_061,
            see_prune_margin: 81,
            history_prune_margin: 2_036,
            drop_lmp_divisor: 2,
            aspiration_widenings: 0,
            correction_apply_max: 67,
            continuation_weight: 245,
        }
    }
}

impl SearchParams {
    #[cfg(feature = "tuning")]
    pub fn usi_options() -> [UsiOption; 13] {
        let defaults = Self::default();
        [
            UsiOption::spin(
                "SearchAspirationWindow",
                i64::from(defaults.aspiration_window),
                20,
                400,
            ),
            UsiOption::spin(
                "SearchReverseFutilityMargin",
                i64::from(defaults.reverse_futility_margin),
                40,
                400,
            ),
            UsiOption::spin("SearchFutilityMargin", i64::from(defaults.futility_margin), 40, 500),
            UsiOption::spin(
                "SearchNullMoveEvalDivisor",
                i64::from(defaults.null_move_eval_divisor),
                50,
                800,
            ),
            UsiOption::spin("SearchLmrDivisor", i64::from(defaults.lmr_divisor), 100, 600),
            UsiOption::spin(
                "SearchQsearchDeltaMargin",
                i64::from(defaults.qsearch_delta_margin),
                0,
                400,
            ),
            UsiOption::spin(
                "SearchCheckBonus",
                i64::from(defaults.check_ordering_bonus),
                500,
                8_000,
            ),
            UsiOption::spin("SearchSeePruneMargin", i64::from(defaults.see_prune_margin), 10, 400),
            UsiOption::spin(
                "SearchHistoryPruneMargin",
                i64::from(defaults.history_prune_margin),
                200,
                8_000,
            ),
            UsiOption::spin("SearchDropLmpDivisor", i64::from(defaults.drop_lmp_divisor), 0, 6),
            UsiOption::spin(
                "SearchAspirationWidenings",
                i64::from(defaults.aspiration_widenings),
                0,
                8,
            ),
            UsiOption::spin(
                "SearchCorrectionApplyMax",
                i64::from(defaults.correction_apply_max),
                0,
                256,
            ),
            UsiOption::spin(
                "SearchContinuationWeight",
                i64::from(defaults.continuation_weight),
                0,
                1_024,
            ),
        ]
    }

    #[cfg(feature = "tuning")]
    pub fn set_option(&mut self, name: &str, value: Option<&str>) -> Result<bool, String> {
        let (min, max) = match name {
            "SearchAspirationWindow" => (20, 400),
            "SearchReverseFutilityMargin" => (40, 400),
            "SearchFutilityMargin" => (40, 500),
            "SearchNullMoveEvalDivisor" => (50, 800),
            "SearchLmrDivisor" => (100, 600),
            "SearchQsearchDeltaMargin" => (0, 400),
            "SearchCheckBonus" => (500, 8_000),
            "SearchSeePruneMargin" => (10, 400),
            "SearchHistoryPruneMargin" => (200, 8_000),
            "SearchDropLmpDivisor" => (0, 6),
            "SearchAspirationWidenings" => (0, 8),
            "SearchCorrectionApplyMax" => (0, 256),
            "SearchContinuationWeight" => (0, 1_024),
            _ => return Ok(false),
        };
        let text = value.ok_or_else(|| format!("{name} requires a value"))?;
        let parsed = text.parse::<i32>().map_err(|_| format!("invalid {name}: {text}"))?;
        if !(min..=max).contains(&parsed) {
            return Err(format!("{name} must be in {min}..={max}"));
        }
        match name {
            "SearchAspirationWindow" => self.aspiration_window = parsed,
            "SearchReverseFutilityMargin" => self.reverse_futility_margin = parsed,
            "SearchFutilityMargin" => self.futility_margin = parsed,
            "SearchNullMoveEvalDivisor" => self.null_move_eval_divisor = parsed,
            "SearchLmrDivisor" => self.lmr_divisor = parsed,
            "SearchQsearchDeltaMargin" => self.qsearch_delta_margin = parsed,
            "SearchCheckBonus" => self.check_ordering_bonus = parsed,
            "SearchSeePruneMargin" => self.see_prune_margin = parsed,
            "SearchHistoryPruneMargin" => self.history_prune_margin = parsed,
            "SearchDropLmpDivisor" => self.drop_lmp_divisor = parsed,
            "SearchAspirationWidenings" => self.aspiration_widenings = parsed,
            "SearchCorrectionApplyMax" => self.correction_apply_max = parsed,
            "SearchContinuationWeight" => self.continuation_weight = parsed,
            _ => unreachable!("validated search option"),
        }
        Ok(true)
    }
}

#[cfg(feature = "tuning")]
pub fn tunable_manifest() -> &'static str {
    concat!(
        "{\"schema_version\":\"shogiarena.usi_tunables.v1\",\"tunables\":[",
        "{\"id\":\"search_aspiration_window\",\"option\":\"SearchAspirationWindow\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":82,\"min\":20,\"max\":400,\"schedule\":{\"c_end\":10,\"r_end\":0.002}},",
        "{\"id\":\"search_reverse_futility_margin\",\"option\":\"SearchReverseFutilityMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":137,\"min\":40,\"max\":400,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_futility_margin\",\"option\":\"SearchFutilityMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":185,\"min\":40,\"max\":500,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_null_move_eval_divisor\",\"option\":\"SearchNullMoveEvalDivisor\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":202,\"min\":50,\"max\":800,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_lmr_divisor\",\"option\":\"SearchLmrDivisor\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":223,\"min\":100,\"max\":600,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_qsearch_delta_margin\",\"option\":\"SearchQsearchDeltaMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":118,\"min\":0,\"max\":400,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_check_bonus\",\"option\":\"SearchCheckBonus\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":4061,\"min\":500,\"max\":8000,\"schedule\":{\"c_end\":200,\"r_end\":0.002}},",
        "{\"id\":\"search_see_prune_margin\",\"option\":\"SearchSeePruneMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":81,\"min\":10,\"max\":400,\"schedule\":{\"c_end\":15,\"r_end\":0.002}},",
        "{\"id\":\"search_history_prune_margin\",\"option\":\"SearchHistoryPruneMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":2036,\"min\":200,\"max\":8000,\"schedule\":{\"c_end\":300,\"r_end\":0.002}},",
        "{\"id\":\"search_drop_lmp_divisor\",\"option\":\"SearchDropLmpDivisor\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":2,\"min\":0,\"max\":6,\"schedule\":{\"c_end\":1,\"r_end\":0.002}},",
        "{\"id\":\"search_aspiration_widenings\",\"option\":\"SearchAspirationWidenings\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":0,\"min\":0,\"max\":8,\"schedule\":{\"c_end\":1,\"r_end\":0.002}},",
        "{\"id\":\"search_correction_apply_max\",\"option\":\"SearchCorrectionApplyMax\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":67,\"min\":0,\"max\":256,\"schedule\":{\"c_end\":10,\"r_end\":0.002}},",
        "{\"id\":\"search_continuation_weight\",\"option\":\"SearchContinuationWeight\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":245,\"min\":0,\"max\":1024,\"schedule\":{\"c_end\":40,\"r_end\":0.002}},",
        "{\"id\":\"fv_scale\",\"option\":\"FV_SCALE\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":24,\"min\":1,\"max\":128,\"schedule\":{\"c_end\":2,\"r_end\":0.002}}",
        "]}"
    )
}

#[cfg(all(test, feature = "tuning"))]
mod tests {
    use super::*;
    use crate::nnue::DEFAULT_FV_SCALE;

    #[test]
    fn tunable_manifest_defaults_match_runtime_defaults() {
        let manifest = tunable_manifest();
        let defaults = SearchParams::default();
        let cases = [
            ("SearchAspirationWindow", i64::from(defaults.aspiration_window)),
            ("SearchReverseFutilityMargin", i64::from(defaults.reverse_futility_margin)),
            ("SearchFutilityMargin", i64::from(defaults.futility_margin)),
            ("SearchNullMoveEvalDivisor", i64::from(defaults.null_move_eval_divisor)),
            ("SearchLmrDivisor", i64::from(defaults.lmr_divisor)),
            ("SearchQsearchDeltaMargin", i64::from(defaults.qsearch_delta_margin)),
            ("SearchCheckBonus", i64::from(defaults.check_ordering_bonus)),
            ("SearchSeePruneMargin", i64::from(defaults.see_prune_margin)),
            ("SearchHistoryPruneMargin", i64::from(defaults.history_prune_margin)),
            ("SearchDropLmpDivisor", i64::from(defaults.drop_lmp_divisor)),
            ("SearchAspirationWidenings", i64::from(defaults.aspiration_widenings)),
            ("SearchCorrectionApplyMax", i64::from(defaults.correction_apply_max)),
            ("SearchContinuationWeight", i64::from(defaults.continuation_weight)),
            ("FV_SCALE", i64::from(DEFAULT_FV_SCALE)),
        ];
        for (option, default) in cases {
            let expected = format!(
                "\"option\":\"{option}\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":{default}"
            );
            assert!(manifest.contains(&expected), "missing manifest fragment: {expected}");
        }
    }

    #[test]
    fn every_search_option_changes_its_backing_value() {
        let cases = [
            ("SearchAspirationWindow", "81"),
            ("SearchReverseFutilityMargin", "141"),
            ("SearchFutilityMargin", "181"),
            ("SearchNullMoveEvalDivisor", "210"),
            ("SearchLmrDivisor", "240"),
            ("SearchQsearchDeltaMargin", "121"),
            ("SearchCheckBonus", "4001"),
            ("SearchSeePruneMargin", "90"),
            ("SearchHistoryPruneMargin", "2200"),
            ("SearchDropLmpDivisor", "3"),
            ("SearchAspirationWidenings", "3"),
            ("SearchCorrectionApplyMax", "48"),
            ("SearchContinuationWeight", "128"),
        ];
        for (name, value) in cases {
            let mut params = SearchParams::default();
            assert!(params.set_option(name, Some(value)).expect("valid option"), "{name}");
            assert_ne!(params, SearchParams::default(), "{name}");
        }
    }
}

#[cfg(feature = "tuning")]
use rsshogi_usi::UsiOption;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchParams {
    pub aspiration_window: i32,
    pub reverse_futility_margin: i32,
    pub futility_margin: i32,
    pub null_move_reduction: u32,
    pub lmp_quiet_limits: [usize; 3],
    pub lmr_min_depth: u32,
    pub lmr_move_index: usize,
    pub lmr_reduction: u32,
    pub qsearch_delta_margin: i32,
    pub check_ordering_bonus: i32,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            aspiration_window: 80,
            reverse_futility_margin: 135,
            futility_margin: 183,
            null_move_reduction: 2,
            lmp_quiet_limits: [8, 12, 19],
            lmr_min_depth: 3,
            lmr_move_index: 4,
            lmr_reduction: 2,
            qsearch_delta_margin: 126,
            check_ordering_bonus: 4_009,
        }
    }
}

impl SearchParams {
    #[cfg(feature = "tuning")]
    pub fn usi_options() -> [UsiOption; 12] {
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
                "SearchNullMoveReduction",
                i64::from(defaults.null_move_reduction),
                1,
                4,
            ),
            UsiOption::spin("SearchLmpDepth1Moves", defaults.lmp_quiet_limits[0] as i64, 2, 24),
            UsiOption::spin("SearchLmpDepth2Moves", defaults.lmp_quiet_limits[1] as i64, 4, 32),
            UsiOption::spin("SearchLmpDepth3Moves", defaults.lmp_quiet_limits[2] as i64, 6, 48),
            UsiOption::spin("SearchLmrMinDepth", i64::from(defaults.lmr_min_depth), 2, 8),
            UsiOption::spin("SearchLmrMoveIndex", defaults.lmr_move_index as i64, 2, 16),
            UsiOption::spin("SearchLmrReduction", i64::from(defaults.lmr_reduction), 1, 3),
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
        ]
    }

    #[cfg(feature = "tuning")]
    pub fn set_option(&mut self, name: &str, value: Option<&str>) -> Result<bool, String> {
        let (min, max) = match name {
            "SearchAspirationWindow" => (20, 400),
            "SearchReverseFutilityMargin" => (40, 400),
            "SearchFutilityMargin" => (40, 500),
            "SearchNullMoveReduction" => (1, 4),
            "SearchLmpDepth1Moves" => (2, 24),
            "SearchLmpDepth2Moves" => (4, 32),
            "SearchLmpDepth3Moves" => (6, 48),
            "SearchLmrMinDepth" => (2, 8),
            "SearchLmrMoveIndex" => (2, 16),
            "SearchLmrReduction" => (1, 3),
            "SearchQsearchDeltaMargin" => (0, 400),
            "SearchCheckBonus" => (500, 8_000),
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
            "SearchNullMoveReduction" => self.null_move_reduction = parsed as u32,
            "SearchLmpDepth1Moves" => self.lmp_quiet_limits[0] = parsed as usize,
            "SearchLmpDepth2Moves" => self.lmp_quiet_limits[1] = parsed as usize,
            "SearchLmpDepth3Moves" => self.lmp_quiet_limits[2] = parsed as usize,
            "SearchLmrMinDepth" => self.lmr_min_depth = parsed as u32,
            "SearchLmrMoveIndex" => self.lmr_move_index = parsed as usize,
            "SearchLmrReduction" => self.lmr_reduction = parsed as u32,
            "SearchQsearchDeltaMargin" => self.qsearch_delta_margin = parsed,
            "SearchCheckBonus" => self.check_ordering_bonus = parsed,
            _ => unreachable!("validated search option"),
        }
        Ok(true)
    }
}

#[cfg(feature = "tuning")]
pub fn tunable_manifest() -> &'static str {
    concat!(
        "{\"schema_version\":\"shogiarena.usi_tunables.v1\",\"tunables\":[",
        "{\"id\":\"search_aspiration_window\",\"option\":\"SearchAspirationWindow\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":80,\"min\":20,\"max\":400,\"schedule\":{\"c_end\":10,\"r_end\":0.002}},",
        "{\"id\":\"search_reverse_futility_margin\",\"option\":\"SearchReverseFutilityMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":135,\"min\":40,\"max\":400,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_futility_margin\",\"option\":\"SearchFutilityMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":183,\"min\":40,\"max\":500,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_null_move_reduction\",\"option\":\"SearchNullMoveReduction\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":2,\"min\":1,\"max\":4,\"schedule\":{\"c_end\":1,\"r_end\":0.002}},",
        "{\"id\":\"search_lmp_depth1_moves\",\"option\":\"SearchLmpDepth1Moves\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":8,\"min\":2,\"max\":24,\"schedule\":{\"c_end\":2,\"r_end\":0.002}},",
        "{\"id\":\"search_lmp_depth2_moves\",\"option\":\"SearchLmpDepth2Moves\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":12,\"min\":4,\"max\":32,\"schedule\":{\"c_end\":2,\"r_end\":0.002}},",
        "{\"id\":\"search_lmp_depth3_moves\",\"option\":\"SearchLmpDepth3Moves\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":19,\"min\":6,\"max\":48,\"schedule\":{\"c_end\":3,\"r_end\":0.002}},",
        "{\"id\":\"search_lmr_min_depth\",\"option\":\"SearchLmrMinDepth\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":3,\"min\":2,\"max\":8,\"schedule\":{\"c_end\":1,\"r_end\":0.002}},",
        "{\"id\":\"search_lmr_move_index\",\"option\":\"SearchLmrMoveIndex\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":4,\"min\":2,\"max\":16,\"schedule\":{\"c_end\":1,\"r_end\":0.002}},",
        "{\"id\":\"search_lmr_reduction\",\"option\":\"SearchLmrReduction\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":2,\"min\":1,\"max\":3,\"schedule\":{\"c_end\":1,\"r_end\":0.002}},",
        "{\"id\":\"search_qsearch_delta_margin\",\"option\":\"SearchQsearchDeltaMargin\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":126,\"min\":0,\"max\":400,\"schedule\":{\"c_end\":20,\"r_end\":0.002}},",
        "{\"id\":\"search_check_bonus\",\"option\":\"SearchCheckBonus\",\"value_type\":\"int\",\"encoding\":\"integer\",\"default\":4009,\"min\":500,\"max\":8000,\"schedule\":{\"c_end\":200,\"r_end\":0.002}},",
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
            ("SearchNullMoveReduction", i64::from(defaults.null_move_reduction)),
            ("SearchLmpDepth1Moves", defaults.lmp_quiet_limits[0] as i64),
            ("SearchLmpDepth2Moves", defaults.lmp_quiet_limits[1] as i64),
            ("SearchLmpDepth3Moves", defaults.lmp_quiet_limits[2] as i64),
            ("SearchLmrMinDepth", i64::from(defaults.lmr_min_depth)),
            ("SearchLmrMoveIndex", defaults.lmr_move_index as i64),
            ("SearchLmrReduction", i64::from(defaults.lmr_reduction)),
            ("SearchQsearchDeltaMargin", i64::from(defaults.qsearch_delta_margin)),
            ("SearchCheckBonus", i64::from(defaults.check_ordering_bonus)),
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
            ("SearchNullMoveReduction", "3"),
            ("SearchLmpDepth1Moves", "9"),
            ("SearchLmpDepth2Moves", "13"),
            ("SearchLmpDepth3Moves", "20"),
            ("SearchLmrMinDepth", "4"),
            ("SearchLmrMoveIndex", "5"),
            ("SearchLmrReduction", "3"),
            ("SearchQsearchDeltaMargin", "121"),
            ("SearchCheckBonus", "4001"),
        ];
        for (name, value) in cases {
            let mut params = SearchParams::default();
            assert!(params.set_option(name, Some(value)).expect("valid option"), "{name}");
            assert_ne!(params, SearchParams::default(), "{name}");
        }
    }
}

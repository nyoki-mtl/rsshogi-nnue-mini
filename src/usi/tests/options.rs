use crate::position::MAX_ROOT_GAME_PLY;
use crate::search::SearchParams;
#[cfg(feature = "tuning")]
use crate::search::tunable_manifest;

use super::super::go_time::DEFAULT_MOVE_OVERHEAD_MS;
use super::super::session::UsiSession;
use super::super::{DEFAULT_ENTERING_KING_RULE, DEFAULT_EVAL_FILE};
use super::output;

#[test]
fn terminal_rule_options_are_advertised() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("usi", &mut writer).expect("usi should succeed");
    assert!(output(&writer).contains("option name EnteringKingRule type combo"));
    assert!(!output(&writer).contains("TryRule"));
    assert!(output(&writer).contains("option name MaxMovesToDraw type spin"));
    assert!(output(&writer).contains("max 65407"));
}

#[test]
fn max_moves_to_draw_uses_the_root_ply_bound() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("setoption name MaxMovesToDraw value 65407", &mut writer)
        .expect("maximum safe value should be accepted");
    assert_eq!(session.max_moves_to_draw, MAX_ROOT_GAME_PLY);

    session
        .handle_line("setoption name MaxMovesToDraw value 65408", &mut writer)
        .expect("out-of-range value should be reported");
    assert_eq!(session.max_moves_to_draw, MAX_ROOT_GAME_PLY);
    assert!(output(&writer).contains("0..=65407"));
}

#[test]
fn move_overhead_is_accepted_and_bounded() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    assert_eq!(session.move_overhead_ms, DEFAULT_MOVE_OVERHEAD_MS);

    session
        .handle_line("setoption name MoveOverhead value 0", &mut writer)
        .expect("zero should be accepted for latency-free environments");
    assert_eq!(session.move_overhead_ms, 0);

    session
        .handle_line("setoption name MoveOverhead value 5000", &mut writer)
        .expect("the advertised maximum should be accepted");
    assert_eq!(session.move_overhead_ms, 5_000);

    session
        .handle_line("setoption name MoveOverhead value 5001", &mut writer)
        .expect("out-of-range value should be reported");
    assert_eq!(session.move_overhead_ms, 5_000);
    assert!(output(&writer).contains("0..=5000"));
}

#[test]
fn try_rule_is_rejected() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("setoption name EnteringKingRule value TryRule", &mut writer)
        .expect("unsupported option value should not terminate the session");

    assert!(output(&writer).contains("unsupported EnteringKingRule"));
    assert_eq!(session.entering_king_rule, DEFAULT_ENTERING_KING_RULE);
}

#[cfg(feature = "tuning")]
#[test]
fn manifest_uses_shogiarena_schema() {
    let manifest = tunable_manifest();
    assert!(manifest.contains("shogiarena.usi_tunables.v1"));
    assert!(manifest.contains("SearchNullMoveEvalDivisor"));
    assert_eq!(manifest.matches("\"id\":").count(), 51);
}

#[test]
fn usi_advertises_runtime_options() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("usi", &mut writer).expect("usi should succeed");

    let text = output(&writer);
    assert!(text.contains("option name USI_Ponder type check default false"));
    assert!(
        text.contains(&format!("option name EvalPackage type string default {DEFAULT_EVAL_FILE}"))
    );
    assert!(text.contains("option name USI_Hash type spin default 16 min 1 max 1024"));
    assert!(text.contains("option name Threads type spin default 1 min 1 max 16"));
    assert!(text.contains("option name MoveOverhead type spin default 500 min 0 max 5000"));
    #[cfg(feature = "tuning")]
    {
        assert!(text.contains("option name SearchAspirationWindow type spin default 88"));
        assert!(text.contains("option name SearchCheckBonus type spin default 4175"));
        assert!(text.contains("option name Clear Hash type button"));
    }
    #[cfg(not(feature = "tuning"))]
    {
        assert!(!text.contains("option name Clear Hash"));
        assert!(!text.contains("option name SearchAspirationWindow"));
        assert!(!text.contains("option name SearchCheckBonus"));
    }
}

#[cfg(not(feature = "tuning"))]
#[test]
fn normal_build_does_not_answer_tunable_manifest_requests() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("usi_tunables", &mut writer).expect("hidden command should be ignored");
    assert!(writer.is_empty());
}

#[cfg(not(feature = "tuning"))]
#[test]
fn normal_build_keeps_tuning_values_fixed() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("setoption name SearchAspirationWindow value 85", &mut writer)
        .expect("hidden option should be ignored");

    assert_eq!(session.eval_file, DEFAULT_EVAL_FILE);
    assert_eq!(session.search_params, SearchParams::default());
}

#[cfg(feature = "tuning")]
#[test]
fn every_search_option_is_applied_to_the_next_snapshot() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    let cases = [
        ("SearchAspirationWindow", "81"),
        ("SearchReverseFutilityMargin", "141"),
        ("SearchFutilityMargin", "181"),
        ("SearchNullMoveEvalDivisor", "210"),
        ("SearchLmrDivisor", "240"),
        ("SearchQsearchDeltaMargin", "121"),
        ("SearchCheckBonus", "4001"),
    ];
    for (name, value) in cases {
        session
            .handle_line(&format!("setoption name {name} value {value}"), &mut writer)
            .expect("valid search option");
    }

    assert_ne!(session.search_params, SearchParams::default());
    assert_eq!(session.search_params.aspiration_window, 81);
    assert_eq!(session.search_params.null_move_eval_divisor, 210);
    assert_eq!(session.search_params.lmr_divisor, 240);
    assert_eq!(session.search_params.check_ordering_bonus, 4001);
}

#[test]
fn nnue_load_failure_does_not_fall_back_silently() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.material_test_mode = false;
    session.eval_file = "_missing/model.rsnn".to_owned();
    session.handle_line("isready", &mut writer).expect("isready should respond");
    session.handle_line("go depth 1", &mut writer).expect("go should respond");

    assert!(output(&writer).contains("NNUE load error:"));
    assert!(!output(&writer).lines().any(|line| line == "readyok"));
    assert!(output(&writer).contains("NNUE is not loaded"));
    assert_eq!(output(&writer).lines().filter(|line| *line == "bestmove resign").count(), 1);
    let lines = output(&writer).lines().collect::<Vec<_>>();
    let bestmove = lines.iter().position(|line| *line == "bestmove resign").expect("bestmove");
    assert!(lines[bestmove - 1].starts_with("info depth 0 "));
}

#[test]
fn eval_package_option_changes_the_load_path() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("setoption name EvalPackage value _missing/other.rsnn", &mut writer)
        .expect("set package path");
    assert_eq!(session.eval_file, "_missing/other.rsnn");
    session.material_test_mode = false;
    session.handle_line("isready", &mut writer).expect("missing package fails closed");
    assert!(!output(&writer).contains("readyok"));
    assert!(output(&writer).contains("NNUE load error:"));
}

#[cfg(feature = "embedded-rsnn")]
#[test]
fn embedded_package_is_the_default_and_can_be_selected_again() {
    let mut session = UsiSession::new();
    session.material_test_mode = false;
    let mut writer = Vec::new();
    session.handle_line("isready", &mut writer).expect("embedded package should load");
    assert!(output(&writer).contains("loaded A-only .rsnn: @default digest="));
    assert!(output(&writer).lines().any(|line| line == "readyok"));

    writer.clear();
    session
        .handle_line("setoption name EvalPackage value _missing/other.rsnn", &mut writer)
        .expect("set missing package");
    session.handle_line("isready", &mut writer).expect("missing package should fail");
    assert!(!output(&writer).lines().any(|line| line == "readyok"));

    writer.clear();
    session
        .handle_line("setoption name EvalPackage value @default", &mut writer)
        .expect("select embedded package");
    session.handle_line("isready", &mut writer).expect("embedded package should reload");
    assert!(output(&writer).lines().any(|line| line == "readyok"));
}

#[test]
fn material_evaluator_ready_still_emits_readyok() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("isready", &mut writer).expect("isready should respond");

    assert!(output(&writer).lines().any(|line| line == "readyok"));
}

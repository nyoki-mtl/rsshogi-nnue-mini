//! Engineを介したUSIセッションの挙動を見るtest群。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use rsshogi::board;

#[cfg(not(feature = "tuning"))]
use crate::nnue::DEFAULT_FV_SCALE;
use crate::params::SearchParams;
#[cfg(feature = "tuning")]
use crate::params::tunable_manifest;
use crate::position::MAX_ROOT_GAME_PLY;

use super::DEFAULT_ENTERING_KING_RULE;
#[cfg(not(feature = "tuning"))]
use super::DEFAULT_EVAL_FILE;
use super::engine::Engine;
use super::output::write_info;
use super::time::DEFAULT_MOVE_OVERHEAD_MS;

fn wait_for_idle(engine: &mut Engine, writer: &mut Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while engine.search.is_some() {
        engine.poll_search(writer).expect("poll should succeed");
        assert!(Instant::now() < deadline, "search did not become idle");
        thread::yield_now();
    }
}

fn wait_for_completed_ponder(engine: &mut Engine, writer: &mut Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while engine.search.as_ref().is_none_or(|task| task.completed.is_none()) {
        engine.poll_search(writer).expect("poll should succeed");
        assert!(Instant::now() < deadline, "ponder did not complete");
        thread::yield_now();
    }
}

fn output(writer: &[u8]) -> &str {
    std::str::from_utf8(writer).expect("USI output must be UTF-8")
}

fn bestmove_count(writer: &[u8]) -> usize {
    output(writer).lines().filter(|line| line.starts_with("bestmove ")).count()
}

#[test]
fn session_accepts_u64_max_time_without_panicking() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("go depth 1 movetime 18446744073709551615", &mut writer)
        .expect("u64 maximum movetime should be accepted");
    wait_for_idle(&mut engine, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn entering_king_declaration_is_reported_as_win() {
    board::init();
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line(
            "position sfen K+N5+L1/G+L+P+B1+R+P2/3+P2G2/9/2+p+n5/3s2+ss1/3+p+p1+s1+r/7g+n/6g+nk b 2L8Pb4p 1",
            &mut writer,
        )
        .expect("position should succeed");
    engine.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut engine, &mut writer);
    assert!(output(&writer).lines().any(|line| line.contains("score mate +")));
    assert!(output(&writer).lines().any(|line| line == "bestmove win"));
}

#[test]
fn root_checkmate_reports_loss_before_resign() {
    board::init();
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("position sfen k3r4/9/9/9/9/9/9/4g4/4K4 b - 1", &mut writer)
        .expect("position should succeed");
    engine.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut engine, &mut writer);

    let lines = output(&writer).lines().collect::<Vec<_>>();
    let bestmove = lines.iter().position(|line| *line == "bestmove resign").expect("bestmove");
    assert!(lines[bestmove - 1].contains("score mate -"));
}

#[test]
fn non_check_no_legal_child_is_a_loss_even_at_qsearch_and_fallback() {
    board::init();
    for command in ["go depth 1 searchmoves 9g9f", "go nodes 1 searchmoves 9g9f"] {
        let mut engine = Engine::new();
        let mut writer = Vec::new();
        engine
            .handle_line("position sfen 8k/6G2/7G1/9/9/9/P8/9/K8 b - 1", &mut writer)
            .expect("position should succeed");
        engine.handle_line(command, &mut writer).expect("go should succeed");
        wait_for_idle(&mut engine, &mut writer);
        assert!(output(&writer).lines().any(|line| line == "bestmove 9g9f"));
        assert!(output(&writer).lines().any(|line| line.contains("score mate 1")));
    }
}

#[test]
fn terminal_rule_options_are_advertised() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("usi", &mut writer).expect("usi should succeed");
    assert!(output(&writer).contains("option name EnteringKingRule type combo"));
    assert!(!output(&writer).contains("TryRule"));
    assert!(output(&writer).contains("option name MaxMovesToDraw type spin"));
    assert!(output(&writer).contains("max 65407"));
}

#[test]
fn max_moves_to_draw_uses_the_root_ply_bound() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("setoption name MaxMovesToDraw value 65407", &mut writer)
        .expect("maximum safe value should be accepted");
    assert_eq!(engine.max_moves_to_draw, MAX_ROOT_GAME_PLY);

    engine
        .handle_line("setoption name MaxMovesToDraw value 65408", &mut writer)
        .expect("out-of-range value should be reported");
    assert_eq!(engine.max_moves_to_draw, MAX_ROOT_GAME_PLY);
    assert!(output(&writer).contains("0..=65407"));
}

#[test]
fn move_overhead_is_accepted_and_bounded() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    assert_eq!(engine.move_overhead_ms, DEFAULT_MOVE_OVERHEAD_MS);

    engine
        .handle_line("setoption name MoveOverhead value 0", &mut writer)
        .expect("zero should be accepted for latency-free environments");
    assert_eq!(engine.move_overhead_ms, 0);

    engine
        .handle_line("setoption name MoveOverhead value 5000", &mut writer)
        .expect("the advertised maximum should be accepted");
    assert_eq!(engine.move_overhead_ms, 5_000);

    engine
        .handle_line("setoption name MoveOverhead value 5001", &mut writer)
        .expect("out-of-range value should be reported");
    assert_eq!(engine.move_overhead_ms, 5_000);
    assert!(output(&writer).contains("0..=5000"));
}

#[test]
fn try_rule_is_rejected() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("setoption name EnteringKingRule value TryRule", &mut writer)
        .expect("unsupported option value should not terminate the session");

    assert!(output(&writer).contains("unsupported EnteringKingRule"));
    assert_eq!(engine.entering_king_rule, DEFAULT_ENTERING_KING_RULE);
}

#[cfg(feature = "tuning")]
#[test]
fn manifest_uses_shogiarena_schema() {
    let manifest = tunable_manifest();
    assert!(manifest.contains("shogiarena.usi_tunables.v1"));
    assert!(manifest.contains("SearchNullMoveReduction"));
    assert!(manifest.contains("\"option\":\"FV_SCALE\""));
    assert!(!manifest.contains("EvalPawnValue"));
    assert_eq!(manifest.matches("\"id\":").count(), 13);
}

#[test]
fn usi_advertises_runtime_options() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("usi", &mut writer).expect("usi should succeed");

    let output = output(&writer);
    assert!(output.contains("option name USI_Ponder type check default false"));
    assert!(output.contains("option name USI_Hash type spin default 16 min 1 max 1024"));
    assert!(output.contains("option name Threads type spin default 1 min 1 max 16"));
    assert!(output.contains("option name MoveOverhead type spin default 500 min 0 max 5000"));
    assert!(!output.contains("option name UseNNUE"));
    assert!(!output.contains("option name EvalFile"));
    assert!(!output.contains("option name EvalPawnValue"));
    #[cfg(feature = "tuning")]
    {
        assert!(output.contains("option name SearchAspirationWindow type spin default 80"));
        assert!(output.contains("option name SearchCheckBonus type spin default 4009"));
        assert!(output.contains("option name FV_SCALE type spin default 24"));
        assert!(output.contains("option name Clear Hash type button"));
    }
    #[cfg(not(feature = "tuning"))]
    {
        assert!(!output.contains("option name FV_SCALE"));
        assert!(!output.contains("option name Clear Hash"));
        assert!(!output.contains("option name SearchAspirationWindow"));
        assert!(!output.contains("option name SearchCheckBonus"));
    }
}

#[cfg(not(feature = "tuning"))]
#[test]
fn normal_build_does_not_answer_tunable_manifest_requests() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("usi_tunables", &mut writer).expect("hidden command should be ignored");
    assert!(writer.is_empty());
}

#[cfg(not(feature = "tuning"))]
#[test]
fn normal_build_keeps_nnue_and_tuning_values_fixed() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    for command in [
        "setoption name UseNNUE value false",
        "setoption name EvalFile value other/nn.bin",
        "setoption name FV_SCALE value 25",
        "setoption name SearchAspirationWindow value 81",
    ] {
        engine.handle_line(command, &mut writer).expect("hidden option should be ignored");
    }

    assert_eq!(engine.eval_file, DEFAULT_EVAL_FILE);
    assert_eq!(engine.fv_scale, DEFAULT_FV_SCALE);
    assert_eq!(engine.search_params, SearchParams::default());
}

#[cfg(feature = "tuning")]
#[test]
fn every_search_option_is_applied_to_the_next_snapshot() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
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
        engine
            .handle_line(&format!("setoption name {name} value {value}"), &mut writer)
            .expect("valid search option");
    }

    assert_ne!(engine.search_params, SearchParams::default());
    assert_eq!(engine.search_params.aspiration_window, 81);
    assert_eq!(engine.search_params.null_move_reduction, 3);
    assert_eq!(engine.search_params.lmp_quiet_limits, [9, 13, 20]);
    assert_eq!(engine.search_params.lmr_reduction, 3);
    assert_eq!(engine.search_params.check_ordering_bonus, 4001);
}

#[test]
fn nnue_load_failure_does_not_fall_back_silently() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.material_test_mode = false;
    engine.eval_file = "_missing/nn.bin".to_owned();
    engine.handle_line("isready", &mut writer).expect("isready should respond");
    engine.handle_line("go depth 1", &mut writer).expect("go should respond");

    assert!(output(&writer).contains("NNUE load error:"));
    assert!(!output(&writer).lines().any(|line| line == "readyok"));
    assert!(output(&writer).contains("NNUE is not loaded"));
    assert_eq!(output(&writer).lines().filter(|line| *line == "bestmove resign").count(), 1);
    let lines = output(&writer).lines().collect::<Vec<_>>();
    let bestmove = lines.iter().position(|line| *line == "bestmove resign").expect("bestmove");
    assert!(lines[bestmove - 1].starts_with("info depth 0 "));
}

#[test]
fn material_evaluator_ready_still_emits_readyok() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("isready", &mut writer).expect("isready should respond");

    assert!(output(&writer).lines().any(|line| line == "readyok"));
}

#[test]
fn stop_publishes_exactly_one_bestmove() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("position startpos", &mut writer).expect("position should succeed");
    engine.handle_line("go infinite", &mut writer).expect("go should succeed");
    engine.handle_line("stop", &mut writer).expect("stop should succeed");

    assert!(engine.search.is_none());
    assert_eq!(bestmove_count(&writer), 1);

    engine.handle_line("stop", &mut writer).expect("duplicate stop should succeed");
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn replacement_position_suppresses_stale_result() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("go infinite", &mut writer).expect("first go should succeed");
    engine
        .handle_line("position startpos moves 7g7f", &mut writer)
        .expect("replacement position should succeed");
    assert_eq!(bestmove_count(&writer), 0, "cancelled search must stay unpublished");

    engine.handle_line("go depth 1", &mut writer).expect("second go should succeed");
    wait_for_idle(&mut engine, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn invalid_sfen_does_not_poison_the_worker() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("position sfen 9/9/9/9/9/9/9/9/4K4 b - 1", &mut writer)
        .expect("invalid position should be reported");
    assert!(output(&writer).contains("position error:"));

    engine.handle_line("position startpos", &mut writer).expect("startpos should succeed");
    engine.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut engine, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn king_capture_replay_does_not_poison_the_worker() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("position sfen 4k4/4R4/9/9/9/9/9/9/4K4 b - 1 moves 5b5a+", &mut writer)
        .expect("king capture should be reported");
    assert!(output(&writer).contains("captures the opposing king"));

    engine.handle_line("position startpos", &mut writer).expect("startpos should succeed");
    engine.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut engine, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn quit_joins_search_without_bestmove() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("go infinite", &mut writer).expect("go should succeed");
    engine.handle_line("quit", &mut writer).expect("quit should succeed");

    assert!(engine.quit);
    assert!(engine.search.is_none());
    assert_eq!(bestmove_count(&writer), 0);
}

#[test]
fn usinewgame_cancels_search_without_bestmove() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("go infinite", &mut writer).expect("go should succeed");
    engine.handle_line("usinewgame", &mut writer).expect("usinewgame should succeed");

    assert!(engine.search.is_none());
    assert_eq!(engine.position.game_ply(), 1);
    assert_eq!(bestmove_count(&writer), 0);
}

#[test]
fn worker_runtime_is_reused_across_searches() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    let runtime = std::ptr::from_ref(&engine.worker);

    engine.handle_line("go depth 1", &mut writer).expect("first go should succeed");
    wait_for_idle(&mut engine, &mut writer);
    engine.handle_line("go depth 1", &mut writer).expect("second go should succeed");
    wait_for_idle(&mut engine, &mut writer);

    assert_eq!(runtime, std::ptr::from_ref(&engine.worker));
    assert_eq!(bestmove_count(&writer), 2);
}

#[test]
fn lazy_smp_publishes_only_the_main_worker_result() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("setoption name Threads value 2", &mut writer)
        .expect("Threads should be accepted");
    engine.handle_line("go depth 2", &mut writer).expect("go should succeed");
    wait_for_idle(&mut engine, &mut writer);

    assert_eq!(engine.threads, 2);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn completed_search_offers_a_legal_ponder_candidate() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("go depth 2", &mut writer).expect("go should succeed");
    wait_for_idle(&mut engine, &mut writer);

    let line = output(&writer)
        .lines()
        .find(|line| line.starts_with("bestmove "))
        .expect("bestmove should be published");
    assert!(line.contains(" ponder "), "depth-two result should offer a ponder move: {line}");
}

#[test]
fn hash_maintenance_cancels_stale_work_and_runtime_survives() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("go infinite", &mut writer).expect("go should succeed");
    engine
        .handle_line("setoption name USI_Hash value 1", &mut writer)
        .expect("Hash resize should succeed");
    assert_eq!(bestmove_count(&writer), 0, "maintenance must not publish cancelled work");

    engine
        .handle_line("setoption name Clear Hash", &mut writer)
        .expect("Clear Hash should succeed");
    engine.handle_line("go depth 1", &mut writer).expect("second go should succeed");
    wait_for_idle(&mut engine, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

/// 時刻付き・時刻なしの両方の`ponderhit`表記で同じ挙動になることを見る。
const PONDERHIT_FORMS: [&str; 2] = ["ponderhit", "ponderhit btime 37082 wtime 23103 byoyomi 10000"];

#[test]
fn ponderhit_releases_a_completed_ponder() {
    for ponderhit in PONDERHIT_FORMS {
        let mut engine = Engine::new();
        let mut writer = Vec::new();
        engine.handle_line("go ponder depth 1", &mut writer).expect("go should succeed");
        wait_for_completed_ponder(&mut engine, &mut writer);
        assert_eq!(bestmove_count(&writer), 0, "{ponderhit}");

        engine.handle_line(ponderhit, &mut writer).expect("ponderhit should succeed");
        assert!(engine.search.is_none(), "{ponderhit}");
        assert_eq!(bestmove_count(&writer), 1, "{ponderhit}");
    }
}

#[test]
fn ponderhit_releases_an_inflight_search_without_cancelling_it() {
    for ponderhit in PONDERHIT_FORMS {
        let mut engine = Engine::new();
        let mut writer = Vec::new();
        engine.handle_line("go ponder infinite", &mut writer).expect("go should succeed");
        let cancel = Arc::clone(&engine.search.as_ref().expect("search task").cancel);

        engine.handle_line(ponderhit, &mut writer).expect("ponderhit should succeed");

        let task = engine.search.as_ref().expect("inflight search must remain owned");
        assert!(!cancel.load(Ordering::SeqCst), "{ponderhit} must not cancel the search");
        assert!(!task.pondering.load(Ordering::SeqCst), "{ponderhit}");
        assert!(!task.defer_bestmove, "{ponderhit}");
        engine.handle_line("stop", &mut writer).expect("stop should clean up");
        assert_eq!(bestmove_count(&writer), 1, "{ponderhit}");
    }
}

#[test]
fn completed_infinite_search_waits_for_stop() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("go infinite depth 1", &mut writer).expect("go should succeed");
    wait_for_completed_ponder(&mut engine, &mut writer);
    assert_eq!(bestmove_count(&writer), 0);

    engine.handle_line("stop", &mut writer).expect("stop should publish");
    assert!(engine.search.is_none());
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn final_info_precedes_bestmove_when_no_iteration_completes() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine
        .handle_line("position sfen 4k4/9/9/9/4r4/4P4/9/9/4K4 b - 1", &mut writer)
        .expect("position should succeed");
    engine.handle_line("go nodes 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut engine, &mut writer);

    let lines = output(&writer).lines().map(str::to_owned).collect::<Vec<_>>();
    let bestmove_index =
        lines.iter().position(|line| line.starts_with("bestmove ")).expect("bestmove line");
    assert!(bestmove_index > 0);
    assert_eq!(lines[bestmove_index], "bestmove 5f5e");
    let final_info = &lines[bestmove_index - 1];
    assert!(final_info.starts_with("info depth 0 "), "{final_info}");
    assert!(final_info.contains(" nodes 1 "), "{final_info}");
    assert!(final_info.ends_with("score cp 1100 pv 5f5e"), "{final_info}");
}

#[test]
fn mate_scores_are_written_as_mate_not_cp() {
    let mut writer = Vec::new();
    write_info(&mut writer, 4, 31_997, 123, Duration::from_millis(10), &[])
        .expect("info should write");

    assert!(output(&writer).contains("score mate 3"));
    assert!(!output(&writer).contains("score cp"));
}

#[test]
fn zero_distance_mate_scores_keep_the_outcome_sign() {
    for (score, expected) in [(32_000, "score mate +"), (-32_000, "score mate -")] {
        let mut writer = Vec::new();
        write_info(&mut writer, 0, score, 0, Duration::ZERO, &[]).expect("info should write");
        assert!(output(&writer).contains(expected));
        assert!(!output(&writer).contains("score mate 0"));
    }
}

#[test]
fn inflight_ponder_can_be_stopped() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.handle_line("go ponder infinite", &mut writer).expect("go should succeed");
    assert!(engine.search.as_ref().is_some_and(|task| task.completed.is_none()));

    engine.handle_line("stop", &mut writer).expect("stop should succeed");
    assert!(engine.search.is_none());
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn panicking_job_fails_closed_and_keeps_the_coordinator_alive() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.threads = 4;
    engine.panic_next_search = true;

    engine.handle_line("go depth 1", &mut writer).expect("go should start");
    let cancel = Arc::clone(&engine.search.as_ref().expect("search task").cancel);
    wait_for_idle(&mut engine, &mut writer);
    assert_eq!(Arc::strong_count(&cancel), 1, "all helper workers must be joined");
    let lines = output(&writer).lines().collect::<Vec<_>>();
    let bestmove_index = lines
        .iter()
        .position(|line| *line == "bestmove resign")
        .expect("panicking search must fail closed");
    assert!(
        lines[..bestmove_index]
            .iter()
            .any(|line| line == &"info string search worker ended without a result")
    );
    assert!(
        lines[bestmove_index - 1].contains("depth 0")
            && lines[bestmove_index - 1].contains("score cp 0"),
        "final structured info: {}",
        lines[bestmove_index - 1]
    );

    engine.handle_line("position startpos", &mut writer).expect("position should recover");
    engine.handle_line("go depth 1", &mut writer).expect("second go should start");
    wait_for_idle(&mut engine, &mut writer);
    assert_eq!(bestmove_count(&writer), 2, "coordinator must accept a second search");
}

#[test]
fn position_after_a_panicking_job_suppresses_stale_diagnostics() {
    let mut engine = Engine::new();
    let mut writer = Vec::new();
    engine.panic_next_search = true;

    engine.handle_line("go depth 1", &mut writer).expect("go should start");
    engine
        .handle_line("position startpos", &mut writer)
        .expect("position should suppress the cancelled search");

    assert!(!output(&writer).contains("search worker ended without a result"));
    assert_eq!(bestmove_count(&writer), 0);
}

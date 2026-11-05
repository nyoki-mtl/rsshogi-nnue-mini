use std::sync::Arc;

use super::super::session::UsiSession;
use super::{bestmove_count, output, wait_for_idle};

#[test]
fn replacement_position_suppresses_stale_result() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("go infinite", &mut writer).expect("first go should succeed");
    session
        .handle_line("position startpos moves 7g7f", &mut writer)
        .expect("replacement position should succeed");
    assert_eq!(bestmove_count(&writer), 0, "cancelled search must stay unpublished");

    session.handle_line("go depth 1", &mut writer).expect("second go should succeed");
    wait_for_idle(&mut session, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn invalid_sfen_does_not_poison_the_worker() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("position sfen 9/9/9/9/9/9/9/9/4K4 b - 1", &mut writer)
        .expect("invalid position should be reported");
    assert!(output(&writer).contains("position error:"));

    session.handle_line("position startpos", &mut writer).expect("startpos should succeed");
    session.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut session, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn king_capture_replay_does_not_poison_the_worker() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("position sfen 4k4/4R4/9/9/9/9/9/9/4K4 b - 1 moves 5b5a+", &mut writer)
        .expect("king capture should be reported");
    assert!(output(&writer).contains("captures the opposing king"));

    session.handle_line("position startpos", &mut writer).expect("startpos should succeed");
    session.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut session, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn quit_joins_search_without_bestmove() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("go infinite", &mut writer).expect("go should succeed");
    session.handle_line("quit", &mut writer).expect("quit should succeed");

    assert!(session.quit);
    assert!(session.search.is_none());
    assert_eq!(bestmove_count(&writer), 0);
}

#[test]
fn usinewgame_cancels_search_without_bestmove() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("go infinite", &mut writer).expect("go should succeed");
    session.handle_line("usinewgame", &mut writer).expect("usinewgame should succeed");

    assert!(session.search.is_none());
    assert_eq!(session.position.game_ply(), 1);
    assert_eq!(bestmove_count(&writer), 0);
}

#[test]
fn search_bridge_is_reused_across_searches() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    let bridge = std::ptr::from_ref(&session.bridge);

    session.handle_line("go depth 1", &mut writer).expect("first go should succeed");
    wait_for_idle(&mut session, &mut writer);
    session.handle_line("go depth 1", &mut writer).expect("second go should succeed");
    wait_for_idle(&mut session, &mut writer);

    assert_eq!(bridge, std::ptr::from_ref(&session.bridge));
    assert_eq!(bestmove_count(&writer), 2);
}

#[test]
fn worker_threads_survive_go_and_maintenance_and_rebuild_after_threads_change() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("setoption name Threads value 3", &mut writer).unwrap();
    session.handle_line("go depth 2", &mut writer).unwrap();
    wait_for_idle(&mut session, &mut writer);
    let ids = session.bridge.thread_ids();
    assert_eq!(ids.len(), 3);
    for command in ["setoption name Clear Hash", "setoption name USI_Hash value 1", "usinewgame"] {
        session.handle_line(command, &mut writer).unwrap();
        session.handle_line("go depth 2", &mut writer).unwrap();
        wait_for_idle(&mut session, &mut writer);
        assert_eq!(session.bridge.thread_ids(), ids, "{command}");
    }
    session.handle_line("setoption name Threads value 2", &mut writer).unwrap();
    session.handle_line("go depth 2", &mut writer).unwrap();
    wait_for_idle(&mut session, &mut writer);
    let next = session.bridge.thread_ids();
    assert_eq!(next.len(), 2);
    assert_eq!(next[0], ids[0], "coordinator remains the main worker");
    assert!(!ids[1..].contains(&next[1]), "helpers are rebuilt after Threads changes");
    assert_eq!(bestmove_count(&writer), 5);
}

#[test]
fn eval_package_change_releases_workers_before_loading_the_next_package() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("setoption name Threads value 2", &mut writer).unwrap();
    session.handle_line("go depth 2", &mut writer).unwrap();
    wait_for_idle(&mut session, &mut writer);
    let old = session.bridge.thread_ids();
    assert_eq!(old.len(), 2);
    session.handle_line("setoption name EvalPackage value next.rsnn", &mut writer).unwrap();
    assert!(session.bridge.thread_ids().is_empty(), "old contexts must be released before isready");
    // material_test_modeでpackageのI/Oと切り離し、再構築の境界だけを検証する。
    session.handle_line("go depth 2", &mut writer).unwrap();
    wait_for_idle(&mut session, &mut writer);
    let next = session.bridge.thread_ids();
    assert_eq!(next.len(), 2);
    assert_eq!(next[0], old[0]);
    assert_ne!(next[1], old[1]);
    assert_eq!(bestmove_count(&writer), 2);
}

#[test]
fn hash_maintenance_cancels_stale_work_and_runtime_survives() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("go infinite", &mut writer).expect("go should succeed");
    session
        .handle_line("setoption name USI_Hash value 1", &mut writer)
        .expect("Hash resize should succeed");
    assert_eq!(bestmove_count(&writer), 0, "maintenance must not publish cancelled work");

    session
        .handle_line("setoption name Clear Hash", &mut writer)
        .expect("Clear Hash should succeed");
    session.handle_line("go depth 1", &mut writer).expect("second go should succeed");
    wait_for_idle(&mut session, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn panicking_job_fails_closed_and_keeps_the_coordinator_alive() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.threads = 4;
    session.panic_next_search = true;

    session.handle_line("go depth 1", &mut writer).expect("go should start");
    let cancel = Arc::clone(&session.search.as_ref().expect("search task").cancel);
    wait_for_idle(&mut session, &mut writer);
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

    session.handle_line("position startpos", &mut writer).expect("position should recover");
    session.handle_line("go depth 1", &mut writer).expect("second go should start");
    wait_for_idle(&mut session, &mut writer);
    assert_eq!(bestmove_count(&writer), 2, "coordinator must accept a second search");
}

#[test]
fn position_after_a_panicking_job_suppresses_stale_diagnostics() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.panic_next_search = true;

    session.handle_line("go depth 1", &mut writer).expect("go should start");
    session
        .handle_line("position startpos", &mut writer)
        .expect("position should suppress the cancelled search");

    assert!(!output(&writer).contains("search worker ended without a result"));
    assert_eq!(bestmove_count(&writer), 0);
}

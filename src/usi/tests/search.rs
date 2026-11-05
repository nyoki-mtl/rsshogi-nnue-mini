use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rsshogi::board;

use super::super::output::write_info;
use super::super::session::UsiSession;
use super::{bestmove_count, output, wait_for_completed_ponder, wait_for_idle};

#[test]
fn session_accepts_u64_max_time_without_panicking() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("go depth 1 movetime 18446744073709551615", &mut writer)
        .expect("u64 maximum movetime should be accepted");
    wait_for_idle(&mut session, &mut writer);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn entering_king_declaration_is_reported_as_win() {
    board::init();
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line(
            "position sfen K+N5+L1/G+L+P+B1+R+P2/3+P2G2/9/2+p+n5/3s2+ss1/3+p+p1+s1+r/7g+n/6g+nk b 2L8Pb4p 1",
            &mut writer,
        )
        .expect("position should succeed");
    session.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut session, &mut writer);
    assert!(output(&writer).lines().any(|line| line.contains("score mate +")));
    assert!(output(&writer).lines().any(|line| line == "bestmove win"));
}

#[test]
fn root_checkmate_reports_loss_before_resign() {
    board::init();
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("position sfen k3r4/9/9/9/9/9/9/4g4/4K4 b - 1", &mut writer)
        .expect("position should succeed");
    session.handle_line("go depth 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut session, &mut writer);

    let lines = output(&writer).lines().collect::<Vec<_>>();
    let bestmove = lines.iter().position(|line| *line == "bestmove resign").expect("bestmove");
    assert!(lines[bestmove - 1].contains("score mate -"));
}

#[test]
fn non_check_no_legal_child_is_a_loss_even_at_qsearch_and_fallback() {
    board::init();
    for command in ["go depth 1 searchmoves 9g9f", "go nodes 1 searchmoves 9g9f"] {
        let mut session = UsiSession::new();
        let mut writer = Vec::new();
        session
            .handle_line("position sfen 8k/6G2/7G1/9/9/9/P8/9/K8 b - 1", &mut writer)
            .expect("position should succeed");
        session.handle_line(command, &mut writer).expect("go should succeed");
        wait_for_idle(&mut session, &mut writer);
        assert!(output(&writer).lines().any(|line| line == "bestmove 9g9f"));
        assert!(output(&writer).lines().any(|line| line.contains("score mate 1")));
    }
}

#[test]
fn stop_publishes_exactly_one_bestmove() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("position startpos", &mut writer).expect("position should succeed");
    session.handle_line("go infinite", &mut writer).expect("go should succeed");
    session.handle_line("stop", &mut writer).expect("stop should succeed");

    assert!(session.search.is_none());
    assert_eq!(bestmove_count(&writer), 1);

    session.handle_line("stop", &mut writer).expect("duplicate stop should succeed");
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn lazy_smp_publishes_only_the_main_worker_result() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("setoption name Threads value 2", &mut writer)
        .expect("Threads should be accepted");
    session.handle_line("go depth 2", &mut writer).expect("go should succeed");
    wait_for_idle(&mut session, &mut writer);

    assert_eq!(session.threads, 2);
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn search_info_reports_hashfull_including_the_final_result() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("setoption name USI_Hash value 1", &mut writer).unwrap();
    session.handle_line("go nodes 5000", &mut writer).unwrap();
    wait_for_idle(&mut session, &mut writer);
    let text = output(&writer);
    let reports = text.lines().filter(|line| line.starts_with("info depth")).collect::<Vec<_>>();
    assert!(!reports.is_empty());
    let mut observed_usage = false;
    for line in reports {
        let words = line.split_whitespace().collect::<Vec<_>>();
        let index =
            words.iter().position(|word| *word == "hashfull").expect("hashfull is reported");
        let value = words[index + 1].parse::<u32>().unwrap();
        assert!(value <= 1000);
        observed_usage |= value > 0;
    }
    assert!(observed_usage);
    let lines = text.lines().collect::<Vec<_>>();
    assert!(lines[lines.len() - 2].contains("hashfull"), "final info precedes bestmove");
    assert!(lines.last().unwrap().starts_with("bestmove"));
}

#[test]
fn completed_search_offers_a_legal_ponder_candidate() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("go depth 2", &mut writer).expect("go should succeed");
    wait_for_idle(&mut session, &mut writer);

    let line = output(&writer)
        .lines()
        .find(|line| line.starts_with("bestmove "))
        .expect("bestmove should be published");
    assert!(line.contains(" ponder "), "depth-two result should offer a ponder move: {line}");
}

/// 時刻付き・時刻なしの両方の`ponderhit`表記で同じ挙動になることを見る。
const PONDERHIT_FORMS: [&str; 2] = ["ponderhit", "ponderhit btime 37082 wtime 23103 byoyomi 10000"];

#[test]
fn ponderhit_releases_a_completed_ponder() {
    for ponderhit in PONDERHIT_FORMS {
        let mut session = UsiSession::new();
        let mut writer = Vec::new();
        session.handle_line("go ponder depth 1", &mut writer).expect("go should succeed");
        wait_for_completed_ponder(&mut session, &mut writer);
        assert_eq!(bestmove_count(&writer), 0, "{ponderhit}");

        session.handle_line(ponderhit, &mut writer).expect("ponderhit should succeed");
        assert!(session.search.is_none(), "{ponderhit}");
        assert_eq!(bestmove_count(&writer), 1, "{ponderhit}");
    }
}

#[test]
fn ponderhit_releases_an_inflight_search_without_cancelling_it() {
    for ponderhit in PONDERHIT_FORMS {
        let mut session = UsiSession::new();
        let mut writer = Vec::new();
        session.handle_line("go ponder infinite", &mut writer).expect("go should succeed");
        let cancel = Arc::clone(&session.search.as_ref().expect("search task").cancel);

        session.handle_line(ponderhit, &mut writer).expect("ponderhit should succeed");

        let task = session.search.as_ref().expect("inflight search must remain owned");
        assert!(!cancel.load(Ordering::SeqCst), "{ponderhit} must not cancel the search");
        assert!(!task.pondering.load(Ordering::SeqCst), "{ponderhit}");
        assert!(!task.defer_bestmove, "{ponderhit}");
        session.handle_line("stop", &mut writer).expect("stop should clean up");
        assert_eq!(bestmove_count(&writer), 1, "{ponderhit}");
    }
}

#[test]
fn completed_infinite_search_waits_for_stop() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("go infinite depth 1", &mut writer).expect("go should succeed");
    wait_for_completed_ponder(&mut session, &mut writer);
    assert_eq!(bestmove_count(&writer), 0);

    session.handle_line("stop", &mut writer).expect("stop should publish");
    assert!(session.search.is_none());
    assert_eq!(bestmove_count(&writer), 1);
}

#[test]
fn final_info_precedes_bestmove_when_no_iteration_completes() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session
        .handle_line("position sfen 4k4/9/9/9/4r4/4P4/9/9/4K4 b - 1", &mut writer)
        .expect("position should succeed");
    session.handle_line("go nodes 1", &mut writer).expect("go should succeed");
    wait_for_idle(&mut session, &mut writer);

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
    write_info(&mut writer, 4, 31_997, 123, Duration::from_millis(10), 0, &[])
        .expect("info should write");

    assert!(output(&writer).contains("score mate 3"));
    assert!(!output(&writer).contains("score cp"));
}

#[test]
fn zero_distance_mate_scores_keep_the_outcome_sign() {
    for (score, expected) in [(32_000, "score mate +"), (-32_000, "score mate -")] {
        let mut writer = Vec::new();
        write_info(&mut writer, 0, score, 0, Duration::ZERO, 0, &[]).expect("info should write");
        assert!(output(&writer).contains(expected));
        assert!(!output(&writer).contains("score mate 0"));
    }
}

#[test]
fn inflight_ponder_can_be_stopped() {
    let mut session = UsiSession::new();
    let mut writer = Vec::new();
    session.handle_line("go ponder infinite", &mut writer).expect("go should succeed");
    assert!(session.search.as_ref().is_some_and(|task| task.completed.is_none()));

    session.handle_line("stop", &mut writer).expect("stop should succeed");
    assert!(session.search.is_none());
    assert_eq!(bestmove_count(&writer), 1);
}

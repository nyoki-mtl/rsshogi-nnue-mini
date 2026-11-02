//! Opt-in, trained-model diagnostics. Move generation and scalar oracles are outside timing.
use super::*;
use crate::nnue::accumulator::AccumulatorStack;
use crate::position::SearchPositionExt;
use rsshogi::board::{self, Move32List, generate_legal_all_move32};
use rsshogi::types::Move32;
use std::hint::black_box;
use std::time::Instant;

enum Op {
    Push(Move32),
    Pop(Move32),
    Eval(i32),
    Null,
    UndoNull,
}
struct Trace {
    sfen: String,
    ops: Vec<Op>,
}
fn random(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state as usize % n
}
fn traces(network: &AonlyNetwork, positions: &[String], eval_rate: usize) -> Vec<Trace> {
    let mut state = 0x0065_2026_1015_u64;
    positions
        .iter()
        .map(|sfen| {
            let mut position = board::position_from_sfen(sfen).expect("benchmark SFEN");
            let mut played = Vec::new();
            let mut ops = Vec::new();
            for _ in 0..1600 {
                let mut moves = Move32List::new();
                generate_legal_all_move32(&position, &mut moves);
                if !played.is_empty()
                    && (played.len() >= 48
                        || moves.as_slice().is_empty()
                        || random(&mut state, 10) < 4)
                {
                    let mv = played.pop().expect("played");
                    position.undo_move32(mv).expect("undo");
                    ops.push(Op::Pop(mv));
                } else if !moves.as_slice().is_empty() {
                    let mv = moves.as_slice()[random(&mut state, moves.as_slice().len())];
                    position.apply_move32(mv);
                    played.push(mv);
                    ops.push(Op::Push(mv));
                }
                if random(&mut state, 100) < eval_rate {
                    ops.push(Op::Eval(network.evaluate(&position).expect("scalar oracle")));
                }
                if random(&mut state, 16) == 0
                    && !position.is_in_check()
                    && position.start_search_null().is_ok()
                {
                    ops.push(Op::Null);
                    ops.push(Op::Eval(network.evaluate(&position).expect("null scalar oracle")));
                    position.finish_search_null().expect("null undo");
                    ops.push(Op::UndoNull);
                }
            }
            Trace { sfen: sfen.clone(), ops }
        })
        .collect()
}
fn replay(network: &AonlyNetwork, traces: &[Trace], verify: bool) -> (u64, String) {
    let mut checksum = 0xcbf2_9ce4_8422_2325_u64;
    let mut stack = AccumulatorStack::default();
    for trace in traces {
        let mut position = board::position_from_sfen(&trace.sfen).expect("trace root");
        stack.reset(network, &position);
        for op in &trace.ops {
            match *op {
                Op::Push(mv) => {
                    position.apply_move32(mv);
                    stack.push(&position, mv);
                }
                Op::Pop(mv) => {
                    position.undo_move32(mv).expect("replay undo");
                    stack.pop();
                }
                Op::Eval(expected) => {
                    let score = stack.evaluate(network, &position);
                    if verify {
                        assert_eq!(score, expected, "trained trace scalar parity");
                    }
                    checksum = (checksum ^ score as u64).wrapping_mul(0x100_0000_01b3);
                }
                Op::Null => {
                    position.start_search_null().expect("replay null");
                }
                Op::UndoNull => {
                    position.finish_search_null().expect("replay null undo");
                }
            }
        }
    }
    (black_box(checksum), format!("{:?}", stack.trace_stats))
}
fn ns_per(n: usize, mut f: impl FnMut(usize)) -> f64 {
    let start = Instant::now();
    for i in 0..n {
        f(i);
    }
    start.elapsed().as_nanos() as f64 / n as f64
}

#[test]
#[ignore = "requires RSSHOGI_TEST_RSNN_PATH and RSSHOGI_BENCH_POSITIONS; run --release --nocapture --test-threads=1"]
fn trained_inference_profile() {
    board::init();
    let model = std::env::var_os("RSSHOGI_TEST_RSNN_PATH").expect("trained package path");
    let network = AonlyNetwork::load(Path::new(&model), None).expect("trained model");
    let book =
        std::fs::read_to_string(std::env::var_os("RSSHOGI_BENCH_POSITIONS").expect("SFEN book"))
            .expect("read book");
    let all_positions: Vec<_> = book.lines().collect();
    let positions: Vec<_> =
        (0..16).map(|i| all_positions[i * all_positions.len() / 16].to_owned()).collect();
    assert_eq!(positions.len(), 16);
    for rate in [25, 75] {
        let trace = traces(&network, &positions, rate);
        let (expected, counters) = replay(&network, &trace, true);
        for repeat in 0..7 {
            let start = Instant::now();
            for _ in 0..8 {
                assert_eq!(replay(&network, &trace, false).0, expected);
            }
            println!(
                "REPLAY rate={rate} repeat={repeat} ns={} checksum={expected} {counters}",
                start.elapsed().as_nanos() / 8
            );
        }
    }
    let mut samples = Vec::new();
    for sfen in &positions {
        let position = board::position_from_sfen(sfen).expect("sample");
        let atoms = position_atoms(&position).expect("atoms");
        let mut columns = [Column::ZERO; 2];
        let mut psqt = [[0; BUCKETS]; 2];
        let mut progress = 0;
        for (p, perspective) in PERSPECTIVES.into_iter().enumerate() {
            progress += network
                .accumulate_checked(
                    &atoms,
                    perspective,
                    position.king_square([Color::BLACK, Color::WHITE][p]).to_index() as u8,
                    &mut columns[p],
                    &mut psqt[p],
                )
                .expect("sample accumulation");
        }
        let bucket = PROGRESS_THRESHOLDS.iter().position(|&t| progress < t).unwrap_or(7);
        let stack = &network.stacks[bucket];
        let mut input = [0; WIDTH];
        simd::pool([&columns[0], &columns[1]], &mut input);
        let mut z0 = [0; HIDDEN];
        simd::dense(&stack.fc0, &stack.fc0_bias, &input, &mut z0);
        let mut x1 = [0; FC1_INPUTS];
        paired_activation(&z0, 7, &mut x1);
        let mut z1 = [0; HIDDEN];
        simd::dense(&stack.fc1, &stack.fc1_bias, &x1, &mut z1);
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            let mut actual = [0; HIDDEN];
            // SAFETY: this test build enables AVX2; the trained reader verified shapes.
            unsafe {
                simd::dense_all(&stack.fc0, &stack.fc0_bias, &input, &mut actual);
            }
            assert_eq!(actual, z0);
            unsafe {
                simd::dense_all(&stack.fc1, &stack.fc1_bias, &x1, &mut actual);
            }
            assert_eq!(actual, z1);
        }
        let mut x2 = [0; FC2_INPUTS];
        x2[..FC1_INPUTS].copy_from_slice(&x1);
        paired_activation(&z1, 6, (&mut x2[FC1_INPUTS..]).try_into().expect("second activation"));
        let nz0 = input.chunks_exact(4).filter(|x| x.iter().any(|&v| v != 0)).count();
        let nz1 = x1.chunks_exact(4).filter(|x| x.iter().any(|&v| v != 0)).count();
        println!("DENSITY bucket={bucket} fc0={nz0}/{} fc1={nz1}/{}", WIDTH / 4, FC1_INPUTS / 4);
        samples.push((columns, psqt, progress, bucket, input, x1, x2, position));
    }
    let n = 200_000;
    for repeat in 0..5 {
        let pool = ns_per(n, |i| {
            let s = black_box(&samples[i % samples.len()]);
            let mut y = [0; WIDTH];
            simd::pool([&s.0[0], &s.0[1]], &mut y);
            black_box(y);
        });
        let fc0 = ns_per(n, |i| {
            let s = black_box(&samples[i % samples.len()]);
            let t = &network.stacks[s.3];
            let mut y = [0; HIDDEN];
            simd::dense(&t.fc0, &t.fc0_bias, &s.4, &mut y);
            black_box(y);
        });
        let fc1 = ns_per(n, |i| {
            let s = black_box(&samples[i % samples.len()]);
            let t = &network.stacks[s.3];
            let mut y = [0; HIDDEN];
            simd::dense(&t.fc1, &t.fc1_bias, &s.5, &mut y);
            black_box(y);
        });
        let fc2 = ns_per(n, |i| {
            let s = black_box(&samples[i % samples.len()]);
            let t = &network.stacks[s.3];
            black_box(simd::dot(&t.fc2, &s.6));
        });
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            // SAFETY: this benchmark is compiled with AVX2 and uses checked tensor shapes.
            let all0 = ns_per(n, |i| {
                let s = black_box(&samples[i % samples.len()]);
                let t = &network.stacks[s.3];
                let mut y = [0; HIDDEN];
                unsafe {
                    simd::dense_all(&t.fc0, &t.fc0_bias, &s.4, &mut y);
                }
                black_box(y);
            });
            let all1 = ns_per(n, |i| {
                let s = black_box(&samples[i % samples.len()]);
                let t = &network.stacks[s.3];
                let mut y = [0; HIDDEN];
                unsafe {
                    simd::dense_all(&t.fc1, &t.fc1_bias, &s.5, &mut y);
                }
                black_box(y);
            });
            println!("DENSE_ALL repeat={repeat} fc0={all0:.2} fc1={all1:.2} ns");
        }
        let output = ns_per(n, |i| {
            let s = black_box(&samples[i % samples.len()]);
            black_box(network.output([&s.0[0], &s.0[1]], [&s.1[0], &s.1[1]], s.2));
        });
        let kp_scan = ns_per(n / 4, |i| {
            let s = black_box(&samples[i % samples.len()]);
            let atoms = position_atoms(&s.7).expect("scan");
            let mut sum = 0;
            for (p, perspective) in PERSPECTIVES.into_iter().enumerate() {
                let king = s.7.king_square([Color::BLACK, Color::WHITE][p]).to_index() as u8;
                sum += atoms
                    .iter()
                    .map(|&a| network.progress_weight(a, perspective, king))
                    .sum::<i64>();
            }
            black_box(sum);
        });
        let ft = ns_per(n, |i| {
            let s = black_box(&samples[i % samples.len()]);
            let mut dst = Column::ZERO;
            simd::update(
                &mut dst,
                &s.0[0],
                &[&network.base_weights[100]],
                &[&network.base_weights[101]],
            );
            black_box(dst);
        });
        println!(
            "LAYERS repeat={repeat} pool={pool:.2} fc0={fc0:.2} fc1={fc1:.2} fc2={fc2:.2} output={output:.2} kp_scan={kp_scan:.2} ft_one={ft:.2} ns"
        );
    }
}

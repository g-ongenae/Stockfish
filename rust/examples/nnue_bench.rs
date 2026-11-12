//! Evaluation throughput, the same measurement as `bench/nnue_bench.cpp`
//! does on the C++ engine (see README.md):
//!
//!     cargo run --release --example nnue_bench -- <network> <fens> <lines>
//!
//! - from scratch: for each position of the FEN file not in check, reset the
//!   accumulator stack and evaluate, as `dump` does (refresh through the
//!   cache, which is kept from one position to the next);
//! - incremental: for each line of the line file (`startpos|fen ... moves
//!   ...`), make the moves with an accumulator push each and evaluate every
//!   position not in check, as search does along a line; the stack is reset
//!   when it is full and the cache cleared at the start of each line.
//!
//! Prints the number of evaluations, the CPU time they took and a checksum of
//! the evaluations, to compare with the C++ output.

use std::io::{BufRead, BufReader};

use stockfish::nnue::{AccumulatorCaches, AccumulatorStack, Network};
use stockfish::position::Position;
use stockfish::uci::{to_move, START_FEN};

/// User plus system CPU time of the process, in seconds (Linux only).
fn cpu_time() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat.rsplit(')').next().unwrap_or("").split_whitespace().collect();
    let ticks = |i: usize| fields.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    // utime and stime are fields 14 and 15, the 12th and 13th after the ')'
    (ticks(11) + ticks(12)) / 100.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: nnue_bench <network> <fens> <lines>");
        std::process::exit(2);
    }
    stockfish::attacks::init();

    let t0 = cpu_time();
    let file = std::fs::File::open(&args[1]).expect("network file");
    let network = Network::load(BufReader::new(file)).expect("a network of this architecture");
    println!("load: {:.2} s", cpu_time() - t0);

    let mut stack = AccumulatorStack::new();
    let mut caches = AccumulatorCaches::new(&network);
    let mut pos = Position::new();

    // From scratch
    let fens: Vec<String> = BufReader::new(std::fs::File::open(&args[2]).expect("fen file"))
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let (mut n, mut sum) = (0u64, 0i64);
    let t0 = cpu_time();
    for fen in &fens {
        if pos.set(fen.as_bytes(), false).is_err() || pos.checkers() != 0 {
            continue;
        }
        stack.reset();
        sum += network.evaluate(&pos, &mut stack, &mut caches) as i64;
        n += 1;
    }
    let t = cpu_time() - t0;
    println!("scratch: {n} evals in {t:.2} s, {:.0} evals/s, checksum {sum}", n as f64 / t);

    // Incremental
    let lines: Vec<String> = BufReader::new(std::fs::File::open(&args[3]).expect("line file"))
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let (mut n, mut sum) = (0u64, 0i64);
    let t0 = cpu_time();
    for line in &lines {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let moves_at = tokens.iter().position(|&t| t == "moves").unwrap_or(tokens.len());
        let fen = if tokens[0] == "startpos" { START_FEN.to_string() } else { tokens[1..moves_at].join(" ") };
        if pos.set(fen.as_bytes(), false).is_err() {
            continue;
        }
        caches.clear(&network);
        stack.reset();
        for token in &tokens[(moves_at + 1).min(tokens.len())..] {
            let m = to_move(&pos, token.as_bytes());
            if m == stockfish::types::Move::none() {
                break;
            }
            if stack.len() == AccumulatorStack::MAX_SIZE {
                stack.reset();
            }
            let gives_check = pos.gives_check(m);
            let dirties = stack.push(&pos);
            pos.do_move_dirties(m, gives_check, dirties);
            if pos.checkers() == 0 {
                sum += network.evaluate(&pos, &mut stack, &mut caches) as i64;
                n += 1;
            }
        }
    }
    let t = cpu_time() - t0;
    println!("incremental: {n} evals in {t:.2} s, {:.0} evals/s, checksum {sum}", n as f64 / t);
}

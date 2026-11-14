//! The deterministic parts of the thread pool, on the port: the same lines as
//! `bench/threads_check.cpp` on the C++ engine (see there and README.md,
//! "Threads"). `threads_check [sizes|engine|best|all]`.

use stockfish::engine::Engine;
use stockfish::history::*;
use stockfish::search::{RootMove, RootMoves, Worker};
use stockfish::thread::{get_best_thread, next_power_of_two};
use stockfish::tt::*;
use stockfish::types::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// uniform in [0, n)
    fn below(&mut self, n: u32) -> u32 {
        (((self.next() >> 32) * n as u64) >> 32) as u32
    }

    /// uniform in [lo, hi]
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u32) as i32
    }
}

fn hex64(v: u64) -> String {
    format!("{v:016X}")
}

/// The digest of the search trace: 64-bit FNV-1a over little-endian words
/// of 4 entries.
struct TraceDigest(u64);

impl TraceDigest {
    fn new() -> Self {
        TraceDigest(0xcbf29ce484222325)
    }

    fn word(&mut self, w: u64) {
        self.0 = (self.0 ^ w).wrapping_mul(0x100000001b3);
    }
}

fn digest(values: impl Iterator<Item = i16>) -> u64 {
    let mut d = TraceDigest::new();
    let (mut w, mut n) = (0u64, 0);
    for v in values {
        w |= (v as u16 as u64) << (16 * n);
        n += 1;
        if n == 4 {
            d.word(w);
            (w, n) = (0, 0);
        }
    }
    assert_eq!(n, 0);
    d.0
}

fn piece_to<const D: i32>(t: &[[AtomicStatsEntry<D>; SQUARE_NB]; PIECE_NB]) -> impl Iterator<Item = i16> + '_ {
    t.iter().flat_map(|r| r.iter().map(|e| e.get()))
}

fn bundles(e: &[CorrectionBundle; COLOR_NB]) -> impl Iterator<Item = i16> + '_ {
    e.iter().flat_map(|b| [b.pawn.get(), b.minor.get(), b.non_pawn_white.get(), b.non_pawn_black.get()])
}

/// The value `dirty()` writes at a flat index.
fn pattern(i: usize, salt: usize) -> i16 {
    ((i * 7 + salt) % 1999) as i16 - 999
}

/// The sizes of the shared histories for N threads, and the part of them
/// each thread of N clears.
fn sizes() {
    for n in [1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 16, 17, 31, 32, 33, 64, 100, 255, 256, 257, 1024, 4096] {
        println!("next_power_of_two {n} {}", next_power_of_two(n));
    }

    for n in 1..=9 {
        let sh = SharedHistories::new(next_power_of_two(n));
        let (mask, pawn_mask) = sh.masks();
        println!(
            "shared {n} size {} pawn {} mask {mask} pawn_mask {pawn_mask}",
            sh.get_size(),
            sh.pawn_history.get_size()
        );
        for t in 0..n {
            sh.correction_history.clear_range(100 + t as i16, t, n);
            sh.pawn_history.clear_range(100 + t as i16, t, n);
        }
        let ranges = |name: &str, size: usize, first: &dyn Fn(usize) -> i16| {
            let mut line = format!("  {name}");
            let mut start = 0;
            for i in 1..=size {
                if i == size || first(i) != first(start) {
                    line += &format!(" {}:{start}-{i}", first(start));
                    start = i;
                }
            }
            println!("{line}");
        };
        ranges("correction", sh.get_size(), &|i| sh.correction_history[i][WHITE as usize].pawn.get());
        ranges("pawn", sh.pawn_history.get_size(), &|i| sh.pawn_history[i][W_PAWN as usize][SQ_A1 as usize].get());
    }
}

fn worker_line(w: &Worker, shared: &SharedHistories) -> String {
    let main = digest(w.main_history.iter().flat_map(|r| r.iter().map(|e| e.get())));
    let capture = digest(w.capture_history.iter().flat_map(|a| a.iter().flat_map(|b| b.iter().map(|e| e.get()))));
    let cch_tables = w.continuation_correction_history.tables();
    let cch = digest(cch_tables.iter().flat_map(|t| t.iter().flat_map(|r| r.iter().map(|e| e.get()))));
    format!(
        "  worker {} numa {}/{} main {} shared {} main_history {} capture_history {} \
         continuation_correction_history {} ttmove_history {}",
        w.thread_idx,
        w.numa_thread_idx,
        w.numa_total,
        w.manager.is_some() as i32,
        std::ptr::eq(&*w.shared_history, shared) as i32,
        hex64(main),
        hex64(capture),
        hex64(cch),
        w.tt_move_history.get()
    )
}

fn print_state(engine: &Engine, when: &str) {
    println!("{when} threads {}", engine.threads.size());

    let shared = engine.shared_histories();
    let mut pawn = TraceDigest::new();
    for i in 0..shared.pawn_history.get_size() {
        pawn.word(digest(piece_to(&shared.pawn_history[i])));
    }
    let mut correction = TraceDigest::new();
    for i in 0..shared.correction_history.get_size() {
        correction.word(digest(bundles(&shared.correction_history[i])));
    }
    let mut continuation = TraceDigest::new();
    for t in shared.continuation_history_block.tables().chunks(CONTINUATION_HISTORY_SIZE) {
        continuation.word(digest(t.iter().flat_map(piece_to)));
    }
    println!(
        "  shared size {} pawn {} pawn_history {} correction_history {} continuation_history {}",
        shared.get_size(),
        shared.pawn_history.get_size(),
        hex64(pawn.0),
        hex64(correction.0),
        hex64(continuation.0)
    );

    let mut lines = Vec::new();
    engine.threads.for_each_worker(|w| lines.push(worker_line(w, shared)));
    for l in lines {
        println!("{l}");
    }

    let w = engine.threads.main_worker();
    let mm = w.manager.as_deref().unwrap();
    println!(
        "  manager {} {} {} {} {} {}",
        mm.best_previous_average_score,
        mm.previous_time_reduction,
        mm.calls_cnt,
        mm.best_previous_score,
        mm.original_time_adjust,
        mm.tm.available_nodes()
    );
    drop(w);

    let tt = engine.tt();
    println!(
        "  tt clusters {} generation {} nonzero {} hashfull {}",
        tt.cluster_count(),
        tt.generation(),
        tt.nonzero_bytes(),
        tt.hashfull(0)
    );
}

/// Garbage in every table the clear of the pool writes, and in the
/// transposition table, then a ucinewgame.
fn dirty_and_clear(engine: &mut Engine, rng: &mut Rng) {
    {
        let shared = engine.shared_histories();
        for i in 0..shared.pawn_history.get_size() {
            for (j, e) in shared.pawn_history[i].iter().flatten().enumerate() {
                e.set(pattern(j, 1 + i));
            }
        }
        for i in 0..shared.correction_history.get_size() {
            for (j, b) in shared.correction_history[i].iter().enumerate() {
                for (k, e) in [&b.pawn, &b.minor, &b.non_pawn_white, &b.non_pawn_black].into_iter().enumerate() {
                    e.set(pattern(4 * j + k, 2 + i));
                }
            }
        }
        let entries = shared.continuation_history_block.tables().iter().flat_map(|t| t.iter().flatten());
        for (j, e) in entries.enumerate() {
            e.set(pattern(j, 3));
        }
    }
    engine.threads.for_each_worker(|w| {
        let idx = w.thread_idx;
        for (j, e) in w.main_history.iter_mut().flatten().enumerate() {
            e.set(pattern(j, 4 + idx));
        }
        for (j, e) in w.capture_history.iter_mut().flatten().flatten().enumerate() {
            e.set(pattern(j, 5 + idx));
        }
        for (j, e) in w.continuation_correction_history.tables_mut().iter_mut().flatten().flatten().enumerate() {
            e.set(pattern(j, 6 + idx));
        }
        w.tt_move_history = TTMoveHistory::new(idx as i16 + 7);
    });
    {
        let mut w = engine.threads.main_worker();
        let mm = w.manager.as_deref_mut().unwrap();
        mm.best_previous_average_score = 11;
        mm.previous_time_reduction = 1.5;
        mm.calls_cnt = 12;
        mm.best_previous_score = 13;
        mm.original_time_adjust = 2.5;
    }

    let tt = engine.tt();
    for _ in 0..3 {
        tt.new_search();
    }
    for _ in 0..5000 {
        let key = rng.next();
        let (_, _, writer) = tt.probe(key);
        let v = rng.range(-500, 500);
        let pv = rng.below(2) != 0;
        let b = 1 + rng.below(3) as Bound;
        let d = rng.range(-6, 60);
        let m = Move(rng.below(65536) as u16);
        let ev = rng.range(-500, 500);
        tt.write(writer, key, v, pv, b, d, m, ev, tt.generation());
    }
    print_state(engine, "dirty");
    engine.search_clear();
    print_state(engine, "ucinewgame");
}

fn engine_states() {
    let mut engine = Engine::new();
    let mut rng = Rng(42);
    print_state(&engine, "start");
    dirty_and_clear(&mut engine, &mut rng);
    for (threads, hash) in [(2, 16), (3, 5), (4, 1), (8, 3), (5, 2), (1, 16), (7, 1), (1, 2)] {
        engine.setoption("Threads", &threads.to_string());
        print_state(&engine, "threads");
        engine.setoption("Hash", &hash.to_string());
        print_state(&engine, "hash");
        dirty_and_clear(&mut engine, &mut rng);
        engine.setoption("Clear Hash", "");
        print_state(&engine, "clear_hash");
    }
}

/// get_best_thread() on synthetic root moves, the same as the C++ driver's.
fn best_thread() {
    let mut rng = Rng(7);
    // e2e4, d2d4, g1f3, c2c4, b1c3
    let moves = [
        Move::new(12, 28),
        Move::new(11, 27),
        Move::new(6, 21),
        Move::new(10, 26),
        Move::new(1, 18),
    ];
    let mut digest: u64 = 0;
    let mut cases = 0;
    for n in [1, 2, 3, 4, 5, 6, 7, 8, 13, 32] {
        for c in 0..4000 {
            let mut line = format!("best {n}");
            let kind = rng.range(0, 3);
            let mut threads: Vec<RootMoves> = Vec::with_capacity(n);
            for _ in 0..n {
                let mut rm = RootMove::new(moves[rng.below(if kind == 3 { 2 } else { 5 }) as usize]);
                let len = rng.range(1, 6);
                for _ in 1..len {
                    rm.pv.push(moves[rng.below(5) as usize]);
                }
                let s = rng.range(0, 9);
                rm.score = match s {
                    0 => -VALUE_INFINITE,
                    1 => VALUE_MATE - rng.range(1, 30),
                    2 => -VALUE_MATE + rng.range(1, 30),
                    3 => VALUE_TB - rng.range(1, 60),
                    4 => -VALUE_TB + rng.range(1, 60),
                    _ if s == 5 || kind == 1 => rng.range(-20, 20),
                    _ => rng.range(-2000, 2000),
                };
                let x = rng.range(0, 5);
                rm.inexact_lower = x == 0;
                rm.inexact_upper = x == 1;
                line += &format!(" {}/{}/{}/{x}", rm.pv[0].0, rm.score, rm.pv.len());
                threads.push(vec![rm]);
            }
            let refs: Vec<&RootMoves> = threads.iter().collect();
            line += &format!(" -> {}", get_best_thread(&refs));
            for b in line.bytes() {
                digest = (digest ^ b as u64).wrapping_mul(0x100000001b3);
            }
            if c < 25 {
                println!("{line}");
            }
            cases += 1;
        }
    }
    println!("best cases {cases} digest {}", hex64(digest));
}

fn main() {
    stockfish::attacks::init();
    let what = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    if what == "sizes" || what == "all" {
        sizes();
    }
    if what == "engine" || what == "all" {
        engine_states();
    }
    if what == "best" || what == "all" {
        best_thread();
    }
}

//! The port's side of `bench/tm_check.cpp`: drives the time manager through
//! the same clocks, in the same order, and prints the same lines. See that
//! file, and README.md ("Time management").
//!
//!     tm_check [lines|digest] [doubles|raw]
//!
//! `raw` (the port only) appends to each line, after the bits of
//! originalTimeAdjust, the bits of the two doubles the optimum and the maximum
//! are truncated from: what the comparison between targets measures.

use std::fmt::Write as _;
use std::io::{self, Write};

use stockfish::search::LimitsType;
use stockfish::timeman::{TimeManagement, TmOptions};
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
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// about log-uniform in [lo, hi], lo >= 1
    fn logu(&mut self, lo: i64, hi: i64) -> i64 {
        let bits_lo = 64 - (lo as u64).leading_zeros() as i32;
        let bits_hi = 64 - (hi as u64).leading_zeros() as i32;
        let bits = bits_lo + self.below((bits_hi - bits_lo + 1) as u64) as i32;
        let v = self.below(1u64 << bits) as i64;
        v.clamp(lo, hi)
    }
}

#[derive(Default)]
struct Digest(u64);

impl Digest {
    fn new() -> Digest {
        Digest(0xCBF29CE484222325)
    }

    fn add(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100000001B3);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Extra {
    None,
    Doubles,
    Raw,
}

struct Out {
    digest: bool,
    extra: Extra,
    block: Digest,
    section: Digest,
    lines: u64,
    section_lines: u64,
    w: io::BufWriter<io::StdoutLock<'static>>,
}

impl Out {
    fn line(&mut self, s: &str) {
        if !self.digest {
            self.w.write_all(s.as_bytes()).unwrap();
            return;
        }
        self.block.add(s);
        self.section.add(s);
        self.lines += 1;
        self.section_lines += 1;
        if self.lines % 4096 == 0 {
            writeln!(self.w, "block {} {:016X}", self.lines / 4096, self.block.0).unwrap();
            self.block = Digest::new();
        }
    }

    fn end_section(&mut self, name: &str) {
        if self.digest {
            writeln!(self.w, "section {} {} lines {:016X}", name, self.section_lines, self.section.0).unwrap();
        }
        self.section = Digest::new();
        self.section_lines = 0;
    }

    /// One call of init(), printed: the inputs, then what it computes.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        tm: &mut TimeManagement,
        limits: &mut LimitsType,
        us: Color,
        ply: i32,
        npmsec: i32,
        overhead: i32,
        ponder: bool,
        adjust: &mut f64,
    ) {
        let mut line = format!(
            "{} {} {} {} {} {} {} {} {} {} {}",
            us,
            ply,
            limits.time[WHITE as usize],
            limits.time[BLACK as usize],
            limits.inc[WHITE as usize],
            limits.inc[BLACK as usize],
            limits.movestogo,
            limits.movetime,
            npmsec,
            overhead,
            ponder as i32
        );

        let options = TmOptions { nodestime: npmsec, move_overhead: overhead, ponder };
        let (opt, max) = tm.init_raw(limits, us, ply, options, adjust);

        let u = us as usize;
        write!(
            line,
            " : {} {} {} {} {} {} {}",
            tm.optimum(),
            tm.maximum(),
            limits.time[u],
            limits.inc[u],
            limits.npmsec,
            limits.movetime,
            tm.available_nodes()
        )
        .unwrap();
        if self.extra != Extra::None {
            write!(line, " {:016X}", adjust.to_bits()).unwrap();
        }
        if self.extra == Extra::Raw {
            write!(line, " {:016X} {:016X}", opt.to_bits(), max.to_bits()).unwrap();
        }
        line.push('\n');
        self.line(&line);
    }
}

fn make_limits(wtime: i64, btime: i64, winc: i64, binc: i64, movestogo: i32, movetime: i64) -> LimitsType {
    let mut limits = LimitsType::default();
    limits.time[WHITE as usize] = wtime;
    limits.time[BLACK as usize] = btime;
    limits.inc[WHITE as usize] = winc;
    limits.inc[BLACK as usize] = binc;
    limits.movestogo = movestogo;
    limits.movetime = movetime;
    limits.start_time = 0;
    limits
}

fn limits_for(us: Color, t: i64, o: i64, inc: i64, oinc: i64, mtg: i32, movetime: i64) -> LimitsType {
    if us == WHITE {
        make_limits(t, o, inc, oinc, mtg, movetime)
    } else {
        make_limits(o, t, oinc, inc, mtg, movetime)
    }
}

fn grid(out: &mut Out) {
    const TIMES: [i64; 20] = [
        1, 2, 5, 10, 30, 99, 100, 250, 500, 999, 1000, 1500, 3000, 10000, 30000, 60000, 180000, 600000, 3600000,
        36000000,
    ];
    const INCS: [i64; 5] = [0, 1, 100, 1000, 10000];
    const MOVESTOGO: [i32; 6] = [0, 1, 2, 10, 40, 60];
    const PLIES: [i32; 6] = [0, 1, 10, 40, 100, 300];
    const OVERHEADS: [i32; 3] = [0, 10, 1000];
    const NPMSECS: [i32; 3] = [0, 1, 600];
    let mut k = 0;

    for t in TIMES {
        for inc in INCS {
            for mtg in MOVESTOGO {
                for ply in PLIES {
                    for ov in OVERHEADS {
                        for np in NPMSECS {
                            for pd in 0..2 {
                                // the opponent's clock: equal, half, double, none
                                let opp = [t, t / 2, 2 * t, 0];
                                let us = (k % 2) as Color;
                                let o = opp[(k / 2) % 4];
                                k += 1;
                                let mut tm = TimeManagement::default();
                                let mut adjust = -1.0;
                                let mut limits = limits_for(us, t, o, inc, inc, mtg, 0);
                                out.call(&mut tm, &mut limits, us, ply, np, ov, pd == 1, &mut adjust);
                            }
                        }
                    }
                }
            }
        }
    }
    out.end_section("grid");
}

fn random_calls(out: &mut Out, rng: &mut Rng, count: i32) {
    const NPMSECS: [i32; 8] = [0, 0, 0, 1, 7, 100, 1000, 10000];
    for _ in 0..count {
        let us = rng.below(2) as Color;
        let t = rng.logu(1, 100000000);
        let o = if rng.below(8) == 0 { 0 } else { rng.logu(1, 100000000) };
        let inc = if rng.below(3) == 0 { 0 } else { rng.logu(1, 100000) };
        let oinc = if rng.below(2) == 0 { inc } else { rng.logu(1, 100000) };
        let mtg = if rng.below(2) == 0 { 0 } else { rng.logu(1, 200) as i32 };
        let ply = rng.below(600) as i32;
        let ov = if rng.below(4) == 0 { 10 } else { rng.below(5001) as i32 };
        let np = NPMSECS[rng.below(8) as usize];
        let pd = rng.below(2) == 1;
        // the previous originalTimeAdjust: none yet, or one of a game
        let mut adjust = if rng.below(2) == 0 { -1.0 } else { (rng.below(4000) as i64 - 500) as f64 / 1000.0 };
        // the nodes left from an earlier move, in 'nodes as time' mode
        let avail = if rng.below(2) == 0 { -1 } else { rng.logu(1, 1i64 << 40) };
        let prev_mtg = rng.below(60) as i32;

        let mut tm = TimeManagement::default();
        let cyclic = if rng.below(2) == 0 { 0 } else { rng.logu(1, 1i64 << 36) };
        tm.set_nodes_time_state(avail, prev_mtg, cyclic);
        let mut limits = limits_for(us, t, o, inc, oinc, mtg, 0);
        out.call(&mut tm, &mut limits, us, ply, np, ov, pd, &mut adjust);
    }
    out.end_section("random");
}

fn games(out: &mut Out, rng: &mut Rng, count: i32) {
    const NPMSECS: [i32; 5] = [0, 0, 1, 50, 1000];
    let mut tm = TimeManagement::default();
    for _ in 0..count {
        // ucinewgame: ThreadPool::clear()
        tm.clear();
        let mut adjust = -1.0;

        let us = rng.below(2) as Color;
        let base = rng.logu(50, 10800000);
        let inc = if rng.below(3) == 0 { 0 } else { rng.logu(1, base / 10 + 1) };
        let cycle = if rng.below(3) == 0 { rng.logu(1, 60) as i32 } else { 0 };
        let ov = if rng.below(3) == 0 { 10 } else { rng.below(1001) as i32 };
        let np = NPMSECS[rng.below(5) as usize];
        let pd = rng.below(4) == 0;
        let moves = 1 + rng.below(120) as i32;
        let mut ply = rng.below(2) as i32 + if us == BLACK { 1 } else { 0 };
        let mut mine = base;
        let mut theirs = base;
        let mut togo = cycle;

        for _ in 0..moves {
            let mut limits = limits_for(us, mine, theirs, inc, inc, togo, 0);
            out.call(&mut tm, &mut limits, us, ply, np, ov, pd, &mut adjust);

            // the time (or nodes) used by this move, up to the maximum
            let cap = if tm.maximum() < (1i64 << 40) { tm.maximum() } else { 1i64 << 40 };
            let mut used = if cap > 0 { rng.below(cap as u64 + 1) as i64 } else { 0 };
            if np != 0 {
                // Worker::start_searching(): the nodes searched minus the
                // increment, in nodes (init() has converted it)
                tm.advance_nodes_time(used - limits.inc[us as usize]);
                used /= np as i64;
            }
            mine -= used;
            if mine <= 0 {
                mine = 1; // the GUI's clock, as parse_limits() sanitizes it
            }
            mine += inc;
            let their_use = rng.below((theirs / 20 + 1) as u64) as i64;
            theirs = if theirs - their_use > 0 { theirs - their_use + inc } else { 1 + inc };
            ply += 2;
            if cycle != 0 {
                togo -= 1;
                if togo == 0 {
                    togo = cycle;
                    mine += base;
                    theirs += base;
                }
            }
            if rng.below(50) == 0 {
                tm.clear();
                adjust = -1.0;
            }
        }
    }
    out.end_section("games");
}

fn edges(out: &mut Out) {
    // wtime, btime, winc, binc, movestogo, movetime, ply, npmsec, overhead,
    // ponder: see bench/tm_check.cpp
    type E = (i64, i64, i64, i64, i32, i64, i32, i32, i32, bool);
    const BIG: i64 = i64::MAX;
    const CASES: [E; 43] = [
        // time below the overhead, and at it
        (5, 5, 0, 0, 0, 0, 0, 0, 10, false),
        (10, 10, 0, 0, 0, 0, 0, 0, 10, false),
        (11, 11, 0, 0, 0, 0, 20, 0, 10, false),
        (100, 100, 0, 0, 0, 0, 20, 0, 5000, false),
        (100, 100, 50, 50, 0, 0, 20, 0, 5000, true),
        (4999, 4999, 0, 0, 0, 0, 20, 0, 5000, false),
        (5000, 5000, 0, 0, 0, 0, 20, 0, 5000, false),
        (5001, 5001, 0, 0, 0, 0, 20, 0, 5000, false),
        (100, 100, 0, 0, 3, 0, 20, 0, 1000, false),
        (100, 100, 0, 0, 3, 0, 20, 10, 1000, false),
        // 1 ms, the sanitized negative time
        (1, 1, 0, 0, 0, 0, 0, 0, 10, false),
        (1, 1, 0, 0, 0, 0, 0, 0, 0, false),
        (1, 1, 0, 0, 1, 0, 0, 0, 0, false),
        (1, 1000, 0, 0, 0, 0, 0, 0, 0, false),
        (1, 1, 0, 0, 0, 0, 0, 10000, 5000, true),
        // 1 move to go, and more
        (1000, 1000, 0, 0, 1, 0, 30, 0, 10, false),
        (60000, 1000, 0, 0, 1, 0, 30, 0, 10, false),
        (60000, 120000, 0, 0, 2, 0, 30, 0, 10, false),
        (60000, 60000, 1000, 1000, 1, 0, 30, 0, 10, false),
        (60000, 60000, 0, 0, 50, 0, 30, 0, 10, false),
        (60000, 60000, 0, 0, 51, 0, 30, 0, 10, false),
        (60000, 60000, 0, 0, 1000000, 0, 30, 0, 10, false),
        // negative increments
        (60000, 60000, -100, -100, 0, 0, 30, 0, 10, false),
        (600, 600, -1000, -1000, 0, 0, 30, 0, 10, false),
        (60000, 60000, -100, -100, 10, 0, 30, 100, 10, false),
        // a clock for the side not to move only (time[us] == 0)
        (0, 60000, 0, 1000, 0, 0, 30, 0, 10, false),
        (0, 60000, 0, 1000, 0, 5000, 30, 100, 10, false),
        // movetime with nodestime, and no clock
        (0, 0, 0, 0, 0, 1000, 0, 1000, 10, false),
        (0, 0, 0, 0, 0, 7, 0, 10000, 10, false),
        // plies far into a game
        (60000, 60000, 0, 0, 0, 0, 1000, 0, 10, false),
        (60000, 60000, 0, 0, 0, 0, 5000, 0, 10, false),
        (60000, 60000, 0, 0, 10, 0, 5000, 0, 10, false),
        // clocks of years, and of millions of years: out of the range of a
        // TimePoint once scaled (undefined in C++, INT64_MIN on x86-64)
        (31536000000, 31536000000, 0, 0, 0, 0, 10, 0, 10, false),
        (31536000000, 31536000000, 0, 0, 0, 0, 10, 10000, 10, false),
        (4000000000000000000, 4000000000000000000, 0, 0, 0, 0, 0, 0, 10, false),
        (4000000000000000000, 4000000000000000000, 0, 0, 0, 0, 0, 0, 10, true),
        (BIG, BIG, 0, 0, 0, 0, 0, 0, 10, false),
        (BIG, BIG, 0, 0, 0, 0, 0, 0, 0, true),
        (BIG, 1, 0, 0, 0, 0, 0, 0, 0, false),
        (BIG, BIG, 0, 0, 40, 0, 0, 0, 10, false),
        // with a ply of thousands (a FEN may say so) the optimum exceeds it
        (BIG, BIG, 0, 0, 0, 0, 5000, 0, 0, false),
        (BIG, BIG, 0, 0, 0, 0, 5000, 0, 10, true),
        (8000000000000000000, 1, 0, 0, 0, 0, 3000, 0, 10, false),
    ];
    for (wtime, btime, winc, binc, movestogo, movetime, ply, npmsec, overhead, ponder) in CASES {
        for c in 0..2 {
            let us = c as Color;
            let mut tm = TimeManagement::default();
            let mut adjust = -1.0;
            let mut limits = if us == WHITE {
                make_limits(wtime, btime, winc, binc, movestogo, movetime)
            } else {
                make_limits(btime, wtime, binc, winc, movestogo, movetime)
            };
            out.call(&mut tm, &mut limits, us, ply + c, npmsec, overhead, ponder, &mut adjust);
        }
    }
    out.end_section("edges");
}

fn main() {
    stockfish::misc::set_double_precision();
    let mut digest = false;
    let mut extra = Extra::None;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "digest" => digest = true,
            "doubles" => extra = Extra::Doubles,
            "raw" => extra = Extra::Raw,
            "lines" => {}
            _ => {
                eprintln!("usage: tm_check [lines|digest] [doubles|raw]");
                std::process::exit(2);
            }
        }
    }

    let mut out = Out {
        digest,
        extra,
        block: Digest::new(),
        section: Digest::new(),
        lines: 0,
        section_lines: 0,
        w: io::BufWriter::new(io::stdout().lock()),
    };
    let mut rng = Rng(0x7130E5);
    grid(&mut out);
    random_calls(&mut out, &mut rng, 100000);
    games(&mut out, &mut rng, 3000);
    edges(&mut out);
    out.w.flush().unwrap();
}

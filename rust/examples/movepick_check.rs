//! The port's side of `bench/movepick_check.cpp`: drives the move picker,
//! `Position::legal()` and the history tables where the `dump` of
//! tests/differential cannot (TT moves, ProbCut, `skip_quiet_moves()`, plies
//! above 0, non-zero histories, the update operator), and prints digests of
//! what they do. The C++ driver does exactly the same with the C++ code, and
//! the two must print exactly the same lines (see README.md):
//!
//!     cargo run --release --example movepick_check -- check <fens> <chess960 0|1> <legal every N> [verbose index]
//!         (verbose index -2 - i: position i of the coarse pass)
//!     cargo run --release --example movepick_check -- bench <fens> <chess960 0|1> <repetitions>
//!
//! Everything random comes from one splitmix64 stream, consumed in the same
//! order on both sides; the tables are filled with values that depend on the
//! position of each entry in the C++ layout, so that a wrong index reads a
//! wrong value. Keep the two drivers in step.
//!
//! It also shows the shape of the API for search: the picker is given the
//! position and a fresh `Histories` at each call, and between two calls the
//! position makes and unmakes a move and the tables are written to.

use std::io::{BufRead, BufReader, Write};

use stockfish::attacks::attacks_bb;
use stockfish::history::*;
use stockfish::movegen::MoveList;
use stockfish::movepick::{Histories, MovePicker};
use stockfish::position::Position;
use stockfish::types::*;
use stockfish::uci::move_to_string;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        mix(self.0)
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

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// The value of the entry of flat index i of table t, in [-d, d]
fn fill_value(t: u64, i: u64, d: i32) -> i16 {
    ((mix((t << 40) ^ i ^ 0x5851F42D4C957F2D) % (2 * d + 1) as u64) as i32 - d) as i16
}

/// The same rounded to a multiple of 1000 (toward zero), for the coarse pass:
/// sums of such values often fall exactly on the thresholds of the picker
/// (-14000 for good quiets), and tie often, which tests the sort on ties
fn coarse_value(t: u64, i: u64, d: i32) -> i16 {
    fill_value(t, i, d) / 1000 * 1000
}

struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Digest(0xCBF29CE484222325)
    }
    fn add(&mut self, v: i64) {
        self.0 = (self.0 ^ v as u64).wrapping_mul(0x100000001B3);
        self.0 ^= self.0 >> 32;
    }
}

/// A bonus for the update operator: mostly within a few D, sometimes huge
fn random_bonus(rng: &mut Rng, d: i32) -> i32 {
    match rng.below(8) {
        0 => (rng.next() >> 32) as u32 as i32, // any int
        1 => rng.range(-d / 8, d / 8),
        _ => rng.range(-3 * d, 3 * d),
    }
}

/// An entry of any table, for `Tables::for_each`.
trait Entry {
    fn value(&self) -> i16;
    fn assign(&mut self, v: i16);
}

impl<const D: i32> Entry for StatsEntry<D> {
    fn value(&self) -> i16 {
        self.get()
    }
    fn assign(&mut self, v: i16) {
        self.set(v)
    }
}

impl<const D: i32> Entry for AtomicStatsEntry<D> {
    fn value(&self) -> i16 {
        self.get()
    }
    fn assign(&mut self, v: i16) {
        self.set(v)
    }
}

struct Tables {
    main_history: Box<ButterflyHistory>,
    low_ply_history: Box<LowPlyHistory>,
    capture_history: Box<CapturePieceToHistory>,
    cont_corr_history: ContinuationCorrectionHistory,
    shared: SharedHistories,
    tt_move_history: TTMoveHistory,
}

/// The flat index of `continuationHistory[ic][cap][pc][s]`.
fn cont(ic: u32, cap: u32, pc: u32, s: u32) -> usize {
    ContinuationHistoryBlock::index(ic != 0, cap as usize, pc as Piece, s as Square)
}

impl Tables {
    fn new(threads: usize) -> Self {
        Tables {
            main_history: new_butterfly_history(),
            low_ply_history: new_low_ply_history(),
            capture_history: new_capture_piece_to_history(),
            cont_corr_history: ContinuationCorrectionHistory::new(),
            shared: SharedHistories::new(threads),
            tt_move_history: TTMoveHistory::new(0),
        }
    }

    /// The tables lent to one call of `next_move()`.
    fn histories(&self, cont_hist: &[usize; 6]) -> Histories<'_> {
        let block = &self.shared.continuation_history_block;
        Histories {
            main_history: &self.main_history,
            low_ply_history: &self.low_ply_history,
            capture_history: &self.capture_history,
            continuation_history: cont_hist.map(|i| block.at(i)),
            shared_history: &self.shared,
        }
    }

    /// Every entry, in the order of the C++ layout, given to f with its table
    /// number, flat index and bound
    // the loops mirror the nested C++ loops of the C++ driver, index by index
    #[allow(clippy::needless_range_loop)]
    fn for_each(&mut self, f: &mut dyn FnMut(u64, u64, i32, &mut dyn Entry)) {
        let mut i = 0;
        for c in 0..COLOR_NB {
            for m in 0..UINT_16_HISTORY_SIZE {
                f(1, i, 7183, &mut self.main_history[c][m]);
                i += 1;
            }
        }
        i = 0;
        for p in 0..LOW_PLY_HISTORY_SIZE {
            for m in 0..UINT_16_HISTORY_SIZE {
                f(2, i, 7183, &mut self.low_ply_history[p][m]);
                i += 1;
            }
        }
        i = 0;
        for pc in 0..PIECE_NB {
            for s in 0..SQUARE_NB {
                for t in 0..PIECE_TYPE_NB {
                    f(3, i, 10692, &mut self.capture_history[pc][s][t]);
                    i += 1;
                }
            }
        }
        i = 0;
        for ic in 0..2 {
            for cap in 0..2 {
                // through the C++ ContinuationHistory of [ic][cap]
                let ch = self.shared.continuation_history_block.continuation_history_mut(ic != 0, cap);
                for pc in 0..PIECE_NB {
                    for s in 0..SQUARE_NB {
                        for pc2 in 0..PIECE_NB {
                            for s2 in 0..SQUARE_NB {
                                f(4, i, 30000, &mut ch[pc * SQUARE_NB + s][pc2][s2]);
                                i += 1;
                            }
                        }
                    }
                }
            }
        }
        i = 0;
        for k in 0..self.shared.pawn_history.get_size() {
            for pc in 0..PIECE_NB {
                for s in 0..SQUARE_NB {
                    f(5, i, 8192, &mut self.shared.pawn_history[k][pc][s]);
                    i += 1;
                }
            }
        }
        i = 0;
        for k in 0..self.shared.correction_history.get_size() {
            for c in 0..COLOR_NB {
                let b = &mut self.shared.correction_history[k][c];
                f(6, i, 1024, &mut b.pawn);
                f(6, i + 1, 1024, &mut b.minor);
                f(6, i + 2, 1024, &mut b.non_pawn_white);
                f(6, i + 3, 1024, &mut b.non_pawn_black);
                i += 4;
            }
        }
        i = 0;
        for pc in 0..PIECE_NB {
            for s in 0..SQUARE_NB {
                let t = self.cont_corr_history.at_mut(ContinuationCorrectionHistory::index(pc as Piece, s as Square));
                for pc2 in 0..PIECE_NB {
                    for s2 in 0..SQUARE_NB {
                        f(7, i, 1024, &mut t[pc2][s2]);
                        i += 1;
                    }
                }
            }
        }
        f(8, 0, 8192, &mut self.tt_move_history);
    }

    fn fill(&mut self) {
        self.for_each(&mut |t, i, d, e| e.assign(fill_value(t, i, d)));
    }

    fn fill_coarse(&mut self) {
        self.for_each(&mut |t, i, d, e| e.assign(coarse_value(t, i, d)));
    }

    /// One digest per table, in order
    fn digests(&mut self) -> String {
        let mut dg: Vec<Digest> = (0..9).map(|_| Digest::new()).collect();
        self.for_each(&mut |t, _, _, e| dg[t as usize].add(e.value() as i64));
        dg[1..].iter().fold(String::new(), |out, d| out + &format!(" {:016X}", d.0))
    }

    /// Random updates of random entries of every table, by update()
    fn random_updates(&mut self, rng: &mut Rng, n: i32) {
        for _ in 0..n {
            let c = rng.below(COLOR_NB as u32) as usize;
            let m = rng.below(UINT_16_HISTORY_SIZE as u32) as usize;
            self.main_history[c][m].update(random_bonus(rng, 7183));
            let p = rng.below(LOW_PLY_HISTORY_SIZE as u32) as usize;
            let m = rng.below(UINT_16_HISTORY_SIZE as u32) as usize;
            self.low_ply_history[p][m].update(random_bonus(rng, 7183));
            let pc = rng.below(PIECE_NB as u32) as usize;
            let s = rng.below(SQUARE_NB as u32) as usize;
            let t = rng.below(PIECE_TYPE_NB as u32) as usize;
            self.capture_history[pc][s][t].update(random_bonus(rng, 10692));
            let ic = rng.below(2);
            let cap = rng.below(2);
            let pc1 = rng.below(PIECE_NB as u32);
            let s1 = rng.below(SQUARE_NB as u32);
            let pc2 = rng.below(PIECE_NB as u32) as usize;
            let s2 = rng.below(SQUARE_NB as u32) as usize;
            self.shared.continuation_history_block.at(cont(ic, cap, pc1, s1))[pc2][s2].update(random_bonus(rng, 30000));
            let kk = rng.below(self.shared.pawn_history.get_size() as u32) as usize;
            let pc = rng.below(PIECE_NB as u32) as usize;
            let s = rng.below(SQUARE_NB as u32) as usize;
            self.shared.pawn_history[kk][pc][s].update(random_bonus(rng, 8192));
            let kk = rng.below(self.shared.correction_history.get_size() as u32) as usize;
            let c = rng.below(COLOR_NB as u32) as usize;
            let b = &self.shared.correction_history[kk][c];
            match rng.below(4) {
                0 => b.pawn.update(random_bonus(rng, 1024)),
                1 => b.minor.update(random_bonus(rng, 1024)),
                2 => b.non_pawn_white.update(random_bonus(rng, 1024)),
                _ => b.non_pawn_black.update(random_bonus(rng, 1024)),
            }
            let pc1 = rng.below(PIECE_NB as u32);
            let s1 = rng.below(SQUARE_NB as u32);
            let pc2 = rng.below(PIECE_NB as u32) as usize;
            let s2 = rng.below(SQUARE_NB as u32) as usize;
            self.cont_corr_history.at_mut(ContinuationCorrectionHistory::index(pc1 as Piece, s1 as Square))[pc2][s2]
                .update(random_bonus(rng, 1024));
            self.tt_move_history.update(random_bonus(rng, 8192));
        }
    }
}

/// The update operator on every value of [-D, D] and a set of bonuses
fn update_digest(d: i32, rng: &mut Rng, update: impl Fn(i16, i32) -> i16) -> u64 {
    let mut bonuses = vec![
        i32::MIN,
        i32::MIN + 1,
        -1000000000,
        -3 * d,
        -d - 1,
        -d,
        -d + 1,
        -d / 2,
        -1000,
        -100,
        -7,
        -1,
        0,
        1,
        7,
        100,
        1000,
        d / 2,
        d - 1,
        d,
        d + 1,
        3 * d,
        1000000000,
        i32::MAX,
    ];
    for _ in 0..16 {
        bonuses.push(random_bonus(rng, d));
    }
    let mut dg = Digest::new();
    for v in -d..=d {
        for &b in &bonuses {
            dg.add(update(v as i16, b) as i64);
        }
    }
    dg.0
}

/// A pseudo-legal but illegal move of a piece other than a pawn (pinned, or
/// not answering a check, or a king move to an attacked square), found from a
/// random start; none if there is none.
fn pseudo_legal_illegal(pos: &Position, rng: &mut Rng) -> Move {
    let start = rng.below(4096);
    for k in 0..4096 {
        let v = (start + k) & 4095;
        let (from, to) = ((v >> 6) as Square, (v & 63) as Square);
        let pc = pos.piece_on(from);
        if pc == NO_PIECE || color_of(pc) != pos.side_to_move() || type_of(pc) == PAWN {
            continue;
        }
        if pos.pieces_c(pos.side_to_move()) & square_bb(to) != 0
            || attacks_bb(type_of(pc), from, pos.pieces()) & square_bb(to) == 0
        {
            continue;
        }
        let m = Move::new(from, to);
        if !pos.legal(m) {
            return m;
        }
    }
    Move::none()
}

fn square_bb(s: Square) -> Bitboard {
    1u64 << s
}

struct Checker<'a> {
    t: &'a mut Tables,
    chess960: bool,
    verbose: bool,
    /// A picker reused through renew() by the odd runs, across positions, so
    /// its move list holds what earlier runs left there (C++ constructs a new
    /// picker every time: the output must not depend on it)
    reused: Option<Box<MovePicker>>,
}

impl Checker<'_> {
    fn log(&self, s: &str) {
        if self.verbose {
            eprintln!("{s}");
        }
    }

    /// Runs one picker to its end, making and unmaking each move and updating
    /// the histories as search does between two calls, with
    /// skip_quiet_moves() called before the move of index skip_at (never if
    /// negative).
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        pos: &mut Position,
        mp: &mut MovePicker,
        cont_hist: &[usize; 6],
        ply: i32,
        skip_at: i32,
        rng: &mut Rng,
        dg: &mut Digest,
    ) {
        let mut n = 0;
        loop {
            if n == skip_at {
                mp.skip_quiet_moves();
                self.log("  skip_quiets");
            }
            let m = mp.next_move(pos, &self.t.histories(cont_hist));
            if m == Move::none() {
                break;
            }
            let tt = mp.trace_is_tt_move();
            let score = if tt { 0 } else { mp.trace_score() };
            dg.add(m.raw() as i64);
            dg.add(tt as i64);
            dg.add(score as i64);
            for c in mp.trace_stage().bytes() {
                dg.add(c as i64);
            }
            if self.verbose {
                let s = if tt { "tt".to_string() } else { score.to_string() };
                self.log(&format!("  {} {} {}", move_to_string(m, self.chess960), mp.trace_stage(), s));
            }
            n += 1;

            pos.do_move(m);
            pos.undo_move(m);

            // History updates between two calls, as search makes them
            if rng.below(2) != 0 {
                let t = &mut *self.t;
                let us = pos.side_to_move() as usize;
                let pc = pos.moved_piece(m);
                let to = m.to_sq();
                let (pci, toi) = (pc as usize, to as usize);
                t.main_history[us][m.raw() as usize].update(random_bonus(rng, 7183));
                if ply < LOW_PLY_HISTORY_SIZE as i32 {
                    t.low_ply_history[ply as usize][m.raw() as usize].update(random_bonus(rng, 7183));
                }
                t.capture_history[pci][toi][type_of(pos.piece_on(to)) as usize].update(random_bonus(rng, 10692));
                // search writes through the same tables as it gives the picker
                let ch = cont_hist[rng.below(6) as usize];
                t.shared.continuation_history_block.at(ch)[pci][toi].update(random_bonus(rng, 30000));
                t.shared.pawn_entry(pos)[pci][toi].update(random_bonus(rng, 8192));
            }
        }
        dg.add(n as i64);
    }

    fn check(&mut self, fen: &str, rng: &mut Rng, all_legal: bool) -> String {
        let mut pos = Position::new();
        if pos.set(fen.as_bytes(), self.chess960).is_err() {
            return "error".to_string();
        }

        let mut dg = Digest::new();
        let mut out = String::new();

        // legal() on every 16-bit value
        if all_legal {
            let mut ld = Digest::new();
            let mut count = 0;
            for v in 0..=u16::MAX {
                if pos.legal(Move(v)) {
                    ld.add(v as i64);
                    count += 1;
                }
            }
            out += &format!(" legal {count} {:016X}", ld.0);
        }

        // The entries of the shared histories for this position
        let us = pos.side_to_move() as usize;
        {
            let sh = &self.t.shared;
            sh.pawn_correction_entry(&pos)[us].pawn.update(random_bonus(rng, 1024));
            sh.minor_piece_correction_entry(&pos)[us].minor.update(random_bonus(rng, 1024));
            sh.nonpawn_correction_entry::<WHITE>(&pos)[us].non_pawn_white.update(random_bonus(rng, 1024));
            sh.nonpawn_correction_entry::<BLACK>(&pos)[us].non_pawn_black.update(random_bonus(rng, 1024));
            dg.add(sh.pawn_correction_entry(&pos)[us].pawn.get() as i64);
            dg.add(sh.minor_piece_correction_entry(&pos)[us].minor.get() as i64);
            dg.add(sh.nonpawn_correction_entry::<WHITE>(&pos)[us].non_pawn_white.get() as i64);
            dg.add(sh.nonpawn_correction_entry::<BLACK>(&pos)[us].non_pawn_black.get() as i64);
            let ppc = rng.below(PIECE_NB as u32) as usize;
            let pto = rng.below(SQUARE_NB as u32) as usize;
            dg.add(sh.pawn_entry(&pos)[ppc][pto].get() as i64);
        }

        let legal_moves = MoveList::legal(&pos);
        let n_legal = legal_moves.len() as u32;

        for r in 0..10 {
            let mut cont_hist = [0usize; 6];
            for ch in cont_hist.iter_mut() {
                if rng.below(8) == 0 {
                    *ch = cont(0, 0, NO_PIECE as u32, 0); // the sentinel of search
                    continue;
                }
                let ic = rng.below(2);
                let cap = rng.below(2);
                let pc = rng.below(PIECE_NB as u32);
                let s = rng.below(SQUARE_NB as u32);
                *ch = cont(ic, cap, pc, s);
            }
            // 0 none, 1 legal, 2 pseudo-legal illegal, 3 garbage
            let ttm = match r % 4 {
                1 => {
                    if n_legal != 0 {
                        legal_moves.as_slice()[rng.below(n_legal) as usize]
                    } else {
                        Move::none()
                    }
                }
                2 => pseudo_legal_illegal(&pos, rng),
                3 => Move(rng.below(65536) as u16),
                _ => Move::none(),
            };
            let ply = rng.range(0, 7);
            let skip_at = if rng.below(3) != 0 { -1 } else { rng.range(0, 40) };

            if r < 6 {
                // main search, or quiescence search (depth 0) for r = 4, 5
                // depths 1 to 3 half of the time: below 4 the quiet moves
                // between -3560 * depth and -14000 are left unsorted
                let depth = if r >= 4 {
                    DEPTH_QS
                } else if rng.below(2) != 0 {
                    rng.range(1, 3)
                } else {
                    rng.range(4, 20)
                };
                self.log(&format!(
                    "main depth {depth} ply {ply} tt {} skip {skip_at}",
                    move_to_string(ttm, self.chess960)
                ));
                if r % 2 == 1 {
                    let mut mp = self.reused.take().expect("picker");
                    mp.renew(&pos, ttm, depth, ply);
                    self.run(&mut pos, &mut mp, &cont_hist, ply, skip_at, rng, &mut dg);
                    self.reused = Some(mp);
                } else {
                    let mut mp = MovePicker::new(&pos, ttm, depth, ply);
                    self.run(&mut pos, &mut mp, &cont_hist, ply, skip_at, rng, &mut dg);
                }
            } else if pos.checkers() == 0 {
                let threshold = rng.range(-600, 1600);
                self.log(&format!(
                    "probcut threshold {threshold} tt {} skip {skip_at}",
                    move_to_string(ttm, self.chess960)
                ));
                if r % 2 == 1 {
                    let mut mp = self.reused.take().expect("picker");
                    mp.renew_probcut(&pos, ttm, threshold);
                    self.run(&mut pos, &mut mp, &cont_hist, ply, skip_at, rng, &mut dg);
                    self.reused = Some(mp);
                } else {
                    let mut mp = MovePicker::new_probcut(&pos, ttm, threshold);
                    self.run(&mut pos, &mut mp, &cont_hist, ply, skip_at, rng, &mut dg);
                }
            }
        }

        out + &format!(" {:016X}", dg.0)
    }
}

/// User plus system CPU time of the process, in seconds (Linux only).
fn cpu_time() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat.rsplit(')').next().unwrap_or("").split_whitespace().collect();
    let ticks = |i: usize| fields.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    // utime and stime are fields 14 and 15, the 12th and 13th after the ')'
    (ticks(11) + ticks(12)) / 100.0
}

/// The same work as the bench of the C++ driver: every move of the main
/// picker at depths 1, 8 and 16 and of the quiescence picker, or only the
/// first three (as a node with an early cutoff would), on each position,
/// `reps` times per round, best of 5 rounds. Then the same with one picker
/// reused through `renew()`, which the C++ driver does not do (C++ never
/// clears the move list).
fn bench(t: &Tables, fens: &[String], chess960: bool, reps: u32) {
    let mut positions = Vec::new();
    for fen in fens {
        let mut pos = Position::new();
        if pos.set(fen.as_bytes(), chess960).is_ok() {
            positions.push(pos);
        }
    }
    let cont_hist: [usize; 6] =
        std::array::from_fn(|i| cont(i as u32 & 1, (i as u32 >> 1) & 1, W_KNIGHT as u32 + i as u32, 20 + i as u32));

    let mut reused = Box::new(MovePicker::new(&positions[0], Move::none(), 1, 0));
    for (first_only, renew) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut best = f64::MAX;
        let (mut sum, mut picks) = (0u64, 0u64);
        for _ in 0..5 {
            sum = 0;
            picks = 0;
            let t0 = cpu_time();
            for _ in 0..reps {
                for pos in &positions {
                    for depth in [1, 8, 16, 0] {
                        let mut fresh;
                        let mp: &mut MovePicker = if renew {
                            reused.renew(pos, Move::none(), depth, 2);
                            &mut reused
                        } else {
                            fresh = MovePicker::new(pos, Move::none(), depth, 2);
                            &mut fresh
                        };
                        let mut n = 0;
                        loop {
                            let m = mp.next_move(pos, &t.histories(&cont_hist));
                            if m == Move::none() {
                                break;
                            }
                            sum = sum.wrapping_mul(31).wrapping_add(m.raw() as u64);
                            picks += 1;
                            n += 1;
                            if first_only && n == 3 {
                                break;
                            }
                        }
                    }
                }
            }
            best = best.min(cpu_time() - t0);
        }
        let pickers = positions.len() as u64 * 4 * reps as u64;
        println!(
            "{}: {pickers} pickers, {picks} moves in {best:.3} s (best of 5), {:.0} pickers/s, checksum {sum:016X}",
            match (first_only, renew) {
                (false, false) => "all moves",
                (true, false) => "first 3 moves",
                (false, true) => "all moves, renew",
                (true, true) => "first 3 moves, renew",
            },
            pickers as f64 / best
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: movepick_check check|bench <fens> <chess960> <n> [verbose index]");
        std::process::exit(2);
    }
    stockfish::attacks::init();

    let mode = args[1].as_str();
    let fens: Vec<String> = BufReader::new(std::fs::File::open(&args[2]).expect("FEN file"))
        .lines()
        .map(|l| l.expect("FEN file"))
        .filter(|l| !l.trim_matches([' ', '\t', '\r']).is_empty())
        .collect();
    let chess960 = args[3] == "1";
    let n: i64 = args[4].parse().expect("n");

    let mut t = Tables::new(1);
    t.fill();

    if mode == "bench" {
        bench(&t, &fens, chess960, n as u32);
        return;
    }

    let mut rng = Rng(0x2545F4914F6CDD1D);
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    let _ = writeln!(out, "fill{}", t.digests());

    let u = [
        update_digest(7183, &mut rng, |v, b| {
            let mut e = StatsEntry::<7183>::new(v);
            e.update(b);
            e.get()
        }),
        update_digest(10692, &mut rng, |v, b| {
            let mut e = StatsEntry::<10692>::new(v);
            e.update(b);
            e.get()
        }),
        update_digest(30000, &mut rng, |v, b| {
            let e = AtomicStatsEntry::<30000>::new(v);
            e.update(b);
            e.get()
        }),
        update_digest(8192, &mut rng, |v, b| {
            let e = AtomicStatsEntry::<8192>::new(v);
            e.update(b);
            e.get()
        }),
        update_digest(1024, &mut rng, |v, b| {
            let e = AtomicStatsEntry::<1024>::new(v);
            e.update(b);
            e.get()
        }),
        update_digest(1024, &mut rng, |v, b| {
            let mut e = StatsEntry::<1024>::new(v);
            e.update(b);
            e.get()
        }),
        update_digest(8192, &mut rng, |v, b| {
            let mut e = TTMoveHistory::new(v);
            e.update(b);
            e.get()
        }),
    ];
    let _ = writeln!(out, "update {:x} {:x} {:x} {:x} {:x} {:x} {:x}", u[0], u[1], u[2], u[3], u[4], u[5], u[6]);

    t.random_updates(&mut rng, 200000);
    let _ = writeln!(out, "updated{}", t.digests());

    let verbose_index: i64 = args.get(5).map_or(-1, |v| v.parse().expect("index"));
    let mut checker = Checker {
        t: &mut t,
        chess960,
        verbose: false,
        reused: Some(Box::new(MovePicker::new(&Position::new(), Move::none(), 1, 0))),
    };
    for (i, fen) in fens.iter().enumerate() {
        checker.verbose = i as i64 == verbose_index;
        let line = checker.check(fen, &mut rng, n > 0 && i as i64 % n == 0);
        let _ = writeln!(out, "{i}{line}");
    }

    let _ = writeln!(out, "end{}", checker.t.digests());

    // The coarse pass: the same on every position again, with coarse tables
    checker.t.fill_coarse();
    let _ = writeln!(out, "coarse{}", checker.t.digests());
    for (i, fen) in fens.iter().enumerate() {
        checker.verbose = i as i64 == -2 - verbose_index;
        let line = checker.check(fen, &mut rng, false);
        let _ = writeln!(out, "c{i}{line}");
    }
    let _ = writeln!(out, "end{}", checker.t.digests());
}

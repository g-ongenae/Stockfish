//! `src/timeman.{h,cpp}`: the time manager. It computes the optimum and
//! maximum time of a move from the clock (`go wtime/btime/winc/binc/
//! movestogo`), the ply of the game and the options `Move Overhead`,
//! `nodestime` and `Ponder`, statement for statement as C++ does.
//!
//! The doubles are computed in the order and with the operations of C++, so
//! that the integers they are truncated to are those of the C++ engine on
//! x86-64; see README.md, "Time management", "Floating point", for what is
//! established about it and on which targets.

use std::sync::OnceLock;
use std::time::Instant;

use crate::search::{LimitsType, SearchOptions};
use crate::types::*;

/// Milliseconds, `TimePoint` in C++.
pub type TimePoint = i64;

/// `now()`: milliseconds since an arbitrary epoch (the first call).
pub fn now() -> TimePoint {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as TimePoint
}

const NO_BOUND: TimePoint = i64::MAX / 2;

/// `std::min` of two doubles: `(b < a) ? b : a`.
#[inline]
pub fn min_f64(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max` of two doubles: `(a < b) ? b : a`.
#[inline]
pub fn max_f64(a: f64, b: f64) -> f64 {
    if a < b {
        b
    } else {
        a
    }
}

/// `std::clamp` of doubles: `v < lo ? lo : hi < v ? hi : v`.
#[inline]
pub fn clamp_f64(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// `TimePoint(x)` for a double, as the C++ engine computes it on x86-64
/// (`cvttsd2si`): truncation toward zero, and `i64::MIN` for a value out of
/// range (undefined behaviour in C++; reached only with clocks of millions of
/// years). Rust's `as` would saturate instead.
#[inline]
pub fn to_timepoint(x: f64) -> TimePoint {
    if (-9.223_372_036_854_775_808e18..9.223_372_036_854_775_808e18).contains(&x) {
        x as TimePoint
    } else {
        i64::MIN
    }
}

/// The options `init()` reads (`options[...]` in C++).
#[derive(Clone, Copy, Debug)]
pub struct TmOptions {
    pub nodestime: i32,
    pub move_overhead: i32,
    pub ponder: bool,
}

impl TmOptions {
    pub fn of(options: &SearchOptions) -> TmOptions {
        TmOptions { nodestime: options.nodestime, move_overhead: options.move_overhead, ponder: options.ponder }
    }
}

/// `TimeManagement`: computes the optimal time to think depending on the
/// maximum available time, the game move number, and other parameters.
pub struct TimeManagement {
    start_time: TimePoint,
    optimum_time: TimePoint,
    maximum_time: TimePoint,

    // Related to 'nodes as time' mode:
    use_nodes_time: bool,
    available_nodes: i64,
    previous_moves_to_go: i32,
    cyclic_budget: i64,
}

impl Default for TimeManagement {
    fn default() -> Self {
        TimeManagement {
            start_time: 0,
            optimum_time: NO_BOUND,
            maximum_time: NO_BOUND,
            use_nodes_time: false,
            available_nodes: -1,
            previous_moves_to_go: 0,
            cyclic_budget: 0,
        }
    }
}

impl TimeManagement {
    pub fn optimum(&self) -> TimePoint {
        self.optimum_time
    }

    pub fn maximum(&self) -> TimePoint {
        self.maximum_time
    }

    /// `elapsed(nodes)`: the nodes searched in 'nodes as time' mode, else the
    /// time since the start of the search.
    #[inline]
    pub fn elapsed(&self, nodes: u64) -> TimePoint {
        if self.use_nodes_time {
            nodes as TimePoint
        } else {
            self.elapsed_time()
        }
    }

    pub fn elapsed_time(&self) -> TimePoint {
        now() - self.start_time
    }

    pub fn clear(&mut self) {
        self.available_nodes = -1; // When in 'nodes as time' mode
        self.previous_moves_to_go = 0;
    }

    pub fn advance_nodes_time(&mut self, nodes: i64) {
        debug_assert!(self.use_nodes_time);
        self.available_nodes = 0.max(self.available_nodes.wrapping_sub(nodes));
    }

    /// The nodes left in 'nodes as time' mode (-1 before the first move of a
    /// game), for the drivers that compare this module with C++.
    pub fn available_nodes(&self) -> i64 {
        self.available_nodes
    }

    /// Sets the state of 'nodes as time' mode that `init()` reads (the nodes
    /// left, the movestogo of the previous call, the cyclic budget), for the
    /// drivers that compare this module with C++.
    pub fn set_nodes_time_state(&mut self, available_nodes: i64, previous_moves_to_go: i32, cyclic_budget: i64) {
        self.available_nodes = available_nodes;
        self.previous_moves_to_go = previous_moves_to_go;
        self.cyclic_budget = cyclic_budget;
    }

    /// `init()`: called at the beginning of the search and calculates the
    /// bounds of time allowed for the current game ply. We currently
    /// support:
    ///      1) x basetime (+ z increment)
    ///      2) x moves in y seconds (+ z increment)
    ///
    /// The integer arithmetic wraps where the C++ one would overflow (with
    /// absurd clocks only), as the C++ engine does in practice.
    pub fn init(
        &mut self,
        limits: &mut LimitsType,
        us: Color,
        ply: i32,
        options: TmOptions,
        original_time_adjust: &mut f64,
    ) {
        self.init_raw(limits, us, ply, options, original_time_adjust);
    }

    /// `init()`, which also returns the two doubles that the optimum and the
    /// maximum time are truncated from (`optScale * timeLeft` and the
    /// argument of the maximum's `TimePoint()`; NaN when there is no clock),
    /// for the driver that measures how they vary between targets
    /// (`examples/tm_check.rs raw`).
    #[inline]
    pub fn init_raw(
        &mut self,
        limits: &mut LimitsType,
        us: Color,
        ply: i32,
        options: TmOptions,
        original_time_adjust: &mut f64,
    ) -> (f64, f64) {
        let us = us as usize;
        let them = us ^ 1;
        let npmsec = options.nodestime as TimePoint;

        // If we have no time, we don't need to fully initialize TM.
        // startTime is used by movetime and useNodesTime is used in elapsed
        // calls.
        self.start_time = limits.start_time;
        self.use_nodes_time = npmsec != 0;

        if self.use_nodes_time {
            limits.movetime = limits.movetime.wrapping_mul(npmsec);
        }

        if limits.time[us] == 0 {
            self.optimum_time = NO_BOUND;
            self.maximum_time = NO_BOUND;
            return (f64::NAN, f64::NAN);
        }

        let mut move_overhead = options.move_overhead as TimePoint;

        // optScale is a percentage of available time to use for the current
        // move. maxScale is a multiplier applied to optimumTime.
        let mut opt_scale: f64;
        let max_scale: f64;

        // If we have to play in 'nodes as time' mode, then convert from time
        // to nodes, and use resulting values in time management formulas.
        // WARNING: to avoid time losses, the given npmsec (nodes per
        // millisecond) must be much lower than the real engine speed.
        if self.use_nodes_time {
            if self.available_nodes == -1 {
                // Only once at game start
                // First time limit includes increment (both are in
                // milliseconds)
                self.available_nodes = npmsec.wrapping_mul(limits.time[us]);
                self.cyclic_budget = npmsec.wrapping_mul(limits.time[us].wrapping_sub(limits.inc[us]));
            } else if limits.movestogo > 0 && limits.movestogo > self.previous_moves_to_go && self.cyclic_budget > 0 {
                self.available_nodes = self.available_nodes.wrapping_add(self.cyclic_budget);
            }

            self.previous_moves_to_go = limits.movestogo;

            // Convert from milliseconds to nodes
            limits.time[us] = self.available_nodes;
            limits.inc[us] = limits.inc[us].wrapping_mul(npmsec);
            limits.npmsec = npmsec;
            move_overhead *= npmsec;
        }

        // These numbers are used where multiplications, divisions, or
        // comparisons with constants are involved.
        let scale_factor: i64 = if self.use_nodes_time { npmsec } else { 1 };
        let scaled_time: TimePoint = 1.max(limits.time[us].wrapping_div(scale_factor));

        // Maximum move horizon
        let mut mtg: i32 = if limits.movestogo != 0 { limits.movestogo.min(50) } else { 50 };

        // If less than one second, gradually reduce mtg. In cyclic time
        // controls we keep the actual movestogo as horizon.
        if scaled_time < 1000 && limits.movestogo == 0 {
            mtg = (scaled_time as f64 * 0.05) as i32;
        }

        // Make sure timeLeft is > 0 since we may use it as a divisor
        let time_left: TimePoint = 1.max(
            limits.time[us]
                .wrapping_add(limits.inc[us].wrapping_mul((mtg - 1) as i64))
                .wrapping_sub(move_overhead.wrapping_mul((2 + mtg) as i64)),
        );

        let time_us = limits.time[us] as f64;

        // x basetime (+ z increment)
        // If there is a healthy increment, timeLeft can exceed the actual
        // available game time for the current move, so also cap to a
        // percentage of available game time.
        if limits.movestogo == 0 {
            // Extra time according to timeLeft
            if *original_time_adjust < 0.0 {
                *original_time_adjust = 0.3272 * (time_left as f64).log10() - 0.4141;
            }

            // Calculate time constants based on current time left.
            let log_time_in_sec = (scaled_time as f64 / 1000.0).log10();
            let opt_constant = min_f64(0.0029869 + 0.00033554 * log_time_in_sec, 0.004905);
            let max_constant = max_f64(3.3744 + 3.0608 * log_time_in_sec, 3.1441);

            opt_scale = min_f64(
                0.012112 + (ply as f64 + 3.22713).powf(0.46866) * opt_constant,
                0.19404 * time_us / time_left as f64,
            ) * *original_time_adjust;

            max_scale = min_f64(6.873, max_constant + ply as f64 / 12.352);
        }
        // x moves in y seconds (+ z increment)
        else {
            opt_scale = min_f64((0.88 + ply as f64 / 116.4) / mtg as f64, 0.88 * time_us / time_left as f64);
            max_scale = 1.3 + 0.11 * mtg as f64;
        }

        // Decrease time usage if behind in time.
        // This is skipped in two cases:
        // - if the nodestime option is used we can't calculate the opponent
        //   nodes budget in a deterministic way.
        // - if we use a cyclic time management (like 40/10) calculating time
        //   advantage for the last move (movestogo = 1) can be vastly off,
        //   because if the opponent had done his last move before us his time
        //   budget includes already the next cycle time increment but our
        //   not. This leads to a unnecessary big decrease in time usage which
        //   favors blunders.
        // Warning: don't remove this conditions.
        if !self.use_nodes_time && limits.movestogo != 1 {
            let time_advantage = limits.time[us].wrapping_sub(limits.time[them]) as f64
                / (1.0 + time_us + limits.time[them] as f64);
            opt_scale *= 1.0 + 0.9 * min_f64(time_advantage, 0.0);
        }

        // Limit the maximum possible time for this move
        let optimum = max_f64(1.0, opt_scale * time_left as f64);
        self.optimum_time = to_timepoint(optimum);
        let maximum = max_f64(
            self.optimum_time as f64,
            min_f64(0.8097 * time_us - move_overhead as f64, max_scale * self.optimum_time as f64),
        );
        self.maximum_time = to_timepoint(maximum);

        if options.ponder {
            self.optimum_time = self.optimum_time.wrapping_add(self.optimum_time / 4);
        }

        (optimum, maximum)
    }
}

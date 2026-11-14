//! The part of `src/timeman.{h,cpp}` that a search without a clock needs:
//! the start time, the elapsed time (or nodes, with `nodestime`), and the
//! bounds left unbounded. Time management proper (`go wtime/btime`) is not
//! ported yet: its doubles and its decisions belong to a later layer, and the
//! engine refuses a search with a clock rather than guess.

use std::sync::OnceLock;
use std::time::Instant;

use crate::search::LimitsType;
use crate::types::*;

/// Milliseconds, `TimePoint` in C++.
pub type TimePoint = i64;

/// `now()`: milliseconds since an arbitrary epoch (the first call).
pub fn now() -> TimePoint {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as TimePoint
}

const NO_BOUND: TimePoint = i64::MAX / 2;

pub struct TimeManagement {
    start_time: TimePoint,
    optimum_time: TimePoint,
    maximum_time: TimePoint,
    use_nodes_time: bool,
    available_nodes: i64,
    previous_moves_to_go: i32,
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
        self.available_nodes = (self.available_nodes - nodes).max(0);
    }

    /// `init()` up to the point where C++ returns for a search without a
    /// clock (`limits.time[us] == 0`), which is all the engine runs.
    pub fn init(&mut self, limits: &mut LimitsType, us: Color, _ply: i32, nodestime: i32) {
        let npmsec = nodestime as TimePoint;

        // If we have no time, we don't need to fully initialize TM.
        // startTime is used by movetime and useNodesTime is used in elapsed
        // calls.
        self.start_time = limits.start_time;
        self.use_nodes_time = npmsec != 0;

        if self.use_nodes_time {
            limits.movetime *= npmsec;
        }

        if limits.time[us as usize] == 0 {
            self.optimum_time = NO_BOUND;
            self.maximum_time = NO_BOUND;
            return;
        }

        unreachable!("time management is not ported: the engine refuses a search with a clock");
    }
}

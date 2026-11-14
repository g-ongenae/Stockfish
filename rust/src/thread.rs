//! The part of `src/thread.{h,cpp}` that one search thread needs.
//!
//! C++ keeps a pool of threads that sleep between searches, each owning a
//! `Search::Worker`. The port has one worker, and runs each search on a
//! thread created for it with a stack of `TH_STACK_SIZE` (8 MiB, as C++ asks
//! for its threads): the search recurses to `MAX_PLY`, and the default stack
//! of a spawned Rust thread (2 MiB) or of a musl thread (128 KiB) is not
//! meant for that. The worker moves to that thread for the search and comes
//! back when it is joined. `go` returns at once, as in C++, so that `stop`
//! and `isready` are read during a search.
//!
//! The seams the thread layer will need are here: the flags the threads
//! share (`stop`, `increaseDepth`, `ponder`), `clear()`, `start_thinking()`,
//! `wait_for_search_finished()` and `nodes_searched()`.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::movegen::MoveList;
use crate::position::Position;
use crate::search::{LimitsType, RootMove, SearchOptions, TbConfig, Worker};
use crate::types::*;
use crate::uci;

/// The stack size of the search threads, as `TH_STACK_SIZE` in C++
/// (`thread_native.h`).
pub const TH_STACK_SIZE: usize = 8 * 1024 * 1024;

/// What the threads of the pool share: `ThreadPool::stop`,
/// `ThreadPool::increaseDepth` and `SearchManager::ponder`.
#[derive(Default)]
pub struct SharedFlags {
    pub stop: AtomicBool,
    pub increase_depth: AtomicBool,
    pub ponder: AtomicBool,
}

pub struct ThreadPool {
    pub flags: Arc<SharedFlags>,
    /// The worker when no search runs
    worker: Option<Box<Worker>>,
    /// The thread of the search that runs, which gives the worker back
    handle: Option<JoinHandle<Box<Worker>>>,
}

impl ThreadPool {
    pub fn new(worker: Box<Worker>, flags: Arc<SharedFlags>) -> ThreadPool {
        ThreadPool { flags, worker: Some(worker), handle: None }
    }

    /// `wait_for_search_finished()`: waits for the search to end, if one
    /// runs.
    pub fn wait_for_search_finished(&mut self) {
        if let Some(h) = self.handle.take() {
            match h.join() {
                Ok(w) => self.worker = Some(w),
                Err(e) => std::panic::resume_unwind(e),
            }
        }
    }

    /// The main worker, once its search has finished.
    pub fn main_worker(&mut self) -> &mut Worker {
        self.wait_for_search_finished();
        self.worker.as_deref_mut().unwrap()
    }

    /// `clear()`: resets the histories of the workers and the main manager's
    /// data, as before a new game.
    pub fn clear(&mut self) {
        let w = self.main_worker();
        w.clear();

        let mm = w.manager.as_deref_mut().unwrap();
        // These two affect the time taken on the first move of a game:
        mm.best_previous_average_score = VALUE_INFINITE;
        mm.previous_time_reduction = 0.85;

        mm.calls_cnt = 0;
        mm.best_previous_score = VALUE_INFINITE;
        mm.original_time_adjust = -1.0;
        mm.tm.clear();
    }

    /// `nodes_searched()`: the nodes of every thread, here of the one
    /// worker. Only when no search runs.
    pub fn nodes_searched(&mut self) -> u64 {
        self.main_worker().nodes
    }

    /// `start_thinking()`: sets up the worker for a search of `pos` and
    /// starts it on its thread. `trace` runs the instance of the search with
    /// the trace hooks, for `searchtrace`.
    pub fn start_thinking(&mut self, pos: &Position, limits: LimitsType, options: SearchOptions, trace: bool) {
        self.wait_for_search_finished();

        self.flags.stop.store(false, Relaxed);
        self.flags.ponder.store(limits.ponder_mode, Relaxed);
        self.flags.increase_depth.store(true, Relaxed);
        let mut w = self.worker.take().unwrap();
        w.manager.as_deref_mut().unwrap().stop_on_ponderhit = false;

        let mut root_moves = Vec::new();
        for uci_move in &limits.searchmoves {
            let m = uci::to_move(pos, uci_move.as_bytes());
            if m != Move::none() {
                root_moves.push(RootMove::new(m));
            }
        }

        if root_moves.is_empty() {
            for &m in &MoveList::legal(pos) {
                root_moves.push(RootMove::new(m));
            }
        }

        // Tablebases::rank_root_moves() without tablebases: no root move is
        // ranked, and no probing in search (cardinality 0)
        let tb_config =
            TbConfig { cardinality: 0, root_in_tb: false, use_rule50: options.syzygy_50_move_rule, probe_depth: 0 };

        w.limits = limits;
        w.nodes = 0;
        w.tb_hits = 0;
        w.best_move_changes = 0;
        w.nmp_min_ply = 0;
        w.root_depth = 0;
        w.root_moves = root_moves;
        // C++ sets the root position from the FEN of `pos` and copies its
        // last state, which links to the states of the moves before it: the
        // same position with the same history, which is what a copy is.
        w.root_pos.clone_from(pos);
        w.tb_config = tb_config;
        w.search_options = options;

        let handle = std::thread::Builder::new()
            .name("search".into())
            .stack_size(TH_STACK_SIZE)
            .spawn(move || {
                if trace {
                    w.start_searching::<true>();
                } else {
                    w.start_searching::<false>();
                }
                w
            })
            .expect("cannot create the search thread");
        self.handle = Some(handle);
    }
}

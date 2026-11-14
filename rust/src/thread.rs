//! Port of `src/thread.{h,cpp}`: the pool of search threads.
//!
//! As in C++, each `Thread` is a native thread created with the pool and
//! parked in `idle_loop()` between jobs; it owns a `Search::Worker`. The
//! main worker (thread 0) starts the others when its search starts, sets
//! `stop` and waits for them when it ends, then chooses the best thread
//! (`get_best_thread()`). Every thread runs on a stack of `TH_STACK_SIZE`
//! (8 MiB, as C++ asks for its threads): the search recurses to `MAX_PLY`,
//! and the default stack of a spawned Rust thread (2 MiB) or of a musl
//! thread (128 KiB) is not meant for that.
//!
//! The worker of a thread is behind a mutex that its job holds for the
//! whole job (a search, a clear): the pool reaches a worker only when its
//! thread is idle, and the main worker reaches the helpers' workers only
//! after waiting for them. What the threads read of each other during a
//! search is in `SharedFlags`: the flags and every worker's counters, all
//! atomic. See README.md, "Threads".

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::history::SharedHistories;
use crate::misc::{BestMoveChanges, RelaxedCounter};
use crate::movegen::MoveList;
use crate::nnue::Network;
use crate::position::Position;
use crate::search::{LimitsType, RootMove, RootMoves, SearchManager, SearchOptions, TbConfig, UpdateContext, Worker};
use crate::tt::TranspositionTable;
use crate::types::*;
use crate::uci;

/// The stack size of the search threads, as `TH_STACK_SIZE` in C++
/// (`thread_native.h`).
pub const TH_STACK_SIZE: usize = 8 * 1024 * 1024;

/// The counters of a worker that the main thread reads during the search:
/// C++ keeps them in the worker as `RelaxedAtomic<u64>`. Each worker has
/// its own, on a cache line of its own (C++ has them inside a worker of
/// several MiB, so no two workers' counters share a line there either).
#[repr(align(64))]
#[derive(Default)]
pub struct ThreadCounters {
    pub nodes: RelaxedCounter,
    pub tb_hits: RelaxedCounter,
    pub best_move_changes: BestMoveChanges,
}

/// What the threads of the pool share during a search: `ThreadPool::stop`,
/// `ThreadPool::increaseDepth`, `SearchManager::ponder`, and the counters of
/// every worker, by thread index (what `ThreadPool::accumulate()` reads).
/// Made anew with the threads, by `ThreadPool::set()`.
pub struct SharedFlags {
    pub stop: AtomicBool,
    pub increase_depth: AtomicBool,
    pub ponder: AtomicBool,
    pub counters: Box<[Arc<ThreadCounters>]>,
}

impl SharedFlags {
    pub fn new(threads: usize) -> SharedFlags {
        SharedFlags {
            stop: AtomicBool::new(false),
            increase_depth: AtomicBool::new(false),
            ponder: AtomicBool::new(false),
            counters: (0..threads).map(|_| Arc::new(ThreadCounters::default())).collect(),
        }
    }

    /// `threads.size()`.
    #[inline]
    pub fn size(&self) -> usize {
        self.counters.len()
    }

    /// `nodes_searched()`: the nodes of every worker, read during the search
    /// as well as after it.
    #[inline]
    pub fn nodes_searched(&self) -> u64 {
        self.counters.iter().fold(0u64, |sum, c| sum.wrapping_add(c.nodes.get()))
    }

    /// `tb_hits()`: the tablebase hits of every worker.
    #[inline]
    pub fn tb_hits(&self) -> u64 {
        self.counters.iter().fold(0u64, |sum, c| sum.wrapping_add(c.tb_hits.get()))
    }
}

/// `std::function<void()> jobFunc`: what a thread runs next, given its worker.
type Job = Box<dyn FnOnce(&mut Worker) + Send>;

struct ThreadState {
    job: Option<Job>,
    // Set before starting the native thread, as in C++
    searching: bool,
    exit: bool,
}

/// The part of a `Thread` that its native thread and the other threads
/// share: the mutex, the condition variable and the worker. The main worker
/// holds those of the helpers, to start them and wait for them.
pub struct ThreadInner {
    state: Mutex<ThreadState>,
    cv: Condvar,
    worker: Mutex<Box<Worker>>,
}

/// Aborts the process when a job panics (debug builds unwind; release
/// builds abort anyway): a thread that died while `searching` would leave
/// whoever waits for it waiting forever.
struct AbortOnPanic;

impl Drop for AbortOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::process::abort();
        }
    }
}

impl ThreadInner {
    /// `idle_loop()`: the thread gets parked here, blocked on the condition
    /// variable when it has no work to do.
    fn idle_loop(&self) {
        loop {
            let job = {
                let mut st = self.state.lock().unwrap();
                st.searching = false;
                self.cv.notify_all(); // Wake up anyone waiting for search finished
                while !st.searching {
                    st = self.cv.wait(st).unwrap();
                }

                if st.exit {
                    return;
                }

                st.job.take()
            };

            if let Some(job) = job {
                let mut w = self.worker.lock().unwrap();
                job(&mut w);
            }
        }
    }

    /// `run_custom_job()`: launches a function in the thread, once it is
    /// idle.
    pub fn run_custom_job(&self, f: Job) {
        {
            let mut st = self.state.lock().unwrap();
            while st.searching {
                st = self.cv.wait(st).unwrap();
            }
            st.job = Some(f);
            st.searching = true;
        }
        self.cv.notify_all();
    }

    /// `wait_for_search_finished()`: blocks until the thread is idle.
    pub fn wait_for_search_finished(&self) {
        let mut st = self.state.lock().unwrap();
        while st.searching {
            st = self.cv.wait(st).unwrap();
        }
    }

    /// `start_searching()`: wakes up the thread, which starts searching.
    /// `trace` runs the instance of the search with the trace hooks (for the
    /// main thread of `searchtrace`).
    pub fn start_searching(&self, trace: bool) {
        self.run_custom_job(Box::new(move |w: &mut Worker| {
            if trace {
                w.start_searching::<true>();
            } else {
                w.start_searching::<false>();
            }
        }));
    }

    /// The worker, once the thread is idle. Only the thread that drives the
    /// pool (or the main worker, for the helpers it waited for) calls it, so
    /// no job can start between the wait and the lock.
    pub fn worker(&self) -> MutexGuard<'_, Box<Worker>> {
        self.wait_for_search_finished();
        self.worker.lock().unwrap()
    }
}

/// `Thread`: a native thread, parked in `idle_loop()` between jobs, and the
/// worker it owns.
pub struct Thread {
    inner: Arc<ThreadInner>,
    handle: Option<JoinHandle<()>>,
}

impl Thread {
    /// Launches the thread and waits until it goes to sleep in idle_loop().
    fn new(worker: Box<Worker>, idx: usize) -> Thread {
        let inner = Arc::new(ThreadInner {
            state: Mutex::new(ThreadState { job: None, searching: true, exit: false }),
            cv: Condvar::new(),
            worker: Mutex::new(worker),
        });
        let native = inner.clone();
        let handle = std::thread::Builder::new()
            .name(format!("search {idx}"))
            .stack_size(TH_STACK_SIZE)
            .spawn(move || {
                let _guard = AbortOnPanic;
                native.idle_loop();
            });
        let Ok(handle) = handle else {
            eprintln!("Failed to create search thread");
            std::process::exit(1);
        };
        inner.wait_for_search_finished();
        Thread { inner, handle: Some(handle) }
    }

    pub fn inner(&self) -> &ThreadInner {
        &self.inner
    }
}

impl Drop for Thread {
    /// Wakes up the thread in idle_loop() and waits for its termination.
    fn drop(&mut self) {
        {
            let mut st = self.inner.state.lock().unwrap();
            while st.searching {
                st = self.inner.cv.wait(st).unwrap();
            }
            st.exit = true;
            st.searching = true;
        }
        self.inner.cv.notify_all();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The helpers of the main worker: the threads other than the main one,
/// which it starts when its search starts and waits for when it ends
/// (`ThreadPool::start_searching()` and `wait_for_search_finished()`, which
/// C++ calls on the pool from the main worker).
#[derive(Clone, Default)]
pub struct Helpers(Arc<[Arc<ThreadInner>]>);

impl Helpers {
    /// `start_searching()`: starts the non-main threads.
    pub fn start_searching(&self) {
        for th in self.0.iter() {
            th.start_searching(false);
        }
    }

    /// `wait_for_search_finished()`: waits for the non-main threads.
    pub fn wait_for_search_finished(&self) {
        for th in self.0.iter() {
            th.wait_for_search_finished();
        }
    }

    /// The workers of the non-main threads, once they are idle.
    pub fn workers(&self) -> Vec<MutexGuard<'_, Box<Worker>>> {
        self.0.iter().map(|th| th.worker()).collect()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `next_power_of_two()` of `thread.cpp`.
pub fn next_power_of_two(count: usize) -> usize {
    if count > 1 {
        2 << (usize::BITS - 1 - (count - 1).leading_zeros())
    } else {
        1
    }
}

/// `get_best_thread()`: the index of the best of the threads whose root
/// moves are given (index 0 is the main thread), by the voting of C++.
pub fn get_best_thread(threads: &[&RootMoves]) -> usize {
    let mut best_thread = 0;
    let mut min_score = VALUE_INFINITE;

    // votes, by move: few distinct moves, a linear search is enough
    let mut votes: Vec<(Move, i64)> = Vec::with_capacity(2 * threads.len().min(threads[0].len()));
    let vote_of = |votes: &[(Move, i64)], m: Move| votes.iter().find(|v| v.0 == m).map_or(0, |v| v.1);

    for th in threads {
        min_score = min_score.min(th[0].score);
    }

    // Vote according to score, and select the best thread
    for th in threads {
        let rm = &th[0];
        let add = (rm.score - min_score + 14) as i64;
        match votes.iter_mut().find(|v| v.0 == rm.pv[0]) {
            Some(v) => v.1 += add,
            None => votes.push((rm.pv[0], add)),
        }
    }

    for (i, th) in threads.iter().enumerate() {
        let best_thread_move: &RootMove = &threads[best_thread][0];
        let new_thread_move: &RootMove = &th[0];

        let best_thread_move_vote = vote_of(&votes, best_thread_move.pv[0]);
        let new_thread_move_vote = vote_of(&votes, new_thread_move.pv[0]);

        // Aborted (d1) searches may lead to inexact win (or loss) scores.
        let best_thread_decisive = best_thread_move.score != -VALUE_INFINITE
            && is_decisive(best_thread_move.score)
            && !best_thread_move.is_inexact();
        let new_thread_decisive = new_thread_move.score != -VALUE_INFINITE
            && is_decisive(new_thread_move.score)
            && !new_thread_move.is_inexact();

        if best_thread_decisive {
            // Make sure we pick the shortest mate / TB conversion.
            if new_thread_decisive && new_thread_move.score.abs() > best_thread_move.score.abs() {
                debug_assert!(
                    (is_win(best_thread_move.score) && is_win(new_thread_move.score))
                        || (is_loss(best_thread_move.score) && is_loss(new_thread_move.score))
                );

                best_thread = i;
            }
        } else if new_thread_decisive
            || (!is_loss(new_thread_move.score)
                && (new_thread_move_vote > best_thread_move_vote
                    || (new_thread_move_vote == best_thread_move_vote
                        && new_thread_move.pv.len() > best_thread_move.pv.len())))
        {
            best_thread = i;
        }
    }

    best_thread
}

/// `ThreadPool`: creates the threads, starts a search, waits for it.
pub struct ThreadPool {
    pub flags: Arc<SharedFlags>,
    threads: Vec<Thread>,
    /// `sharedHistories` (C++ keeps the map in the engine and `set()` makes
    /// it anew): the shared histories of the one NUMA node.
    shared_histories: Option<Arc<SharedHistories>>,
}

impl Default for ThreadPool {
    fn default() -> Self {
        ThreadPool::new()
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        // destroy any existing thread(s)
        if !self.threads.is_empty() {
            self.main_thread().wait_for_search_finished();
            self.threads.clear();
        }
    }
}

impl ThreadPool {
    /// An empty pool; `set()` creates the threads.
    pub fn new() -> ThreadPool {
        ThreadPool { flags: Arc::new(SharedFlags::new(0)), threads: Vec::new(), shared_histories: None }
    }

    pub fn main_thread(&self) -> &ThreadInner {
        &self.threads[0].inner
    }

    /// `size()`.
    pub fn size(&self) -> usize {
        self.threads.len()
    }

    pub fn shared_histories(&self) -> &SharedHistories {
        self.shared_histories.as_deref().unwrap()
    }

    /// `set()`: creates or destroys threads to match the requested number.
    /// Created and launched threads immediately go to sleep in idle_loop().
    /// Upon resizing, threads are recreated, with their shared histories,
    /// sized for the new count, and their workers, cleared. One NUMA node:
    /// threads are not bound (see README.md, "Threads").
    pub fn set(
        &mut self,
        requested: usize,
        tt: &Arc<TranspositionTable>,
        network: Option<Arc<Network>>,
        updates: Arc<UpdateContext>,
    ) {
        // C++ keeps the generator of the skill level in a static: it
        // survives the threads
        let mut skill_rng = None;

        if !self.threads.is_empty() {
            // destroy any existing thread(s)
            self.main_thread().wait_for_search_finished();
            skill_rng = self.main_worker().manager.as_deref_mut().unwrap().skill_rng.take();
            self.threads.clear();
        }

        // Freed before the new ones are allocated, as in C++
        self.shared_histories = None;

        if requested > 0 {
            // create new thread(s)
            let shared = Arc::new(SharedHistories::new(next_power_of_two(requested)));
            self.flags = Arc::new(SharedFlags::new(requested));

            while self.threads.len() < requested {
                let thread_id = self.threads.len();
                let manager = if thread_id == 0 {
                    let mut m = Box::new(SearchManager::new(updates.clone()));
                    m.skill_rng = skill_rng.take();
                    Some(m)
                } else {
                    None
                };
                // All threads are part of NUMA node 0: their index in it is
                // their index, and the node holds them all
                let mut worker = Box::new(Worker::new(
                    shared.clone(),
                    manager,
                    thread_id,
                    thread_id,
                    requested,
                    tt.clone(),
                    self.flags.clone(),
                ));
                // (cleared by its constructor, as in C++, then by clear()
                // below, with the network for its refresh table)
                if let Some(net) = &network {
                    worker.set_network(net.clone());
                }
                self.threads.push(Thread::new(worker, thread_id));
            }

            let helpers: Arc<[Arc<ThreadInner>]> = self.threads[1..].iter().map(|t| t.inner.clone()).collect();
            self.main_worker().helpers = Helpers(helpers);
            self.shared_histories = Some(shared);

            self.clear();

            self.main_thread().wait_for_search_finished();
        }
    }

    /// The main worker, once its search has finished.
    pub fn main_worker(&self) -> MutexGuard<'_, Box<Worker>> {
        self.main_thread().worker()
    }

    /// Runs `f` on each worker, from the thread driving the pool, once its
    /// thread is idle (no search runs).
    pub fn for_each_worker(&self, mut f: impl FnMut(&mut Worker)) {
        for th in &self.threads {
            f(&mut th.inner.worker());
        }
    }

    /// `clear()`: resets the histories of the workers, each on its thread,
    /// and the main manager's data, as before a new game.
    pub fn clear(&mut self) {
        if self.threads.is_empty() {
            return;
        }

        for th in &self.threads {
            th.inner.run_custom_job(Box::new(|w: &mut Worker| w.clear()));
        }

        for th in &self.threads {
            th.inner.wait_for_search_finished();
        }

        let mut w = self.main_worker();
        let mm = w.manager.as_deref_mut().unwrap();
        // These two affect the time taken on the first move of a game:
        mm.best_previous_average_score = VALUE_INFINITE;
        mm.previous_time_reduction = 0.85;

        mm.calls_cnt = 0;
        mm.best_previous_score = VALUE_INFINITE;
        mm.original_time_adjust = -1.0;
        mm.tm.clear();
    }

    /// `run_on_thread()`: launches a function on a thread, once it is idle.
    pub fn run_on_thread(&self, thread_id: usize, f: impl FnOnce(&mut Worker) + Send + 'static) {
        self.threads[thread_id].inner.run_custom_job(Box::new(f));
    }

    /// `wait_on_thread()`.
    pub fn wait_on_thread(&self, thread_id: usize) {
        self.threads[thread_id].inner.wait_for_search_finished();
    }

    /// `num_threads()`.
    pub fn num_threads(&self) -> usize {
        self.threads.len()
    }

    /// `wait_for_search_finished()` of the engine: waits for the main
    /// thread, which waits for the others.
    pub fn wait_for_search_finished(&self) {
        if !self.threads.is_empty() {
            self.main_thread().wait_for_search_finished();
        }
    }

    /// `nodes_searched()`: the nodes of every thread.
    pub fn nodes_searched(&self) -> u64 {
        self.flags.nodes_searched()
    }

    /// `start_thinking()`: sets up every worker for a search of `pos` and
    /// wakes up the main thread, which wakes up the others and starts the
    /// search; returns at once. `trace` runs the main thread's search with
    /// the trace hooks, for `searchtrace`.
    pub fn start_thinking(&mut self, pos: &Position, limits: LimitsType, options: SearchOptions, trace: bool) {
        self.main_thread().wait_for_search_finished();

        self.main_worker().manager.as_deref_mut().unwrap().stop_on_ponderhit = false;
        self.flags.stop.store(false, Relaxed);
        self.flags.ponder.store(limits.ponder_mode, Relaxed);

        self.flags.increase_depth.store(true, Relaxed);

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

        // C++ sets the root position of each worker from the FEN of `pos`
        // and copies its last state, which links to the states of the moves
        // before it (shared, read-only): the same position with the same
        // history, which is what a copy is. C++ does this on each thread
        // (run_custom_job); here the thread driving the pool does it, each
        // thread being idle, which is the same.
        for (i, th) in self.threads.iter().enumerate() {
            let mut w = th.inner.worker();
            w.limits = limits.clone();
            let c = &self.flags.counters[i];
            c.nodes.set(0);
            c.tb_hits.set(0);
            c.best_move_changes.store(0, Relaxed);
            w.nmp_min_ply = 0;
            w.root_depth = 0;
            w.root_moves.clone_from(&root_moves);
            w.root_pos.clone_from(pos);
            w.tb_config = tb_config;
            w.search_options = options.clone();
        }

        self.main_thread().start_searching(trace);
    }
}

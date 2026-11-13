//! Port of `src/search.{h,cpp}`: iterative deepening, aspiration windows,
//! the main search and the quiescence search, with every pruning, reduction
//! and extension of C++, the history updates, the root move handling, and the
//! UCI output of a search.
//!
//! Read it next to `search.cpp`: the functions, their order and the names of
//! the variables are those of C++, and the comments of C++ are kept. What
//! differs, and why:
//!
//! - **The search stack** (`Stack ss[]` in C++, with `ss` a pointer into it)
//!   is a vector of the worker, allocated once, and `ss` is an index into it
//!   (`ss - 1` is `ss - 1`). Instead of the pointers `ss->continuationHistory`
//!   and `ss->continuationCorrectionHistory` it holds flat indices into the
//!   tables (see `history.rs`); instead of `ss->pv`, a pointer to the local
//!   PV of the parent node or null, a flag: the line it points to is always
//!   `pv_lines[ply]` (see `PVMoves`).
//! - **The PV lines** (`PVMoves pv` local to each node in C++) are rows of a
//!   table of the worker: the local line of a node at ply `p` is row `p + 1`,
//!   row 0 is that of `iterative_deepening()`. A node empties its row on entry,
//!   as the C++ constructor does.
//! - **The move pickers** (local to each node in C++) are kept in the worker,
//!   two per ply, and renewed (see `movepick.rs`): one for the nodes searched
//!   without an excluded move, one for the singular extension searches, which
//!   run at the ply of a node whose picker is still in use. No other search
//!   runs at the ply of a live picker (the null move verification search and
//!   the quiescence search of razoring come before the node makes its first
//!   picker).
//! - **The position** is passed down as in C++; `StateInfo`s live in it.
//! - **No prefetch**: the C++ prefetches change no result.
//! - **The search trace** (`TRACE(...)` in C++): the search functions take a
//!   `const TRACE: bool`. The default search is the instance with `false`, in
//!   which every hook is removed at compile time; `searchtrace` runs the one
//!   with `true`. See `searchtrace.rs`.
//! - **One thread.** The worker reads what C++ reads from the thread pool
//!   (`threads.stop`, `increaseDepth`, `nodes_searched()`) through the shared
//!   flags of the pool or from itself. See `thread.rs`.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use crate::history::*;
use crate::misc::{Prng, ValueList};
use crate::movegen::MoveList;
use crate::movepick::{Histories, MovePicker};
use crate::nnue::{AccumulatorCaches, AccumulatorStack, Network};
use crate::position::Position;
use crate::score::Score;
use crate::searchtrace::SearchTracer;
use crate::thread::SharedFlags;
use crate::timeman::{now, TimeManagement, TimePoint};
use crate::tt::*;
use crate::types::*;
use crate::uci;

/// The node types, a const parameter of the search functions as the C++
/// template parameter.
pub type NodeType = u8;
pub const NON_PV: NodeType = 0;
pub const PV: NodeType = 1;
pub const ROOT: NodeType = 2;

#[inline(always)]
fn lmr_divisor(depth: i32) -> i32 {
    let d = depth.min(16);
    3000 + 7 * (d - 8) * (d - 8)
}

const NODES_LIMIT_OUTPUT: u64 = 10_000_000;

const SEARCHEDLIST_CAPACITY: usize = 32;
type SearchedList = ValueList<Move, SEARCHEDLIST_CAPACITY>;

/// `reductions[i] = int(2872 / 128.0 * std::log(i))` (0 for i = 0), as C++
/// computes it at start-up. Kept as constants rather than computed, so that
/// no math library and no x87 rounding can change an entry: the test
/// `reductions_are_the_cpp_ones` recomputes them, and each exact value lies
/// at least 0.0027 from an integer (checked at 60 digits), so any `ln`
/// within a relative 1e-5 of the true one gives this table.
#[rustfmt::skip]
const REDUCTIONS: [i32; MAX_MOVES] = [
    0, 0, 15, 24, 31, 36, 40, 43, 46, 49, 51, 53, 55, 57, 59, 60, 62, 63, 64, 66, 67, 68, 69, 70, 71, 72, 73, 73, 74,
    75, 76, 77, 77, 78, 79, 79, 80, 81, 81, 82, 82, 83, 83, 84, 84, 85, 85, 86, 86, 87, 87, 88, 88, 89, 89, 89, 90,
    90, 91, 91, 91, 92, 92, 92, 93, 93, 94, 94, 94, 95, 95, 95, 95, 96, 96, 96, 97, 97, 97, 98, 98, 98, 98, 99, 99,
    99, 99, 100, 100, 100, 100, 101, 101, 101, 101, 102, 102, 102, 102, 103, 103, 103, 103, 103, 104, 104, 104, 104,
    105, 105, 105, 105, 105, 106, 106, 106, 106, 106, 107, 107, 107, 107, 107, 107, 108, 108, 108, 108, 108, 109,
    109, 109, 109, 109, 109, 110, 110, 110, 110, 110, 110, 111, 111, 111, 111, 111, 111, 111, 112, 112, 112, 112,
    112, 112, 113, 113, 113, 113, 113, 113, 113, 114, 114, 114, 114, 114, 114, 114, 114, 115, 115, 115, 115, 115,
    115, 115, 116, 116, 116, 116, 116, 116, 116, 116, 117, 117, 117, 117, 117, 117, 117, 117, 117, 118, 118, 118,
    118, 118, 118, 118, 118, 118, 119, 119, 119, 119, 119, 119, 119, 119, 119, 120, 120, 120, 120, 120, 120, 120,
    120, 120, 121, 121, 121, 121, 121, 121, 121, 121, 121, 121, 122, 122, 122, 122, 122, 122, 122, 122, 122, 122,
    122, 123, 123, 123, 123, 123, 123, 123, 123, 123, 123, 123, 124, 124, 124, 124,
];

// ---------------------------------------------------------------------------
// search.h

/// `PVMoves`: a principal variation of at most `MAX_PLY + 1` moves.
#[derive(Clone)]
pub struct PVMoves {
    moves: [Move; MAX_PLY + 1],
    length: usize,
}

impl Default for PVMoves {
    fn default() -> Self {
        PVMoves { moves: [Move::none(); MAX_PLY + 1], length: 0 }
    }
}

impl PVMoves {
    #[inline(always)]
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.length]
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.length
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    #[inline(always)]
    pub fn clear(&mut self) {
        self.length = 0;
    }

    /// `operator=(const RootPVMoves&)`: at most `MAX_PLY` moves.
    fn assign(&mut self, rhs: &[Move]) {
        self.length = rhs.len().min(MAX_PLY);
        self.moves[..self.length].copy_from_slice(&rhs[..self.length]);
    }
}

/// `Stack`: what a node keeps for the nodes above and below it. See the
/// module documentation for the members that replace pointers.
#[derive(Clone, Copy, Default)]
pub struct Stack {
    /// `pv != nullptr`: the node's PV line, `pv_lines[ply]`, is in use
    pub pv: bool,
    pub continuation_history: usize,
    pub continuation_correction_history: usize,
    pub ply: i32,
    pub current_move: Move,
    pub excluded_move: Move,
    pub static_eval: Value,
    pub stat_score: i32,
    pub move_count: i32,
    pub in_check: bool,
    pub tt_pv: bool,
    pub tt_hit: bool,
    pub follow_pv: bool,
    pub cutoff_cnt: i32,
    pub reduction: i32,
    pub prior_nmp_fail_high: i32,
}

/// The stack has room for `(ss - 7)` to `(ss + 2)` around plies 0 to
/// `MAX_PLY`, as in C++.
const STACK_SIZE: usize = MAX_PLY + 10;
const STACK_OFFSET: usize = 7;

/// `RootMove`: a move at the root, with its score and its PV (a refutation
/// for the moves that fail low).
#[derive(Clone)]
pub struct RootMove {
    pub effort: u64,
    pub score: Value,
    pub previous_score: Value,
    pub average_score: Value,
    pub mean_squared_score: Value,
    pub uci_score: Value,
    pub inexact_lower: bool,
    pub inexact_upper: bool,
    pub previous_score_exact: bool,
    pub sel_depth: i32,
    pub tb_rank: i32,
    pub tb_score: Value,
    pub pv: Vec<Move>,
    pub previous_pv: Vec<Move>,
}

impl RootMove {
    pub fn new(m: Move) -> RootMove {
        let mut pv = Vec::with_capacity(MAX_PLY);
        pv.push(m);
        RootMove {
            effort: 0,
            score: -VALUE_INFINITE,
            previous_score: -VALUE_INFINITE,
            average_score: -VALUE_INFINITE,
            mean_squared_score: -VALUE_INFINITE * VALUE_INFINITE,
            uci_score: -VALUE_INFINITE,
            inexact_lower: false,
            inexact_upper: false,
            previous_score_exact: false,
            sel_depth: 0,
            tb_rank: 0,
            tb_score: 0,
            pv,
            previous_pv: Vec::with_capacity(MAX_PLY),
        }
    }

    pub fn is_inexact(&self) -> bool {
        self.inexact_lower || self.inexact_upper
    }

    pub fn is_exact_loss(&self) -> bool {
        self.score != -VALUE_INFINITE && is_loss(self.score) && !self.is_inexact()
    }

    pub fn unset_inexact(&mut self) {
        self.inexact_lower = false;
        self.inexact_upper = false;
    }

    /// `operator<`: sorts in descending order of score, then of previous
    /// score.
    #[inline]
    fn before(&self, m: &RootMove) -> bool {
        if m.score != self.score {
            m.score < self.score
        } else {
            m.previous_score < self.previous_score
        }
    }

    /// Called in case we have no ponder move before exiting the search, for
    /// instance, in case we stop the search during a fail high at root. We
    /// try hard to have a ponder move to return to the GUI.
    pub fn extract_ponder_from_tt(&mut self, tt: &TranspositionTable, pos: &mut Position) -> bool {
        debug_assert!(self.pv.len() == 1 && self.pv[0] != Move::none());

        pos.do_move(self.pv[0]);

        if !pos.is_draw(1) {
            let (tt_hit, tt_data, _) = tt.probe(pos.key());
            if tt_hit && tt_data.mv != Move::none() && pos.legal(tt_data.mv) {
                self.pv.push(tt_data.mv);
            }
        }

        pos.undo_move(self.pv[0]);
        self.pv.len() > 1
    }
}

/// `std::stable_sort` of root moves by `operator<`: an insertion sort,
/// which is stable and does not allocate (C++'s may use a buffer; any stable
/// sort gives the same order).
fn stable_sort(moves: &mut [RootMove]) {
    for i in 1..moves.len() {
        let mut j = i;
        while j > 0 && moves[j].before(&moves[j - 1]) {
            moves.swap(j, j - 1);
            j -= 1;
        }
    }
}

pub type RootMoves = Vec<RootMove>;

/// `LimitsType`: what the caller asks of the search.
#[derive(Clone, Default)]
pub struct LimitsType {
    pub searchmoves: Vec<String>,
    pub time: [TimePoint; COLOR_NB],
    pub inc: [TimePoint; COLOR_NB],
    pub npmsec: TimePoint,
    pub movetime: TimePoint,
    pub start_time: TimePoint,
    pub movestogo: i32,
    pub depth: i32,
    pub mate: i32,
    pub perft: i32,
    pub infinite: i32,
    pub nodes: u64,
    pub ponder_mode: bool,
}

impl LimitsType {
    pub fn use_time_management(&self) -> bool {
        self.time[WHITE as usize] != 0 || self.time[BLACK as usize] != 0
    }
}

/// The options the search reads (`options[...]` in C++), as they are when
/// the search starts.
#[derive(Clone)]
pub struct SearchOptions {
    pub multi_pv: usize,
    pub skill_level: i32,
    pub limit_strength: bool,
    pub uci_elo: i32,
    pub show_wdl: bool,
    pub move_overhead: i32,
    pub nodestime: i32,
    pub syzygy_50_move_rule: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            multi_pv: 1,
            skill_level: 20,
            limit_strength: false,
            uci_elo: Skill::LOWEST_ELO,
            show_wdl: false,
            move_overhead: 10,
            nodestime: 0,
            syzygy_50_move_rule: true,
        }
    }
}

/// `Tablebases::Config` as `rank_root_moves()` returns it without
/// tablebases (they are not ported): no probing anywhere.
#[derive(Clone, Copy, Default)]
pub struct TbConfig {
    pub cardinality: i32,
    pub root_in_tb: bool,
    pub use_rule50: bool,
    pub probe_depth: i32,
}

/// `InfoShort`.
pub struct InfoShort {
    pub depth: i32,
    pub score: Score,
}

/// `InfoFull`.
pub struct InfoFull<'a> {
    pub depth: i32,
    pub score: Score,
    pub sel_depth: i32,
    pub multi_pv: usize,
    pub wdl: &'a str,
    pub bound: &'a str,
    pub time_ms: u64,
    pub nodes: u64,
    pub nps: u64,
    pub tb_hits: u64,
    pub pv: &'a str,
    pub hashfull: i32,
}

/// `InfoIteration`.
pub struct InfoIteration<'a> {
    pub depth: i32,
    pub currmove: &'a str,
    pub currmovenumber: usize,
}

/// The callback of the `bestmove` line, given the best move and the ponder
/// move (empty when there is none).
pub type BestmoveFn = Box<dyn Fn(&str, &str) + Send + Sync>;

/// `SearchManager::UpdateContext`: where the search sends its output.
pub struct UpdateContext {
    pub on_update_no_moves: Box<dyn Fn(&InfoShort) + Send + Sync>,
    pub on_update_full: Box<dyn Fn(&InfoFull) + Send + Sync>,
    pub on_iter: Box<dyn Fn(&InfoIteration) + Send + Sync>,
    pub on_bestmove: BestmoveFn,
    pub on_start: Box<dyn Fn() + Send + Sync>,
}

/// `Skill`: the strength limit. Ported but not checked against C++ (the
/// trace fixes `Skill Level` to 20, which disables it).
pub struct Skill {
    pub level: f64,
    pub best: Move,
}

impl Skill {
    pub const LOWEST_ELO: i32 = 1320;
    pub const HIGHEST_ELO: i32 = 3190;

    pub fn new(skill_level: i32, uci_elo: i32) -> Skill {
        let level = if uci_elo != 0 {
            let e = (uci_elo - Self::LOWEST_ELO) as f64 / (Self::HIGHEST_ELO - Self::LOWEST_ELO) as f64;
            (((37.2473 * e - 40.8525) * e + 22.2943) * e - 0.311438).clamp(0.0, 19.0)
        } else {
            skill_level as f64
        };
        Skill { level, best: Move::none() }
    }

    pub fn enabled(&self) -> bool {
        self.level < 20.0
    }

    pub fn time_to_pick(&self, depth: Depth) -> bool {
        depth == 1 + self.level as i32
    }

    /// When playing with strength handicap, choose the best move among a set
    /// of RootMoves using a statistical rule dependent on 'level'.
    pub fn pick_best(&mut self, root_moves: &RootMoves, multi_pv: usize, rng: &mut Prng) -> Move {
        let mut top_score = root_moves[0].score;
        let mut min_score = root_moves[0].score;
        for rm in &root_moves[1..multi_pv] {
            top_score = top_score.max(rm.score);
            min_score = min_score.min(rm.score);
        }
        let delta = (top_score - min_score).min(PAWN_VALUE);
        let mut max_score = -VALUE_INFINITE;
        let weakness = 120.0 - 2.0 * self.level;

        // Choose best move. For each move score we add two terms dependent on
        // weakness. One is deterministic and bigger for weaker levels, and one
        // is random. Then we choose the move with the resulting highest score.
        for rm in &root_moves[..multi_pv] {
            let r = rng.rand() as u32;
            // This is our magic formula
            let push = ((weakness * (top_score - rm.score) as f64
                + (delta as u32 * (r % weakness as i32 as u32)) as f64) as i32)
                / 128;

            if rm.score + push >= max_score {
                max_score = rm.score + push;
                self.best = rm.pv[0];
            }
        }
        self.best
    }
}

/// `SearchManager`: what the main thread keeps for itself.
pub struct SearchManager {
    pub tm: TimeManagement,
    pub original_time_adjust: f64,
    pub calls_cnt: i32,
    pub ponder: bool,
    pub iter_value: [Value; 4],
    pub previous_time_reduction: f64,
    pub best_previous_score: Value,
    pub best_previous_average_score: Value,
    pub stop_on_ponderhit: bool,
    pub updates: Arc<UpdateContext>,
    /// The generator of `Skill::pick_best` (`static PRNG rng(now())`).
    pub skill_rng: Prng,
}

impl SearchManager {
    pub fn new(updates: Arc<UpdateContext>) -> SearchManager {
        SearchManager {
            tm: TimeManagement::default(),
            original_time_adjust: 0.0,
            calls_cnt: 0,
            ponder: false,
            iter_value: [0; 4],
            previous_time_reduction: 0.0,
            best_previous_score: 0,
            best_previous_average_score: 0,
            stop_on_ponderhit: false,
            updates,
            skill_rng: Prng::new(now() as u64 | 1),
        }
    }
}

// ---------------------------------------------------------------------------
// The worker

/// `Search::Worker`: the search of one thread, and its tables.
pub struct Worker {
    // Public because they need to be updatable by the stats
    pub main_history: Box<ButterflyHistory>,
    pub low_ply_history: Box<LowPlyHistory>,
    pub capture_history: Box<CapturePieceToHistory>,
    pub continuation_correction_history: ContinuationCorrectionHistory,
    pub tt_move_history: TTMoveHistory,
    /// The histories shared with the other threads of the NUMA node (one
    /// thread for now); the continuation history is in it.
    pub shared_history: Arc<SharedHistories>,

    /// Receives the events of the search when set, see `searchtrace.rs`.
    pub tracer: Option<Box<SearchTracer>>,

    pub limits: LimitsType,
    pub search_options: SearchOptions,

    pv_idx: usize,
    pv_last: usize,
    // C++ keeps these three in RelaxedAtomic<u64> for the other threads to
    // read; with one thread they are plain counters (32-bit PowerPC has no
    // 64-bit atomics: the thread layer will have to choose).
    pub nodes: u64,
    pub tb_hits: u64,
    pub best_move_changes: u64,
    sel_depth: i32,
    pub nmp_min_ply: i32,

    optimism: [Value; COLOR_NB],

    pub root_pos: Position,
    pub root_moves: RootMoves,
    pub root_depth: Depth,
    seek_mate: bool,

    last_iteration_idx_pv: PVMoves,

    pub thread_idx: usize,
    numa_thread_idx: usize,
    numa_total: usize,

    /// The main thread has a SearchManager, the others have none
    pub manager: Option<Box<SearchManager>>,

    pub tb_config: TbConfig,

    pub tt: Arc<TranspositionTable>,
    pub network: Option<Arc<Network>>,
    pub flags: Arc<SharedFlags>,

    // Used by NNUE
    accumulator_stack: AccumulatorStack,
    refresh_table: AccumulatorCaches,

    // Allocated once, see the module documentation
    stack: Box<[Stack]>,
    pv_lines: Box<[PVMoves]>,
    pickers: Box<[MovePicker]>,
}

/// The tables a move picker reads (`Histories`), with the continuation
/// histories of `(ss - 1)` to `(ss - 6)`, borrowing only the fields of the
/// worker it reads, so that its pickers can be borrowed at the same time.
macro_rules! histories {
    ($w:ident, $ss:expr) => {{
        let block = &$w.shared_history.continuation_history_block;
        let st = &$w.stack;
        let ss = $ss;
        Histories {
            main_history: &$w.main_history,
            low_ply_history: &$w.low_ply_history,
            capture_history: &$w.capture_history,
            continuation_history: [
                block.at(st[ss - 1].continuation_history),
                block.at(st[ss - 2].continuation_history),
                block.at(st[ss - 3].continuation_history),
                block.at(st[ss - 4].continuation_history),
                block.at(st[ss - 5].continuation_history),
                block.at(st[ss - 6].continuation_history),
            ],
            shared_history: &$w.shared_history,
        }
    }};
}

/// The two pickers of a ply (see the module documentation).
#[inline(always)]
fn picker_slot(ply: i32, excluded: bool) -> usize {
    2 * ply as usize + excluded as usize
}

impl Worker {
    pub fn new(
        shared_history: Arc<SharedHistories>,
        manager: Option<Box<SearchManager>>,
        thread_id: usize,
        numa_thread_id: usize,
        numa_total_threads: usize,
        tt: Arc<TranspositionTable>,
        flags: Arc<SharedFlags>,
    ) -> Worker {
        let mut root_pos = Position::new();
        root_pos.set(uci::START_FEN.as_bytes(), false).expect("start position");
        let mut w = Worker {
            main_history: new_butterfly_history(),
            low_ply_history: new_low_ply_history(),
            capture_history: new_capture_piece_to_history(),
            continuation_correction_history: ContinuationCorrectionHistory::new(),
            tt_move_history: TTMoveHistory::new(0),
            shared_history,
            tracer: None,
            limits: LimitsType::default(),
            search_options: SearchOptions::default(),
            pv_idx: 0,
            pv_last: 0,
            nodes: 0,
            tb_hits: 0,
            best_move_changes: 0,
            sel_depth: 0,
            nmp_min_ply: 0,
            optimism: [0; COLOR_NB],
            root_pos,
            root_moves: Vec::new(),
            root_depth: 0,
            seek_mate: false,
            last_iteration_idx_pv: PVMoves::default(),
            thread_idx: thread_id,
            numa_thread_idx: numa_thread_id,
            numa_total: numa_total_threads,
            manager,
            tb_config: TbConfig::default(),
            tt,
            network: None,
            flags,
            accumulator_stack: AccumulatorStack::new(),
            refresh_table: AccumulatorCaches::empty(),
            stack: vec![Stack::default(); STACK_SIZE].into_boxed_slice(),
            pv_lines: vec![PVMoves::default(); MAX_PLY + 3].into_boxed_slice(),
            pickers: (0..2 * (MAX_PLY + 1)).map(|_| MovePicker::empty_for_search()).collect(),
        };
        w.clear();
        w
    }

    pub fn is_mainthread(&self) -> bool {
        self.thread_idx == 0
    }

    /// Gives the worker the network to evaluate with (C++ reaches it through
    /// the shared state). The refresh table is cleared by `clear()`, which
    /// the engine calls whenever the network changes, as C++ does.
    pub fn set_network(&mut self, network: Arc<Network>) {
        self.network = Some(network);
    }

    /// `clear()`: resets the histories, usually before a new game.
    pub fn clear(&mut self) {
        self.main_history.fill_all(-5);
        self.capture_history.fill_all(-742);

        // Each thread clears its part of the dynamically-sized shared
        // histories. The constant-size continuation history is initialized by
        // thread 0 of each NUMA node. Both through shared references, as in
        // C++, so that this works unchanged with several threads.
        self.shared_history.correction_history.clear_range(-5, self.numa_thread_idx, self.numa_total);
        self.shared_history.pawn_history.clear_range(-1338, self.numa_thread_idx, self.numa_total);

        if self.numa_thread_idx == 0 {
            self.shared_history.continuation_history_block.fill_shared(-586);
        }

        self.tt_move_history = TTMoveHistory::new(0);

        self.continuation_correction_history.tables_mut().fill_all(5);

        if let Some(net) = &self.network {
            self.refresh_table.clear(net);
        }
    }

    // -----------------------------------------------------------------------
    // Access to the tables through the indices of the stack

    #[inline(always)]
    fn cont_hist(&self, index: usize) -> &PieceToHistory {
        self.shared_history.continuation_history_block.at(index)
    }

    /// `pv_lines[ply] = [m] + (pv_lines[ply + 1] if child)`: C++
    /// `ss->pv->update(move, (ss + 1)->pv)`.
    #[inline(always)]
    fn pv_update(&mut self, ply: usize, m: Move, child: bool) {
        let (head, tail) = self.pv_lines.split_at_mut(ply + 1);
        let dst = &mut head[ply];
        let len = if child { tail[0].length } else { 0 };
        debug_assert!(len <= MAX_PLY);
        if child {
            dst.moves[1..=len].copy_from_slice(&tail[0].moves[..len]);
        }
        dst.moves[0] = m;
        dst.length = len + 1;
    }

    #[inline(always)]
    fn main_manager(&mut self) -> &mut SearchManager {
        debug_assert!(self.thread_idx == 0);
        self.manager.as_deref_mut().unwrap()
    }

    // -----------------------------------------------------------------------
    // start_searching and iterative deepening

    /// `start_searching()`: called when the program receives the UCI 'go'
    /// command. It searches from the root position and outputs the
    /// "bestmove". `TRACE` selects the instance of the search with the trace
    /// hooks (see `searchtrace.rs`).
    pub fn start_searching<const TRACE: bool>(&mut self) {
        self.accumulator_stack.reset();

        // Non-main threads go directly to iterative_deepening()
        if !self.is_mainthread() {
            self.iterative_deepening::<TRACE>();
            return;
        }

        let us = self.root_pos.side_to_move();
        let game_ply = self.root_pos.game_ply();
        let mut limits = std::mem::take(&mut self.limits);
        let nodestime = self.search_options.nodestime;
        let mm = self.main_manager();
        mm.tm.init(&mut limits, us, game_ply, nodestime);
        self.limits = limits;
        self.tt.new_search();
        (self.main_manager().updates.on_start)();

        if self.root_moves.is_empty() {
            let v = if self.root_pos.checkers() != 0 { -VALUE_MATE } else { VALUE_DRAW };
            let info = InfoShort { depth: 0, score: Score::new(v, &self.root_pos) };
            let updates = self.main_manager().updates.clone();
            (updates.on_update_no_moves)(&info);
            (updates.on_bestmove)(&uci::move_to_string(Move::none(), false), "");
            return;
        }

        // Main thread starts non-main threads, and begins own search (one
        // thread for now: there are none to start)
        let mut uci_pv_sent = self.iterative_deepening::<TRACE>();

        // When we reach the maximum depth, we can arrive here without a raise
        // of threads.stop. However, if we are pondering or in an infinite
        // search, the UCI protocol states that we shouldn't print the best
        // move before the GUI sends a "stop" or "ponderhit" command. We
        // therefore simply wait here until the GUI sends one of those
        // commands.
        while !self.flags.stop.load(Relaxed) && (self.flags.ponder.load(Relaxed) || self.limits.infinite != 0) {
            std::thread::yield_now();
        }

        // Stop the threads if not already stopped (also raise the stop if
        // "ponderhit" just reset threads.ponder).
        self.flags.stop.store(true, Relaxed);

        // Wait until all threads have finished (there are none)

        // When playing in 'nodes as time' mode, subtract the searched nodes
        // from the available ones before exiting.
        if self.limits.npmsec != 0 {
            let n = self.nodes as i64 - self.limits.inc[us as usize];
            self.main_manager().tm.advance_nodes_time(n);
        }

        // With one thread, get_best_thread() is this one, as is bestThread
        // when there is a depth limit or a skill level.
        let skill = Skill::new(
            self.search_options.skill_level,
            if self.search_options.limit_strength { self.search_options.uci_elo } else { 0 },
        );
        let _ = self.limits.depth == 0 && !skill.enabled();

        let (score0, avg0) = (self.root_moves[0].score, self.root_moves[0].average_score);
        let mm = self.main_manager();
        mm.best_previous_score = score0;
        mm.best_previous_average_score = avg0;

        if self.root_moves[0].pv.len() == 1 {
            let tt = self.tt.clone();
            if self.root_moves[0].extract_ponder_from_tt(&tt, &mut self.root_pos) {
                uci_pv_sent = false;
            }
        }

        // Send PV info if it has changed since last output in
        // iterative_deepening()
        if !uci_pv_sent {
            self.output_pv(self.root_depth);
        }

        // In rare cases, output_pv() may change the ponder move through
        // syzygy_extend_pv()
        let chess960 = self.root_pos.is_chess960();
        let ponder = if self.root_moves[0].pv.len() > 1 {
            uci::move_to_string(self.root_moves[0].pv[1], chess960)
        } else {
            String::new()
        };

        let bestmove = uci::move_to_string(self.root_moves[0].pv[0], chess960);
        let updates = self.main_manager().updates.clone();
        (updates.on_bestmove)(&bestmove, &ponder);
    }

    /// Main iterative deepening loop. It calls search() repeatedly with
    /// increasing depth until the allocated thinking time has been consumed,
    /// the user stops the search, or the maximum search depth is reached.
    fn iterative_deepening<const TRACE: bool>(&mut self) -> bool {
        let main_thread = self.is_mainthread();

        let mut last_best_move_pv: Vec<Move> = Vec::with_capacity(MAX_PLY);
        let mut last_best_move_depth: Depth = 0;
        let mut last_best_move_score: Value = -VALUE_INFINITE;

        let mut alpha: Value;
        let mut beta: Value;
        let mut best_value: Value = -VALUE_INFINITE;
        let us = self.root_pos.side_to_move();
        let mut time_reduction: f64 = 1.0;
        let mut tot_best_move_changes: f64 = 0.0;
        let mut delta: i32;
        let mut iter_idx: usize = 0;

        // The stack, as C++ allocates it zeroed for each search, with room
        // for (ss - 7) to (ss + 2)
        self.stack.fill(Stack::default());
        let ss = STACK_OFFSET;

        for i in (1..=7).rev() {
            let s = &mut self.stack[ss - i];
            s.continuation_history = ContinuationHistoryBlock::index(false, NO_CAPTURES, NO_PIECE, 0); // Use as a sentinel
            s.continuation_correction_history = ContinuationCorrectionHistory::index(NO_PIECE, 0);
            s.static_eval = VALUE_NONE;
        }

        for i in 0..=MAX_PLY + 2 {
            self.stack[ss + i].ply = i as i32;
        }

        // ss->pv = &pv: row 0 of the PV lines
        self.stack[ss].pv = true;
        self.pv_lines[0].clear();

        if main_thread {
            let mm = self.main_manager();
            if mm.best_previous_score == VALUE_INFINITE {
                mm.iter_value = [VALUE_ZERO; 4];
            } else {
                mm.iter_value = [mm.best_previous_score; 4];
            }
        }

        let mut multi_pv = self.search_options.multi_pv;
        let mut skill = Skill::new(
            self.search_options.skill_level,
            if self.search_options.limit_strength { self.search_options.uci_elo } else { 0 },
        );

        // When playing with strength handicap enable MultiPV search that we
        // will use behind-the-scenes to retrieve a set of possible moves.
        if skill.enabled() {
            multi_pv = multi_pv.max(4);
        }

        multi_pv = multi_pv.min(self.root_moves.len());

        let mut search_again_counter = 0;
        let mut fail_high_recovery = 0;
        let mut uci_pv_sent = false;

        self.low_ply_history.fill_all(102);

        for c in [WHITE, BLACK] {
            for e in self.main_history[c as usize].iter_mut() {
                e.set((e.get() as i32 * 729 / 1024) as i16);
            }
        }

        let (generation, n_root) = (self.tt.generation(), self.root_moves.len());
        trace!(self, TRACE, start(generation, n_root));

        // The position searched: the root position, lent to the search
        let mut pos = std::mem::take(&mut self.root_pos);
        pos.reserve_states(MAX_PLY + 10);

        // Iterative deepening loop until requested to stop or the target
        // depth is reached
        while self.root_depth + 1 < MAX_PLY as Depth
            && !self.flags.stop.load(Relaxed)
            && !(self.limits.depth != 0 && main_thread && self.root_depth >= self.limits.depth)
        {
            self.root_depth += 1;
            let root_depth = self.root_depth;
            trace!(self, TRACE, iteration(root_depth));

            // Do not tune these values. They are not intended for playing
            // strength.
            let seek_mate_threshold = 750 + 220000 / (self.root_depth * self.root_depth);

            // Signal the start of a new iteration
            if main_thread {
                uci_pv_sent = false;
            }

            // Save the last iteration's scores before the first PV line is
            // searched and all the move scores except the (new) PV are set to
            // -VALUE_INFINITE.
            for (i, rm) in self.root_moves.iter_mut().enumerate() {
                rm.previous_score = rm.score;
                rm.previous_pv.clone_from(&rm.pv);
                rm.previous_score_exact = i < multi_pv;
            }

            let mut pv_first = 0;
            self.pv_last = 0;

            if !self.flags.increase_depth.load(Relaxed) {
                search_again_counter += 1;
            }

            // MultiPV loop: we perform a full root search for each PV line
            self.pv_idx = 0;
            while self.pv_idx < multi_pv {
                let pv_idx = self.pv_idx;
                if pv_idx == self.pv_last {
                    pv_first = self.pv_last;
                    self.pv_last += 1;
                    while self.pv_last < self.root_moves.len() {
                        if self.root_moves[self.pv_last].tb_rank != self.root_moves[pv_first].tb_rank {
                            break;
                        }
                        self.pv_last += 1;
                    }
                }

                self.last_iteration_idx_pv.assign(&self.root_moves[pv_idx].previous_pv);

                // Reset UCI info selDepth for each depth and each PV line
                self.sel_depth = 0;

                // Reset aspiration window starting size
                delta = (5 + self.thread_idx % 8 + (self.root_moves[pv_idx].mean_squared_score.abs() / 10193) as usize)
                    as i32;
                let avg = self.root_moves[pv_idx].average_score;
                alpha = (avg - delta).max(-VALUE_INFINITE);
                beta = (avg + delta).min(VALUE_INFINITE);

                self.seek_mate = self.root_moves[pv_idx].previous_score != -VALUE_INFINITE
                    && self.root_moves[pv_idx].previous_score.abs() >= seek_mate_threshold;

                // Adjust optimism based on root move's averageScore
                self.optimism[us as usize] = 456 * avg / (avg.abs() + 85);
                self.optimism[not_color(us) as usize] = -self.optimism[us as usize];

                // Start with a small aspiration window and, in the case of a
                // fail high/low, enlarge the window progressively.
                let mut failed_high_cnt = 0;
                if pv_idx == 0 {
                    fail_high_recovery = (fail_high_recovery - 2).max(0);
                }
                loop {
                    // Adjust the effective depth searched, but ensure at least
                    // one effective increment for every four searchAgain steps
                    // (see issue #2717).
                    let adjusted_depth = 1.max(
                        self.root_depth - failed_high_cnt - fail_high_recovery - 3 * (search_again_counter + 1) / 4,
                    );
                    let (opt, seek_mate) = (self.optimism[us as usize], self.seek_mate);
                    trace!(self, TRACE, aspiration(adjusted_depth, alpha, beta, delta, avg, opt, seek_mate));
                    best_value = self.search::<ROOT, TRACE>(&mut pos, ss, alpha, beta, adjusted_depth, false);

                    // Bring the best move to the front. It is critical that
                    // sorting is done with a stable algorithm because all the
                    // values but the first and eventually the new best one is
                    // set to -VALUE_INFINITE and we want to keep the same
                    // order for all the moves except the new PV that goes to
                    // the front. Note that in the case of MultiPV search the
                    // already searched PV lines are preserved.
                    stable_sort(&mut self.root_moves[pv_idx..self.pv_last]);
                    let stopped = self.flags.stop.load(Relaxed);
                    trace!(self, TRACE, aspiration_result(best_value, alpha, beta, stopped));

                    // If search has been stopped, we break immediately.
                    // Sorting is safe because RootMoves is still valid,
                    // although it refers to the previous iteration.
                    if stopped {
                        break;
                    }

                    // When failing high/low give some update before a
                    // re-search. To avoid excessive output that could hang
                    // GUIs like Fritz 19, only start at nodes > 10M (rather
                    // than depth N, which can be reached quickly).
                    if main_thread
                        && multi_pv == 1
                        && (best_value <= alpha || best_value >= beta)
                        && self.nodes > NODES_LIMIT_OUTPUT
                    {
                        self.root_pos = std::mem::take(&mut pos);
                        self.output_pv(self.root_depth);
                        pos = std::mem::take(&mut self.root_pos);
                    }

                    // In case of failing low/high increase aspiration window
                    // and re-search, otherwise exit the loop.
                    if best_value <= alpha {
                        beta = alpha;
                        alpha = (best_value - delta).max(-VALUE_INFINITE);

                        failed_high_cnt = 0;
                        if main_thread {
                            self.main_manager().stop_on_ponderhit = false;
                        }
                    } else if best_value >= beta {
                        alpha = (beta - delta).max(alpha);
                        beta = (best_value + delta).min(VALUE_INFINITE);
                        failed_high_cnt += 1;
                    } else {
                        break;
                    }

                    delta += 47 * delta / 128;

                    debug_assert!(alpha >= -VALUE_INFINITE && beta <= VALUE_INFINITE);
                }

                // Gradually increase depth after reduced depth search
                if failed_high_cnt > 0 && pv_idx == 0 {
                    fail_high_recovery = (failed_high_cnt + 1) / 2 + 2;
                }

                let stopped = self.flags.stop.load(Relaxed);
                if stopped && pv_idx != 0 {
                    self.guard_aborted_multipv_line(pv_idx, multi_pv);
                }

                // Sort the PV lines searched so far and update the GUI
                stable_sort(&mut self.root_moves[pv_first..pv_idx + 1]);

                if main_thread && !stopped && (pv_idx + 1 == multi_pv || self.nodes > NODES_LIMIT_OUTPUT) {
                    self.root_pos = std::mem::take(&mut pos);
                    self.output_pv(self.root_depth);
                    pos = std::mem::take(&mut self.root_pos);
                    uci_pv_sent = pv_idx + 1 == multi_pv;
                }

                if stopped {
                    break;
                }
                self.pv_idx += 1;
            }

            let stopped = self.flags.stop.load(Relaxed);
            let forgotten_mate = last_best_move_score != -VALUE_INFINITE
                && is_mate_or_mated(last_best_move_score)
                && (self.root_moves[0].score.abs() < last_best_move_score.abs() || self.root_moves[0].is_inexact());

            if !stopped {
                if last_best_move_pv.is_empty() || last_best_move_pv[0] != self.root_moves[0].pv[0] {
                    last_best_move_depth = self.root_depth;
                }

                // Do not replace (shorter) mate scores from a previous
                // iteration
                if !forgotten_mate {
                    last_best_move_pv.clone_from(&self.root_moves[0].pv);
                    last_best_move_score = self.root_moves[0].score;
                }
            }

            let aborted_loss_search = stopped && self.pv_idx == 0 && self.root_moves[0].is_exact_loss();

            // An exact mated-in/TB-loss score from an aborted search cannot be
            // trusted: the loss could be delayed or refuted upon exploring the
            // remaining root-moves. Thus here we roll back to the score from
            // the previous iteration. We do the same if a search has failed to
            // recover a mate score that was found in a previous iteration.
            if aborted_loss_search || (self.root_moves[0].score != -VALUE_INFINITE && forgotten_mate) {
                // Bring the last best move to the front for best thread
                // selection
                if !last_best_move_pv.is_empty() {
                    if let Some(i) = self.root_moves.iter().position(|rm| rm.pv[0] == last_best_move_pv[0]) {
                        self.root_moves[..=i].rotate_right(1);
                    }
                    let rm = &mut self.root_moves[0];
                    rm.score = last_best_move_score;
                    rm.uci_score = last_best_move_score;
                    rm.pv.clone_from(&last_best_move_pv);
                    rm.unset_inexact();

                    if main_thread {
                        uci_pv_sent = false;
                    }
                }
                // For an aborted d1 search we label the loss score as inexact
                else if aborted_loss_search {
                    self.root_moves[0].inexact_lower = true;
                }
            }

            // Have we found a "mate in x" after a completed iteration?
            if self.limits.mate != 0
                && !stopped
                && is_mate_or_mated(self.root_moves[0].score)
                && VALUE_MATE - self.root_moves[0].score.abs() <= 2 * self.limits.mate
            {
                self.flags.stop.store(true, Relaxed);
            }

            if TRACE {
                let stopped = self.flags.stop.load(Relaxed);
                if let Some(t) = self.tracer.as_deref_mut() {
                    t.iteration_end(&self.root_moves[0], self.nodes, self.root_depth, stopped);
                }
            }

            if !main_thread {
                continue;
            }

            // If the skill level is enabled and time is up, pick a
            // sub-optimal best move
            if skill.enabled() && skill.time_to_pick(self.root_depth) {
                let mm = self.manager.as_deref_mut().unwrap();
                skill.pick_best(&self.root_moves, multi_pv, &mut mm.skill_rng);
            }

            // Do we have time for the next iteration? Can we stop searching
            // now? Time management is not ported yet (see README.md): the
            // engine refuses a search with a clock.
            if self.limits.use_time_management() && !self.flags.stop.load(Relaxed) {
                let _ = (&mut time_reduction, &mut tot_best_move_changes, last_best_move_depth, iter_idx);
                unreachable!("time management is not ported");
            }

            self.main_manager().iter_value[iter_idx] = best_value;
            iter_idx = (iter_idx + 1) & 3;
        }

        self.root_pos = pos;

        if !main_thread {
            return false;
        }

        self.main_manager().previous_time_reduction = time_reduction;

        // If the skill level is enabled, swap the best PV line with the
        // sub-optimal one
        if skill.enabled() {
            let pick = if skill.best != Move::none() {
                skill.best
            } else {
                let mm = self.manager.as_deref_mut().unwrap();
                skill.pick_best(&self.root_moves, multi_pv, &mut mm.skill_rng)
            };
            if pick != self.root_moves[0].pv[0] {
                let i = self.root_moves.iter().position(|rm| rm.pv[0] == pick).unwrap();
                self.root_moves.swap(0, i);
                uci_pv_sent = false;
            }
        }

        uci_pv_sent
    }

    /// The handling of an aborted search of a MultiPV line other than the
    /// first, in `iterative_deepening()`. MultiPV is checked by nothing yet.
    fn guard_aborted_multipv_line(&mut self, pv_idx: usize, multi_pv: usize) {
        let rms = &mut self.root_moves;
        // In multiPV analysis we do not let aborted searches spoil
        // mated-in/TB loss scores from a completed search in an earlier PV
        // line. Hence we guard against an aborted pvIdx line overtaking
        // pvIdx - 1 when pvIdx - 1 is a proven loss. Moreover, we do not
        // trust an exact loss score from an aborted search.
        if (is_loss(rms[pv_idx - 1].score) && rms[pv_idx].before(&rms[pv_idx - 1])) || rms[pv_idx].is_exact_loss() {
            // If previousScore is exact and worse than pvIdx - 1, we can
            // safely use it. If it is equal, we make sure it cannot overtake
            // pvIdx - 1.
            if rms[pv_idx].previous_score != -VALUE_INFINITE
                && rms[pv_idx].previous_score_exact
                && rms[pv_idx].previous_score <= rms[pv_idx - 1].score
            {
                let rm = &mut rms[pv_idx];
                rm.score = rm.previous_score;
                rm.uci_score = rm.previous_score;
                rm.previous_score = -VALUE_INFINITE;
                let prev = std::mem::take(&mut rm.previous_pv);
                rm.pv.clone_from(&prev);
                rm.previous_pv = prev;
                rm.unset_inexact();
            }
            // Otherwise, if we can, we cap the score to the best possible, and
            // mark the score as inexact (also a valid excuse for the
            // incomplete PV).
            else {
                if is_loss(rms[pv_idx - 1].score) {
                    let s = rms[pv_idx - 1].score;
                    let rm = &mut rms[pv_idx];
                    rm.score = s;
                    rm.uci_score = s;
                    rm.previous_score = -VALUE_INFINITE;
                    rm.pv.truncate(1);
                    rm.inexact_upper = true;
                } else {
                    rms[pv_idx].inexact_upper = false;
                }

                rms[pv_idx].inexact_lower = !rms[pv_idx].inexact_upper;
            }
        }

        // Finally, we mark all loss scores from partially searched moves as
        // inexact.
        for rm in &mut rms[pv_idx + 1..multi_pv] {
            if rm.is_exact_loss() {
                rm.inexact_lower = true;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Making moves

    /// `do_move(pos, move, st, givesCheck, capture, ss)`: makes a move of the
    /// search, counts the node, and records it in the stack (`ss` none for
    /// the moves outside the tree).
    #[inline(always)]
    fn do_move(&mut self, pos: &mut Position, m: Move, gives_check: bool, capture: bool, ss: usize) {
        self.nodes += 1;

        let dirties = self.accumulator_stack.push(pos);
        pos.do_move_dirties(m, gives_check, dirties);

        let pc = self.accumulator_stack.latest().dirties.dirty_piece.pc;
        let in_check = self.stack[ss].in_check;
        let s = &mut self.stack[ss];
        s.current_move = m;
        s.continuation_history = ContinuationHistoryBlock::index(in_check, capture as StatsType, pc, m.to_sq());
        s.continuation_correction_history = ContinuationCorrectionHistory::index(pc, m.to_sq());
    }

    #[inline(always)]
    fn do_null_move(&mut self, pos: &mut Position, ss: usize) {
        pos.do_null_move();
        let s = &mut self.stack[ss];
        s.current_move = Move::null();
        s.continuation_history = ContinuationHistoryBlock::index(false, NO_CAPTURES, NO_PIECE, 0);
        s.continuation_correction_history = ContinuationCorrectionHistory::index(NO_PIECE, 0);
    }

    #[inline(always)]
    fn undo_move(&mut self, pos: &mut Position, m: Move) {
        pos.undo_move(m);
        self.accumulator_stack.pop();
    }

    #[inline(always)]
    fn undo_null_move(&mut self, pos: &mut Position) {
        pos.undo_null_move();
    }

    /// Evaluates the current position of the game tree, from the point of
    /// view of the side to move.
    #[inline(always)]
    fn evaluate(&mut self, pos: &Position) -> Value {
        let net = self.network.as_deref().expect("a network");
        crate::evaluate::evaluate(
            net,
            pos,
            &mut self.accumulator_stack,
            &mut self.refresh_table,
            self.optimism[pos.side_to_move() as usize],
        )
    }

    #[inline(always)]
    fn reduction(&self, i: bool, d: Depth, mn: i32) -> i32 {
        let reduction_scale = REDUCTIONS[d as usize] * REDUCTIONS[mn as usize];
        reduction_scale + (!i) as i32 * reduction_scale * 197 / 512 + 948
    }

    // -----------------------------------------------------------------------
    // The correction history

    fn correction_value(&self, pos: &Position, ss: usize) -> i32 {
        let us = pos.side_to_move() as usize;
        let m = self.stack[ss - 1].current_move;
        let shared = &*self.shared_history;
        let pcv = shared.pawn_correction_entry(pos)[us].pawn.get() as i32;
        let micv = shared.minor_piece_correction_entry(pos)[us].minor.get() as i32;
        let wnpcv = shared.nonpawn_correction_entry::<WHITE>(pos)[us].non_pawn_white.get() as i32;
        let bnpcv = shared.nonpawn_correction_entry::<BLACK>(pos)[us].non_pawn_black.get() as i32;
        let cntcv = if m.is_ok() {
            let to = m.to_sq();
            let pc = pos.piece_on(to);
            let cch = |i: usize| {
                self.continuation_correction_history.at(self.stack[ss - i].continuation_correction_history)[pc as usize]
                    [to as usize]
                    .get() as i32
            };
            7885 * (cch(2) + cch(4)) + 6307 * cch(6)
        } else {
            80695
        };

        13806 * pcv + 9512 * micv + 11615 * (wnpcv + bnpcv) + cntcv
    }

    fn update_correction_history<const TRACE: bool>(&mut self, pos: &Position, ss: usize, bonus: i32) {
        let m = self.stack[ss - 1].current_move;
        let us = pos.side_to_move() as usize;

        const NON_PAWN_WEIGHT: i32 = 186;
        let shared = &*self.shared_history;

        shared.pawn_correction_entry(pos)[us].pawn.update(bonus);
        shared.minor_piece_correction_entry(pos)[us].minor.update(bonus * 150 / 128);
        shared.nonpawn_correction_entry::<WHITE>(pos)[us].non_pawn_white.update(bonus * NON_PAWN_WEIGHT / 128);
        shared.nonpawn_correction_entry::<BLACK>(pos)[us].non_pawn_black.update(bonus * NON_PAWN_WEIGHT / 128);

        if m.is_ok() {
            let to = m.to_sq();
            let pc = pos.piece_on(to);
            for (i, w) in [(2, 130), (4, 70), (6, 35)] {
                let idx = self.stack[ss - i].continuation_correction_history;
                self.continuation_correction_history.at_mut(idx)[pc as usize][to as usize].update(bonus * w / 128);
            }
        }

        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.correction_update(
                    &self.shared_history,
                    &self.continuation_correction_history,
                    &self.stack,
                    pos,
                    ss,
                    bonus,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // The history updates

    /// Updates the continuation histories for the move pairs formed by the
    /// current move and the moves played in previous plies.
    fn update_continuation_histories(&self, ss: usize, pc: Piece, to: Square, bonus: i32) {
        const CONTHIST_BONUSES: [(usize, i32); 5] = [(1, 520), (2, 390), (3, 145), (4, 251), (6, 209)];

        // Multipliers for positive history consistency
        const CMHC_MULTIPLIERS: [i32; 6] = [94, 103, 110, 106, 119, 126];
        let mut positive_count = 0;

        for (i, weight) in CONTHIST_BONUSES {
            // Only update the first 2 continuation histories if we are in
            // check
            if self.stack[ss].in_check && i > 2 {
                break;
            }

            if self.stack[ss - i].current_move.is_ok() {
                let history_entry = &self.cont_hist(self.stack[ss - i].continuation_history)[pc as usize][to as usize];
                if history_entry.get() > 0 {
                    positive_count += 1;
                }

                let multiplier = CMHC_MULTIPLIERS[positive_count];
                history_entry.update(bonus * weight * multiplier / 65536 + 73 * (i < 2) as i32);
            }
        }
    }

    /// Updates move sorting heuristics.
    fn update_quiet_histories<const TRACE: bool>(&mut self, pos: &Position, ss: usize, m: Move, bonus: i32) {
        let us = pos.side_to_move() as usize;
        self.main_history[us][m.raw() as usize].update(bonus); // Untuned to prevent duplicate effort

        let ply = self.stack[ss].ply as usize;
        if ply < LOW_PLY_HISTORY_SIZE {
            self.low_ply_history[ply][m.raw() as usize].update(bonus * 712 / 1024);
        }

        self.update_continuation_histories(ss, pos.moved_piece(m), m.to_sq(), bonus * 750 / 1024);

        self.shared_history.pawn_entry(pos)[pos.moved_piece(m) as usize][m.to_sq() as usize]
            .update(bonus * (if bonus > -4 { 1104 } else { 459 }) / 1024);

        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.quiet_update(
                    &self.main_history,
                    &self.low_ply_history,
                    &self.shared_history,
                    &self.stack,
                    pos,
                    ss,
                    m,
                    bonus,
                );
            }
        }
    }

    /// The trace of a direct call of `update_continuation_histories()`.
    #[inline(always)]
    fn trace_continuation_update<const TRACE: bool>(&mut self, ss: usize, pc: Piece, to: Square, bonus: i32) {
        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.continuation_update(&self.shared_history, &self.stack, ss, pc, to, bonus);
            }
        }
    }

    /// Updates stats at the end of search() when a bestMove is found.
    #[allow(clippy::too_many_arguments)]
    fn update_all_stats<const TRACE: bool>(
        &mut self,
        pos: &Position,
        ss: usize,
        best_move: Move,
        prev_sq: Square,
        quiets_searched: &SearchedList,
        captures_searched: &SearchedList,
        depth: Depth,
        tt_move: Move,
        pv_node: bool,
    ) {
        let mut moved_piece = pos.moved_piece(best_move);
        let mut captured_piece: PieceType;

        let mut bonus =
            (133 * depth - 81).min(1487) + 364 * (best_move == tt_move) as i32 + self.stack[ss - 1].stat_score / 28;
        let malus = (968 * depth - 235).min(2244);

        if !pv_node {
            // Important: don't remove the cast to a 64-bit number else the
            // multiplication can overflow on 32-bit platforms which would
            // change the bench signature. C++ converts the int to u64 (a
            // negative bonus wraps), divides unsigned, and truncates back.
            let n = (quiets_searched.len() + captures_searched.len()) as u64;
            bonus += ((bonus as i64 as u64).wrapping_mul(n) / 256) as i32;
        }

        trace!(self, TRACE, event2("all_stats", "bonus", bonus as i64, "malus", malus as i64));

        if !pos.capture_stage(best_move) {
            self.update_quiet_histories::<TRACE>(pos, ss, best_move, bonus * 899 / 1024);

            // Decrease stats for all non-best quiet moves
            let mut actual_malus = malus * 1159 / 1024;
            for &m in quiets_searched.as_slice() {
                actual_malus = actual_malus * 921 / 1024;
                self.update_quiet_histories::<TRACE>(pos, ss, m, -actual_malus);
            }
        } else {
            // Increase stats for the best move in case it was a capture move
            captured_piece = type_of(pos.piece_on(best_move.to_sq()));
            self.capture_history[moved_piece as usize][best_move.to_sq() as usize][captured_piece as usize]
                .update(bonus * 1427 / 1024);
            self.trace_capture_update::<TRACE>(pos, best_move, bonus * 1427 / 1024);
        }

        // Extra penalty for a quiet early move that was not a TT move in
        // previous ply when it gets refuted.
        if prev_sq != SQ_NONE
            && self.stack[ss - 1].move_count == 1 + self.stack[ss - 1].tt_hit as i32
            && pos.captured_piece() == NO_PIECE
        {
            let pc = pos.piece_on(prev_sq);
            self.update_continuation_histories(ss - 1, pc, prev_sq, -malus * 713 / 1024);
            self.trace_continuation_update::<TRACE>(ss - 1, pc, prev_sq, -malus * 713 / 1024);
        }

        // Decrease stats for all non-best capture moves
        for &m in captures_searched.as_slice() {
            moved_piece = pos.moved_piece(m);
            captured_piece = type_of(pos.piece_on(m.to_sq()));
            self.capture_history[moved_piece as usize][m.to_sq() as usize][captured_piece as usize]
                .update(-malus * 1489 / 1024);
            self.trace_capture_update::<TRACE>(pos, m, -malus * 1489 / 1024);
        }
    }

    #[inline(always)]
    fn trace_capture_update<const TRACE: bool>(&mut self, pos: &Position, m: Move, bonus: i32) {
        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.capture_update(&self.capture_history, pos, m, bonus);
            }
        }
    }

    #[inline(always)]
    fn trace_ttmove_update<const TRACE: bool>(&mut self, bonus: i32) {
        if TRACE {
            let v = self.tt_move_history.get();
            if let Some(t) = self.tracer.as_deref_mut() {
                t.ttmove_update(bonus, v);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free functions of search.cpp

/// Adds a small random component to draw evaluations to avoid 3-fold
/// blindness.
#[inline(always)]
fn value_draw(nodes: u64) -> Value {
    VALUE_DRAW - 1 + (nodes & 0x2) as Value
}

/// Adds the correction history value to the raw static evaluation and
/// guarantees that the evaluation does not hit the tablebase range.
#[inline(always)]
fn to_corrected_static_eval(v: Value, cv: i32) -> Value {
    (v + cv / 131072).clamp(VALUE_TB_LOSS_IN_MAX_PLY + 1, VALUE_TB_WIN_IN_MAX_PLY - 1)
}

/// Adjusts a mate or TB score from "plies to mate from the root" to "plies
/// to mate from the current position". Standard scores are unchanged. The
/// function is called before storing a value in the transposition table.
#[inline(always)]
fn value_to_tt(v: Value, ply: i32) -> Value {
    if is_win(v) {
        v + ply
    } else if is_loss(v) {
        v - ply
    } else {
        v
    }
}

/// Inverse of value_to_tt(): it adjusts a mate or TB score from the
/// transposition table (which refers to the plies to mate/be mated from
/// current position) to "plies to mate/be mated (TB win/loss) from the root".
/// However, to avoid potentially false mate or TB scores related to the 50
/// moves rule and the graph history interaction, we return the highest non-TB
/// score instead.
#[inline(always)]
fn value_from_tt(v: Value, ply: i32, r50c: i32) -> Value {
    if !is_valid(v) {
        return VALUE_NONE;
    }

    // Handle TB win or better
    if is_win(v) {
        // Downgrade a potentially false mate score
        if is_mate(v) && VALUE_MATE - v > 100 - r50c {
            return VALUE_TB_WIN_IN_MAX_PLY - 1;
        }

        // Downgrade a potentially false TB score
        if VALUE_TB - v > 100 - r50c {
            return VALUE_TB_WIN_IN_MAX_PLY - 1;
        }

        return v - ply;
    }

    // Handle TB loss or worse
    if is_loss(v) {
        // Downgrade a potentially false mate score
        if is_mated(v) && VALUE_MATE + v > 100 - r50c {
            return VALUE_TB_LOSS_IN_MAX_PLY + 1;
        }

        // Downgrade a potentially false TB score
        if VALUE_TB + v > 100 - r50c {
            return VALUE_TB_LOSS_IN_MAX_PLY + 1;
        }

        return v + ply;
    }

    v
}

/// Detects shuffling moves in order to limit search explosions. This code
/// was added in #6447 as non-regression, and so its parameters should not be
/// tuned.
#[inline(always)]
fn is_shuffling(m: Move, stack: &[Stack], ss: usize, pos: &Position) -> bool {
    if pos.capture_stage(m) || pos.rule50_count() < 10 {
        return false;
    }
    if pos.st().plies_from_null < 6 || stack[ss].ply < 20 {
        return false;
    }
    m.from_sq() == stack[ss - 2].current_move.to_sq()
        && stack[ss - 2].current_move.from_sq() == stack[ss - 4].current_move.to_sq()
}

// ---------------------------------------------------------------------------
// search() and qsearch()

impl Worker {
    /// `search<nodeType>()`, the main search function for both PV and non-PV
    /// nodes. With `TRACE`, the entry and the result of each node are
    /// recorded around `search_node()`, as in the C++ tracing binary;
    /// without it this is `search_node()` itself.
    #[inline(always)]
    fn search<const NT: NodeType, const TRACE: bool>(
        &mut self,
        pos: &mut Position,
        ss: usize,
        alpha: Value,
        beta: Value,
        depth: Depth,
        cut_node: bool,
    ) -> Value {
        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.enter_search(pos, &self.stack[ss], alpha, beta, depth, NT, cut_node, self.nodes);
            }
            let value = self.search_node::<NT, TRACE>(pos, ss, alpha, beta, depth, cut_node);
            trace!(self, TRACE, leave(value));
            value
        } else {
            self.search_node::<NT, TRACE>(pos, ss, alpha, beta, depth, cut_node)
        }
    }

    /// `qsearch<nodeType>()`, with the same wrapping as `search()`.
    #[inline(always)]
    fn qsearch<const NT: NodeType, const TRACE: bool>(
        &mut self,
        pos: &mut Position,
        ss: usize,
        alpha: Value,
        beta: Value,
    ) -> Value {
        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.enter_qsearch(pos, &self.stack[ss], alpha, beta, NT == PV, self.nodes);
            }
            let value = self.qsearch_node::<NT, TRACE>(pos, ss, alpha, beta);
            trace!(self, TRACE, leave(value));
            value
        } else {
            self.qsearch_node::<NT, TRACE>(pos, ss, alpha, beta)
        }
    }

    /// Main search function for both PV and non-PV nodes. (Its conditions
    /// are written as in C++, hence the allowed lint.)
    #[allow(clippy::cognitive_complexity, clippy::nonminimal_bool)]
    fn search_node<const NT: NodeType, const TRACE: bool>(
        &mut self,
        pos: &mut Position,
        ss: usize,
        mut alpha: Value,
        mut beta: Value,
        mut depth: Depth,
        cut_node: bool,
    ) -> Value {
        let pv_node = NT != NON_PV;
        let root_node = NT == ROOT;
        let all_node = !(pv_node || cut_node);

        // Dive into quiescence search when the depth reaches zero
        if depth <= 0 {
            return if pv_node {
                self.qsearch::<PV, TRACE>(pos, ss, alpha, beta)
            } else {
                self.qsearch::<NON_PV, TRACE>(pos, ss, alpha, beta)
            };
        }

        // Limit the depth if extensions made it too large
        depth = depth.min(MAX_PLY as Depth - 1);

        // Check if we have an upcoming move that draws by repetition
        if !root_node && alpha < VALUE_DRAW && pos.upcoming_repetition(self.stack[ss].ply) {
            alpha = value_draw(self.nodes);
            trace!(self, TRACE, repetition_draw(alpha));
            if alpha >= beta {
                return alpha;
            }
        }

        debug_assert!(-VALUE_INFINITE <= alpha && alpha < beta && beta <= VALUE_INFINITE);
        debug_assert!(pv_node || (alpha == beta - 1));
        debug_assert!(0 < depth && depth < MAX_PLY as Depth);
        debug_assert!(!(pv_node && cut_node));

        // The local PV line of the node (`PVMoves pv`), empty
        let ply = self.stack[ss].ply;
        if pv_node {
            self.pv_lines[ply as usize + 1].clear();
        }

        let mut mv: Move;
        let mut extension: Depth;
        let mut new_depth: Depth;
        let mut value: Value;
        let mut capture: bool;

        let mut captures_searched = SearchedList::new(Move::none());
        let mut quiets_searched = SearchedList::new(Move::none());

        // Step 1. Initialize node
        let in_check = pos.checkers() != 0;
        self.stack[ss].in_check = in_check;
        let prior_capture = pos.captured_piece() != NO_PIECE;
        let us = pos.side_to_move();
        self.stack[ss].move_count = 0;
        let mut best_value = -VALUE_INFINITE;
        // (lowered only by the tablebase probe of step 7, not ported)
        let max_value = VALUE_INFINITE;

        self.stack[ss].follow_pv = root_node
            || (self.stack[ss - 1].follow_pv
                && ((ply - 1) as usize) < self.last_iteration_idx_pv.len()
                && self.stack[ss - 1].current_move == self.last_iteration_idx_pv.as_slice()[(ply - 1) as usize]);

        // Check for the available remaining time
        if self.is_mainthread() {
            self.check_time();
        }

        // Used to send selDepth info to GUI (selDepth counts from 1, ply from 0)
        if pv_node && self.sel_depth < ply + 1 {
            self.sel_depth = ply + 1;
        }

        if !root_node {
            // Step 2. Check for aborted search or immediate draw
            let stop = self.flags.stop.load(Relaxed);
            if stop || pos.is_draw(ply) || ply >= MAX_PLY as i32 {
                if TRACE {
                    let reason = if stop {
                        "stop"
                    } else if pos.is_draw(ply) {
                        "draw"
                    } else {
                        "max_ply"
                    };
                    trace!(self, TRACE, immediate(reason));
                }
                return if ply >= MAX_PLY as i32 && !in_check { self.evaluate(pos) } else { value_draw(self.nodes) };
            }

            // Step 3. Mate distance pruning. Even if we mate at the next move
            // our score would be at best mate_in(ss->ply + 1), but if alpha is
            // already bigger because a shorter mate was found upward in the
            // tree then there is no need to search because we will never beat
            // the current alpha. Equal and opposite logic applies when being
            // mated. In either case, return a fail-high score.
            alpha = mated_in(ply).max(alpha);
            beta = mate_in(ply + 1).min(beta);
            if alpha >= beta {
                trace!(self, TRACE, event("mate_distance_cutoff"));
                return alpha;
            }
        }

        debug_assert!(0 <= ply && ply < MAX_PLY as i32);

        let prev_move = self.stack[ss - 1].current_move;
        let prev_sq = if prev_move.is_ok() { prev_move.to_sq() } else { SQ_NONE };
        let mut best_move = Move::none();
        let prior_reduction = self.stack[ss - 1].reduction;
        self.stack[ss - 1].reduction = 0;
        self.stack[ss].stat_score = 0;
        self.stack[ss + 2].cutoff_cnt = 0;
        self.stack[ss + 1].prior_nmp_fail_high = 0;

        let correction_value = self.correction_value(pos, ss);

        // Step 4. Transposition table lookup
        let excluded_move = self.stack[ss].excluded_move;
        let pos_key = pos.key();
        let (tt_hit, mut tt_data, tt_writer) = self.tt.probe(pos_key);

        self.stack[ss].tt_hit = tt_hit;
        tt_data.mv = if root_node {
            self.root_moves[self.pv_idx].pv[0]
        } else if tt_hit {
            tt_data.mv
        } else {
            Move::none()
        };
        tt_data.value = if tt_hit { value_from_tt(tt_data.value, ply, pos.rule50_count()) } else { VALUE_NONE };
        if excluded_move == Move::none() {
            self.stack[ss].tt_pv = pv_node || (tt_hit && tt_data.is_pv);
        }
        let tt_capture = tt_data.mv != Move::none() && pos.capture_stage(tt_data.mv);
        if TRACE {
            let tt_pv = self.stack[ss].tt_pv;
            if let Some(t) = self.tracer.as_deref_mut() {
                t.tt(&self.tt, tt_writer, tt_hit, &tt_data, tt_pv, Some(tt_capture));
            }
        }

        // Step 5. Static evaluation of the position
        let mut unadjusted_static_eval = VALUE_NONE;
        let mut eval: Value;

        // Skip early pruning when in check
        if in_check {
            eval = self.stack[ss - 2].static_eval;
            self.stack[ss].static_eval = eval;
        } else if excluded_move != Move::none() {
            eval = self.stack[ss].static_eval;
            unadjusted_static_eval = eval;
        } else if tt_hit {
            // Never assume anything about values stored in TT
            unadjusted_static_eval = tt_data.eval;
            if !is_valid(unadjusted_static_eval) {
                unadjusted_static_eval = self.evaluate(pos);
            }

            eval = to_corrected_static_eval(unadjusted_static_eval, correction_value);
            self.stack[ss].static_eval = eval;

            // ttValue can be used as a better position evaluation
            if is_valid(tt_data.value)
                && (tt_data.bound & (if tt_data.value > eval { BOUND_LOWER } else { BOUND_UPPER })) != 0
            {
                eval = tt_data.value;
            }
        } else {
            unadjusted_static_eval = self.evaluate(pos);
            eval = to_corrected_static_eval(unadjusted_static_eval, correction_value);
            self.stack[ss].static_eval = eval;

            // Static evaluation is saved as it was before adjustment by
            // correction history
            let tt_pv = self.stack[ss].tt_pv;
            self.tt.write(
                tt_writer,
                pos_key,
                VALUE_NONE,
                tt_pv,
                BOUND_NONE,
                DEPTH_UNSEARCHED,
                Move::none(),
                unadjusted_static_eval,
                self.tt.generation(),
            );
            self.trace_store::<TRACE>("store", tt_writer);
        }

        // Set up the improving flag, which is true if current static
        // evaluation is bigger than the previous static evaluation at our turn
        // (if we were in check at our previous move we go back until we
        // weren't in check) and is false otherwise. The improving flag is used
        // in various pruning heuristics. Similarly, opponentWorsening is true
        // if our static evaluation is better for us than at the last ply.
        let static_eval = self.stack[ss].static_eval;
        let mut improving = static_eval > self.stack[ss - 2].static_eval;
        let opponent_worsening = static_eval > -self.stack[ss - 1].static_eval;
        trace!(
            self,
            TRACE,
            static_eval(unadjusted_static_eval, static_eval, eval, correction_value, improving, opponent_worsening)
        );

        // Hindsight adjustment of reductions based on static evaluation
        // difference
        if prior_reduction >= 3 && !opponent_worsening {
            depth += 1;
            trace!(self, TRACE, event1("hindsight_extension", "depth", depth as i64));
        }
        if prior_reduction >= 2 && depth >= 2 && static_eval + self.stack[ss - 1].static_eval > 166 {
            depth -= 1;
            trace!(self, TRACE, event1("hindsight_reduction", "depth", depth as i64));
        }

        // Step 6. At non-PV nodes we check for an early TT cutoff. Note that
        // we always check the validity of the TT value because of access
        // races.
        if !pv_node
            && excluded_move == Move::none()
            && is_valid(tt_data.value)
            && tt_data.depth > depth - (tt_data.value <= beta) as i32
        {
            // Case A: TT entry can produce a cutoff
            if (tt_data.bound & (if tt_data.value >= beta { BOUND_LOWER } else { BOUND_UPPER })) != 0
                && (cut_node == (tt_data.value >= beta) || depth > 4)
            {
                // If the ttMove is quiet, update move sorting heuristics on TT
                // hit
                if tt_data.mv != Move::none() && tt_data.value >= beta {
                    // Bonus for a quiet ttMove that fails high
                    if !tt_capture {
                        self.update_quiet_histories::<TRACE>(pos, ss, tt_data.mv, 131 * depth);
                    }

                    // Extra penalty for early quiet moves of the previous ply
                    if prev_sq != SQ_NONE && self.stack[ss - 1].move_count < 5 && !prior_capture {
                        let pc = pos.piece_on(prev_sq);
                        self.update_continuation_histories(ss - 1, pc, prev_sq, -2210);
                        self.trace_continuation_update::<TRACE>(ss - 1, pc, prev_sq, -2210);
                    }
                }

                // Partial workaround for the graph history interaction
                // problem. For high rule50 counts don't produce transposition
                // table cutoffs.
                if pos.rule50_count() < 96 {
                    if depth >= 7 && tt_data.mv != Move::none() && pos.legal(tt_data.mv) && !is_decisive(tt_data.value)
                    {
                        pos.do_move(tt_data.mv);
                        let next_pos_key = pos.key();
                        let (_, tt_data_next, _) = self.tt.probe(next_pos_key);
                        pos.undo_move(tt_data.mv);

                        // Check that the ttValue after the tt move would also
                        // trigger a cutoff
                        if !is_valid(tt_data_next.value) {
                            trace!(self, TRACE, tt_cutoff(true));
                            return tt_data.value;
                        }

                        if (tt_data.value >= beta) == (-tt_data_next.value >= beta) {
                            trace!(self, TRACE, tt_cutoff(true));
                            return tt_data.value;
                        }

                        trace!(self, TRACE, event1("tt_cutoff_refused", "next_value", tt_data_next.value as i64));
                    } else {
                        trace!(self, TRACE, tt_cutoff(false));
                        return tt_data.value;
                    }
                } else {
                    trace!(self, TRACE, event("tt_cutoff_rule50"));
                }
            }
            // Case B: No cutoff, but depth was sufficient, so the bound points
            // the wrong way.
            else if depth > 5 {
                // If such a mismatch is the only reason cutoff failed, the TT
                // entry is now useless
                self.tt.penalize(tt_writer, 1);
                self.trace_store::<TRACE>("tt_penalize", tt_writer);
            }
        }

        // Step 7. Tablebases probe: not ported (no tablebases), and the
        // cardinality is 0 without them, so C++ never probes either.
        debug_assert!(self.tb_config.cardinality == 0);

        'moves_loop: {
            if in_check {
                break 'moves_loop;
            }

            // Use static evaluation difference to improve quiet move ordering
            if prev_move.is_ok() && !self.stack[ss - 1].in_check && !prior_capture {
                let eval_diff = (-(self.stack[ss - 1].static_eval + static_eval)).clamp(-189, 194) + 60;
                self.main_history[not_color(us) as usize][prev_move.raw() as usize].update(eval_diff * 11);
                let pawn = !tt_hit && type_of(pos.piece_on(prev_sq)) != PAWN && prev_move.type_of() != PROMOTION;
                if pawn {
                    self.shared_history.pawn_entry(pos)[pos.piece_on(prev_sq) as usize][prev_sq as usize]
                        .update(eval_diff * 13);
                }
                if TRACE {
                    if let Some(t) = self.tracer.as_deref_mut() {
                        t.eval_diff_update(&self.main_history, &self.shared_history, pos, prev_move, eval_diff, pawn);
                    }
                }
            }

            // Step 8. Razoring
            // If eval is really low, skip search entirely and return the
            // qsearch value
            if all_node && eval < alpha - 342 * depth && !self.seek_mate {
                trace!(self, TRACE, event("razoring"));
                return self.qsearch::<NON_PV, TRACE>(pos, ss, alpha, beta);
            }

            // Step 9. Futility pruning: child node
            // The depth condition is important for mate finding. It should NOT
            // be tuned.
            if !self.stack[ss].tt_pv
                && depth < (if self.seek_mate { 6 } else { 19 })
                && eval >= beta
                && (tt_data.mv == Move::none() || tt_capture)
                && !is_loss(beta)
                && !is_win(eval)
            {
                let mut futility_mult: Value = (45 + depth * 4).min(85);
                futility_mult -= 20 * (!self.stack[ss].tt_hit) as i32;

                let futility_margin = futility_mult * depth
                    - (2789 * improving as i32 + 335 * opponent_worsening as i32) * futility_mult / 1024
                    + correction_value.abs() / 198435;

                if eval - futility_margin >= beta {
                    trace!(self, TRACE, event1("futility_pruning", "margin", futility_margin as i64));
                    return (661 * beta + 363 * eval) / 1024;
                }
            }

            // Step 10. Null move search with verification search
            if cut_node
                && static_eval + 50 * self.stack[ss].prior_nmp_fail_high
                    >= beta - 13 * depth - 47 * improving as i32 + 365
                && excluded_move == Move::none()
                && pos.non_pawn_material(us) != 0
                && ply >= self.nmp_min_ply
                && beta >= -2000
            {
                debug_assert!(self.stack[ss - 1].current_move != Move::null());

                // Null move dynamic reduction based on depth
                // Odd threads reduce one ply less
                let mut r: Depth = 7 + depth / 3 + ((static_eval - beta) / 256).max(0) - (self.thread_idx & 1) as i32;
                r -= self.seek_mate as i32 * r / 2;
                self.do_null_move(pos, ss);
                trace!(self, TRACE, null_move(r));

                let null_value = -self.search::<NON_PV, TRACE>(pos, ss + 1, -beta, -beta + 1, depth - r, false);

                self.undo_null_move(pos);
                trace!(self, TRACE, event1("null_move_result", "value", null_value as i64));

                // Do not return unproven mate or TB scores
                if null_value >= beta && !is_win(null_value) {
                    if self.nmp_min_ply != 0 || depth < 16 {
                        self.stack[ss].prior_nmp_fail_high += 1;
                        return null_value;
                    }

                    // Recursive verification is not allowed
                    debug_assert!(self.nmp_min_ply == 0);

                    // Do verification search at high depths, with null move
                    // pruning disabled until ply exceeds nmpMinPly.
                    self.nmp_min_ply = ply + 3 * (depth - r) / 4;
                    let min_ply = self.nmp_min_ply;
                    trace!(self, TRACE, event1("null_move_verification", "min_ply", min_ply as i64));

                    let v = self.search::<NON_PV, TRACE>(pos, ss, beta - 1, beta, depth - r, false);

                    self.nmp_min_ply = 0;
                    trace!(self, TRACE, event1("null_move_verification_result", "value", v as i64));

                    if v >= beta {
                        self.stack[ss].prior_nmp_fail_high += 1;
                        return null_value;
                    }
                }
            }

            improving |= static_eval >= beta;

            // Step 11. Internal iterative reductions
            // At sufficient depth, reduce depth for PV/Cut nodes without a
            // TTMove. (*Scaler) Making IIR more aggressive scales poorly.
            if !self.stack[ss].follow_pv && !all_node && depth >= 6 && tt_data.mv == Move::none() {
                depth -= 1;
                trace!(self, TRACE, event1("iir", "depth", depth as i64));
            }

            // Step 12. ProbCut
            // If we have a good enough capture (or queen promotion) and a
            // reduced search returns a value much above beta, we can (almost)
            // safely prune the previous move.
            let prob_cut_beta = beta + 241 - 64 * improving as i32;
            if depth >= 3 && !is_decisive(beta) && !(is_valid(tt_data.value) && tt_data.value < prob_cut_beta) {
                debug_assert!(prob_cut_beta < VALUE_INFINITE && prob_cut_beta > beta);

                let slot = picker_slot(ply, excluded_move != Move::none());
                self.pickers[slot].renew_probcut(pos, tt_data.mv, prob_cut_beta - static_eval);
                let prob_cut_depth = depth - if improving { 5 } else { 3 };
                trace!(self, TRACE, event2("probcut", "beta", prob_cut_beta as i64, "depth", prob_cut_depth as i64));

                loop {
                    mv = {
                        let h = histories!(self, ss);
                        self.pickers[slot].next_move(pos, &h)
                    };
                    if mv == Move::none() {
                        break;
                    }
                    self.trace_pick::<TRACE>(slot, mv);
                    debug_assert!(mv.is_ok());

                    if mv == excluded_move {
                        trace!(self, TRACE, skip(mv, "excluded"));
                        continue;
                    }

                    capture = pos.capture_stage(mv);
                    debug_assert!(capture);

                    let gives_check = pos.gives_check(mv);
                    self.do_move(pos, mv, gives_check, capture, ss);
                    trace!(self, TRACE, probcut_move(mv));
                    // Perform a preliminary qsearch to verify that the move
                    // holds
                    value = -self.qsearch::<NON_PV, TRACE>(pos, ss + 1, -prob_cut_beta, -prob_cut_beta + 1);

                    // If the qsearch held, perform the regular search
                    if value >= prob_cut_beta && prob_cut_depth > 0 {
                        value = -self.search::<NON_PV, TRACE>(
                            pos,
                            ss + 1,
                            -prob_cut_beta,
                            -prob_cut_beta + 1,
                            prob_cut_depth,
                            !cut_node,
                        );
                    }

                    self.undo_move(pos, mv);
                    trace!(self, TRACE, move_value(mv, value));

                    if value >= prob_cut_beta {
                        // Save ProbCut data into transposition table
                        let tt_pv = self.stack[ss].tt_pv;
                        self.tt.write(
                            tt_writer,
                            pos_key,
                            value_to_tt(value, ply),
                            tt_pv,
                            BOUND_LOWER,
                            prob_cut_depth + 1,
                            mv,
                            unadjusted_static_eval,
                            self.tt.generation(),
                        );
                        self.trace_store::<TRACE>("store", tt_writer);

                        if !is_decisive(value) {
                            trace!(self, TRACE, event("probcut_cutoff"));
                            return value - (prob_cut_beta - beta);
                        }
                    }
                }
            }
        }

        // moves_loop: When in check, search starts here

        // Step 13. A small ProbCut idea
        let prob_cut_beta = beta + 428;
        if (tt_data.bound & BOUND_LOWER) != 0
            && tt_data.depth >= depth - 4
            && tt_data.value >= prob_cut_beta
            && !is_decisive(beta)
            && is_valid(tt_data.value)
            && !is_decisive(tt_data.value)
            && !self.seek_mate
        {
            trace!(self, TRACE, event("small_probcut"));
            return prob_cut_beta;
        }

        let slot = picker_slot(ply, excluded_move != Move::none());
        self.pickers[slot].renew(pos, tt_data.mv, depth, ply);

        value = best_value;

        let mut move_count = 0;

        // Step 14. Loop through all legal moves until no moves remain or a
        // beta cutoff occurs.
        loop {
            mv = {
                let h = histories!(self, ss);
                self.pickers[slot].next_move(pos, &h)
            };
            if mv == Move::none() {
                break;
            }
            self.trace_pick::<TRACE>(slot, mv);
            debug_assert!(mv.is_ok());

            if mv == excluded_move {
                trace!(self, TRACE, skip(mv, "excluded"));
                continue;
            }

            // At root obey the "searchmoves" option and skip moves not listed
            // in Root Move List. In MultiPV mode we also skip PV moves that
            // have been already searched and those of lower "TB rank" if we
            // are in a TB root position.
            if root_node && !self.root_moves[self.pv_idx..self.pv_last].iter().any(|rm| rm.pv[0] == mv) {
                trace!(self, TRACE, skip(mv, "root"));
                continue;
            }

            move_count += 1;
            self.stack[ss].move_count = move_count;

            if root_node && self.is_mainthread() && self.nodes > NODES_LIMIT_OUTPUT {
                let s = uci::move_to_string(mv, pos.is_chess960());
                let info = InfoIteration { depth, currmove: &s, currmovenumber: move_count as usize + self.pv_idx };
                (self.main_manager().updates.on_iter)(&info);
            }
            if pv_node {
                self.stack[ss + 1].pv = false;
            }

            extension = 0;
            capture = pos.capture_stage(mv);
            let moved_piece = pos.moved_piece(mv);
            let gives_check = pos.gives_check(mv);

            // Calculate new depth for this move
            new_depth = depth - 1;

            let mut r = self.reduction(improving, depth, move_count);

            // Decrease reduction for PvNodes
            if pv_node {
                r -= 512;
            }

            // Increase reduction for ttPv nodes
            // (*Scaler) Larger values scale well.
            if self.stack[ss].tt_pv {
                r += 929;
            }

            trace!(self, TRACE, try_move(mv, move_count, capture, gives_check, r));

            // Step 15. Pruning at shallow depths.
            // Depth conditions are important for mate finding.
            if !root_node && pos.non_pawn_material(us) != 0 && !is_loss(best_value) {
                // Skip quiet moves if movecount exceeds our threshold
                if move_count >= (3 + depth * depth) / (2 - improving as i32) {
                    self.pickers[slot].skip_quiet_moves();
                    trace!(self, TRACE, event("skip_quiets"));
                }

                // Reduced depth of the next LMR search
                let mut lmr_depth = new_depth - r / 1024;

                if capture || gives_check {
                    let captured_piece = pos.piece_on(mv.to_sq());
                    let capt_hist = self.capture_history[moved_piece as usize][mv.to_sq() as usize]
                        [type_of(captured_piece) as usize]
                        .get() as i32;

                    // Futility pruning for captures
                    if !gives_check && lmr_depth < 8 {
                        let futility_value = static_eval
                            + 234
                            + 247 * lmr_depth
                            + PIECE_VALUE[captured_piece as usize]
                            + 134 * capt_hist / 1024;

                        if futility_value <= alpha {
                            trace!(self, TRACE, prune1("capture_futility", "value", futility_value as i64));
                            continue;
                        }
                    }

                    // SEE based pruning for captures and checks.
                    // Avoid pruning sacrifices of our last piece for
                    // stalemate.
                    let margin = 177 * depth + capt_hist * 34 / 1024;
                    if (alpha >= VALUE_DRAW || pos.non_pawn_material(us) != PIECE_VALUE[moved_piece as usize])
                        && !pos.see_ge(mv, -margin)
                    {
                        trace!(self, TRACE, prune1("capture_see", "margin", margin as i64));
                        continue;
                    }
                } else if !self.stack[ss].follow_pv || !pv_node {
                    let to = mv.to_sq() as usize;
                    let mut history = self.cont_hist(self.stack[ss - 1].continuation_history)[moved_piece as usize][to]
                        .get() as i32
                        + self.cont_hist(self.stack[ss - 2].continuation_history)[moved_piece as usize][to].get()
                            as i32
                        + self.shared_history.pawn_entry(pos)[moved_piece as usize][to].get() as i32;

                    // Continuation history based pruning
                    if history < -4136 * depth {
                        trace!(self, TRACE, prune1("continuation_history", "history", history as i64));
                        continue;
                    }

                    history += 69 * self.main_history[us as usize][mv.raw() as usize].get() as i32 / 32;

                    // (*Scaler): Generally, lower divisors scale well
                    lmr_depth += history / lmr_divisor(depth);

                    let futility_value = static_eval + 119 * lmr_depth + 90 * (static_eval > alpha) as i32 + 164;

                    // Futility pruning: parent node
                    // (*Scaler): Generally, more frequent futility pruning
                    // scales well
                    if !in_check && lmr_depth < 12 && futility_value <= alpha {
                        if best_value <= futility_value && !is_decisive(best_value) && !is_win(futility_value) {
                            best_value = futility_value;
                        }
                        trace!(
                            self,
                            TRACE,
                            prune2("quiet_futility", "value", futility_value as i64, "lmr_depth", lmr_depth as i64)
                        );
                        continue;
                    }

                    lmr_depth = lmr_depth.max(0);

                    // Prune moves with negative SEE
                    if !pos.see_ge(mv, -23 * lmr_depth * lmr_depth) {
                        trace!(self, TRACE, prune1("quiet_see", "lmr_depth", lmr_depth as i64));
                        continue;
                    }
                }
            }

            // Step 16. Singular Extensions
            //
            // We check for "only moves": if one move fails high on (alpha,
            // beta) but all others fail low on (alpha-s, beta-s), then that
            // move is singular. Singular moves are extended to better estimate
            // the result of the (putatively) forced line. If it's
            // non-singular, we may do some pruning or reduction.
            //
            // Recursive excluded search is avoided. The `excludedMove`
            // mechanism was historically a hack and remains a bit fragile.
            //
            // (*Scaler) Generally, higher singularBeta (i.e closer to ttValue)
            // and lower extension margins scale well.
            if !root_node
                && mv == tt_data.mv
                && excluded_move == Move::none()
                && depth >= 6 + self.stack[ss].tt_pv as i32
                && is_valid(tt_data.value)
                && !is_decisive(tt_data.value)
                && (tt_data.bound & BOUND_LOWER) != 0
                && tt_data.depth >= depth - 3
                && !is_shuffling(mv, &self.stack, ss, pos)
                && !self.seek_mate
            {
                let tt_pv = self.stack[ss].tt_pv;
                let singular_beta = tt_data.value - (59 + 66 * (tt_pv && !pv_node) as i32) * depth / 63;
                let singular_depth = new_depth / 2;

                self.stack[ss].excluded_move = mv;
                trace!(self, TRACE, event2("singular", "beta", singular_beta as i64, "depth", singular_depth as i64));
                value =
                    self.search::<NON_PV, TRACE>(pos, ss, singular_beta - 1, singular_beta, singular_depth, cut_node);
                self.stack[ss].excluded_move = Move::none();

                if value < singular_beta {
                    let corr_val_adj = correction_value.abs() / 198368;
                    let double_margin = -2 + 204 * pv_node as i32
                        - 152 * (!tt_capture) as i32
                        - corr_val_adj
                        - 1175 * self.tt_move_history.get() as i32 / 114178
                        - (ply > self.root_depth) as i32 * 38;
                    let triple_margin = 70 + 279 * pv_node as i32 - 188 * (!tt_capture) as i32
                        + 81 * self.stack[ss].tt_pv as i32
                        - corr_val_adj
                        - (ply > self.root_depth) as i32 * 43;

                    extension = 1
                        + (value < singular_beta - double_margin) as i32
                        + (value < singular_beta - triple_margin) as i32;

                    depth += 1;
                    trace!(
                        self,
                        TRACE,
                        event2("singular_extension", "extension", extension as i64, "depth", depth as i64)
                    );
                }
                // Multi-cut pruning
                // Our ttMove is assumed to fail high based on the bound of the
                // TT entry, and if after excluding the ttMove with a reduced
                // search we fail high over the original beta, we assume this
                // expected cut-node is not singular (multiple moves fail
                // high), and we can prune the whole subtree by returning a
                // softbound.
                else if value >= beta && !is_decisive(value) {
                    self.tt_move_history.update(-421 - 110 * depth);
                    self.trace_ttmove_update::<TRACE>(-421 - 110 * depth);

                    if !in_check && value > static_eval {
                        let bonus = ((value - static_eval) * 664 / 1024)
                            .clamp(-CORRECTION_HISTORY_LIMIT / 4, CORRECTION_HISTORY_LIMIT / 4);
                        self.update_correction_history::<TRACE>(pos, ss, bonus);
                    }

                    trace!(self, TRACE, event("multi_cut"));
                    return value;
                }
                // Negative extensions
                // If other moves failed high over (ttValue - margin) without
                // the ttMove on a reduced search, but we cannot do multi-cut
                // because (ttValue - margin) is lower than the original beta,
                // we do not know if the ttMove is singular or can do a
                // multi-cut, so we reduce the ttMove in favor of other moves
                // based on some conditions:

                // If the ttMove is assumed to fail high over current beta or
                // if we are on a cutNode
                else if tt_data.value >= beta || cut_node {
                    extension = -3;
                    trace!(self, TRACE, event("negative_extension"));
                }
            }

            let node_count = if root_node { self.nodes } else { 0 };

            // Step 17. Make the move
            self.do_move(pos, mv, gives_check, capture, ss);

            // Add extension to new depth
            new_depth += extension;

            // Step 18. Compute and apply late moves reductions/extensions (LMR)

            // Decrease reduction for PvNodes (*Scaler)
            if self.stack[ss].tt_pv {
                r -= 3023
                    + pv_node as i32 * 1004
                    + (tt_data.value > alpha) as i32 * 885
                    + (tt_data.depth >= depth) as i32 * (816 + cut_node as i32 * 940);
            }

            // Base reduction offset to compensate for other tweaks
            r += 697;

            r -= move_count * 65;
            r -= correction_value.abs() / 26310;

            // Increase reduction for cut nodes
            if cut_node {
                r += 4026 + 933 * (tt_data.mv == Move::none()) as i32;
            }

            // Increase reduction if ttMove is a capture
            if tt_capture {
                r += 1079;
            }

            // Increase reduction if next ply has a lot of fail high
            if self.stack[ss + 1].cutoff_cnt > 1 {
                r += 264 + 1095 * (self.stack[ss + 1].cutoff_cnt > 2) as i32 + 1138 * all_node as i32;
            }
            // For first picked move (ttMove) reduce reduction
            else if mv == tt_data.mv {
                r -= 2179;
            }

            let stat_score = if capture {
                873 * PIECE_VALUE[pos.captured_piece() as usize] / 128
                    + self.capture_history[moved_piece as usize][mv.to_sq() as usize]
                        [type_of(pos.captured_piece()) as usize]
                        .get() as i32
            } else {
                let to = mv.to_sq() as usize;
                (2252 * self.main_history[us as usize][mv.raw() as usize].get() as i32
                    + 1126
                        * self.cont_hist(self.stack[ss - 1].continuation_history)[moved_piece as usize][to].get()
                            as i32
                    + 1093
                        * self.cont_hist(self.stack[ss - 2].continuation_history)[moved_piece as usize][to].get()
                            as i32)
                    / 1024
            };
            self.stack[ss].stat_score = stat_score;

            // Decrease/increase reduction for moves with a good/bad history
            r -= stat_score * 439 / 4096;

            if !capture && !is_decisive(alpha) {
                r += 3 * (alpha - eval).clamp(-64, 96);
            }

            // Scale up reductions for expected ALL nodes
            if all_node {
                r += r * 276 / (256 * depth + 268);
            }

            // When seeking a mate, reduce non-checking moves more and checking
            // moves less
            if self.seek_mate && r > 0 {
                r += (if gives_check { -r } else { r }) / 4;
            }

            // Apply the computed LMR
            if depth >= 2 && move_count > 1 {
                // In general we want to cap the LMR depth search at newDepth,
                // but when reduction is negative, we allow this move a limited
                // search extension beyond the first move depth. To avoid
                // search explosion, extensions are not allowed deep, relative
                // to rootDepth, in the search tree.
                let d: Depth =
                    1.max(new_depth + (-r / 1024).min(if ply < 2 * self.root_depth { 2 } else { 0 })) + pv_node as i32;

                self.stack[ss].reduction = new_depth - d;
                trace!(self, TRACE, event3("lmr", "reduction", r as i64, "depth", d as i64, "stat", stat_score as i64));
                value = -self.search::<NON_PV, TRACE>(pos, ss + 1, -(alpha + 1), -alpha, d, true);
                self.stack[ss].reduction = 0;

                // Do a full-depth search when reduced LMR search fails high
                // (*Scaler) Shallower searches here don't scale well
                if value > alpha {
                    // Adjust full-depth search based on LMR results - if the
                    // result was good enough search deeper, if it was bad
                    // enough search shallower.
                    let do_deeper_search = d < new_depth && value > best_value + 53;
                    let do_shallower_search = value < best_value + 8;

                    new_depth += do_deeper_search as i32 - do_shallower_search as i32;

                    if new_depth > d {
                        trace!(self, TRACE, event1("lmr_research", "depth", new_depth as i64));
                        value = -self.search::<NON_PV, TRACE>(pos, ss + 1, -(alpha + 1), -alpha, new_depth, !cut_node);
                    }

                    // Post LMR continuation history updates
                    self.update_continuation_histories(ss, moved_piece, mv.to_sq(), 1334);
                    self.trace_continuation_update::<TRACE>(ss, moved_piece, mv.to_sq(), 1334);
                }
            }
            // Step 19. Full-depth search when LMR is skipped
            else if !pv_node || move_count > 1 {
                // Increase reduction if ttMove is not present
                if tt_data.mv == Move::none() {
                    r += 1127;
                }

                // If expected reduction is high, we reduce search depth here
                trace!(self, TRACE, event2("full_depth", "reduction", r as i64, "stat", stat_score as i64));
                value = -self.search::<NON_PV, TRACE>(
                    pos,
                    ss + 1,
                    -(alpha + 1),
                    -alpha,
                    new_depth - (r > 5234) as i32 - (r > 5487 && new_depth > 2) as i32,
                    !cut_node,
                );
            }

            // Step 20. For PV nodes only, do a full PV search on the first
            // move or after a fail high, otherwise let the parent node fail
            // low with value <= alpha and try another move.
            if pv_node && (move_count == 1 || value > alpha) {
                self.stack[ss + 1].pv = true;
                self.pv_lines[ply as usize + 1].clear();

                // Extend move from transposition table if we are about to dive
                // into qsearch. Decisive score handling improves mate finding
                // and retrograde analysis.
                if mv == tt_data.mv
                    && ((is_valid(tt_data.value) && is_decisive(tt_data.value) && tt_data.depth > 0)
                        || tt_data.depth > 1)
                {
                    new_depth = new_depth.max(1);
                }

                trace!(self, TRACE, event1("pv_search", "depth", new_depth as i64));
                value = -self.search::<PV, TRACE>(pos, ss + 1, -beta, -alpha, new_depth, false);
            }

            // Step 21. Undo move
            self.undo_move(pos, mv);
            trace!(self, TRACE, move_value(mv, value));

            debug_assert!(value > -VALUE_INFINITE && value < VALUE_INFINITE);

            // Step 22. Check for a new best move
            // If a stop occurred, the value of the search cannot be trusted,
            // and we return immediately without updating the best move,
            // principal variation or transposition table.
            if self.flags.stop.load(Relaxed) {
                trace!(self, TRACE, event("stopped"));
                return VALUE_ZERO;
            }

            if root_node {
                self.update_root_move::<TRACE>(mv, value, node_count, move_count, alpha, beta, ply);
            }

            // If we have an alternative move equal in value to the current
            // bestmove, we sometimes promote it to bestmove by pretending it
            // just exceeds alpha (but not beta).
            let inc = (value == best_value
                && ply + 2 >= self.root_depth
                && (self.nodes as i32 & 14) == 0
                && !is_win(value.abs() + 1)) as i32;

            if value + inc > best_value {
                best_value = value;
                trace!(self, TRACE, event2("new_best", "value", value as i64, "inc", inc as i64));

                if value + inc > alpha {
                    best_move = mv;

                    // Update PV even in fail-high case
                    if pv_node && !root_node {
                        let child = self.stack[ss + 1].pv;
                        self.pv_update(ply as usize, mv, child);
                    }

                    if value >= beta {
                        // (*Scaler) Infrequent and small updates scale well
                        self.stack[ss].cutoff_cnt += ((extension < 2) || pv_node) as i32;
                        let cnt = self.stack[ss].cutoff_cnt;
                        trace!(self, TRACE, event1("cutoff", "cutoff_count", cnt as i64));
                        debug_assert!(value >= beta); // Fail high
                        break;
                    }

                    // Reduce other moves if we have found at least one score
                    // improvement
                    if depth > 3 && depth < 12 && !is_decisive(value) {
                        depth -= 3;
                    }

                    debug_assert!(depth > 0);
                    alpha = value; // Update alpha! Always alpha < beta
                    trace!(self, TRACE, event2("new_alpha", "alpha", alpha as i64, "depth", depth as i64));
                }
            }

            // If the move is worse than some previously searched move,
            // remember it, to update its stats later.
            if mv != best_move && move_count as usize <= SEARCHEDLIST_CAPACITY {
                if capture {
                    captures_searched.push(mv);
                } else {
                    quiets_searched.push(mv);
                }
            }
        }

        // Step 23. Check for mate and stalemate, otherwise update
        // bestmove/countermove stats

        debug_assert!(move_count != 0 || !in_check || excluded_move != Move::none() || MoveList::legal(pos).is_empty());

        // Adjust best value for fail high cases
        if best_value >= beta && !is_decisive(best_value) && !is_decisive(alpha) {
            best_value = (best_value * depth + beta) / (depth + 1);
            trace!(self, TRACE, event1("fail_high_adjust", "value", best_value as i64));
        }

        // All legal moves have been searched: if there are no legal moves, it
        // must be a mate or a stalemate (just a fail low score if we are in a
        // singular extension search).
        if move_count == 0 {
            best_value = if excluded_move != Move::none() {
                alpha
            } else if in_check {
                mated_in(ply)
            } else {
                VALUE_DRAW
            };
            trace!(self, TRACE, event1("no_moves", "value", best_value as i64));
        }
        // If there is a move that produces search value greater than alpha,
        // we update the stats of searched moves.
        else if best_move != Move::none() {
            self.update_all_stats::<TRACE>(
                pos,
                ss,
                best_move,
                prev_sq,
                &quiets_searched,
                &captures_searched,
                depth,
                tt_data.mv,
                pv_node,
            );
            if !pv_node {
                let bonus = if best_move == tt_data.mv { 918 } else { -747 };
                self.tt_move_history.update(bonus);
                self.trace_ttmove_update::<TRACE>(bonus);
            }
        }
        // Bonus for prior quiet countermove that caused the fail low
        else if !prior_capture && prev_sq != SQ_NONE {
            let mut bonus_scale = -241;
            bonus_scale -= self.stack[ss - 1].stat_score / 98;
            bonus_scale += (59 * depth).min(420);
            bonus_scale += 186 * (self.stack[ss - 1].move_count > 9) as i32;
            bonus_scale += 142 * (!in_check && best_value <= static_eval - 106) as i32;
            bonus_scale +=
                159 * (!self.stack[ss - 1].in_check && best_value <= -self.stack[ss - 1].static_eval - 68) as i32;

            bonus_scale = bonus_scale.max(0);

            // scaledBonus ranges from 0 to roughly 2.3M, overflows happen for
            // multipliers larger than 900
            let scaled_bonus = (150 * depth - 85).min(1337) * bonus_scale;

            let pc = pos.piece_on(prev_sq);
            self.update_continuation_histories(ss - 1, pc, prev_sq, scaled_bonus * 263 / 16384);
            self.trace_continuation_update::<TRACE>(ss - 1, pc, prev_sq, scaled_bonus * 263 / 16384);

            self.main_history[not_color(us) as usize][prev_move.raw() as usize].update(scaled_bonus * 215 / 32768);

            let pawn = type_of(pc) != PAWN && prev_move.type_of() != PROMOTION;
            if pawn {
                self.shared_history.pawn_entry(pos)[pc as usize][prev_sq as usize].update(scaled_bonus * 324 / 8192);
            }
            if TRACE {
                if let Some(t) = self.tracer.as_deref_mut() {
                    t.prior_quiet_update(&self.main_history, &self.shared_history, pos, prev_move, scaled_bonus, pawn);
                }
            }
        }
        // Bonus for prior capture countermove that caused the fail low
        else if prev_sq != SQ_NONE {
            debug_assert!(prior_capture);

            let captured_piece = pos.captured_piece();
            debug_assert!(captured_piece != NO_PIECE);
            self.capture_history[pos.piece_on(prev_sq) as usize][prev_sq as usize][type_of(captured_piece) as usize]
                .update(892);
            if TRACE {
                if let Some(t) = self.tracer.as_deref_mut() {
                    t.prior_capture_update(&self.capture_history, pos, prev_sq);
                }
            }
        }

        if pv_node {
            best_value = best_value.min(max_value);
        }

        // If no good move is found and the previous position was ttPv, then
        // the previous opponent move is probably good and the new position is
        // added to the search tree.
        if best_value <= alpha {
            self.stack[ss].tt_pv = self.stack[ss].tt_pv || self.stack[ss - 1].tt_pv;
        }

        // Step 24. Write gathered information in transposition table. Note
        // that the static evaluation is saved as it was before correction
        // history.
        if excluded_move == Move::none() && !(root_node && self.pv_idx != 0) {
            let bound = if best_value >= beta {
                BOUND_LOWER
            } else if pv_node && best_move != Move::none() {
                BOUND_EXACT
            } else {
                BOUND_UPPER
            };
            let tt_pv = self.stack[ss].tt_pv;
            self.tt.write(
                tt_writer,
                pos_key,
                value_to_tt(best_value, ply),
                tt_pv,
                bound,
                if move_count != 0 { depth } else { (MAX_PLY as Depth - 1).min(depth + 6) },
                best_move,
                unadjusted_static_eval,
                self.tt.generation(),
            );
            self.trace_store::<TRACE>("store", tt_writer);
        }

        // Adjust correction history if the best move is not a capture and the
        // error direction matches whether we are above/below bounds.
        if !in_check
            && !(best_move != Move::none() && pos.capture(best_move))
            && (best_value > static_eval) == (best_move != Move::none())
        {
            let bonus = ((best_value - static_eval) * depth * (if best_move != Move::none() { 12 } else { 18 }) / 128)
                .clamp(-CORRECTION_HISTORY_LIMIT / 4, CORRECTION_HISTORY_LIMIT / 4);
            self.update_correction_history::<TRACE>(pos, ss, 1061 * bonus / 1024);
        }

        // The search is now complete
        debug_assert!(-VALUE_INFINITE < best_value && best_value < VALUE_INFINITE);
        best_value
    }

    /// The update of a root move after its search, in step 22 of `search()`.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn update_root_move<const TRACE: bool>(
        &mut self,
        mv: Move,
        value: Value,
        node_count: u64,
        move_count: i32,
        alpha: Value,
        beta: Value,
        _ply: i32,
    ) {
        let nodes = self.nodes;
        let sel_depth = self.sel_depth;
        let pv_idx = self.pv_idx;
        let i = self.root_moves.iter().position(|rm| rm.pv[0] == mv).unwrap();

        // The PV of the child, before borrowing the root move
        let child_pv = self.stack[STACK_OFFSET + 1].pv;
        debug_assert!(child_pv || !(move_count == 1 || value > alpha));

        let rm = &mut self.root_moves[i];
        rm.effort += nodes - node_count;

        let n = nodes - node_count;
        let e_prev = 1u64.max(rm.effort - n);

        // Exponential moving average parameters for root move
        const SCALE: u64 = 32;
        const CHI_NUMERATOR: u64 = 3;
        const CHI_DENOMINATOR: u64 = 2; // Chi = 3/2 = 1.5
        const MIN_WEIGHT: u64 = 12; // 37.5% minimum weight
        const MAX_WEIGHT: u64 = 24; // 75% maximum weight

        let w = ((SCALE * n * CHI_DENOMINATOR) / (n * CHI_DENOMINATOR + CHI_NUMERATOR * e_prev))
            .clamp(MIN_WEIGHT, MAX_WEIGHT);
        let w_mss = w.min(16);
        let v2: i64 = value as i64 * value.abs() as i64;

        // C++ mixes int and u64 here: the signed values are converted to u64
        // (wrapping when negative), summed, divided unsigned and truncated
        // back to int, which floors rather than truncates; done the same way.
        if rm.average_score == -VALUE_INFINITE {
            rm.average_score = value;
        } else {
            rm.average_score = ((value as i64 as u64)
                .wrapping_mul(w)
                .wrapping_add((rm.average_score as i64 as u64).wrapping_mul(SCALE - w))
                / SCALE) as Value;
        }

        if rm.mean_squared_score == -VALUE_INFINITE * VALUE_INFINITE {
            rm.mean_squared_score = value * value.abs();
        } else {
            rm.mean_squared_score = ((v2 as u64)
                .wrapping_mul(w_mss)
                .wrapping_add((rm.mean_squared_score as i64 as u64).wrapping_mul(SCALE - w_mss))
                / SCALE) as Value;
        }

        // PV move or new best move?
        if move_count == 1 || value > alpha {
            rm.score = value;
            rm.uci_score = value;
            rm.sel_depth = sel_depth;
            rm.unset_inexact();

            if value >= beta {
                rm.inexact_lower = true;
                rm.uci_score = beta;
            } else if value <= alpha {
                rm.inexact_upper = true;
                rm.uci_score = alpha;
            }

            rm.pv.truncate(1);

            if child_pv {
                rm.pv.extend_from_slice(self.pv_lines[1].as_slice());
            }

            // We record how often the best move has been changed in each
            // iteration. This information is used for time management. In
            // MultiPV mode, we must take care to only do this for the first PV
            // line.
            if move_count > 1 && pv_idx == 0 {
                self.best_move_changes += 1;
            }
        } else {
            // All other moves but the PV are set to the lowest value: this is
            // not a problem when sorting because the sort is stable and the
            // move position in the list is preserved -- just the PV is pushed
            // up.
            rm.score = -VALUE_INFINITE;
        }

        if TRACE {
            let rm = &self.root_moves[i];
            let (s, u, a, m, d, l, up) = (
                rm.score,
                rm.uci_score,
                rm.average_score,
                rm.mean_squared_score,
                rm.sel_depth,
                rm.inexact_lower,
                rm.inexact_upper,
            );
            trace!(self, TRACE, root_move(mv, s, u, a, m as i64, d, l, up));
        }
    }

    /// Quiescence search function, which is called by the main search
    /// function with depth zero, or recursively with further decreasing
    /// depth. With depth <= 0, we "should" be using static eval only, but
    /// tactical moves may confuse the static eval. To fight this horizon
    /// effect, we implement this qsearch of tactical moves.
    fn qsearch_node<const NT: NodeType, const TRACE: bool>(
        &mut self,
        pos: &mut Position,
        ss: usize,
        mut alpha: Value,
        beta: Value,
    ) -> Value {
        const { assert!(NT != ROOT) };
        let pv_node = NT == PV;

        debug_assert!(alpha >= -VALUE_INFINITE && alpha < beta && beta <= VALUE_INFINITE);
        debug_assert!(pv_node || (alpha == beta - 1));

        let ply = self.stack[ss].ply;

        // Check if we have an upcoming move that draws by repetition
        if alpha < VALUE_DRAW && pos.upcoming_repetition(ply) {
            alpha = value_draw(self.nodes);
            trace!(self, TRACE, repetition_draw(alpha));
            if alpha >= beta {
                return alpha;
            }
        }

        let mut best_value: Value;
        let mut value: Value;
        let futility_base: Value;

        // Step 1. Initialize node
        if pv_node {
            // (ss + 1)->pv = &pv, the local line (row ply + 1), empty as C++
            // constructs it; then the line of the parent, ss->pv, cleared
            self.pv_lines[ply as usize + 1].clear();
            self.stack[ss + 1].pv = true;
            debug_assert!(self.stack[ss].pv);
            self.pv_lines[ply as usize].clear();
        }

        let mut best_move = Move::none();
        let in_check = pos.checkers() != 0;
        self.stack[ss].in_check = in_check;
        let mut move_count = 0;

        // Used to send selDepth info to GUI (selDepth counts from 1, ply from 0)
        if pv_node && self.sel_depth < ply + 1 {
            self.sel_depth = ply + 1;
        }

        // Step 2. Check for an immediate draw or maximum ply reached
        if pos.is_draw(ply) || ply >= MAX_PLY as i32 {
            if TRACE {
                let reason = if pos.is_draw(ply) { "draw" } else { "max_ply" };
                trace!(self, TRACE, immediate(reason));
            }
            return if ply >= MAX_PLY as i32 && !in_check { self.evaluate(pos) } else { VALUE_DRAW };
        }

        debug_assert!(0 <= ply && ply < MAX_PLY as i32);

        // Step 3. Transposition table lookup
        let pos_key = pos.key();
        let (tt_hit, mut tt_data, tt_writer) = self.tt.probe(pos_key);

        self.stack[ss].tt_hit = tt_hit;
        tt_data.mv = if tt_hit { tt_data.mv } else { Move::none() };
        tt_data.value = if tt_hit { value_from_tt(tt_data.value, ply, pos.rule50_count()) } else { VALUE_NONE };
        let pv_hit = tt_hit && tt_data.is_pv;
        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.tt(&self.tt, tt_writer, tt_hit, &tt_data, pv_hit, None);
            }
        }

        // At non-PV nodes we check for an early TT cutoff
        if !pv_node
            && tt_data.depth >= DEPTH_QS
            && is_valid(tt_data.value)
            && (tt_data.bound & (if tt_data.value >= beta { BOUND_LOWER } else { BOUND_UPPER })) != 0
        {
            trace!(self, TRACE, tt_cutoff(false));
            return tt_data.value;
        }

        // Step 4. Static evaluation of the position
        let mut unadjusted_static_eval = VALUE_NONE;
        if in_check {
            best_value = -VALUE_INFINITE;
            futility_base = -VALUE_INFINITE;
            trace!(self, TRACE, qsearch_eval_in_check());
        } else {
            let correction_value = self.correction_value(pos, ss);

            if tt_hit {
                // Never assume anything about values stored in TT
                unadjusted_static_eval = tt_data.eval;

                if !is_valid(unadjusted_static_eval) {
                    unadjusted_static_eval = self.evaluate(pos);
                }

                best_value = to_corrected_static_eval(unadjusted_static_eval, correction_value);
                self.stack[ss].static_eval = best_value;

                // ttValue can be used as a better position evaluation
                if is_valid(tt_data.value)
                    && !is_decisive(tt_data.value)
                    && (tt_data.bound & (if tt_data.value > best_value { BOUND_LOWER } else { BOUND_UPPER })) != 0
                {
                    best_value = tt_data.value;
                }
            } else {
                unadjusted_static_eval = self.evaluate(pos);
                best_value = to_corrected_static_eval(unadjusted_static_eval, correction_value);
                self.stack[ss].static_eval = best_value;
            }

            let static_eval = self.stack[ss].static_eval;
            trace!(self, TRACE, qsearch_eval(unadjusted_static_eval, static_eval, best_value, correction_value));

            // Stand pat. Return immediately if static value is at least beta
            if best_value >= beta {
                debug_assert!(!is_decisive(best_value));
                best_value = (441 * best_value + 583 * beta) / 1024;

                if !tt_hit {
                    self.tt.write(
                        tt_writer,
                        pos_key,
                        VALUE_NONE,
                        false,
                        BOUND_LOWER,
                        DEPTH_UNSEARCHED,
                        Move::none(),
                        unadjusted_static_eval,
                        self.tt.generation(),
                    );
                    self.trace_store::<TRACE>("store", tt_writer);
                }
                trace!(self, TRACE, event("stand_pat"));
                return best_value;
            }

            if best_value > alpha {
                alpha = best_value;
            }

            futility_base = static_eval + 306;
        }

        let prev_move = self.stack[ss - 1].current_move;
        let prev_sq = if prev_move.is_ok() { prev_move.to_sq() } else { SQ_NONE };

        // Initialize a MovePicker object for the current position, and
        // prepare to search the moves. We presently use two stages of move
        // generator in quiescence search: captures, or evasions only when in
        // check.
        let slot = picker_slot(ply, self.stack[ss].excluded_move != Move::none());
        self.pickers[slot].renew(pos, tt_data.mv, DEPTH_QS, ply);

        // Step 5. Loop through all legal moves until no moves remain or a
        // beta cutoff occurs.
        loop {
            let mv = {
                let block = &self.shared_history.continuation_history_block;
                let c1 = block.at(self.stack[ss - 1].continuation_history);
                // contHist[] = {(ss - 1)->continuationHistory}: the picker
                // reads no other in quiescence search
                let h = Histories {
                    main_history: &self.main_history,
                    low_ply_history: &self.low_ply_history,
                    capture_history: &self.capture_history,
                    continuation_history: [c1; 6],
                    shared_history: &self.shared_history,
                };
                self.pickers[slot].next_move(pos, &h)
            };
            if mv == Move::none() {
                break;
            }
            self.trace_pick::<TRACE>(slot, mv);
            debug_assert!(mv.is_ok());

            let gives_check = pos.gives_check(mv);
            let capture = pos.capture_stage(mv);

            move_count += 1;
            trace!(self, TRACE, try_qmove(mv, move_count, capture, gives_check));

            // Step 6. Pruning
            if !is_loss(best_value) {
                // Futility pruning and moveCount pruning
                if !gives_check && mv.to_sq() != prev_sq && !is_loss(futility_base) && mv.type_of() != PROMOTION {
                    if move_count > 2 {
                        trace!(self, TRACE, prune0("move_count"));
                        continue;
                    }

                    let futility_value = futility_base + PIECE_VALUE[pos.piece_on(mv.to_sq()) as usize];

                    // If static eval + value of piece we are going to capture
                    // is much lower than alpha, we can prune this move.
                    if futility_value <= alpha {
                        best_value = best_value.max(futility_value);
                        trace!(self, TRACE, prune1("futility", "value", futility_value as i64));
                        continue;
                    }

                    // If static exchange evaluation is low enough, we can
                    // prune
                    if !pos.see_ge(mv, alpha - futility_base) {
                        best_value = best_value.max(alpha.min(futility_base));
                        trace!(self, TRACE, prune1("futility_see", "threshold", (alpha - futility_base) as i64));
                        continue;
                    }
                }

                // Skip non-captures
                if !capture {
                    trace!(self, TRACE, prune0("not_capture"));
                    continue;
                }

                // Do not search moves with bad enough SEE values
                if !pos.see_ge(mv, -74) {
                    trace!(self, TRACE, prune0("see"));
                    continue;
                }
            }

            // Step 7. Make and search the move
            self.do_move(pos, mv, gives_check, capture, ss);

            value = -self.qsearch::<NT, TRACE>(pos, ss + 1, -beta, -alpha);
            self.undo_move(pos, mv);
            trace!(self, TRACE, move_value(mv, value));

            debug_assert!(value > -VALUE_INFINITE && value < VALUE_INFINITE);

            // Step 8. Check for a new best move
            if value > best_value {
                best_value = value;
                trace!(self, TRACE, event1("new_best", "value", value as i64));

                if value > alpha {
                    best_move = mv;

                    // Update pv even in fail-high case
                    if pv_node {
                        let child = self.stack[ss + 1].pv;
                        self.pv_update(ply as usize, mv, child);
                    }

                    if value < beta {
                        // Update alpha here!
                        alpha = value;
                        trace!(self, TRACE, event1("new_alpha", "alpha", alpha as i64));
                    } else {
                        trace!(self, TRACE, event("cutoff"));
                        break; // Fail high
                    }
                }
            }
        }

        // Step 9. Check for mate and stalemate
        // All legal moves have been searched. A special case: if we are in
        // check and no legal moves were found, it is checkmate.
        if move_count == 0 {
            if in_check {
                // Checkmate!
                debug_assert!(MoveList::legal(pos).is_empty());
                trace!(self, TRACE, event("checkmate"));
                return mated_in(ply); // Plies to mate from the root
            }

            // Only check for stalemate under specific conditions
            let us = pos.side_to_move();
            if crate::bitboard::shift(pos.pieces_cp(us, PAWN), crate::types::pawn_push(us)) & !pos.pieces() == 0
                && pos.non_pawn_material(us) == 0
                && type_of(pos.captured_piece()) >= KNIGHT
                && MoveList::legal(pos).is_empty()
            {
                best_value = VALUE_DRAW;
                trace!(self, TRACE, event("stalemate"));
            }
        }

        if !is_decisive(best_value) && best_value > beta {
            best_value = (462 * best_value + 562 * beta) / 1024;
            trace!(self, TRACE, event1("fail_high_adjust", "value", best_value as i64));
        }

        // Step 10. Save gathered info in transposition table. The static
        // evaluation is saved as it was before adjustment by correction
        // history.
        self.tt.write(
            tt_writer,
            pos_key,
            value_to_tt(best_value, ply),
            pv_hit,
            if best_value >= beta { BOUND_LOWER } else { BOUND_UPPER },
            DEPTH_QS,
            best_move,
            unadjusted_static_eval,
            self.tt.generation(),
        );
        self.trace_store::<TRACE>("store", tt_writer);

        // The search is now complete
        debug_assert!(-VALUE_INFINITE < best_value && best_value < VALUE_INFINITE);
        best_value
    }

    #[inline(always)]
    fn trace_store<const TRACE: bool>(&mut self, name: &'static str, w: TTWriter) {
        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.store(name, &self.tt, w);
            }
        }
    }

    #[inline(always)]
    fn trace_pick<const TRACE: bool>(&mut self, slot: usize, mv: Move) {
        if TRACE {
            if let Some(t) = self.tracer.as_deref_mut() {
                t.pick(&self.pickers[slot], mv);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The output, the time check, and the PV extension

impl Worker {
    /// `SearchManager::check_time()`: stops the search when out of time or
    /// nodes. With one thread `threads.nodes_searched()` is this worker's
    /// count.
    #[inline(always)]
    fn check_time(&mut self) {
        let nodes = self.nodes;
        let limits_nodes = self.limits.nodes;
        let mm = self.manager.as_deref_mut().unwrap();
        mm.calls_cnt -= 1;
        if mm.calls_cnt > 0 {
            return;
        }

        // When using nodes, ensure checking rate is not lower than 0.1% of
        // nodes
        mm.calls_cnt = if limits_nodes != 0 { 512.min((limits_nodes / 1024) as i32) } else { 512 };

        // (C++ prints the debug statistics of dbg_hit_on() and friends here
        // once a second; the engine records none.)

        // We should not stop pondering until told so by the GUI
        if self.flags.ponder.load(Relaxed) {
            return;
        }

        let elapsed = mm.tm.elapsed(nodes);
        let limits = &self.limits;
        if (limits.use_time_management() && (elapsed > mm.tm.maximum() || mm.stop_on_ponderhit))
            || (limits.movetime != 0 && elapsed >= limits.movetime)
            || (limits.nodes != 0 && nodes >= limits.nodes)
        {
            self.flags.stop.store(true, Relaxed);
        }
    }

    /// `SearchManager::output_pv()`: sends the `info` lines of the PV lines.
    pub fn output_pv(&mut self, depth: Depth) {
        let nodes = self.nodes;
        let multi_pv = self.search_options.multi_pv.min(self.root_moves.len());
        let tb_hits = self.tb_hits + if self.tb_config.root_in_tb { self.root_moves.len() as u64 } else { 0 };
        let chess960 = self.root_pos.is_chess960();

        for i in 0..multi_pv {
            let use_previous_score = self.root_moves[i].score == -VALUE_INFINITE;

            if depth == 1 && use_previous_score && i > 0 {
                continue;
            }

            let d = if use_previous_score { 1.max(depth - 1) } else { depth };
            let mut v =
                if use_previous_score { self.root_moves[i].previous_score } else { self.root_moves[i].uci_score };

            if v == -VALUE_INFINITE {
                v = VALUE_ZERO;
            }

            let is_tb_score = self.tb_config.root_in_tb && !is_mate_or_mated(v);
            if is_tb_score {
                v = self.root_moves[i].tb_score;
            }

            // Potentially correct and extend the PV, and in exceptional cases
            // v. Previous PVs have already been extended. Inexact flags
            // indicate an unreliable PV.
            if is_decisive(v)
                && !is_mate_or_mated(v)
                && !use_previous_score
                && (!self.root_moves[i].is_inexact() || is_tb_score)
            {
                v = syzygy_extend_pv(&mut self.root_pos, &self.root_moves[i], v);
            }

            let rm = &self.root_moves[i];
            let mut pv = String::new();
            for &m in if use_previous_score { &rm.previous_pv } else { &rm.pv } {
                pv += &uci::move_to_string(m, chess960);
                pv.push(' ');
            }

            // Remove last whitespace
            pv.pop();

            let wdl = if self.search_options.show_wdl { uci::wdl(v, &self.root_pos) } else { String::new() };

            // Scores cannot be both exact and inexact
            debug_assert!(!(rm.inexact_lower && rm.inexact_upper));
            let bound = if rm.inexact_lower {
                "lowerbound"
            } else if rm.inexact_upper {
                "upperbound"
            } else {
                ""
            };

            let mm = self.manager.as_deref().unwrap();
            let time = mm.tm.elapsed_time().max(1) as u64;
            let info = InfoFull {
                depth: d,
                sel_depth: rm.sel_depth,
                multi_pv: i + 1,
                score: Score::new(v, &self.root_pos),
                wdl: &wdl,
                // TB and previous scores are exact, even though their flags
                // may say otherwise
                bound: if !(is_tb_score || use_previous_score) { bound } else { "" },
                time_ms: time,
                nodes,
                nps: nodes * 1000 / time,
                tb_hits,
                pv: &pv,
                hashfull: self.tt.hashfull(0),
            };

            (mm.updates.on_update_full)(&info);
        }
    }
}

/// `syzygy_extend_pv()` without tablebases (they are not ported): the PV is
/// walked and kept whole, no move ranks below another, and the extension to
/// mate stops at once as the position is never in the tables; what is left of
/// it is the draw check at the end of the PV. Reached only for a tablebase
/// range score, which nothing gives without tablebases. Checked by nothing.
fn syzygy_extend_pv(pos: &mut Position, root_move: &RootMove, mut v: Value) -> Value {
    for &m in &root_move.pv {
        pos.do_move(m);
    }

    if pos.is_draw(0) {
        v = VALUE_DRAW;
    }

    // Undo the PV moves
    for &m in root_move.pv.iter().rev() {
        pos.undo_move(m);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reductions_are_the_cpp_ones() {
        for (i, &r) in REDUCTIONS.iter().enumerate().skip(1) {
            assert_eq!(r, (2872.0 / 128.0 * (i as f64).ln()) as i32, "reductions[{i}]");
        }
    }

    #[test]
    fn stable_sort_keeps_ties_in_order() {
        let mut v: Vec<RootMove> = (1..=6).map(|i| RootMove::new(Move(i))).collect();
        let scores = [5, 7, 5, 7, -VALUE_INFINITE, 7];
        for (rm, s) in v.iter_mut().zip(scores) {
            rm.score = s;
        }
        v[3].previous_score = 100;
        stable_sort(&mut v);
        let order: Vec<u16> = v.iter().map(|rm| rm.pv[0].0).collect();
        assert_eq!(order, vec![4, 2, 6, 1, 3, 5]);
    }

    #[test]
    fn average_score_floors_like_cpp() {
        // (value * w + avg * (32 - w)) / 32 with C++'s conversions to u64:
        // -10 * 12 + -3 * 20 = -180, -180 / 32 = -5.625: C++ gives -6
        let (value, avg, w) = (-10i32, -3i32, 12u64);
        let r =
            ((value as i64 as u64).wrapping_mul(w).wrapping_add((avg as i64 as u64).wrapping_mul(32 - w)) / 32) as i32;
        assert_eq!(r, -6);
    }
}

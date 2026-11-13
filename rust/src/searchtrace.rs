//! Port of `src/searchtrace.{h,cpp}`: the search trace, which records what a
//! single-threaded search does, node by node, as a stream of events, so that
//! the port can be compared with the C++ search decision by decision. See
//! "The search trace" in `tests/differential/README.md` for the format.
//!
//! # How the port carries the trace
//!
//! C++ builds a second binary, `stockfish-trace`, with `SEARCH_TRACE`
//! defined; in the default binary the hooks are empty statements. The port
//! has one binary and two instances of the search: the search functions take
//! a `const TRACE: bool`, every hook is `if TRACE { ... }` (the `trace!`
//! macro), and the default search is the instance with `TRACE = false`, in
//! which the compiler removes the hooks entirely. `searchtrace` runs the
//! instance with `TRACE = true`, whose hooks test that a tracer is set (as
//! the C++ ones test a pointer). So the default search pays nothing for the
//! trace but code size, and both instances run the same code otherwise:
//! `bench` gives the same signature with either (see README.md for the
//! measurements).
//!
//! The events are formatted exactly as in C++ (the digest is over their
//! bytes), and emitted at the same points of the search, in the same order.

use std::collections::BTreeMap;
use std::io::Write;

use crate::history::*;
use crate::movepick::MovePicker;
use crate::position::{square_to_string, Position};
use crate::search::{NodeType, RootMove, Stack, NON_PV, PV, ROOT};
use crate::tt::*;
use crate::types::*;
use crate::uci;

/// `TRACE(call)` of C++: emits an event when tracing. `$T` is the
/// `const TRACE: bool` of the search function; with `false` the whole
/// statement is removed at compile time. The arguments must not borrow the
/// worker as a whole (they may read its fields).
#[macro_export]
macro_rules! trace {
    ($w:expr, $T:ident, $method:ident ( $($arg:expr),* $(,)? )) => {
        if $T {
            if let Some(t) = $w.tracer.as_deref_mut() {
                t.$method($($arg),*);
            }
        }
    };
}

/// The digest of the event stream and of the state left behind by a search:
/// 64-bit FNV-1a, over bytes for the events and over 64-bit words for the
/// tables.
#[derive(Clone, Copy)]
pub struct TraceDigest {
    pub value: u64,
}

impl Default for TraceDigest {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceDigest {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    pub const fn new() -> TraceDigest {
        TraceDigest { value: Self::OFFSET }
    }

    #[inline]
    pub fn bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.value = (self.value ^ b as u64).wrapping_mul(Self::PRIME);
        }
    }

    #[inline]
    pub fn word(&mut self, w: u64) {
        self.value = (self.value ^ w).wrapping_mul(Self::PRIME);
    }
}

/// The events of one search of each session to print, from the 'events'
/// argument of the 'searchtrace' command: those of index [from, to) of search
/// number 'search' of each session (counted from 1, 0 for none, -1 for all).
#[derive(Clone, Copy, Default)]
pub struct SearchTraceWindow {
    pub search: i32,
    pub from: u64,
    pub to: u64,
    /// print the number of events of each kind at the end
    pub counts: bool,
}

/// One iteration of iterative deepening, as recorded at its end.
pub struct TraceIteration {
    pub depth: i32,
    pub sel_depth: i32,
    pub completed: bool,
    pub score: Value,
    pub uci_score: Value,
    pub bound: &'static str,
    pub nodes: u64,
    pub pv: Vec<Move>,
}

struct Frame {
    entry: String,
    mv: String,
    call: String,
}

fn hex64(v: u64) -> String {
    format!("{v:016X}")
}

fn bound_name(b: Bound) -> &'static str {
    match b {
        BOUND_EXACT => "exact",
        BOUND_LOWER => "lower",
        BOUND_UPPER => "upper",
        _ => "none",
    }
}

const PIECE_TO_CHAR: &[u8] = b" PNBRQK  pnbrqk";

/// `SearchTracer`: receives the events of a search. Each event is formatted
/// as one line of text, a name followed by key=value fields, and fed to a
/// running digest; a checkpoint of the digest is kept every 'interval'
/// events. The events whose index lies in a given window are also printed,
/// preceded by the nodes open at the start of the window.
pub struct SearchTracer {
    chess960: bool,
    interval: u64,
    count: u64,
    running: TraceDigest,
    saved: Vec<u64>,
    line: String,

    // Printing of a window of events, with the stack of open nodes as context
    out: bool,
    window_from: u64,
    window_to: u64,
    frames: Vec<Frame>,
    last: String,

    counting: bool,
    kind_counts: BTreeMap<String, u64>,

    pub iterations: Vec<TraceIteration>,
}

impl SearchTracer {
    pub fn new(chess960: bool, interval: u64) -> SearchTracer {
        SearchTracer {
            chess960,
            interval,
            count: 0,
            running: TraceDigest::new(),
            saved: Vec::new(),
            line: String::with_capacity(256),
            out: false,
            window_from: 0,
            window_to: 0,
            frames: Vec::new(),
            last: String::new(),
            counting: false,
            kind_counts: BTreeMap::new(),
            iterations: Vec::new(),
        }
    }

    /// Prints the events of index [from, to) of the next searches on stdout
    /// (`out` false for none).
    pub fn set_window(&mut self, from: u64, to: u64, out: bool) {
        self.window_from = from;
        self.window_to = to;
        self.out = out;
    }

    /// Counts the events by kind, for statistics on what a corpus covers.
    pub fn count_kinds(&mut self) {
        self.counting = true;
    }

    pub fn kinds(&self) -> &BTreeMap<String, u64> {
        &self.kind_counts
    }

    pub fn begin_search(&mut self) {
        self.count = 0;
        self.running = TraceDigest::new();
        self.saved.clear();
        self.frames.clear();
        self.last.clear();
        self.iterations.clear();
    }

    pub fn events(&self) -> u64 {
        self.count
    }

    pub fn digest(&self) -> u64 {
        self.running.value
    }

    pub fn checkpoints(&self) -> &[u64] {
        &self.saved
    }

    fn move_str(&self, m: Move) -> String {
        if m == Move::none() {
            "-".to_string()
        } else if m == Move::null() {
            "0000".to_string()
        } else {
            uci::move_to_string(m, self.chess960)
        }
    }

    // -----------------------------------------------------------------------
    // Formatting

    #[inline]
    fn begin(&mut self, name: &str) -> &mut Self {
        self.line.clear();
        self.line.push_str(name);
        self
    }

    #[inline]
    fn put(&mut self, key: &str, value: i64) -> &mut Self {
        use std::fmt::Write as _;
        self.line.push(' ');
        self.line.push_str(key);
        self.line.push('=');
        let _ = write!(self.line, "{value}");
        self
    }

    #[inline]
    fn put_move(&mut self, key: &str, m: Move) -> &mut Self {
        let s = self.move_str(m);
        self.put_str(key, &s)
    }

    #[inline]
    fn put_str(&mut self, key: &str, text: &str) -> &mut Self {
        self.line.push(' ');
        self.line.push_str(key);
        self.line.push('=');
        self.line.push_str(text);
        self
    }

    fn put_key(&mut self, key: &str, k: u64) -> &mut Self {
        let s = hex64(k);
        self.put_str(key, &s)
    }

    /// Feeds the line to the digest, keeps a checkpoint every 'interval'
    /// events, and prints the line if it lies in the window, preceded by the
    /// open nodes the first time.
    fn commit(&mut self, is_move: bool) {
        self.running.bytes(self.line.as_bytes());
        self.running.bytes(b"\n");

        // The kind of an event is its name, and for some the value of the
        // first field
        if self.counting {
            let line = &self.line;
            let end = line.find(' ').unwrap_or(line.len());
            let name = &line[..end];
            let kind = if name == "prune" || name == "immediate" || name == "pick" || name == "skip" {
                let field = match name {
                    "pick" => line.find(" stage=").unwrap_or(line.len()),
                    "skip" => line.find(" reason=").unwrap_or(line.len()),
                    _ => end,
                };
                let eq = line[field..].find('=').map(|i| i + field).unwrap_or(line.len());
                let stop = line[eq..].find(' ').map(|i| i + eq).unwrap_or(line.len());
                format!("{}:{}", name, &line[(eq + 1).min(line.len())..stop])
            } else {
                name.to_string()
            };
            *self.kind_counts.entry(kind).or_insert(0) += 1;
        }

        if self.out && self.count < self.window_to {
            let indexed = format!("{} {}", self.count, self.line);
            if self.count >= self.window_from {
                let stdout = std::io::stdout();
                let mut o = stdout.lock();
                if self.count == self.window_from {
                    for f in &self.frames {
                        for s in [&f.entry, &f.mv, &f.call] {
                            if !s.is_empty() {
                                let _ = writeln!(o, "context {s}");
                            }
                        }
                    }
                }
                let _ = writeln!(o, "{indexed}");
            }
            if is_move {
                if let Some(f) = self.frames.last_mut() {
                    f.mv = indexed.clone();
                }
            }
            self.last = indexed;
        }

        self.count += 1;
        if self.count % self.interval == 0 {
            self.saved.push(self.running.value);
        }
    }

    fn slot(&mut self, table: &TranspositionTable, writer: TTWriter) {
        use std::fmt::Write as _;
        let e = table.trace_entry(writer);
        let _ = write!(self.line, " slot={}:{}", e.cluster, e.slot);
        self.put_str("key21", &format!("{:06X}", e.key21))
            .put_move("move", e.data.mv)
            .put("value", e.data.value as i64)
            .put("eval", e.data.eval as i64)
            .put("depth", e.data.depth as i64)
            .put_str("bound", bound_name(e.data.bound))
            .put("pv", e.data.is_pv as i64)
            .put("generation", e.generation as i64);
    }

    // -----------------------------------------------------------------------
    // Iterative deepening

    pub fn start(&mut self, generation: u8, root_moves: usize) {
        self.begin("start").put("generation", generation as i64).put("root_moves", root_moves as i64).commit(false);
    }

    pub fn iteration(&mut self, root_depth: Depth) {
        self.begin("iteration").put("depth", root_depth as i64).commit(false);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn aspiration(
        &mut self,
        depth: Depth,
        alpha: Value,
        beta: Value,
        delta: i32,
        avg: Value,
        optimism: Value,
        seek_mate: bool,
    ) {
        self.begin("aspiration")
            .put("depth", depth as i64)
            .put("alpha", alpha as i64)
            .put("beta", beta as i64)
            .put("delta", delta as i64)
            .put("average", avg as i64)
            .put("optimism", optimism as i64)
            .put("seek_mate", seek_mate as i64)
            .commit(false);
    }

    pub fn aspiration_result(&mut self, best_value: Value, alpha: Value, beta: Value, stopped: bool) {
        let fail = if best_value <= alpha {
            "low"
        } else if best_value >= beta {
            "high"
        } else {
            "none"
        };
        self.begin("aspiration_result")
            .put("value", best_value as i64)
            .put_str("fail", fail)
            .put("stopped", stopped as i64)
            .commit(false);
    }

    pub fn iteration_end(&mut self, rm: &RootMove, nodes: u64, root_depth: Depth, stopped: bool) {
        let it = TraceIteration {
            depth: root_depth,
            sel_depth: rm.sel_depth,
            completed: !stopped,
            score: rm.score,
            uci_score: rm.uci_score,
            bound: if rm.inexact_lower {
                "lower"
            } else if rm.inexact_upper {
                "upper"
            } else {
                "exact"
            },
            nodes,
            pv: rm.pv.clone(),
        };

        let pv = it.pv.iter().map(|&m| self.move_str(m)).collect::<Vec<_>>().join(",");

        self.begin("iteration_end")
            .put("depth", root_depth as i64)
            .put("completed", it.completed as i64)
            .put("score", it.score as i64)
            .put("uci_score", it.uci_score as i64)
            .put_str("bound", it.bound)
            .put("seldepth", it.sel_depth as i64)
            .put("nodes", it.nodes as i64)
            .put_str("pv", &pv)
            .commit(false);

        self.iterations.push(it);
    }

    // -----------------------------------------------------------------------
    // Nodes

    #[allow(clippy::too_many_arguments)]
    pub fn enter_search(
        &mut self,
        pos: &Position,
        ss: &Stack,
        alpha: Value,
        beta: Value,
        depth: Depth,
        node_type: NodeType,
        cut_node: bool,
        nodes: u64,
    ) {
        let node = match node_type {
            ROOT => "root",
            PV => "pv",
            NON_PV => "nonpv",
            _ => unreachable!(),
        };
        self.begin("search")
            .put("ply", ss.ply as i64)
            .put_key("key", pos.key())
            .put("depth", depth as i64)
            .put("alpha", alpha as i64)
            .put("beta", beta as i64)
            .put_str("node", node)
            .put("cut", cut_node as i64)
            .put_move("excluded", ss.excluded_move)
            .put("nodes", nodes as i64);
        self.enter();
    }

    pub fn enter_qsearch(&mut self, pos: &Position, ss: &Stack, alpha: Value, beta: Value, pv_node: bool, nodes: u64) {
        self.begin("qsearch")
            .put("ply", ss.ply as i64)
            .put_key("key", pos.key())
            .put("alpha", alpha as i64)
            .put("beta", beta as i64)
            .put_str("node", if pv_node { "pv" } else { "nonpv" })
            .put("nodes", nodes as i64);
        self.enter();
    }

    fn enter(&mut self) {
        if self.out && self.count < self.window_to {
            if let Some(f) = self.frames.last_mut() {
                if f.entry != self.last {
                    f.call = self.last.clone();
                }
            }
        }
        self.commit(false);
        if self.out && self.count <= self.window_to {
            self.frames.push(Frame { entry: self.last.clone(), mv: String::new(), call: String::new() });
        }
    }

    pub fn leave(&mut self, value: Value) {
        self.begin("return").put("value", value as i64).commit(false);
        if self.out {
            self.frames.pop();
        }
    }

    // -----------------------------------------------------------------------
    // Reads

    pub fn repetition_draw(&mut self, alpha: Value) {
        self.begin("repetition_draw").put("alpha", alpha as i64).commit(false);
    }

    pub fn immediate(&mut self, reason: &str) {
        self.begin("immediate").put_str("reason", reason).commit(false);
    }

    /// The `tt` event, of search (with `tt_capture`) or of qsearch.
    pub fn tt(
        &mut self,
        table: &TranspositionTable,
        writer: TTWriter,
        hit: bool,
        adjusted: &TTData,
        tt_pv: bool,
        tt_capture: Option<bool>,
    ) {
        self.begin("tt").put("hit", hit as i64);
        self.slot(table, writer);
        self.put_move("tt_move", adjusted.mv).put("tt_value", adjusted.value as i64).put("tt_pv", tt_pv as i64);
        if let Some(c) = tt_capture {
            self.put("tt_capture", c as i64);
        }
        self.commit(false);
    }

    pub fn static_eval(
        &mut self,
        raw: Value,
        corrected: Value,
        eval: Value,
        correction: i32,
        improving: bool,
        worsening: bool,
    ) {
        self.begin("static_eval")
            .put("raw", raw as i64)
            .put("static", corrected as i64)
            .put("eval", eval as i64)
            .put("correction", correction as i64)
            .put("improving", improving as i64)
            .put("worsening", worsening as i64)
            .commit(false);
    }

    pub fn qsearch_eval(&mut self, raw: Value, corrected: Value, stand_pat: Value, correction: i32) {
        self.begin("static_eval")
            .put("raw", raw as i64)
            .put("static", corrected as i64)
            .put("stand_pat", stand_pat as i64)
            .put("correction", correction as i64)
            .commit(false);
    }

    pub fn qsearch_eval_in_check(&mut self) {
        self.begin("static_eval").put("in_check", 1).commit(false);
    }

    // -----------------------------------------------------------------------
    // Decisions

    pub fn event(&mut self, name: &str) {
        self.begin(name).commit(false);
    }

    pub fn event1(&mut self, name: &str, key: &str, value: i64) {
        self.begin(name).put(key, value).commit(false);
    }

    pub fn event2(&mut self, name: &str, key1: &str, value1: i64, key2: &str, value2: i64) {
        self.begin(name).put(key1, value1).put(key2, value2).commit(false);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn event3(&mut self, name: &str, key1: &str, value1: i64, key2: &str, value2: i64, key3: &str, value3: i64) {
        self.begin(name).put(key1, value1).put(key2, value2).put(key3, value3).commit(false);
    }

    pub fn tt_cutoff(&mut self, checked: bool) {
        self.begin("tt_cutoff").put("checked", checked as i64).commit(false);
    }

    pub fn null_move(&mut self, reduction: Depth) {
        self.begin("null_move").put("reduction", reduction as i64).commit(true);
    }

    pub fn probcut_move(&mut self, m: Move) {
        self.begin("probcut_move").put_move("move", m).commit(true);
    }

    pub fn pick(&mut self, mp: &MovePicker, m: Move) {
        self.begin("pick").put_move("move", m).put_str("stage", mp.trace_stage());
        if !mp.trace_is_tt_move() {
            self.put("score", mp.trace_score() as i64);
        }
        self.commit(false);
    }

    pub fn skip(&mut self, m: Move, reason: &str) {
        self.begin("skip").put_move("move", m).put_str("reason", reason).commit(false);
    }

    pub fn try_move(&mut self, m: Move, move_count: i32, capture: bool, gives_check: bool, reduction: i32) {
        self.begin("move")
            .put_move("move", m)
            .put("count", move_count as i64)
            .put("capture", capture as i64)
            .put("check", gives_check as i64)
            .put("reduction", reduction as i64)
            .commit(true);
    }

    pub fn try_qmove(&mut self, m: Move, move_count: i32, capture: bool, gives_check: bool) {
        self.begin("move")
            .put_move("move", m)
            .put("count", move_count as i64)
            .put("capture", capture as i64)
            .put("check", gives_check as i64)
            .commit(true);
    }

    pub fn prune0(&mut self, reason: &str) {
        self.begin("prune").put_str("reason", reason).commit(false);
    }

    pub fn prune1(&mut self, reason: &str, key: &str, value: i64) {
        self.begin("prune").put_str("reason", reason).put(key, value).commit(false);
    }

    pub fn prune2(&mut self, reason: &str, key1: &str, value1: i64, key2: &str, value2: i64) {
        self.begin("prune").put_str("reason", reason).put(key1, value1).put(key2, value2).commit(false);
    }

    pub fn move_value(&mut self, m: Move, value: Value) {
        self.begin("value").put_move("move", m).put("value", value as i64).commit(false);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn root_move(
        &mut self,
        m: Move,
        score: Value,
        uci_score: Value,
        average: Value,
        mean_squared: i64,
        sel_depth: i32,
        lower: bool,
        upper: bool,
    ) {
        let bound = if lower {
            "lower"
        } else if upper {
            "upper"
        } else {
            "exact"
        };
        self.begin("root_move")
            .put_move("move", m)
            .put("score", score as i64)
            .put("uci_score", uci_score as i64)
            .put("average", average as i64)
            .put("mean_squared", mean_squared)
            .put("seldepth", sel_depth as i64)
            .put_str("bound", bound)
            .commit(false);
    }

    // -----------------------------------------------------------------------
    // Writes

    pub fn store(&mut self, name: &str, table: &TranspositionTable, writer: TTWriter) {
        self.begin(name);
        self.slot(table, writer);
        self.commit(false);
    }

    /// The continuation history entries updated by
    /// update_continuation_histories().
    fn continuation_values(shared: &SharedHistories, stack: &[Stack], ss: usize, pc: Piece, to: Square) -> String {
        let mut s = String::new();
        for i in [1, 2, 3, 4, 6] {
            if stack[ss].in_check && i > 2 {
                break;
            }
            if !s.is_empty() {
                s.push(',');
            }
            if stack[ss - i].current_move.is_ok() {
                let h = shared.continuation_history_block.at(stack[ss - i].continuation_history);
                s += &h[pc as usize][to as usize].get().to_string();
            } else {
                s.push('-');
            }
        }
        s
    }

    #[allow(clippy::too_many_arguments)]
    pub fn quiet_update(
        &mut self,
        main_history: &ButterflyHistory,
        low_ply_history: &LowPlyHistory,
        shared: &SharedHistories,
        stack: &[Stack],
        pos: &Position,
        ss: usize,
        m: Move,
        bonus: i32,
    ) {
        let us = pos.side_to_move() as usize;
        let pc = pos.moved_piece(m);
        let to = m.to_sq();

        self.begin("quiet_update")
            .put_move("move", m)
            .put("bonus", bonus as i64)
            .put("main", main_history[us][m.raw() as usize].get() as i64);
        let ply = stack[ss].ply as usize;
        if ply < LOW_PLY_HISTORY_SIZE {
            self.put("low_ply", low_ply_history[ply][m.raw() as usize].get() as i64);
        }
        let values = Self::continuation_values(shared, stack, ss, pc, to);
        self.put_str("continuation", &values)
            .put("pawn", shared.pawn_entry(pos)[pc as usize][to as usize].get() as i64)
            .commit(false);
    }

    pub fn continuation_update(
        &mut self,
        shared: &SharedHistories,
        stack: &[Stack],
        ss: usize,
        pc: Piece,
        to: Square,
        bonus: i32,
    ) {
        let piece = (PIECE_TO_CHAR[pc as usize] as char).to_string();
        let values = Self::continuation_values(shared, stack, ss, pc, to);
        self.begin("continuation_update")
            .put_str("piece", &piece)
            .put_str("to", &square_to_string(to))
            .put("bonus", bonus as i64)
            .put_str("values", &values)
            .commit(false);
    }

    pub fn capture_update(&mut self, capture_history: &CapturePieceToHistory, pos: &Position, m: Move, bonus: i32) {
        let pc = pos.moved_piece(m);
        let captured = type_of(pos.piece_on(m.to_sq()));
        self.begin("capture_update")
            .put_move("move", m)
            .put("bonus", bonus as i64)
            .put("value", capture_history[pc as usize][m.to_sq() as usize][captured as usize].get() as i64)
            .commit(false);
    }

    pub fn correction_update(
        &mut self,
        shared: &SharedHistories,
        cch: &ContinuationCorrectionHistory,
        stack: &[Stack],
        pos: &Position,
        ss: usize,
        bonus: i32,
    ) {
        let us = pos.side_to_move() as usize;
        let m = stack[ss - 1].current_move;

        self.begin("correction_update")
            .put("bonus", bonus as i64)
            .put("pawn", shared.pawn_correction_entry(pos)[us].pawn.get() as i64)
            .put("minor", shared.minor_piece_correction_entry(pos)[us].minor.get() as i64)
            .put("white", shared.nonpawn_correction_entry::<WHITE>(pos)[us].non_pawn_white.get() as i64)
            .put("black", shared.nonpawn_correction_entry::<BLACK>(pos)[us].non_pawn_black.get() as i64);
        if m.is_ok() {
            let to = m.to_sq() as usize;
            let pc = pos.piece_on(m.to_sq()) as usize;
            let v = |i: usize| cch.at(stack[ss - i].continuation_correction_history)[pc][to].get();
            let s = format!("{},{},{}", v(2), v(4), v(6));
            self.put_str("continuation", &s);
        }
        self.commit(false);
    }

    pub fn eval_diff_update(
        &mut self,
        main_history: &ButterflyHistory,
        shared: &SharedHistories,
        pos: &Position,
        m: Move,
        bonus: i32,
        pawn: bool,
    ) {
        self.prior_update("eval_diff_update", main_history, shared, pos, m, bonus, pawn);
    }

    pub fn prior_quiet_update(
        &mut self,
        main_history: &ButterflyHistory,
        shared: &SharedHistories,
        pos: &Position,
        m: Move,
        bonus: i32,
        pawn: bool,
    ) {
        self.prior_update("prior_quiet_update", main_history, shared, pos, m, bonus, pawn);
    }

    #[allow(clippy::too_many_arguments)]
    fn prior_update(
        &mut self,
        name: &str,
        main_history: &ButterflyHistory,
        shared: &SharedHistories,
        pos: &Position,
        m: Move,
        bonus: i32,
        pawn: bool,
    ) {
        let prev_sq = m.to_sq();
        self.begin(name)
            .put_move("move", m)
            .put("bonus", bonus as i64)
            .put("main", main_history[not_color(pos.side_to_move()) as usize][m.raw() as usize].get() as i64);
        if pawn {
            self.put("pawn", shared.pawn_entry(pos)[pos.piece_on(prev_sq) as usize][prev_sq as usize].get() as i64);
        }
        self.commit(false);
    }

    pub fn prior_capture_update(&mut self, capture_history: &CapturePieceToHistory, pos: &Position, prev_sq: Square) {
        let v = capture_history[pos.piece_on(prev_sq) as usize][prev_sq as usize]
            [type_of(pos.captured_piece()) as usize]
            .get();
        self.begin("prior_capture_update")
            .put_str("to", &square_to_string(prev_sq))
            .put("value", v as i64)
            .commit(false);
    }

    pub fn ttmove_update(&mut self, bonus: i32, value: i16) {
        self.begin("ttmove_update").put("bonus", bonus as i64).put("value", value as i64).commit(false);
    }
}

// ---------------------------------------------------------------------------
// The tables a search leaves behind for the next one

/// A digest builder over the 16-bit entries of a table in row-major order,
/// four entries to a little-endian 64-bit word (`digest_i16` in C++, which
/// reads the memory of the table: the port reads the values, so the result
/// does not depend on the byte order).
struct I16Digest {
    d: TraceDigest,
    w: u64,
    n: u32,
}

impl I16Digest {
    fn new() -> I16Digest {
        I16Digest { d: TraceDigest::new(), w: 0, n: 0 }
    }

    #[inline(always)]
    fn add(&mut self, v: i16) {
        self.w |= (v as u16 as u64) << (16 * self.n);
        self.n += 1;
        if self.n == 4 {
            self.d.word(self.w);
            self.w = 0;
            self.n = 0;
        }
    }

    fn value(self) -> u64 {
        debug_assert!(self.n == 0, "a table of 16-bit entries whose size is not a multiple of 8 bytes");
        self.d.value
    }
}

fn digest_entries(entries: impl Iterator<Item = i16>) -> u64 {
    let mut d = I16Digest::new();
    for v in entries {
        d.add(v);
    }
    d.value()
}

fn piece_to_values<const D: i32>(t: &[[AtomicStatsEntry<D>; SQUARE_NB]; PIECE_NB]) -> impl Iterator<Item = i16> + '_ {
    t.iter().flat_map(|row| row.iter().map(|e| e.get()))
}

/// `SearchTracer::state_digests()`: the digests of the tables a search
/// leaves behind, as a JSON object. The low ply history is filled at the
/// start of each search and not cleared between games: it is left out
/// (null) when no search ran.
pub fn state_digests(w: &crate::search::Worker, table: &TranspositionTable, searched: bool) -> String {
    let shared = &*w.shared_history;

    let mut continuation = TraceDigest::new();
    for in_check in [false, true] {
        for capture in [NO_CAPTURES, CAPTURES] {
            let start = ContinuationHistoryBlock::index(in_check, capture, 0, 0);
            let tables = &shared.continuation_history_block.tables()[start..start + CONTINUATION_HISTORY_SIZE];
            continuation.word(digest_entries(tables.iter().flat_map(piece_to_values)));
        }
    }

    let main = digest_entries(w.main_history.iter().flat_map(|r| r.iter().map(|e| e.get())));
    let low_ply = digest_entries(w.low_ply_history.iter().flat_map(|r| r.iter().map(|e| e.get())));
    let capture =
        digest_entries(w.capture_history.iter().flat_map(|a| a.iter().flat_map(|b| b.iter().map(|e| e.get()))));

    let mut pawn = TraceDigest::new();
    for i in 0..shared.pawn_history.get_size() {
        pawn.word(digest_entries(piece_to_values(&shared.pawn_history[i])));
    }

    let mut correction = TraceDigest::new();
    for i in 0..shared.correction_history.get_size() {
        let e = &shared.correction_history[i];
        correction.word(digest_entries(
            e.iter().flat_map(|b| [b.pawn.get(), b.minor.get(), b.non_pawn_white.get(), b.non_pawn_black.get()]),
        ));
    }

    let cch = digest_entries(
        w.continuation_correction_history
            .tables()
            .iter()
            .flat_map(|t| t.iter().flat_map(|r| r.iter().map(|e| e.get()))),
    );

    let mut s = String::new();
    let mut add = |name: &str, value: String, quote: bool| {
        s.push(if s.is_empty() { '{' } else { ',' });
        if quote {
            s += &format!("\"{name}\":\"{value}\"");
        } else {
            s += &format!("\"{name}\":{value}");
        }
    };

    add("main_history", hex64(main), true);
    add("low_ply_history", if searched { hex64(low_ply) } else { "null".to_string() }, searched);
    add("capture_history", hex64(capture), true);
    add("continuation_history", hex64(continuation.value), true);
    add("pawn_history", hex64(pawn.value), true);
    add("correction_history", hex64(correction.value), true);
    add("continuation_correction_history", hex64(cch), true);
    add("ttmove_history", w.tt_move_history.get().to_string(), false);
    add("tt_generation", table.generation().to_string(), false);
    add("tt", hex64(table.trace_digest()), true);
    s + "}"
}

// ---------------------------------------------------------------------------
// The 'searchtrace' command

pub const CHECKPOINT_INTERVAL: u64 = 4096;

pub fn json_string(s: &[u8]) -> String {
    let mut r = Vec::with_capacity(s.len() + 2);
    r.push(b'"');
    for &c in s {
        if c == b'"' || c == b'\\' {
            r.push(b'\\');
        }
        if c < 0x20 {
            r.push(b' ');
        } else {
            r.push(c);
        }
    }
    r.push(b'"');
    String::from_utf8_lossy(&r).into_owned()
}

pub fn trim(s: &[u8]) -> &[u8] {
    let is_ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\r' | b'\n');
    let b = match s.iter().position(|c| !is_ws(c)) {
        Some(b) => b,
        None => return &s[..0],
    };
    let e = s.iter().rposition(|c| !is_ws(c)).unwrap();
    &s[b..=e]
}

/// One command of a session: a position to set up, or a search to run.
pub struct TraceCommand {
    pub is_go: bool,
    pub fen: Vec<u8>,
    pub moves: Vec<Vec<u8>>,
    pub depth: i32,
    pub nodes: u64,
    pub text: Vec<u8>,
}

/// `is >> n` of an `i64` on a whole token as `std::istringstream` reads it:
/// an optional sign and digits; the rest of the token would be read as the
/// next token, which the callers below reject anyway.
fn parse_i64_prefix(t: &[u8]) -> Option<(i64, &[u8])> {
    let sign_len = (t.first() == Some(&b'+') || t.first() == Some(&b'-')) as usize;
    let digits = t[sign_len..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let end = sign_len + digits;
    let v: i64 = std::str::from_utf8(&t[..end]).ok()?.parse().ok()?;
    Some((v, &t[end..]))
}

fn tokens(s: &[u8]) -> Vec<&[u8]> {
    s.split(|c| matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')).filter(|t| !t.is_empty()).collect()
}

/// Parses a session, "position ... ; go depth 5 ; position ... ; go nodes
/// 1000", returns an error message, or none.
pub fn parse_session(line: &[u8], commands: &mut Vec<TraceCommand>) -> Option<String> {
    let mut has_position = false;

    // std::getline(all, part, ';'): a trailing ';' gives no empty last part
    let mut parts: Vec<&[u8]> = line.split(|&c| c == b';').collect();
    if parts.len() > 1 && parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }

    for part in parts {
        let mut toks = tokens(part).into_iter().peekable();
        let mut cmd = TraceCommand {
            is_go: false,
            fen: Vec::new(),
            moves: Vec::new(),
            depth: 0,
            nodes: 0,
            text: trim(part).to_vec(),
        };

        let Some(token) = toks.next() else {
            return Some("Empty command".into());
        };

        if token == b"position" {
            cmd.is_go = false;
            let mut tok = toks.next();
            // `is >> token` keeps the old token when it fails
            let mut last = tok.unwrap_or(b"position");
            let mut failed = tok.is_none();
            if tok == Some(b"startpos") {
                cmd.fen = crate::uci::START_FEN.as_bytes().to_vec();
                tok = toks.next();
                failed = tok.is_none();
                if let Some(t) = tok {
                    last = t;
                }
            } else if tok == Some(b"fen") {
                loop {
                    match toks.next() {
                        Some(t) => {
                            last = t;
                            if t == b"moves" {
                                break;
                            }
                            cmd.fen.extend_from_slice(t);
                            cmd.fen.push(b' ');
                        }
                        None => {
                            failed = true;
                            break;
                        }
                    }
                }
            } else {
                return Some("Expected startpos or fen after position".into());
            }

            if !failed && last != b"moves" {
                return Some("Expected moves after the position".into());
            }

            for t in toks {
                cmd.moves.push(t.to_vec());
            }
            has_position = true;
        } else if token == b"go" {
            if !has_position {
                return Some("go before any position".into());
            }
            cmd.is_go = true;
            const ONLY: &str = "Only 'go depth N' and 'go nodes N' are allowed";
            while let Some(t) = toks.next() {
                if t != b"depth" && t != b"nodes" {
                    return Some(ONLY.into());
                }
                let n = match toks.next().and_then(parse_i64_prefix) {
                    Some((n, rest)) if n > 0 => {
                        if !rest.is_empty() {
                            // the rest would be read as the next token, which
                            // is neither "depth" nor "nodes"
                            return Some(ONLY.into());
                        }
                        n
                    }
                    _ => return Some(ONLY.into()),
                };
                if t == b"depth" {
                    cmd.depth = n.min(MAX_PLY as i64) as i32;
                } else {
                    cmd.nodes = n as u64;
                }
            }
            if cmd.depth == 0 && cmd.nodes == 0 {
                return Some("go needs a depth or a nodes limit".into());
            }

            // The limits as given back in the records, in a fixed form
            let mut text = String::new();
            if cmd.depth != 0 {
                text = format!("depth {}", cmd.depth);
            }
            if cmd.nodes != 0 {
                if !text.is_empty() {
                    text.push(' ');
                }
                text += &format!("nodes {}", cmd.nodes);
            }
            cmd.text = text.into_bytes();
        } else {
            let mut e = b"Unknown command ".to_vec();
            e.extend_from_slice(token);
            return Some(String::from_utf8_lossy(&e).into_owned());
        }

        commands.push(cmd);
    }

    if commands.is_empty() {
        return Some("Empty session".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_of_words_is_fnv1a() {
        let mut d = TraceDigest::new();
        d.bytes(b"a");
        assert_eq!(d.value, 0xaf63dc4c8601ec8c);
        let mut d = I16Digest::new();
        for v in [1i16, -1, 0, 2] {
            d.add(v);
        }
        let mut e = TraceDigest::new();
        e.word(0x0002_0000_FFFF_0001);
        assert_eq!(d.value(), e.value);
    }

    #[test]
    fn sessions_parse_like_cpp() {
        let mut c = Vec::new();
        assert_eq!(parse_session(b"position startpos ; go depth 3 nodes 100", &mut c), None);
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].text, b"depth 3 nodes 100");
        let mut c = Vec::new();
        assert_eq!(parse_session(b"go depth 3", &mut c).as_deref(), Some("go before any position"));
        let mut c = Vec::new();
        assert_eq!(
            parse_session(b"position startpos ; go movetime 3", &mut c).as_deref(),
            Some("Only 'go depth N' and 'go nodes N' are allowed")
        );
        let mut c = Vec::new();
        assert_eq!(parse_session(b"position fen 8/8/8/8/8/8/8/K1k5 w - - 0 1 moves a1a2 ; go nodes 7", &mut c), None);
        assert_eq!(c[0].fen, b"8/8/8/8/8/8/8/K1k5 w - - 0 1 ");
        assert_eq!(c[0].moves, vec![b"a1a2".to_vec()]);
    }
}

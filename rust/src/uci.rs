//! Port of `src/uci.{h,cpp}`: the command loop, the parsing of `go`, the
//! output of a search (`info` lines with their score formatting, `bestmove`),
//! `bench`, and the debug commands of the fork (`dump`, `dumpline`,
//! `searchtrace`).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

use crate::dump::{LineDumper, PositionDumper};
use crate::engine::Engine;
use crate::movegen::MoveList;
use crate::perft::perft_fen;
use crate::position::{square_to_string, Position};
use crate::score::Score;
use crate::search::{InfoFull, InfoIteration, InfoShort, LimitsType, UpdateContext};
use crate::searchtrace::SearchTraceWindow;
use crate::timeman::now;
use crate::types::*;

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// `UCIEngine::move()`: castling is `e1g1` in standard chess and king takes
/// rook in Chess960.
pub fn move_to_string(m: Move, chess960: bool) -> String {
    if m == Move::none() {
        return "(none)".to_string();
    }
    if m == Move::null() {
        return "0000".to_string();
    }

    let from = m.from_sq();
    let mut to = m.to_sq();

    if m.type_of() == CASTLING && !chess960 {
        to = make_square(if to > from { FILE_G } else { FILE_C }, rank_of(from));
    }

    let mut s = square_to_string(from);
    s += &square_to_string(to);

    if m.type_of() == PROMOTION {
        s.push(b" pnbrqk"[m.promotion_type() as usize] as char);
    }
    s
}

/// `UCIEngine::to_move()`: the legal move with this UCI notation, or none.
pub fn to_move(pos: &Position, s: &[u8]) -> Move {
    let s = s.to_ascii_lowercase();
    for &m in &MoveList::legal(pos) {
        if s == move_to_string(m, pos.is_chess960()).as_bytes() {
            return m;
        }
    }
    Move::none()
}

pub fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// `is >> n` for an int, applied to the start of a token: an optional sign
/// and the longest run of digits. Returns the value and the rest of the token,
/// which the stream would read as the next token, or None if C++ would fail
/// (no digits, or out of the range of int).
fn parse_int_prefix(t: &[u8]) -> Option<(i32, &[u8])> {
    let (v, rest) = parse_i64_prefix(t)?;
    Some((i32::try_from(v).ok()?, rest))
}

/// The same for a 64-bit integer (`TimePoint`).
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

/// The same for an unsigned 64-bit integer: `strtoull` accepts a sign, and
/// negates in unsigned arithmetic.
fn parse_u64_prefix(t: &[u8]) -> Option<(u64, &[u8])> {
    let neg = t.first() == Some(&b'-');
    let sign_len = (neg || t.first() == Some(&b'+')) as usize;
    let digits = t[sign_len..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let end = sign_len + digits;
    let v: u64 = std::str::from_utf8(&t[sign_len..end]).ok()?.parse().ok()?;
    Some((if neg { v.wrapping_neg() } else { v }, &t[end..]))
}

/// The whitespace separated tokens of a command, as `is >> token` reads them.
/// Commands are kept as bytes, as in C++, so that a byte which is not UTF-8
/// reaches the FEN parser and the file system unchanged.
struct Tokens<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        let start = self.rest.iter().position(|&c| !is_c_space(c))?;
        let rest = &self.rest[start..];
        let len = rest.iter().position(|&c| is_c_space(c)).unwrap_or(rest.len());
        self.rest = &rest[len..];
        Some(&rest[..len])
    }
}

pub struct UciEngine {
    engine: Engine,
    current_cmd: Vec<u8>,
}

impl Default for UciEngine {
    fn default() -> Self {
        UciEngine::new()
    }
}

impl UciEngine {
    pub fn new() -> UciEngine {
        UciEngine { engine: Engine::new(), current_cmd: Vec::new() }
    }

    fn terminate_on_critical_error(&self, message: &[u8]) -> ! {
        let mut out = io::stdout().lock();
        let _ = out.write_all(b"info string CRITICAL ERROR: Command `");
        let _ = out.write_all(&self.current_cmd);
        let _ = out.write_all(b"` failed. Reason: ");
        let _ = out.write_all(message);
        let _ = out.write_all(b"\n\n");
        let _ = out.flush();
        std::process::exit(1);
    }

    /// Reads commands from the command line (one shot) or from stdin.
    pub fn run(&mut self, args: &[std::ffi::OsString]) {
        if !args.is_empty() {
            let mut cmd = Vec::new();
            for a in args {
                cmd.extend_from_slice(os_bytes(a).as_ref());
                cmd.push(b' ');
            }
            self.execute(&cmd);
            self.engine.wait_for_search_finished();
            return;
        }

        let stdin = io::stdin();
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = stdin.lock().read_until(b'\n', &mut line).unwrap_or(0);
            if n == 0 {
                line.extend_from_slice(b"quit");
            } else if line.last() == Some(&b'\n') {
                line.pop();
            }
            if !self.execute(&line) {
                break;
            }
        }
        self.engine.wait_for_search_finished();
    }

    /// Executes one command, returns false on `quit`.
    pub fn execute(&mut self, cmd: &[u8]) -> bool {
        self.current_cmd = cmd.to_vec();
        let mut tokens = Tokens { rest: cmd };
        let token = tokens.next().unwrap_or(b"");

        match token {
            b"quit" | b"stop" => {
                self.engine.stop();
                return token != b"quit";
            }
            // The GUI sends 'ponderhit' to tell that the user has played the
            // expected move. So, 'ponderhit' is sent if pondering was done on
            // the same move that the user has played. The search should
            // continue, but should also switch from pondering to the normal
            // search.
            b"ponderhit" => self.engine.set_ponderhit(false),
            b"uci" => {
                let mut out = io::stdout().lock();
                let _ = writeln!(
                    out,
                    "id name Stockfish Rust port\nid author the Stockfish developers (see AUTHORS file)\n{}",
                    self.engine.options.uci_list()
                );
                let _ = writeln!(out, "uciok");
            }
            b"isready" => {
                let _ = writeln!(io::stdout().lock(), "readyok");
            }
            b"ucinewgame" => self.engine.search_clear(),
            b"setoption" => self.setoption(tokens),
            b"position" => self.position(tokens),
            b"go" => self.go(tokens),
            b"bench" => self.bench(tokens),
            b"dump" => self.dump(tokens.next()),
            b"dumpline" => {
                // std::getline(args >> std::ws, rest)
                let start = tokens.rest.iter().position(|&c| !is_c_space(c)).unwrap_or(tokens.rest.len());
                self.dump_line(&tokens.rest[start..]);
            }
            b"searchtrace" => self.search_trace(tokens),
            b"d" => {
                let _ = writeln!(
                    io::stdout().lock(),
                    "\nFen: {}\nKey: {:016X}",
                    self.engine.pos.fen(),
                    self.engine.pos.key()
                );
            }
            b"" => {}
            t if t[0] == b'#' => {}
            _ => {
                let mut out = io::stdout().lock();
                let _ = out.write_all(b"Unknown command: '");
                let _ = out.write_all(cmd);
                let _ = out.write_all(b"'. Type help for more information.\n");
            }
        }
        true
    }

    fn setoption<'a>(&mut self, mut tokens: impl Iterator<Item = &'a [u8]>) {
        self.engine.wait_for_search_finished();
        tokens.next(); // Consume the "name" token
        let mut name = Vec::new();
        let mut value = Vec::new();
        for t in tokens.by_ref() {
            if t == b"value" {
                break;
            }
            if !name.is_empty() {
                name.push(b' ');
            }
            name.extend_from_slice(t);
        }
        for t in tokens {
            if !value.is_empty() {
                value.push(b' ');
            }
            value.extend_from_slice(t);
        }
        let name = String::from_utf8_lossy(&name).into_owned();
        let value = String::from_utf8_lossy(&value).into_owned();
        self.engine.setoption(&name, &value);
    }

    fn position<'a>(&mut self, mut tokens: impl Iterator<Item = &'a [u8]>) {
        let mut fen = Vec::new();
        match tokens.next() {
            Some(b"startpos") => {
                fen.extend_from_slice(START_FEN.as_bytes());
                tokens.next(); // Consume the "moves" token, if any
            }
            Some(b"fen") => {
                for t in tokens.by_ref() {
                    if t == b"moves" {
                        break;
                    }
                    fen.extend_from_slice(t);
                    fen.push(b' ');
                }
            }
            _ => return,
        }

        let moves: Vec<&[u8]> = tokens.collect();
        if let Err(e) = self.engine.set_position(&fen, &moves) {
            self.terminate_on_critical_error(e.what());
        }
    }

    /// `parse_limits()`: the arguments of `go`. A numeric argument is read
    /// as `is >> n` reads it: its numeric prefix, the rest of the token being
    /// read as the next token; a failure ends the program.
    fn parse_limits<'a>(&self, mut tokens: impl Iterator<Item = &'a [u8]>) -> LimitsType {
        let mut limits = LimitsType { start_time: now(), ..LimitsType::default() }; // The search starts as early as possible

        let mut pending: Option<&[u8]> = None;
        while let Some(t) = pending.take().or_else(|| tokens.next()) {
            if t == b"searchmoves" {
                // Needs to be the last command on the line
                for m in tokens.by_ref() {
                    limits.searchmoves.push(String::from_utf8_lossy(&m.to_ascii_lowercase()).into_owned());
                }
                break;
            }

            macro_rules! read {
                ($parse:ident, $field:expr) => {{
                    match tokens.next().and_then($parse) {
                        Some((v, rest)) => {
                            $field = v;
                            if !rest.is_empty() {
                                pending = Some(rest);
                            }
                        }
                        None => {
                            let mut msg = b"Invalid argument for '".to_vec();
                            msg.extend_from_slice(t);
                            msg.push(b'\'');
                            self.terminate_on_critical_error(&msg);
                        }
                    }
                }};
            }

            match t {
                b"wtime" => read!(parse_i64_prefix, limits.time[WHITE as usize]),
                b"btime" => read!(parse_i64_prefix, limits.time[BLACK as usize]),
                b"winc" => read!(parse_i64_prefix, limits.inc[WHITE as usize]),
                b"binc" => read!(parse_i64_prefix, limits.inc[BLACK as usize]),
                b"movestogo" => read!(parse_int_prefix, limits.movestogo),
                b"depth" => read!(parse_int_prefix, limits.depth),
                b"nodes" => read!(parse_u64_prefix, limits.nodes),
                b"movetime" => read!(parse_i64_prefix, limits.movetime),
                b"mate" => read!(parse_int_prefix, limits.mate),
                b"perft" => read!(parse_int_prefix, limits.perft),
                b"infinite" => limits.infinite = 1,
                b"ponder" => limits.ponder_mode = true,
                _ => {}
            }
        }

        // Sanitize negative time values, zero for stm means no TM
        for t in limits.time.iter_mut().chain(std::iter::once(&mut limits.movetime)) {
            if *t < 0 {
                *t = 1;
            }
        }

        limits
    }

    fn go<'a>(&mut self, tokens: impl Iterator<Item = &'a [u8]>) {
        let limits = self.parse_limits(tokens);

        if limits.perft != 0 {
            self.perft(limits.perft);
        } else if limits.use_time_management() {
            println!("info string Time management is not ported yet: use go depth, nodes, movetime or infinite");
        } else {
            self.engine.go(limits);
        }
    }

    fn perft(&mut self, depth: Depth) -> u64 {
        self.engine.wait_for_search_finished();
        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        let fen = self.engine.pos.fen();
        match perft_fen(fen.as_bytes(), depth, self.engine.chess960(), &mut out) {
            Ok(nodes) => {
                let _ = writeln!(out, "\nNodes searched: {nodes}\n");
                nodes
            }
            Err(e) => {
                drop(out);
                self.terminate_on_critical_error(e.what());
            }
        }
    }

    /// `bench`: searches the positions of the benchmark (or of a file) and
    /// prints the total of the nodes searched, the bench signature.
    fn bench<'a>(&mut self, tokens: impl Iterator<Item = &'a [u8]>) {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        use std::sync::Arc;

        let nodes_searched = Arc::new(AtomicU64::new(0));
        let saved = self.engine.update_context();
        let ns = nodes_searched.clone();
        self.engine.set_update_context(UpdateContext {
            on_update_no_moves: Box::new(on_update_no_moves),
            on_update_full: Box::new(move |i: &InfoFull| {
                ns.store(i.nodes, Relaxed);
                on_update_full(i);
            }),
            on_iter: Box::new(on_iter),
            on_bestmove: Box::new(on_bestmove),
            on_start: Box::new(|| {}),
        });

        let args: Vec<&[u8]> = tokens.collect();
        let list = crate::benchmark::setup_bench(&self.engine.pos.fen(), &args);

        let num = list.iter().filter(|s| s.starts_with(b"go ") || s.starts_with(b"eval")).count();

        let mut elapsed = now();
        let mut nodes: u64 = 0;
        let mut cnt = 1;

        for cmd in &list {
            let mut toks = Tokens { rest: cmd };
            let token = toks.next().unwrap_or(b"");

            if token == b"go" || token == b"eval" {
                eprintln!("\nPosition: {cnt}/{num} ({})", self.engine.pos.fen());
                cnt += 1;
                if token == b"go" {
                    let limits = self.parse_limits(toks);

                    if limits.perft != 0 {
                        let n = self.perft(limits.perft);
                        nodes_searched.store(n, Relaxed);
                    } else {
                        self.engine.go(limits);
                        self.engine.wait_for_search_finished();
                    }

                    nodes += nodes_searched.swap(0, Relaxed);
                } else {
                    println!("info string eval is not ported");
                }
            } else if token == b"setoption" {
                self.setoption(toks);
            } else if token == b"position" {
                self.position(toks);
            } else if token == b"ucinewgame" {
                self.engine.search_clear(); // search_clear may take a while
                elapsed = now();
            }
        }

        let elapsed = (now() - elapsed + 1) as u64; // Ensure positivity to avoid a 'divide by zero'

        eprintln!(
            "\n===========================\nTotal time (ms) : {elapsed}\nNodes searched  : {nodes}\nNodes/second    : {}",
            1000 * nodes / elapsed
        );

        self.engine.restore_update_context(saved);
    }

    /// `searchtrace <file> [events <search>|all <from> <to>] [counts]`.
    fn search_trace<'a>(&mut self, mut tokens: impl Iterator<Item = &'a [u8]>) {
        let filename = tokens.next().unwrap_or(b"").to_vec();
        let mut window = SearchTraceWindow::default();
        let mut valid = true;

        while valid {
            let Some(token) = tokens.next() else { break };
            if token == b"counts" {
                window.counts = true;
            } else if token == b"events" {
                let which = tokens.next();
                let from = tokens.next().and_then(|t| std::str::from_utf8(t).ok()?.parse::<u64>().ok());
                let to = tokens.next().and_then(|t| std::str::from_utf8(t).ok()?.parse::<u64>().ok());
                match (which, from, to) {
                    (Some(w), Some(f), Some(t)) if f < t => {
                        window.from = f;
                        window.to = t;
                        if w == b"all" {
                            window.search = -1;
                        } else if w.is_empty() || !w.iter().all(|c| c.is_ascii_digit()) {
                            valid = false;
                        } else {
                            window.search = std::str::from_utf8(w).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
                            if window.search < 1 {
                                valid = false;
                            }
                        }
                    }
                    _ => valid = false,
                }
            } else {
                valid = false;
            }
        }

        if !valid {
            self.terminate_on_critical_error(b"Usage: searchtrace <file> [events <search>|all <from> <to>] [counts]");
        }

        if filename.is_empty() || !self.engine.search_trace(&filename, window) {
            let mut msg = b"Unable to read file ".to_vec();
            msg.extend_from_slice(&filename);
            self.terminate_on_critical_error(&msg);
        }
    }

    /// `dump`: the current position. `dump <file>`: each position of a file
    /// of FENs, one per line, blank lines skipped, streamed.
    fn dump(&mut self, filename: Option<&[u8]>) {
        // C++ opens the file before it verifies the network
        let file = filename.map(|f| (f, File::open(path_from_bytes(f))));
        if let Some((filename, Err(_))) = &file {
            let mut msg = b"Unable to open file ".to_vec();
            msg.extend_from_slice(filename);
            self.terminate_on_critical_error(&msg);
        }

        let network = self.engine.verify_network();
        let chess960 = self.engine.chess960();
        let mut dumper = PositionDumper::new(&network);
        let mut buf = Vec::with_capacity(8192);

        let Some((_, Ok(file))) = file else {
            dumper.dump(self.engine.pos.fen().as_bytes(), chess960, &mut buf);
            buf.push(b'\n');
            let _ = io::stdout().lock().write_all(&buf);
            return;
        };

        let stdout = io::stdout();
        let mut out = io::BufWriter::with_capacity(1 << 16, stdout.lock());
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            // getline() drops the newline only
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.iter().all(|&c| matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')) {
                continue;
            }
            buf.clear();
            dumper.dump(&line, chess960, &mut buf);
            buf.push(b'\n');
            if out.write_all(&buf).is_err() {
                break;
            }
        }
        let _ = out.flush();
    }

    /// `dumpline startpos|fen ... moves ...`: the given line of play.
    /// `dumpline <file>`: each line of a file, one per line, blank lines
    /// skipped, streamed.
    fn dump_line(&mut self, rest: &[u8]) {
        let first = Tokens { rest }.next().unwrap_or(b"");
        let mut buf = Vec::with_capacity(1 << 16);
        let chess960 = self.engine.chess960();

        if first.is_empty() || first == b"startpos" || first == b"fen" {
            let network = self.engine.verify_network();
            let mut dumper = LineDumper::new(&network);
            dumper.dump(rest, chess960, &mut buf);
            let mut out = io::stdout().lock();
            let _ = out.write_all(&buf);
            let _ = out.flush();
            return;
        }

        let Ok(file) = File::open(path_from_bytes(first)) else {
            let mut msg = b"Unable to read file ".to_vec();
            msg.extend_from_slice(first);
            self.terminate_on_critical_error(&msg);
        };

        let network = self.engine.verify_network();
        let mut dumper = LineDumper::new(&network);
        let stdout = io::stdout();
        let mut out = io::BufWriter::with_capacity(1 << 16, stdout.lock());
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut read_error = false;
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Err(_) => {
                    read_error = true;
                    break;
                }
                Ok(_) => {}
            }
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.iter().all(|&c| is_c_space(c)) {
                continue;
            }
            buf.clear();
            dumper.dump(&line, chess960, &mut buf);
            if out.write_all(&buf).is_err() {
                break;
            }
        }
        let _ = out.flush();
        drop(out);
        // A read error must not pass for the end of the file
        if read_error {
            let mut msg = b"Unable to read file ".to_vec();
            msg.extend_from_slice(first);
            self.terminate_on_critical_error(&msg);
        }
    }
}

// ---------------------------------------------------------------------------
// The output of a search

/// `win_rate_params()`: the parameters of the win rate model for the
/// material of a position.
///
/// The doubles here reach the output (`score cp` through `to_cp`, `wdl`
/// through `win_rate_model`). They are computed as C++ does, in the same
/// order, which x86-64 evaluates in IEEE doubles on both sides (Rust never
/// fuses operations; the C++ builds have no FMA). On targets that compute
/// with more precision (x87) or another `exp`, the results stay the same
/// because no input comes near a rounding boundary: over every material
/// count and every value, `100 * v / a` stays 3.1e-7 away from a half
/// integer and `0.5 + 1000 / (1 + exp(...))` 2.5e-6 away from an integer,
/// far beyond any error of a double computation. The tests
/// `to_cp_is_far_from_rounding_boundaries` and
/// `win_rate_is_far_from_rounding_boundaries` check this on each target.
fn win_rate_params(pos: &Position) -> (f64, f64) {
    let material = pos.count_both(PAWN)
        + 3 * pos.count_both(KNIGHT)
        + 3 * pos.count_both(BISHOP)
        + 5 * pos.count_both(ROOK)
        + 9 * pos.count_both(QUEEN);
    win_rate_params_of(material)
}

fn win_rate_params_of(material: i32) -> (f64, f64) {
    // The fitted model only uses data for material counts in [17, 78], and is
    // anchored at count 58.
    let m = material.clamp(17, 78) as f64 / 58.0;

    // Return a = p_a(material) and b = p_b(material), see
    // github.com/official-stockfish/WDL_model
    const AS: [f64; 4] = [-142.72052667, 372.35176398, -340.71073572, 415.23490212];
    const BS: [f64; 4] = [5.93832785, 15.61267078, -30.57816876, 69.63866711];

    let a = (((AS[0] * m + AS[1]) * m + AS[2]) * m) + AS[3];
    let b = (((BS[0] * m + BS[1]) * m + BS[2]) * m) + BS[3];

    (a, b)
}

/// The win rate model is 1 / (1 + exp((a - eval) / b)), where a =
/// p_a(material) and b = p_b(material). It fits the LTC fishtest statistics
/// rather accurately. Returns the win rate in per mille units, rounded to
/// the nearest integer.
fn win_rate_model_of(v: Value, a: f64, b: f64) -> i32 {
    (0.5 + 1000.0 / (1.0 + ((a - v as f64) / b).exp())) as i32
}

/// `to_cp()`: turns a Value to an integer centipawn number, without
/// treatment of mate and similar special scores.
pub fn to_cp(v: Value, pos: &Position) -> i32 {
    // In general, the score can be defined via the WDL as
    // (log(1/L - 1) - log(1/W - 1)) / (log(1/L - 1) + log(1/W - 1)).
    // Based on our win_rate_model, this simply yields v / a.
    let (a, _) = win_rate_params(pos);
    to_cp_of(v, a)
}

/// `int(std::round(100 * int(v) / a))`: `round` goes half away from zero, as
/// Rust's does.
fn to_cp_of(v: Value, a: f64) -> i32 {
    ((100 * v) as f64 / a).round() as i32
}

/// `wdl()`: the "wdl W D L" of the `info` lines with `UCI_ShowWDL`.
pub fn wdl(v: Value, pos: &Position) -> String {
    let (a, b) = win_rate_params(pos);
    let wdl_w = win_rate_model_of(v, a, b);
    let wdl_l = win_rate_model_of(-v, a, b);
    let wdl_d = 1000 - wdl_w - wdl_l;
    format!("{wdl_w} {wdl_d} {wdl_l}")
}

/// `format_score()`.
pub fn format_score(s: &Score) -> String {
    const TB_CP: i32 = 20000;
    match *s {
        Score::Mate { plies } => {
            let m = (if plies > 0 { plies + 1 } else { plies }) / 2;
            format!("mate {m}")
        }
        Score::Tablebase { plies, win } => format!("cp {}", (if win { TB_CP } else { -TB_CP }) - plies),
        Score::InternalUnits { value } => format!("cp {value}"),
    }
}

pub fn on_update_no_moves(info: &InfoShort) {
    println!("info depth {} score {}", info.depth, format_score(&info.score));
}

pub fn on_update_full(info: &InfoFull) {
    let mut s = format!(
        "info depth {} seldepth {} multipv {} score {}",
        info.depth,
        info.sel_depth,
        info.multi_pv,
        format_score(&info.score)
    );

    if !info.bound.is_empty() {
        s.push(' ');
        s += info.bound;
    }

    // The wdl is computed only when UCI_ShowWDL is set
    if !info.wdl.is_empty() {
        s += " wdl ";
        s += info.wdl;
    }

    s += &format!(
        " nodes {} nps {} hashfull {} tbhits {} time {} pv {}",
        info.nodes, info.nps, info.hashfull, info.tb_hits, info.time_ms, info.pv
    );

    println!("{s}");
}

pub fn on_iter(info: &InfoIteration) {
    println!("info depth {} currmove {} currmovenumber {}", info.depth, info.currmove, info.currmovenumber);
}

pub fn on_bestmove(bestmove: &str, ponder: &str) {
    if ponder.is_empty() {
        println!("bestmove {bestmove}");
    } else {
        println!("bestmove {bestmove} ponder {ponder}");
    }
}

/// A command line argument as bytes, unchanged on Unix.
fn os_bytes(s: &std::ffi::OsStr) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::borrow::Cow::Borrowed(s.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::borrow::Cow::Owned(s.to_string_lossy().into_owned().into_bytes())
    }
}

/// A file name given as bytes, unchanged on Unix.
pub fn path_from_bytes(b: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_prefix_like_istream() {
        assert_eq!(parse_int_prefix(b"5"), Some((5, &b""[..])));
        assert_eq!(parse_int_prefix(b"1x"), Some((1, &b"x"[..])));
        assert_eq!(parse_int_prefix(b"+3"), Some((3, &b""[..])));
        assert_eq!(parse_int_prefix(b"-2"), Some((-2, &b""[..])));
        assert_eq!(parse_int_prefix(b"x1"), None);
        assert_eq!(parse_int_prefix(b"99999999999"), None);
    }

    /// Every value `to_cp` can get (not decisive) for every material count:
    /// the quotient is far enough from a half integer that this target's
    /// doubles round it as IEEE doubles on x86-64 do (see win_rate_params).
    #[test]
    fn to_cp_is_far_from_rounding_boundaries() {
        let mut min = f64::MAX;
        for material in 17..=78 {
            let (a, _) = win_rate_params_of(material);
            for v in -(VALUE_TB_WIN_IN_MAX_PLY - 1)..VALUE_TB_WIN_IN_MAX_PLY {
                let q = (100 * v) as f64 / a;
                let d = (q - (q.floor() + 0.5)).abs();
                min = min.min(d);
            }
        }
        assert!(min > 3.0e-7, "{min}");
        // (material, internal value, cp) as the C++ binary prints them, from
        // the golden file of the trace sample
        for (material, v, cp) in
            [(78, -35, -12), (70, -150, -51), (58, 853, 280), (40, -2313, -745), (17, -3100, -902), (10, 2613, 760)]
        {
            let (a, _) = win_rate_params_of(material);
            assert_eq!(to_cp_of(v, a), cp, "material {material}, value {v}");
        }
    }

    /// The same for the win rate of `wdl`, over every value a score can
    /// take.
    #[test]
    fn win_rate_is_far_from_rounding_boundaries() {
        let mut min = f64::MAX;
        for material in 17..=78 {
            let (a, b) = win_rate_params_of(material);
            for v in -VALUE_INFINITE..=VALUE_INFINITE {
                let x = 0.5 + 1000.0 / (1.0 + ((a - v as f64) / b).exp());
                min = min.min((x - x.round()).abs());
            }
        }
        assert!(min > 2.0e-6, "{min}");
    }

    #[test]
    fn tokens_keep_bytes() {
        let t: Vec<&[u8]> = Tokens { rest: b"  position fen 4k3/8/8/8/8/8/8/4K2\xFF\tw\x0b- " }.collect();
        assert_eq!(t.len(), 5);
        let raw: &[u8] = b"a \xff\xfe b";
        let t: Vec<&[u8]> = Tokens { rest: raw }.collect();
        assert_eq!(t, vec![&b"a"[..], &b"\xff\xfe"[..], &b"b"[..]]);
    }
}

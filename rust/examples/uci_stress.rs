//! Stress test of an engine over UCI, for the thread pool (README.md,
//! "Threads"): random sessions as a GUI makes them, with every command a
//! deadline, checking each answer. It drives any UCI engine (the port, or
//! the C++ engine to check the checks), under an emulator if `RUN_PREFIX`
//! is set; it uses the port's library only to judge moves.
//!
//!     uci_stress [--seconds S] [--searches N] [--seed X] [--threads LIST]
//!                [--hash LIST] [--slow F] [--light] ENGINE...
//!
//! Sessions draw from: `setoption` of `Threads` (LIST, default 1,2,3,4,8 and
//! more threads than cores), `Hash` (LIST, default 1,2,16), `MultiPV`,
//! `UCI_Chess960`, `Ponder`, `Clear Hash`; `ucinewgame`; positions
//! (standard, Chess960, mates, stalemates, positions after random legal
//! moves); and `go` with `nodes`, `depth`, `movetime`, a clock (with
//! increment or `movestogo`), `mate`, `infinite`, `ponder`, `searchmoves`,
//! followed at random by nothing, an immediate `stop`, a `stop` or a
//! `ponderhit` after a random delay, or `quit` (the engine is restarted).
//!
//! Each search must end with one `bestmove` within its deadline (a hang
//! fails the run, it never blocks it); the `bestmove` and the `ponder` move
//! must be legal, and `(none)` exactly when there is no legal move; with
//! `searchmoves`, among them; every PV a legal line; MultiPV indices within
//! the option; node counts that never decrease within a search; `go nodes
//! N` within N plus an allowance; `go depth D` reaching D when not stopped;
//! known mates reported with their sign and distance and the mating move;
//! nothing printed after `bestmove` (an `isready` sent at once must be
//! answered by the next line); a `quit` in a search must end the process
//! with status 0. The deadlines are multiplied by `--slow`, and `--light`
//! keeps the searches short (at most 20,000 nodes, depth 7, 200 ms, clocks
//! of 2 s): for emulation.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use stockfish::movegen::MoveList;
use stockfish::position::Position;
use stockfish::types::*;
use stockfish::uci;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// A position of the test, and what a search of it must find.
struct TestPosition {
    fen: &'static str,
    chess960: bool,
    /// The mate the side to move gives (> 0) or receives (< 0) and the
    /// mating move, for the mate positions
    mate: Option<(i32, Option<&'static str>)>,
    /// A searchmoves that keeps the mate, for the mate positions
    searchmoves: Option<&'static str>,
}

const fn p(fen: &'static str) -> TestPosition {
    TestPosition { fen, chess960: false, mate: None, searchmoves: None }
}

const POSITIONS: &[TestPosition] = &[
    p("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
    p("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 10"),
    p("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 11"),
    p("4rrk1/pp1n3p/3q2pQ/2p1pb2/2PP4/2P3N1/P2B2PP/4RRK1 b - - 7 19"),
    p("r3r1k1/2p2ppp/p1p1bn2/8/1q2P3/2NPQN2/PPP3PP/R4RK1 b - - 2 15"),
    p("3q2k1/pb3p1p/4pbp1/2r5/PpN2N2/1P2P2P/5PP1/Q2R2K1 b - - 4 26"),
    p("8/6pk/1p6/8/PP3p1p/5P2/4KP1q/3Q4 w - - 0 1"),
    p("5rk1/q6p/2p3bR/1pPp1rP1/1P1Pp3/P3B1Q1/1K3P2/R7 w - - 93 90"),
    p("k7/2n1n3/1nbNbn2/2NbRBn1/1nbRQR2/2NBRBN1/3N1N2/7K w - - 0 1"),
    p("8/8/8/5N2/8/p7/8/2NK3k w - - 0 1"),
    p("8/R7/2q5/8/6k1/8/1P5p/K6R w - - 0 124"),
    TestPosition {
        fen: "bbqnnrkr/pppppppp/8/8/8/8/PPPPPPPP/BBQNNRKR w HFhf - 0 1",
        chess960: true,
        mate: None,
        searchmoves: None,
    },
    TestPosition {
        fen: "nqbnrkrb/pppppppp/8/8/8/8/PPPPPPPP/NQBNRKRB w KQkq - 0 1",
        chess960: true,
        mate: None,
        searchmoves: None,
    },
    TestPosition {
        fen: "5K2/8/2qk4/2nPp3/3r4/6B1/B7/3R4 w - e6 0 1",
        chess960: false,
        mate: Some((1, Some("d5e6"))),
        searchmoves: None,
    },
    TestPosition {
        fen: "r1b2r1k/pp1p2pp/2p5/2B1q3/8/8/P1PN2PP/R4RK1 w - - 0 18",
        chess960: false,
        mate: Some((1, None)),
        searchmoves: None,
    },
    TestPosition {
        fen: "2brrb2/8/p7/Q7/1p1kpPp1/1P1pN1K1/3P4/8 b - - 0 1",
        chess960: false,
        mate: Some((-1, None)),
        searchmoves: None,
    },
    TestPosition {
        fen: "8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - - 0 1",
        chess960: false,
        mate: Some((2, Some("c6d7"))),
        searchmoves: Some("c6d7"),
    },
    TestPosition {
        fen: "8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - - 0 1 moves c6d7 f2f1q",
        chess960: false,
        mate: Some((1, Some("f7f5"))),
        searchmoves: None,
    },
    // mated, stalemated
    p("R5k1/5ppp/8/8/8/8/8/4K3 b - - 0 1"),
    p("8/8/8/8/8/6k1/6p1/6K1 w - - 0 1"),
    p("7k/7P/6K1/8/3B4/8/8/8 b - - 0 1"),
];

/// The engine under test, its output read by a thread into a channel.
struct Engine {
    child: Child,
    stdin: ChildStdin,
    out: Receiver<Option<String>>,
    /// The last lines, for the report of a failure
    log: Vec<String>,
}

impl Engine {
    fn start(cmd: &[String]) -> Engine {
        let mut child = Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {cmd:?}: {e}"));
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(Some(l)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(None);
        });
        Engine { child, stdin, out: rx, log: Vec::new() }
    }

    fn send(&mut self, cmd: &str) -> Result<(), String> {
        self.log.push(format!("> {cmd}"));
        writeln!(self.stdin, "{cmd}").and_then(|_| self.stdin.flush()).map_err(|e| format!("cannot send '{cmd}': {e}"))
    }

    /// The next line, within the deadline.
    fn line(&mut self, deadline: Instant, waiting_for: &str) -> Result<String, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        match self.out.recv_timeout(left) {
            Ok(Some(l)) => {
                self.log.push(l.clone());
                if self.log.len() > 400 {
                    self.log.drain(..200);
                }
                Ok(l)
            }
            Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                Err(format!("the engine exited, waiting for {waiting_for}"))
            }
            Err(RecvTimeoutError::Timeout) => Err(format!("no {waiting_for} before the deadline (hang)")),
        }
    }

    /// Reads until a line starting with `prefix`, returns the lines read.
    fn until(&mut self, prefix: &str, deadline: Instant) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        loop {
            let l = self.line(deadline, prefix)?;
            let done = l.starts_with(prefix);
            lines.push(l);
            if done {
                return Ok(lines);
            }
        }
    }

    /// `isready`, whose answer must be the very next line.
    fn ready(&mut self, deadline: Instant) -> Result<(), String> {
        self.send("isready")?;
        let l = self.line(deadline, "readyok")?;
        if l != "readyok" {
            return Err(format!("'{l}' where readyok was expected (output after bestmove?)"));
        }
        Ok(())
    }

    /// Waits for the process to end after `quit`.
    fn wait_exit(&mut self, within: Duration) -> Result<(), String> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                return if status.success() { Ok(()) } else { Err(format!("quit: exit status {status}")) };
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                return Err("quit: the engine did not exit (hang)".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Stats {
    searches: u64,
    by_kind: std::collections::BTreeMap<&'static str, u64>,
    by_threads: std::collections::BTreeMap<usize, u64>,
    stops: u64,
    quits: u64,
    mates_checked: u64,
    /// go nodes N: the largest (reported - N) and (reported / N)
    nodes_over: Vec<(usize, u64, u64)>,
}

struct Session {
    engine: Engine,
    cmd: Vec<String>,
    rng: Rng,
    slow: f64,
    threads: usize,
    multipv: usize,
    chess960: bool,
    threads_list: Vec<usize>,
    hash_list: Vec<usize>,
    light: bool,
    stats: Stats,
}

/// What a search was, to judge its output.
struct Search<'a> {
    pos: Position,
    tp: &'a TestPosition,
    extra_moves: usize,
    go: String,
    kind: &'static str,
    searchmoves: Vec<String>,
    stopped: bool,
}

impl Session {
    fn deadline(&self, secs: f64) -> Instant {
        Instant::now() + Duration::from_secs_f64(secs * self.slow)
    }

    fn setoption(&mut self, name: &str, value: &str) -> Result<(), String> {
        self.engine.send(&format!("setoption name {name} value {value}"))
    }

    fn restart(&mut self) -> Result<(), String> {
        self.engine = Engine::start(&self.cmd);
        self.threads = 1;
        self.multipv = 1;
        self.chess960 = false;
        let d = self.deadline(60.0);
        self.engine.send("uci")?;
        self.engine.until("uciok", d)?;
        self.engine.ready(d)
    }

    /// The options of the next search, at random.
    fn options(&mut self) -> Result<(), String> {
        if self.rng.chance(30) {
            let t = *self.rng.pick(&self.threads_list.clone());
            self.setoption("Threads", &t.to_string())?;
            // the answer of C++ and the port
            let d = self.deadline(60.0);
            let want = format!("info string Using {t} {}", if t > 1 { "threads" } else { "thread" });
            let l = self.engine.until("info string Using", d)?;
            if l.last().unwrap() != &want {
                return Err(format!("'{}' after setoption Threads {t}", l.last().unwrap()));
            }
            self.threads = t;
        }
        if self.rng.chance(15) {
            let h = *self.rng.pick(&self.hash_list.clone());
            self.setoption("Hash", &h.to_string())?;
        }
        if self.rng.chance(15) {
            self.multipv = *self.rng.pick(&[1, 1, 2, 3, 5]);
            self.setoption("MultiPV", &self.multipv.to_string())?;
        }
        if self.rng.chance(10) {
            self.engine.send("ucinewgame")?;
        }
        if self.rng.chance(5) {
            self.engine.send("setoption name Clear Hash")?;
        }
        if self.rng.chance(10) {
            let v = if self.rng.chance(50) { "true" } else { "false" };
            self.setoption("Ponder", v)?;
        }
        Ok(())
    }

    fn one(&mut self) -> Result<(), String> {
        self.options()?;

        let tp = self.rng.pick(POSITIONS);
        if tp.chess960 != self.chess960 {
            self.chess960 = tp.chess960;
            self.setoption("UCI_Chess960", if tp.chess960 { "true" } else { "false" })?;
        }
        let (fen, moves) = match tp.fen.split_once(" moves ") {
            Some((f, m)) => (f, m.split(' ').collect::<Vec<_>>()),
            None => (tp.fen, vec![]),
        };
        let mut pos = Position::new();
        pos.set(fen.as_bytes(), tp.chess960).map_err(|e| format!("{fen}: {}", String::from_utf8_lossy(e.what())))?;
        let mut all_moves: Vec<String> = moves.iter().map(|s| s.to_string()).collect();
        for m in &moves {
            pos.do_move(uci::to_move(&pos, m.as_bytes()));
        }
        // a few random legal moves after the positions without a known
        // result, so that the positions vary
        let mut extra = 0;
        if tp.mate.is_none() && self.rng.chance(50) {
            for _ in 0..self.rng.below(8) {
                let legal = MoveList::legal(&pos);
                if legal.is_empty() {
                    break;
                }
                let m = *self.rng.pick(legal.as_slice());
                all_moves.push(uci::move_to_string(m, tp.chess960));
                pos.do_move(m);
                extra += 1;
            }
        }
        let mut cmd = format!("position fen {fen}");
        if !all_moves.is_empty() {
            cmd += " moves ";
            cmd += &all_moves.join(" ");
        }
        self.engine.send(&cmd)?;

        // the go command
        let mut searchmoves = Vec::new();
        let legal: Vec<String> =
            MoveList::legal(&pos).as_slice().iter().map(|&m| uci::move_to_string(m, tp.chess960)).collect();
        let k = self.rng.below(100);
        let (kind, mut go): (&'static str, String) = if tp.mate.is_some() && k < 40 {
            let (m, _) = tp.mate.unwrap();
            if m > 0 && self.rng.chance(50) {
                ("mate", format!("go mate {m}"))
            } else {
                ("depth", format!("go depth {}", 6 + self.rng.below(if self.light { 2 } else { 8 })))
            }
        } else if k < 25 {
            let max = if self.rng.chance(30) { 1000 } else if self.light { 20_000 } else { 300_000 };
            ("nodes", format!("go nodes {}", 1 + self.rng.below(max)))
        } else if k < 45 {
            ("depth", format!("go depth {}", 1 + self.rng.below(if self.light { 7 } else { 13 })))
        } else if k < 57 {
            ("movetime", format!("go movetime {}", 1 + self.rng.below(if self.light { 200 } else { 300 })))
        } else if k < 72 {
            let t = 50 + self.rng.below(if self.light { 2000 } else { 5000 });
            let inc = if self.rng.chance(50) { self.rng.below(100) } else { 0 };
            let mut g = format!("go wtime {t} btime {t} winc {inc} binc {inc}");
            if self.rng.chance(30) {
                g += &format!(" movestogo {}", 1 + self.rng.below(40));
            }
            ("clock", g)
        } else if k < 84 {
            ("infinite", "go infinite".to_string())
        } else {
            let t = 100 + self.rng.below(if self.light { 1000 } else { 3000 });
            ("ponder", format!("go ponder wtime {t} btime {t} winc 20 binc 20"))
        };
        if let Some(sm) = tp.searchmoves {
            if self.rng.chance(60) {
                searchmoves = vec![sm.to_string()];
            }
        } else if tp.mate.is_none() && !legal.is_empty() && self.rng.chance(12) {
            for _ in 0..1 + self.rng.below(3) {
                searchmoves.push(self.rng.pick(&legal).clone());
            }
        }
        if !searchmoves.is_empty() {
            go += " searchmoves ";
            go += &searchmoves.join(" ");
        }

        // what follows it
        let then = self.rng.below(100);
        let must_stop = kind == "infinite" || kind == "ponder";
        self.engine.send(&go)?;
        let mut stopped = false;
        if then < 8 && !must_stop {
            // quit in the search
            std::thread::sleep(Duration::from_millis(self.rng.below(50)));
            self.engine.send("quit")?;
            self.stats.quits += 1;
            self.engine.wait_exit(Duration::from_secs_f64(30.0 * self.slow))?;
            return self.restart();
        } else if then < 25 || must_stop {
            let delay = if self.rng.chance(30) { 0 } else { self.rng.below(400) };
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            let c = if kind == "ponder" && self.rng.chance(60) { "ponderhit" } else { "stop" };
            self.engine.send(c)?;
            stopped = c == "stop";
            self.stats.stops += 1;
        }

        let deadline = self.deadline(if kind == "depth" || kind == "mate" { 240.0 } else { 60.0 });
        let lines = self.engine.until("bestmove", deadline)?;
        self.engine.ready(self.deadline(30.0))?;

        let search = Search { pos, tp, extra_moves: extra, go, kind, searchmoves, stopped };
        self.judge(&search, &lines).map_err(|e| format!("{e}\n  after: {}\n  position: {}", search.go, cmd))?;

        self.stats.searches += 1;
        *self.stats.by_kind.entry(kind).or_default() += 1;
        *self.stats.by_threads.entry(self.threads).or_default() += 1;
        Ok(())
    }

    fn judge(&mut self, s: &Search, lines: &[String]) -> Result<(), String> {
        let chess960 = s.tp.chess960;
        let legal: Vec<Move> = MoveList::legal(&s.pos).as_slice().to_vec();
        let words: Vec<&str> = lines.last().unwrap().split_whitespace().collect();
        let bestmove = words.get(1).copied().unwrap_or("");

        let mut last_nodes = 0u64;
        let mut last_depth = 0;
        let mut last_score: Option<(String, i64)> = None;
        let mut last_pv: Vec<String> = Vec::new();
        for l in &lines[..lines.len() - 1] {
            if l.starts_with("bestmove") {
                return Err(format!("two bestmove lines: {l}"));
            }
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.first() != Some(&"info") || w.get(1) == Some(&"string") {
                continue;
            }
            let field = |name: &str| w.iter().position(|&x| x == name).map(|i| w[i + 1]);
            if let Some(n) = field("nodes") {
                let n: u64 = n.parse().map_err(|_| format!("bad nodes: {l}"))?;
                if n < last_nodes {
                    return Err(format!("nodes decrease from {last_nodes}: {l}"));
                }
                last_nodes = n;
            }
            if let Some(mpv) = field("multipv") {
                let mpv: usize = mpv.parse().map_err(|_| format!("bad multipv: {l}"))?;
                if mpv == 0 || mpv > self.multipv.max(1) {
                    return Err(format!("multipv {mpv} with MultiPV {}: {l}", self.multipv));
                }
            }
            if let Some(i) = w.iter().position(|&x| x == "pv") {
                let mut p = s.pos.clone();
                for m in &w[i + 1..] {
                    let mv = uci::to_move(&p, m.as_bytes());
                    if mv == Move::none() {
                        return Err(format!("illegal move {m} in the PV: {l}"));
                    }
                    p.do_move(mv);
                }
                if w.get(i + 1).is_some() && !s.searchmoves.is_empty() && !s.searchmoves.iter().any(|x| x == w[i + 1]) {
                    return Err(format!("PV outside searchmoves: {l}"));
                }
                if field("multipv") == Some("1") || field("multipv").is_none() {
                    last_depth = field("depth").and_then(|d| d.parse().ok()).unwrap_or(0);
                    if let Some(si) = w.iter().position(|&x| x == "score") {
                        let v: i64 = w[si + 2].parse().map_err(|_| format!("bad score: {l}"))?;
                        last_score = Some((w[si + 1].to_string(), v));
                        if w[si + 1] == "cp" && v.abs() > 32000 || w[si + 1] == "mate" && v.abs() > 128 {
                            return Err(format!("score out of range: {l}"));
                        }
                    }
                    last_pv = w[i + 1..].iter().map(|x| x.to_string()).collect();
                }
            }
        }

        // the best move and the ponder move
        if legal.is_empty() {
            if bestmove != "(none)" {
                return Err(format!("bestmove {bestmove} without a legal move"));
            }
            return Ok(());
        }
        let bm = uci::to_move(&s.pos, bestmove.as_bytes());
        if bm == Move::none() || !legal.contains(&bm) {
            return Err(format!("illegal bestmove: {}", lines.last().unwrap()));
        }
        if !s.searchmoves.is_empty() && !s.searchmoves.iter().any(|x| x == bestmove) {
            return Err(format!("bestmove {bestmove} outside searchmoves {:?}", s.searchmoves));
        }
        if words.get(2) == Some(&"ponder") {
            let mut p = s.pos.clone();
            p.do_move(bm);
            let pm = words.get(3).copied().unwrap_or("");
            if uci::to_move(&p, pm.as_bytes()) == Move::none() {
                return Err(format!("illegal ponder move: {}", lines.last().unwrap()));
            }
        }
        if let Some(first) = last_pv.first() {
            // the last PV printed is the one of the best move, except after
            // a stop or a limit that cut the iteration printed earlier
            let _ = first;
        }
        let _ = uci::move_to_string(bm, chess960);

        // go nodes N: within N plus what the threads search before the
        // main thread sees the limit and they see its stop. The main thread
        // checks every min(512, N / 1024) of its nodes, but its countdown
        // carries over from the previous search (up to 512 nodes), and the
        // helpers search from their start to the stop: measured on the C++
        // engine, up to some 11,000 nodes with 4 threads on 2 cores. The
        // bound is loose; the run prints the largest excess.
        if s.kind == "nodes" && !s.stopped {
            let n: u64 = s.go.split_whitespace().nth(2).unwrap().parse().unwrap();
            let allowance = 1024 + 8192 * self.threads as u64;
            if last_nodes > n + allowance {
                return Err(format!("go nodes {n} searched {last_nodes} with {} threads", self.threads));
            }
            self.stats.nodes_over.push((self.threads, n, last_nodes));
        }

        // go depth D, not stopped: the last iteration printed is D (unless
        // the depth limit is beyond what the position allows)
        if s.kind == "depth" && !s.stopped && s.searchmoves.is_empty() {
            let d: i32 = s.go.split_whitespace().nth(2).unwrap().parse().unwrap();
            if last_depth != d {
                return Err(format!("go depth {d}: last depth printed {last_depth}"));
            }
        }

        // known mates, when the search was not cut
        if let Some((m, mv)) = s.tp.mate {
            let complete = !s.stopped && (s.kind == "mate" || (s.kind == "depth" && last_depth >= 6));
            if complete && s.extra_moves == 0 && (s.tp.searchmoves.is_none() || !s.searchmoves.is_empty()) {
                if last_score != Some(("mate".to_string(), m as i64)) {
                    return Err(format!("expected mate {m}, got {last_score:?}"));
                }
                if let Some(mv) = mv {
                    if bestmove != mv {
                        return Err(format!("expected bestmove {mv} for the mate, got {bestmove}"));
                    }
                }
                self.stats.mates_checked += 1;
            }
        }
        Ok(())
    }
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut seconds = 60.0;
    let mut searches = u64::MAX;
    let mut seed = 1u64;
    let mut slow = 1.0;
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let mut threads_list = vec![1, 2, 3, 4, 8, cores + 1];
    let mut hash_list = vec![1, 2, 16];
    let mut light = false;
    let list = |s: String| s.split(',').map(|x| x.parse().expect("a number")).collect::<Vec<usize>>();
    while let Some(a) = args.peek() {
        if !a.starts_with("--") {
            break;
        }
        let a = args.next().unwrap();
        if a == "--light" {
            light = true;
            continue;
        }
        let v = args.next().expect("a value");
        match a.as_str() {
            "--seconds" => seconds = v.parse().expect("seconds"),
            "--searches" => searches = v.parse().expect("a count"),
            "--seed" => seed = v.parse().expect("a seed"),
            "--slow" => slow = v.parse().expect("a factor"),
            "--threads" => threads_list = list(v),
            "--hash" => hash_list = list(v),
            _ => panic!("unknown option {a}"),
        }
    }
    let prefix = std::env::var("RUN_PREFIX").unwrap_or_default();
    let mut cmd: Vec<String> = prefix.split_whitespace().map(String::from).collect();
    cmd.extend(args);
    assert!(!cmd.is_empty(), "usage: uci_stress [options] ENGINE...");

    stockfish::attacks::init();
    let mut s = Session {
        engine: Engine::start(&cmd),
        cmd,
        rng: Rng(seed),
        slow,
        threads: 1,
        multipv: 1,
        chess960: false,
        threads_list,
        hash_list,
        light,
        stats: Stats::default(),
    };
    let start = Instant::now();
    let mut result = s.restart();
    while result.is_ok() && s.stats.searches < searches && start.elapsed().as_secs_f64() < seconds {
        result = s.one();
    }
    if result.is_ok() {
        result = s.engine.send("quit").and_then(|_| s.engine.wait_exit(Duration::from_secs_f64(30.0 * slow)));
    }

    let st = &s.stats;
    println!(
        "{} searches in {:.0} s (seed {seed}): {:?}; by thread count {:?}; {} stopped or ponderhit, \
         {} quit in a search, {} mates checked",
        st.searches,
        start.elapsed().as_secs_f64(),
        st.by_kind,
        st.by_threads,
        st.stops,
        st.quits,
        st.mates_checked
    );
    let mut over: std::collections::BTreeMap<usize, (u64, i64, f64)> = Default::default();
    for &(t, n, got) in &st.nodes_over {
        let e = over.entry(t).or_insert((0, i64::MIN, 0.0));
        e.0 += 1;
        e.1 = e.1.max(got as i64 - n as i64);
        e.2 = e.2.max(got as f64 / n as f64);
    }
    for (t, (count, diff, ratio)) in over {
        println!("go nodes, {t} threads: {count} searches, largest excess {diff} nodes, largest ratio {ratio:.3}");
    }
    if let Err(e) = result {
        println!("FAILED: {e}\nlast lines:");
        for l in s.engine.log.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
            println!("  {l}");
        }
        std::process::exit(1);
    }
    println!("OK");
}

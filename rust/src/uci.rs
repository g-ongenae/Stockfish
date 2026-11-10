//! The part of `src/uci.cpp` this port needs: the command loop with
//! `setoption`, `position`, `go perft`, `dump` and `quit`, and the UCI
//! notation of moves.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

use crate::dump::PositionDumper;
use crate::movegen::MoveList;
use crate::perft::perft_fen;
use crate::position::{square_to_string, Position};
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
pub fn to_move(pos: &Position, s: &str) -> Move {
    let s = s.to_ascii_lowercase();
    for &m in &MoveList::legal(pos) {
        if s == move_to_string(m, pos.is_chess960()) {
            return m;
        }
    }
    Move::none()
}

fn is_c_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r')
}

pub struct UciEngine {
    /// The position as set by `position`, and the moves leading to it.
    pos: Position,
    chess960: bool,
    current_cmd: String,
}

impl Default for UciEngine {
    fn default() -> Self {
        UciEngine::new()
    }
}

impl UciEngine {
    pub fn new() -> UciEngine {
        let mut pos = Position::new();
        pos.set(START_FEN.as_bytes(), false).expect("start position");
        UciEngine { pos, chess960: false, current_cmd: String::new() }
    }

    fn terminate_on_critical_error(&self, message: &[u8]) -> ! {
        let mut out = io::stdout().lock();
        let _ = write!(out, "info string CRITICAL ERROR: Command `{}` failed. Reason: ", self.current_cmd);
        let _ = out.write_all(message);
        let _ = out.write_all(b"\n\n");
        let _ = out.flush();
        std::process::exit(1);
    }

    /// Reads commands from the command line (one shot) or from stdin.
    pub fn run(&mut self, args: &[String]) {
        if !args.is_empty() {
            let cmd = args.iter().map(|a| format!("{a} ")).collect::<String>();
            self.execute(&cmd);
            return;
        }

        let stdin = io::stdin();
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = stdin.lock().read_until(b'\n', &mut line).unwrap_or(0);
            let cmd = if n == 0 {
                "quit".to_string()
            } else {
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                String::from_utf8_lossy(&line).into_owned()
            };
            if !self.execute(&cmd) {
                break;
            }
        }
    }

    /// Executes one command, returns false on `quit`.
    pub fn execute(&mut self, cmd: &str) -> bool {
        self.current_cmd = cmd.to_string();
        let mut tokens = cmd.split(is_c_space).filter(|t| !t.is_empty());
        let token = tokens.next().unwrap_or("");
        let mut out = io::stdout().lock();

        match token {
            "quit" | "stop" => return token != "quit",
            "uci" => {
                let _ = writeln!(out, "id name Stockfish Rust port\nid author the Stockfish developers (see AUTHORS file)\n");
                let _ = writeln!(out, "option name UCI_Chess960 type check default false");
                let _ = writeln!(out, "uciok");
            }
            "isready" => {
                let _ = writeln!(out, "readyok");
            }
            "ucinewgame" => {}
            "setoption" => {
                drop(out);
                self.setoption(tokens);
            }
            "position" => {
                drop(out);
                self.position(tokens);
            }
            "go" => {
                drop(out);
                self.go(tokens);
            }
            "dump" => {
                drop(out);
                self.dump(tokens.next());
            }
            "d" => {
                let _ = writeln!(out, "\nFen: {}\nKey: {:016X}", self.pos.fen(), self.pos.key());
            }
            "" => {}
            t if t.starts_with('#') => {}
            _ => {
                let _ = writeln!(out, "Unknown command: '{cmd}'. Type help for more information.");
            }
        }
        true
    }

    fn setoption<'a>(&mut self, mut tokens: impl Iterator<Item = &'a str>) {
        tokens.next(); // Consume the "name" token
        let mut name = String::new();
        let mut value = String::new();
        for t in tokens.by_ref() {
            if t == "value" {
                break;
            }
            if !name.is_empty() {
                name.push(' ');
            }
            name += t;
        }
        for t in tokens {
            if !value.is_empty() {
                value.push(' ');
            }
            value += t;
        }

        if name.eq_ignore_ascii_case("UCI_Chess960") {
            // A check option only takes "true" or "false"
            if value == "true" || value == "false" {
                self.chess960 = value == "true";
            }
        } else {
            println!("No such option: {name}");
        }
    }

    fn position<'a>(&mut self, mut tokens: impl Iterator<Item = &'a str>) {
        let mut fen = String::new();
        match tokens.next() {
            Some("startpos") => {
                fen = START_FEN.to_string();
                tokens.next(); // Consume the "moves" token, if any
            }
            Some("fen") => {
                for t in tokens.by_ref() {
                    if t == "moves" {
                        break;
                    }
                    fen += t;
                    fen.push(' ');
                }
            }
            _ => return,
        }

        let mut pos = Position::new();
        if let Err(e) = pos.set(fen.as_bytes(), self.chess960) {
            self.terminate_on_critical_error(e.what());
        }
        for t in tokens {
            let m = to_move(&pos, t);
            if m == Move::none() {
                self.terminate_on_critical_error(format!("Illegal move: {t}").as_bytes());
            }
            pos.do_move(m);
        }
        self.pos = pos;
    }

    fn go<'a>(&mut self, mut tokens: impl Iterator<Item = &'a str>) {
        let mut depth: Depth = 0;
        while let Some(t) = tokens.next() {
            if t == "perft" {
                match tokens.next().map(|v| v.parse::<Depth>()) {
                    Some(Ok(d)) => depth = d,
                    _ => self.terminate_on_critical_error(b"Invalid argument for 'perft'"),
                }
            }
        }
        if depth == 0 {
            println!("info string only 'go perft <depth>' is supported by this port");
            return;
        }

        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        let fen = self.pos.fen();
        match perft_fen(fen.as_bytes(), depth, self.chess960, &mut out) {
            Ok(nodes) => {
                let _ = writeln!(out, "\nNodes searched: {nodes}\n");
            }
            Err(e) => {
                drop(out);
                self.terminate_on_critical_error(e.what());
            }
        }
    }

    /// `dump`: the current position. `dump <file>`: each position of a file
    /// of FENs, one per line, blank lines skipped, streamed.
    fn dump(&mut self, filename: Option<&str>) {
        let mut dumper = PositionDumper::new();
        let mut buf = Vec::with_capacity(8192);

        let Some(filename) = filename else {
            dumper.dump(self.pos.fen().as_bytes(), self.chess960, &mut buf);
            buf.push(b'\n');
            let _ = io::stdout().lock().write_all(&buf);
            return;
        };

        let Ok(file) = File::open(filename) else {
            self.terminate_on_critical_error(format!("Unable to open file {filename}").as_bytes());
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
            dumper.dump(&line, self.chess960, &mut buf);
            buf.push(b'\n');
            if out.write_all(&buf).is_err() {
                break;
            }
        }
        let _ = out.flush();
    }
}

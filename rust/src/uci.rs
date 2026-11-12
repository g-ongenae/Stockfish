//! The part of `src/uci.cpp` this port needs: the command loop with
//! `setoption`, `position`, `go perft`, `dump`, `dumpline` and `quit`, and the
//! UCI notation of moves.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

use crate::dump::{LineDumper, PositionDumper};
use crate::nnue::network::EVAL_FILE_DEFAULT_NAME;
use crate::nnue::Network;
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
pub fn to_move(pos: &Position, s: &[u8]) -> Move {
    let s = s.to_ascii_lowercase();
    for &m in &MoveList::legal(pos) {
        if s == move_to_string(m, pos.is_chess960()).as_bytes() {
            return m;
        }
    }
    Move::none()
}

fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// `is >> n` for an int, applied to the start of a token: an optional sign
/// and the longest run of digits. Returns the value and the rest of the token,
/// which the stream would read as the next token, or None if C++ would fail
/// (no digits, or out of the range of int).
fn parse_int_prefix(t: &[u8]) -> Option<(i32, &[u8])> {
    let sign_len = (t.first() == Some(&b'+') || t.first() == Some(&b'-')) as usize;
    let digits = t[sign_len..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let end = sign_len + digits;
    let v: i32 = std::str::from_utf8(&t[..end]).ok()?.parse().ok()?;
    Some((v, &t[end..]))
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
    /// The position as set by `position`, and the moves leading to it.
    pos: Position,
    chess960: bool,
    current_cmd: Vec<u8>,
    /// The `EvalFile` option, and the network read from it, loaded on first use.
    eval_file: Vec<u8>,
    network: Option<(Vec<u8>, Network)>,
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
        UciEngine {
            pos,
            chess960: false,
            current_cmd: Vec::new(),
            eval_file: EVAL_FILE_DEFAULT_NAME.as_bytes().to_vec(),
            network: None,
        }
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
    }

    /// Executes one command, returns false on `quit`.
    pub fn execute(&mut self, cmd: &[u8]) -> bool {
        self.current_cmd = cmd.to_vec();
        let mut tokens = Tokens { rest: cmd };
        let token = tokens.next().unwrap_or(b"");
        let mut out = io::stdout().lock();

        match token {
            b"quit" | b"stop" => return token != b"quit",
            b"uci" => {
                let _ = writeln!(out, "id name Stockfish Rust port\nid author the Stockfish developers (see AUTHORS file)\n");
                let _ = writeln!(out, "option name EvalFile type string default {EVAL_FILE_DEFAULT_NAME}");
                let _ = writeln!(out, "option name UCI_Chess960 type check default false");
                let _ = writeln!(out, "uciok");
            }
            b"isready" => {
                let _ = writeln!(out, "readyok");
            }
            b"ucinewgame" => {}
            b"setoption" => {
                drop(out);
                self.setoption(tokens);
            }
            b"position" => {
                drop(out);
                self.position(tokens);
            }
            b"go" => {
                drop(out);
                self.go(tokens);
            }
            b"dump" => {
                drop(out);
                self.dump(tokens.next());
            }
            b"dumpline" => {
                drop(out);
                // std::getline(args >> std::ws, rest)
                let start = tokens.rest.iter().position(|&c| !is_c_space(c)).unwrap_or(tokens.rest.len());
                self.dump_line(&tokens.rest[start..]);
            }
            b"d" => {
                let _ = writeln!(out, "\nFen: {}\nKey: {:016X}", self.pos.fen(), self.pos.key());
            }
            b"" => {}
            t if t[0] == b'#' => {}
            _ => {
                let _ = out.write_all(b"Unknown command: '");
                let _ = out.write_all(cmd);
                let _ = out.write_all(b"'. Type help for more information.\n");
            }
        }
        true
    }

    fn setoption<'a>(&mut self, mut tokens: impl Iterator<Item = &'a [u8]>) {
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

        if name.eq_ignore_ascii_case(b"EvalFile") {
            self.eval_file = value;
        } else if name.eq_ignore_ascii_case(b"UCI_Chess960") {
            // A check option only takes "true" or "false"
            if value == b"true" || value == b"false" {
                self.chess960 = value == b"true";
            }
        } else {
            let mut out = io::stdout().lock();
            let _ = out.write_all(b"No such option: ");
            let _ = out.write_all(&name);
            let _ = out.write_all(b"\n");
        }
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

        let mut pos = Position::new();
        if let Err(e) = pos.set(&fen, self.chess960) {
            self.terminate_on_critical_error(e.what());
        }
        for t in tokens {
            let m = to_move(&pos, t);
            if m == Move::none() {
                let mut msg = b"Illegal move: ".to_vec();
                msg.extend_from_slice(t);
                self.terminate_on_critical_error(&msg);
            }
            pos.do_move(m);
        }
        self.pos = pos;
    }

    fn go<'a>(&mut self, mut tokens: impl Iterator<Item = &'a [u8]>) {
        let mut depth: Depth = 0;
        let mut pending: Option<&[u8]> = None;
        while let Some(t) = pending.take().or_else(|| tokens.next()) {
            if t == b"perft" {
                // `is >> limits.perft`: reads a numeric prefix, and the rest
                // of the token is read as the next token.
                match tokens.next().and_then(parse_int_prefix) {
                    Some((d, rest)) => {
                        depth = d;
                        if !rest.is_empty() {
                            pending = Some(rest);
                        }
                    }
                    None => self.terminate_on_critical_error(b"Invalid argument for 'perft'"),
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

    /// Loads the network of the `EvalFile` option if it is not loaded yet, or
    /// terminates as C++ does when it cannot (`Engine::verify_network`). The
    /// file is looked for as given (relative to the working directory), then
    /// in the directory of the binary. It is read, not embedded in the binary.
    fn verify_network(&mut self) -> &Network {
        let mut path = self.eval_file.clone();
        if path.is_empty() {
            path = EVAL_FILE_DEFAULT_NAME.as_bytes().to_vec();
        }

        if self.network.as_ref().map(|(p, _)| p) != Some(&path) {
            self.network = None;
            let mut dirs = vec![std::path::PathBuf::new()];
            if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
                dirs.push(dir);
            }
            for dir in dirs {
                let Ok(file) = File::open(dir.join(path_from_bytes(&path))) else { continue };
                if let Some(net) = Network::load(BufReader::with_capacity(1 << 16, file)) {
                    self.network = Some((path.clone(), net));
                    break;
                }
            }

            let name = String::from_utf8_lossy(&path).into_owned();
            let mut out = io::stdout().lock();
            match &self.network {
                Some((_, net)) => {
                    let _ = writeln!(
                        out,
                        "info string NNUE evaluation using {name} ({}MiB, ({}, {}, {}, {}, 1))",
                        net.reported_size_mib(),
                        crate::nnue::feature_transformer::INPUT_DIMENSIONS,
                        crate::nnue::L1,
                        crate::nnue::L2,
                        crate::nnue::L3
                    );
                }
                None => {
                    let msg = [
                        "Network evaluation parameters compatible with the engine must be available.".to_string(),
                        format!("The network file {name} was not loaded successfully."),
                        "The UCI option EvalFile might need to specify the full path, including the directory name, to the network file.".to_string(),
                        format!("The default net can be downloaded from: https://tests.stockfishchess.org/api/nn/{EVAL_FILE_DEFAULT_NAME}"),
                        "The engine will be terminated now.".to_string(),
                    ];
                    for line in msg {
                        let _ = writeln!(out, "info string ERROR: {line}");
                    }
                    let _ = out.flush();
                    std::process::exit(1);
                }
            }
        }
        &self.network.as_ref().unwrap().1
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

        self.verify_network();
        let network = &self.network.as_ref().unwrap().1;
        let mut dumper = PositionDumper::new(network);
        let mut buf = Vec::with_capacity(8192);

        let Some((_, Ok(file))) = file else {
            dumper.dump(self.pos.fen().as_bytes(), self.chess960, &mut buf);
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
            dumper.dump(&line, self.chess960, &mut buf);
            buf.push(b'\n');
            if out.write_all(&buf).is_err() {
                break;
            }
        }
        let _ = out.flush();
    }
}

impl UciEngine {
    /// `dumpline startpos|fen ... moves ...`: the given line of play.
    /// `dumpline <file>`: each line of a file, one per line, blank lines
    /// skipped, streamed.
    fn dump_line(&mut self, rest: &[u8]) {
        let first = Tokens { rest }.next().unwrap_or(b"");
        let mut buf = Vec::with_capacity(1 << 16);

        if first.is_empty() || first == b"startpos" || first == b"fen" {
            self.verify_network();
            let network = &self.network.as_ref().unwrap().1;
            let mut dumper = LineDumper::new(network);
            dumper.dump(rest, self.chess960, &mut buf);
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

        self.verify_network();
        let network = &self.network.as_ref().unwrap().1;
        let mut dumper = LineDumper::new(network);
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
            dumper.dump(&line, self.chess960, &mut buf);
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
fn path_from_bytes(b: &[u8]) -> std::path::PathBuf {
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

    #[test]
    fn tokens_keep_bytes() {
        let t: Vec<&[u8]> = Tokens { rest: b"  position fen 4k3/8/8/8/8/8/8/4K2\xFF\tw\x0b- " }.collect();
        assert_eq!(t.len(), 5);
        let raw: &[u8] = b"a \xff\xfe b";
        let t: Vec<&[u8]> = Tokens { rest: raw }.collect();
        assert_eq!(t, vec![&b"a"[..], &b"\xff\xfe"[..], &b"b"[..]]);
    }
}

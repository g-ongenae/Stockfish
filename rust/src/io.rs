//! Standard output, and the logger of `misc.cpp` (`Debug Log File`).
//!
//! C++ replaces the stream buffers of `std::cin` and `std::cout` with `Tie`
//! objects that copy every character read or written to the log file, each
//! line prefixed with `>> ` (read) or `<< ` (written); one `last` character,
//! shared by both directions, decides when a prefix is due. Everything the
//! engine prints goes through `cout` there, so here everything it prints goes
//! through `write_out()` (the `sync_println!` macro, the `Out` writer), and
//! every line the UCI loop reads through `log_input()`. Standard error is not
//! logged, nor the start-up line (printed before any option can be set), nor
//! the arguments of the command line (not read from `cin`).

use std::fs::File;
use std::io::{self, Write};
use std::sync::Mutex;

struct Logger {
    file: Option<File>,
    /// The last character logged (C++'s `static int last`), initially a
    /// new line.
    last: u8,
}

static LOGGER: Mutex<Logger> = Mutex::new(Logger { file: None, last: b'\n' });

impl Logger {
    fn log(&mut self, bytes: &[u8], prefix: &[u8]) {
        let Some(file) = self.file.as_mut() else { return };
        // One write per call, the prefixes inserted where a line starts
        let mut buf = Vec::with_capacity(bytes.len() + 8);
        for &c in bytes {
            if self.last == b'\n' {
                buf.extend_from_slice(prefix);
            }
            buf.push(c);
            self.last = c;
        }
        let _ = file.write_all(&buf);
    }
}

fn logger() -> std::sync::MutexGuard<'static, Logger> {
    LOGGER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Writes to standard output, and to the log if one is open. The lock is
/// held across both, so that lines of several threads do not mix (C++'s
/// `sync_cout`).
pub fn write_out(bytes: &[u8]) {
    let mut log = logger();
    let mut out = io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
    log.log(bytes, b"<< ");
}

/// A line read by the UCI loop, its new line included if it had one.
pub fn log_input(bytes: &[u8]) {
    logger().log(bytes, b">> ");
}

/// `Logger::start()`: closes the log if one is open, then opens the file
/// (truncated) unless the name is empty. A file that cannot be opened ends
/// the program, as in C++, which prints the path as `std::filesystem::path`
/// does (in double quotes, `"` and `\` escaped with `\`).
pub fn start_logger(path: &[u8]) {
    let mut log = logger();
    log.file = None;
    if path.is_empty() {
        return;
    }
    match File::create(crate::uci::path_from_bytes(path)) {
        Ok(f) => log.file = Some(f),
        Err(_) => {
            let mut quoted = vec![b'"'];
            for &c in path {
                if c == b'"' || c == b'\\' {
                    quoted.push(b'\\');
                }
                quoted.push(c);
            }
            quoted.push(b'"');
            let mut err = io::stderr().lock();
            let _ = err.write_all(b"Unable to open debug log file ");
            let _ = err.write_all(&quoted);
            let _ = err.write_all(b"\n");
            drop(log);
            std::process::exit(1);
        }
    }
}

/// A writer to `write_out()`, for output written in pieces (perft, `dump`
/// through a `BufWriter`).
pub struct Out;

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write_out(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `sync_cout << ... << sync_endl`: one line to standard output (and the
/// log).
#[macro_export]
macro_rules! sync_println {
    ($($arg:tt)*) => {{
        let mut s = format!($($arg)*);
        s.push('\n');
        $crate::io::write_out(s.as_bytes());
    }};
}

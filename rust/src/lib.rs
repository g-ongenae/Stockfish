//! Rust port of Stockfish, reproducing the C++ engine bit for bit at the
//! pinned commit `43e01e07` (bench 3464885). See README.md.
//!
//! Module names follow the C++ files they port.

pub mod attacks;
pub mod bitboard;
pub mod dump;
pub mod evaluate;
pub mod history;
pub mod misc;
pub mod movegen;
pub mod movepick;
pub mod nnue;
pub mod perft;
pub mod position;
pub mod types;
pub mod uci;

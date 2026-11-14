//! Rust port of Stockfish, reproducing the C++ engine bit for bit at the
//! pinned commit `43e01e07` (bench 3464885). See README.md.
//!
//! Module names follow the C++ files they port.

// The trace! macro, used by search.rs
#[macro_use]
pub mod searchtrace;

pub mod attacks;
pub mod benchmark;
pub mod bitboard;
pub mod dump;
pub mod engine;
pub mod evaluate;
pub mod history;
pub mod misc;
pub mod movegen;
pub mod movepick;
pub mod nnue;
pub mod perft;
pub mod position;
pub mod score;
pub mod search;
pub mod thread;
pub mod timeman;
pub mod tt;
pub mod types;
pub mod uci;
pub mod ucioption;

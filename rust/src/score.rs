//! Port of `src/score.{h,cpp}`: a score as the UCI output shows it.

use crate::position::Position;
use crate::types::*;
use crate::uci;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Score {
    /// Mate in this many plies (negative: mated)
    Mate { plies: i32 },
    /// A tablebase win or loss, this many plies from a conversion
    Tablebase { plies: i32, win: bool },
    /// Centipawns
    InternalUnits { value: i32 },
}

impl Score {
    pub fn new(v: Value, pos: &Position) -> Score {
        debug_assert!(-VALUE_INFINITE < v && v < VALUE_INFINITE);

        if !is_decisive(v) {
            Score::InternalUnits { value: uci::to_cp(v, pos) }
        } else if v.abs() <= VALUE_TB {
            let distance = VALUE_TB - v.abs();
            if v > 0 {
                Score::Tablebase { plies: distance, win: true }
            } else {
                Score::Tablebase { plies: -distance, win: false }
            }
        } else {
            let distance = VALUE_MATE - v.abs();
            Score::Mate { plies: if v > 0 { distance } else { -distance } }
        }
    }
}

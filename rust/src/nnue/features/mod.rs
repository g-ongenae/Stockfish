//! Port of `src/nnue/features/`: the three input feature sets of the network.

pub mod full_threats;
pub mod k32h3;
pub mod pp_3wide;

/// Lists of threat and pawn pair feature indices share one type, as in C++
/// (`ValueList<u16, 256>`): the two sets are concatenated in one weight array.
pub type ThreatIndexList = crate::misc::ValueList<u16, 256>;

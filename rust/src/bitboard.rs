//! Port of `src/bitboard.h`.
//!
//! Bit counting and scanning use the Rust integer intrinsics (`count_ones`,
//! `trailing_zeros`, `leading_zeros`). They compile to the hardware
//! instructions only when the target enables them (`-C target-feature=+popcnt`
//! or `-C target-cpu=native`), and otherwise to the same kind of portable
//! fallback the C++ build uses without `USE_POPCNT`: no build of this crate
//! requires popcnt, BMI or SIMD, and on a 32-bit target the 64-bit operations
//! are split in two halves by the compiler, as the MSVC 32-bit path of the C++
//! code does by hand.

use crate::types::*;

pub const FILE_A_BB: Bitboard = 0x0101010101010101;
pub const FILE_B_BB: Bitboard = FILE_A_BB << 1;
pub const FILE_H_BB: Bitboard = FILE_A_BB << 7;

pub const RANK_1_BB: Bitboard = 0xFF;
pub const RANK_2_BB: Bitboard = RANK_1_BB << 8;
pub const RANK_3_BB: Bitboard = RANK_1_BB << (8 * 2);
pub const RANK_6_BB: Bitboard = RANK_1_BB << (8 * 5);
pub const RANK_7_BB: Bitboard = RANK_1_BB << (8 * 6);
pub const RANK_8_BB: Bitboard = RANK_1_BB << (8 * 7);

#[inline(always)]
pub const fn square_bb(s: Square) -> Bitboard {
    debug_assert!(is_ok_square(s));
    1u64 << (s & 63)
}

#[inline(always)]
pub const fn more_than_one(b: Bitboard) -> bool {
    b & b.wrapping_sub(1) != 0
}

#[inline(always)]
pub const fn rank_bb(r: Rank) -> Bitboard {
    RANK_1_BB << (8 * r)
}

#[inline(always)]
pub const fn rank_bb_of(s: Square) -> Bitboard {
    rank_bb(rank_of(s))
}

#[inline(always)]
pub const fn file_bb(f: File) -> Bitboard {
    FILE_A_BB << f
}

#[inline(always)]
pub const fn file_bb_of(s: Square) -> Bitboard {
    file_bb(file_of(s))
}

/// Moves a bitboard one or two steps as specified by the direction.
#[inline(always)]
pub const fn shift(b: Bitboard, d: Direction) -> Bitboard {
    match d {
        NORTH => b << 8,
        SOUTH => b >> 8,
        16 => b << 16,
        -16 => b >> 16,
        EAST => (b & !FILE_H_BB) << 1,
        WEST => (b & !FILE_A_BB) >> 1,
        NORTH_EAST => (b & !FILE_H_BB) << 9,
        NORTH_WEST => (b & !FILE_A_BB) << 7,
        SOUTH_EAST => (b & !FILE_H_BB) >> 7,
        SOUTH_WEST => (b & !FILE_A_BB) >> 9,
        _ => 0,
    }
}

/// Squares attacked by pawns of the given color from the squares in `b`.
#[inline(always)]
pub const fn pawn_attacks_bb(c: Color, b: Bitboard) -> Bitboard {
    if c == WHITE {
        shift(b, NORTH_WEST) | shift(b, NORTH_EAST)
    } else {
        shift(b, SOUTH_WEST) | shift(b, SOUTH_EAST)
    }
}

/// Number of bits set. `constexpr_popcount` in C++ is the same SWAR algorithm
/// the compiler emits for this when popcnt is not available.
#[inline(always)]
pub const fn popcount(b: Bitboard) -> i32 {
    b.count_ones() as i32
}

/// Least significant square of a non-zero bitboard.
#[inline(always)]
pub const fn lsb(b: Bitboard) -> Square {
    debug_assert!(b != 0);
    b.trailing_zeros() as Square
}

/// Most significant square of a non-zero bitboard.
#[inline(always)]
pub const fn msb(b: Bitboard) -> Square {
    debug_assert!(b != 0);
    (63 ^ b.leading_zeros()) as Square
}

#[inline(always)]
pub const fn least_significant_square_bb(b: Bitboard) -> Bitboard {
    debug_assert!(b != 0);
    b & b.wrapping_neg()
}

/// Finds and clears the least significant bit of a non-zero bitboard.
#[inline(always)]
pub fn pop_lsb(b: &mut Bitboard) -> Square {
    debug_assert!(*b != 0);
    let s = lsb(*b);
    *b &= b.wrapping_sub(1);
    s
}

/// Iterates over the squares of a bitboard, from a1 to h8, as the C++
/// `while (b) { Square s = pop_lsb(b); ... }` loops do.
#[derive(Clone, Copy)]
pub struct Squares(pub Bitboard);

impl Iterator for Squares {
    type Item = Square;

    #[inline(always)]
    fn next(&mut self) -> Option<Square> {
        if self.0 == 0 {
            None
        } else {
            Some(pop_lsb(&mut self.0))
        }
    }
}

/// Text drawing of a bitboard, for debugging (`Bitboards::pretty`).
pub fn pretty(b: Bitboard) -> String {
    let mut s = String::from("+---+---+---+---+---+---+---+---+\n");
    for r in (0..8u8).rev() {
        for f in 0..8u8 {
            s += if b & square_bb(make_square(f, r)) != 0 { "| X " } else { "|   " };
        }
        s += &format!("| {}\n+---+---+---+---+---+---+---+---+\n", 1 + r);
    }
    s += "  a   b   c   d   e   f   g   h\n";
    s
}

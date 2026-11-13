//! Port of `src/attacks.{h,cpp}`: attack tables and sliding attacks.
//!
//! Sliding attacks use the "fancy magic bitboards" of the C++ fallback path
//! (no `USE_HYPERBOLA_QUINT`, no `USE_DUAL_HYPERBOLA_QUINT`), which needs no
//! SIMD, no bit reversal and no PEXT. The magics are searched at startup with
//! the same PRNG and seeds as the C++ engine, so the tables have the same size
//! (rook 0x19000 + bishop 0x1480 entries, 841 KiB) and the same layout.
//!
//! On a 32-bit target the index is computed with two 32-bit multiplications,
//! with the 32-bit seeds, as in C++ when `IS_64BIT` is not defined. The
//! `magic32` feature selects that variant on a 64-bit target too, so that it
//! can be checked on a 64-bit machine.

use crate::bitboard::*;
use crate::misc::Prng;
use crate::types::*;
use std::sync::OnceLock;

/// `Is64Bit` of the C++ code, which selects the magic index computation.
pub const IS_64BIT: bool = cfg!(target_pointer_width = "64") && !cfg!(feature = "magic32");

/// Returns the bitboard of the target square for the given step from the given
/// square, or an empty bitboard if the step leaves the board.
pub const fn safe_destination(s: Square, step: i32) -> Bitboard {
    let to = s as i32 + step;
    if to < 0 || to > 63 {
        return 0;
    }
    let df = (s as i32 & 7) - (to & 7);
    if df.abs() <= 2 {
        square_bb(to as Square)
    } else {
        0
    }
}

pub const fn sliding_attack(pt: PieceType, sq: Square, occupied: Bitboard) -> Bitboard {
    const ROOK_DIRECTIONS: [Direction; 4] = [NORTH, SOUTH, EAST, WEST];
    const BISHOP_DIRECTIONS: [Direction; 4] = [NORTH_EAST, SOUTH_EAST, SOUTH_WEST, NORTH_WEST];
    let dirs = if pt == ROOK { ROOK_DIRECTIONS } else { BISHOP_DIRECTIONS };
    let mut attacks = 0;
    let mut i = 0;
    while i < 4 {
        let d = dirs[i];
        let mut s = sq;
        loop {
            let dest = safe_destination(s, d);
            if dest == 0 {
                break;
            }
            attacks |= dest;
            if occupied & dest != 0 {
                break;
            }
            s = sq_add(s, d);
        }
        i += 1;
    }
    attacks
}

const fn steps_attack(sq: Square, steps: [i32; 8]) -> Bitboard {
    let mut b = 0;
    let mut i = 0;
    while i < 8 {
        b |= safe_destination(sq, steps[i]);
        i += 1;
    }
    b
}

pub const fn knight_attack(sq: Square) -> Bitboard {
    steps_attack(sq, [-17, -15, -10, -6, 6, 10, 15, 17])
}

pub const fn king_attack(sq: Square) -> Bitboard {
    steps_attack(sq, [-9, -8, -7, -1, 1, 7, 8, 9])
}

/// Attacks on an empty board, indexed by piece type, and by color for pawns
/// (WHITE == 0 and BLACK == 1 share the slots of NO_PIECE_TYPE and PAWN), as
/// the C++ `PseudoAttacks`. Computed at compile time, as in C++.
pub static PSEUDO_ATTACKS: [[Bitboard; SQUARE_NB]; PIECE_TYPE_NB] = {
    let mut attacks = [[0u64; SQUARE_NB]; PIECE_TYPE_NB];
    let mut s = 0;
    while s < 64 {
        let sq = s as Square;
        attacks[WHITE as usize][s] = pawn_attacks_bb(WHITE, square_bb(sq));
        attacks[BLACK as usize][s] = pawn_attacks_bb(BLACK, square_bb(sq));
        attacks[KING as usize][s] = king_attack(sq);
        attacks[KNIGHT as usize][s] = knight_attack(sq);
        attacks[BISHOP as usize][s] = sliding_attack(BISHOP, sq, 0);
        attacks[ROOK as usize][s] = sliding_attack(ROOK, sq, 0);
        attacks[QUEEN as usize][s] = attacks[BISHOP as usize][s] | attacks[ROOK as usize][s];
        s += 1;
    }
    attacks
};

const fn line_mask(sq: Square, d1: Direction, d2: Direction) -> Bitboard {
    let mut mask = 0;
    let dirs = [d1, d2];
    let mut i = 0;
    while i < 2 {
        let d = dirs[i];
        let mut s = sq;
        loop {
            let dest = safe_destination(s, d);
            if dest == 0 {
                break;
            }
            mask |= dest;
            s = sq_add(s, d);
        }
        i += 1;
    }
    mask
}

static DIAG_BB: [Bitboard; SQUARE_NB] = {
    let mut t = [0; SQUARE_NB];
    let mut s = 0;
    while s < 64 {
        t[s] = line_mask(s as Square, NORTH_EAST, SOUTH_WEST) | square_bb(s as Square);
        s += 1;
    }
    t
};

static ANTI_DIAG_BB: [Bitboard; SQUARE_NB] = {
    let mut t = [0; SQUARE_NB];
    let mut s = 0;
    while s < 64 {
        t[s] = line_mask(s as Square, NORTH_WEST, SOUTH_EAST) | square_bb(s as Square);
        s += 1;
    }
    t
};

/// All the data of the magic bitboards of one square. `offset` locates its
/// attacks in `Tables::slider`, where the C++ `Magic` has a pointer.
#[derive(Clone, Copy, Default)]
pub struct Magic {
    pub mask: Bitboard,
    pub magic: Bitboard,
    pub offset: u32,
    pub shift: u32,
}

impl Magic {
    /// Compute the attack's index using the 'magic bitboards' approach.
    #[inline(always)]
    pub fn index(&self, occupied: Bitboard) -> usize {
        if IS_64BIT {
            ((occupied & self.mask).wrapping_mul(self.magic) >> self.shift) as usize
        } else {
            let lo = (occupied as u32) & (self.mask as u32);
            let hi = ((occupied >> 32) as u32) & ((self.mask >> 32) as u32);
            ((lo.wrapping_mul(self.magic as u32) ^ hi.wrapping_mul((self.magic >> 32) as u32)) >> self.shift) as usize
        }
    }
}

const ROOK_TABLE_SIZE: usize = 0x19000;
const BISHOP_TABLE_SIZE: usize = 0x1480;

/// The tables the C++ code fills in `Attacks::init()`.
pub struct Tables {
    /// Indexed by square, then by `pt - BISHOP`.
    magics: [[Magic; 2]; SQUARE_NB],
    /// Rook attacks then bishop attacks, as the C++ `RookTable` and `BishopTable`.
    slider: Box<[Bitboard]>,
    line_bb: Box<[[Bitboard; SQUARE_NB]; SQUARE_NB]>,
    between_bb: Box<[[Bitboard; SQUARE_NB]; SQUARE_NB]>,
    inverse_ray_pass_bb: Box<[[Bitboard; SQUARE_NB]; SQUARE_NB]>,
}

fn boxed_square_table() -> Box<[[Bitboard; SQUARE_NB]; SQUARE_NB]> {
    vec![[0u64; SQUARE_NB]; SQUARE_NB].into_boxed_slice().try_into().unwrap()
}

/// Port of the C++ `init_magics()`: finds the magics by trial with the same
/// seeds, and fills the attack table of the given piece type.
fn init_magics(pt: PieceType, table: &mut [Bitboard], base: u32, magics: &mut [[Magic; 2]; SQUARE_NB]) {
    #[rustfmt::skip]
    const SEEDS: [[u64; RANK_NB]; 2] = [
        [8977, 44560, 54343, 38998, 5731, 95205, 104912, 17020],
        [728, 10316, 55013, 32803, 12281, 15100, 16645, 255],
    ];

    let mut occupancy = vec![0u64; 4096];
    let mut reference = vec![0u64; 4096];
    let mut epoch = vec![0i32; 4096];
    let mut cnt = 0;
    let mut size = 0usize;
    let mut offset = 0usize; // relative to `table`
    let k = (pt - BISHOP) as usize;

    for s in 0..64u8 {
        let edges = ((RANK_1_BB | RANK_8_BB) & !rank_bb_of(s)) | ((FILE_A_BB | FILE_H_BB) & !file_bb_of(s));
        let mut m = Magic::default();
        let attacks = sliding_attack(pt, s, 0);
        m.mask = attacks & !edges;
        m.shift = (if IS_64BIT { 64 } else { 32 }) - popcount(m.mask) as u32;
        offset += size; // attacks of square s follow those of square s - 1
        m.offset = base + offset as u32;
        size = 0;

        // Carry-Rippler trick to enumerate all subsets of m.mask
        let mut b: Bitboard = 0;
        loop {
            occupancy[size] = b;
            reference[size] = sliding_attack(pt, s, b);
            size += 1;
            b = b.wrapping_sub(m.mask) & m.mask;
            if b == 0 {
                break;
            }
        }

        let mut rng = Prng::new(SEEDS[IS_64BIT as usize][rank_of(s) as usize]);
        let attacks = &mut table[offset..];

        // Find a magic for square 's' picking up an (almost) random number
        // until we find the one that passes the verification test.
        let mut i = 0;
        while i < size {
            m.magic = 0;
            while popcount(m.magic.wrapping_mul(m.mask) >> 56) < 6 {
                m.magic = rng.sparse_rand();
            }

            cnt += 1;
            i = 0;
            while i < size {
                let idx = m.index(occupancy[i]);
                if epoch[idx] < cnt {
                    epoch[idx] = cnt;
                    attacks[idx] = reference[i];
                } else if attacks[idx] != reference[i] {
                    break;
                }
                i += 1;
            }
        }
        magics[s as usize][k] = m;
    }
}

impl Tables {
    fn new() -> Box<Tables> {
        let mut magics = [[Magic::default(); 2]; SQUARE_NB];
        let mut slider = vec![0u64; ROOK_TABLE_SIZE + BISHOP_TABLE_SIZE].into_boxed_slice();

        init_magics(ROOK, &mut slider[..ROOK_TABLE_SIZE], 0, &mut magics);
        init_magics(BISHOP, &mut slider[ROOK_TABLE_SIZE..], ROOK_TABLE_SIZE as u32, &mut magics);

        let mut t = Box::new(Tables {
            magics,
            slider,
            line_bb: boxed_square_table(),
            between_bb: boxed_square_table(),
            inverse_ray_pass_bb: boxed_square_table(),
        });

        for s1 in 0..64u8 {
            for pt in [BISHOP, ROOK] {
                for s2 in 0..64u8 {
                    let (i1, i2) = (s1 as usize, s2 as usize);
                    if PSEUDO_ATTACKS[pt as usize][i1] & square_bb(s2) != 0 {
                        t.line_bb[i1][i2] =
                            (t.attacks(pt, s1, 0) & t.attacks(pt, s2, 0)) | square_bb(s1) | square_bb(s2);
                        t.between_bb[i1][i2] = t.attacks(pt, s1, square_bb(s2)) & t.attacks(pt, s2, square_bb(s1));
                        t.inverse_ray_pass_bb[i2][i1] =
                            t.attacks(pt, s1, 0) & (t.attacks(pt, s2, square_bb(s1)) | square_bb(s2));
                    }
                    t.between_bb[i1][i2] |= square_bb(s2);
                }
            }
        }
        t
    }

    /// Attacks of a bishop or a rook through the magic tables.
    #[inline(always)]
    fn attacks(&self, pt: PieceType, s: Square, occupied: Bitboard) -> Bitboard {
        let m = &self.magics[s as usize & 63][(pt - BISHOP) as usize & 1];
        self.slider[m.offset as usize + m.index(occupied)]
    }
}

static TABLES: OnceLock<Box<Tables>> = OnceLock::new();

/// The tables, built on first use. `init()` builds them up front.
#[inline(always)]
fn tables() -> &'static Tables {
    TABLES.get_or_init(Tables::new)
}

/// Builds the tables now rather than on first use (`Attacks::init()`).
pub fn init() {
    tables();
}

/// `magic(s, pt)`, for tests.
pub fn magic(s: Square, pt: PieceType) -> Magic {
    tables().magics[s as usize][(pt - BISHOP) as usize]
}

/// Horizontal, vertical or diagonal line through s1 and s2, or 0.
#[inline(always)]
pub fn line_bb(s1: Square, s2: Square) -> Bitboard {
    debug_assert!(is_ok_square(s1) && is_ok_square(s2));
    tables().line_bb[s1 as usize & 63][s2 as usize & 63]
}

/// Segment from s1 to s2, excluding s1 but including s2.
#[inline(always)]
pub fn between_bb(s1: Square, s2: Square) -> Bitboard {
    debug_assert!(is_ok_square(s1) && is_ok_square(s2));
    tables().between_bb[s1 as usize & 63][s2 as usize & 63]
}

/// Ray going from s1 to s2 and past, excluding s1.
#[inline(always)]
pub fn ray_pass_bb(s1: Square, s2: Square) -> Bitboard {
    debug_assert!(is_ok_square(s1) && is_ok_square(s2));
    tables().inverse_ray_pass_bb[s2 as usize & 63][s1 as usize & 63]
}

#[inline(always)]
pub fn diag_bb(s: Square) -> Bitboard {
    DIAG_BB[s as usize & 63]
}

#[inline(always)]
pub fn antidiag_bb(s: Square) -> Bitboard {
    ANTI_DIAG_BB[s as usize & 63]
}

/// Attacks of a piece type on an empty board (`attacks_bb(pt, s)`); for pawns
/// use `pawn_attacks`.
#[inline(always)]
pub fn pseudo_attacks(pt: PieceType, s: Square) -> Bitboard {
    PSEUDO_ATTACKS[pt as usize & 7][s as usize & 63]
}

/// `attacks_bb(PAWN, s, c)`.
#[inline(always)]
pub fn pawn_attacks(c: Color, s: Square) -> Bitboard {
    PSEUDO_ATTACKS[c as usize & 1][s as usize & 63]
}

/// Attacks of a piece type (not a pawn) from s given the occupancy. Sliding
/// attacks stop at the first occupied square.
#[inline(always)]
pub fn attacks_bb(pt: PieceType, s: Square, occupied: Bitboard) -> Bitboard {
    debug_assert!(pt != PAWN && is_ok_square(s));
    match pt {
        BISHOP | ROOK => tables().attacks(pt, s, occupied),
        QUEEN => {
            let t = tables();
            t.attacks(BISHOP, s, occupied) | t.attacks(ROOK, s, occupied)
        }
        _ => pseudo_attacks(pt, s),
    }
}

/// Bishop and rook attacks from s, in this order.
#[inline(always)]
pub fn both_attacks_bb(s: Square, occupied: Bitboard) -> (Bitboard, Bitboard) {
    let t = tables();
    (t.attacks(BISHOP, s, occupied), t.attacks(ROOK, s, occupied))
}

/// Attacks of a given piece, pawns included.
#[inline(always)]
pub fn attacks_bb_piece(pc: Piece, s: Square, occupied: Bitboard) -> Bitboard {
    if type_of(pc) == PAWN {
        pawn_attacks(color_of(pc), s)
    } else {
        attacks_bb(type_of(pc), s, occupied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::misc::Prng;

    #[test]
    fn magics_match_slow_attacks() {
        let mut rng = Prng::new(1234);
        for s in 0..64u8 {
            for _ in 0..200 {
                let occ: u64 = rng.rand() & rng.rand();
                assert_eq!(attacks_bb(ROOK, s, occ), sliding_attack(ROOK, s, occ));
                assert_eq!(attacks_bb(BISHOP, s, occ), sliding_attack(BISHOP, s, occ));
            }
        }
    }

    #[test]
    fn line_and_between() {
        // a1-h8 diagonal
        assert_eq!(line_bb(0, 63), 0x8040201008040201);
        assert_eq!(between_bb(0, 63), 0x8040201008040200);
        assert_eq!(between_bb(0, 10), square_bb(10)); // not aligned: only s2
        assert_eq!(line_bb(0, 10), 0);
        // ray from a1 through b2 and past
        assert_eq!(ray_pass_bb(0, 9), 0x8040201008040200);
    }
}

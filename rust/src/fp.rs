//! Doubles computed as the C++ engine computes them on x86-64, on every
//! target: the arithmetic of time management (README.md, "Time management",
//! "Floating point").
//!
//! On x86-64, ARM and PowerPC an `f64` operation is one IEEE operation,
//! rounded to a double, as with SSE2 in the C++ engine. On 32-bit x86 without
//! SSE2 (the i586 targets) it runs on the x87, whose registers keep 64-bit
//! mantissas: a product or a quotient is not rounded to a double before the
//! next operation. The time manager meets that often: with round clocks the
//! exact value of `optScale * timeLeft` is often an integer, which the double
//! computation misses by one unit in the last place (5279.999...) and the x87
//! does not (5280), and the truncation to milliseconds differs. Setting the
//! x87 precision control to 53 bits is not an option: the C libraries' own
//! functions there (`round`, `pow`) rely on the 64-bit setting.
//!
//! So `F64` does `+ - * /` and the conversion from an integer in software
//! on those targets, with the rounding of IEEE 754 (to nearest, ties to
//! even), and with the native operations elsewhere; the feature `softfloat`
//! forces the software path on every target, to test it against the C++
//! engine on x86-64. Comparisons, and the truncation to an integer of a
//! value that is a double, are exact on every target and stay native.
//! `log10` and `pow` are those of the platform's C library (see the README
//! for what that leaves).

use std::ops::{Add, Div, Mul, Sub};

/// Whether the arithmetic of `F64` is done in software.
pub const SOFT: bool = cfg!(any(feature = "softfloat", all(target_arch = "x86", not(target_feature = "sse2"))));

/// A double whose arithmetic is rounded as on x86-64 with SSE2.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct F64(pub f64);

impl F64 {
    /// `double(x)` of an `i64`.
    #[inline]
    pub fn from_i64(x: i64) -> F64 {
        if SOFT {
            F64(soft_from_i64(x))
        } else {
            F64(x as f64)
        }
    }

    /// `double(x)` of a `u64`.
    #[inline]
    pub fn from_u64(x: u64) -> F64 {
        if SOFT {
            F64(if x == 0 { 0.0 } else { round_pack(false, x as u128, 0).unwrap_or(x as f64) })
        } else {
            F64(x as f64)
        }
    }

    /// `double(x)` of an `i32` (exact on every target).
    #[inline]
    pub fn from_i32(x: i32) -> F64 {
        F64(x as f64)
    }

    /// `TimePoint(x)` (`cvttsd2si`): truncation toward zero, and `i64::MIN`
    /// out of range, as the C++ engine on x86-64 (undefined behaviour in
    /// C++; Rust's `as` would saturate).
    #[inline]
    pub fn to_i64(self) -> i64 {
        // [-2^63, 2^63), the doubles that convert to an i64
        const LIMIT: f64 = 9_223_372_036_854_775_808u64 as f64;
        if (-LIMIT..LIMIT).contains(&self.0) {
            self.0 as i64
        } else {
            i64::MIN
        }
    }

    /// `int(x)`: truncation toward zero, for values in the range of an i32.
    #[inline]
    pub fn to_i32(self) -> i32 {
        self.0 as i32
    }

    /// `std::log10`, of the platform's C library.
    #[inline]
    pub fn log10(self) -> F64 {
        F64(self.0.log10())
    }

    /// `std::pow`, of the platform's C library.
    #[inline]
    pub fn powf(self, e: f64) -> F64 {
        F64(self.0.powf(e))
    }
}

/// `std::min` of two doubles: `(b < a) ? b : a`.
#[inline]
pub fn min(a: F64, b: F64) -> F64 {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max` of two doubles: `(a < b) ? b : a`.
#[inline]
pub fn max(a: F64, b: F64) -> F64 {
    if a < b {
        b
    } else {
        a
    }
}

/// `std::clamp` of doubles: `v < lo ? lo : hi < v ? hi : v`.
#[inline]
pub fn clamp(v: F64, lo: f64, hi: f64) -> F64 {
    if v.0 < lo {
        F64(lo)
    } else if hi < v.0 {
        F64(hi)
    } else {
        v
    }
}

macro_rules! op {
    ($trait:ident, $method:ident, $soft:ident, $op:tt) => {
        impl $trait for F64 {
            type Output = F64;
            #[inline]
            fn $method(self, rhs: F64) -> F64 {
                if SOFT {
                    F64($soft(self.0, rhs.0))
                } else {
                    F64(self.0 $op rhs.0)
                }
            }
        }
        impl $trait<f64> for F64 {
            type Output = F64;
            #[inline]
            fn $method(self, rhs: f64) -> F64 {
                self.$method(F64(rhs))
            }
        }
        impl $trait<F64> for f64 {
            type Output = F64;
            #[inline]
            fn $method(self, rhs: F64) -> F64 {
                F64(self).$method(rhs)
            }
        }
    };
}

op!(Add, add, soft_add, +);
op!(Sub, sub, soft_sub, -);
op!(Mul, mul, soft_mul, *);
op!(Div, div, soft_div, /);

// ---------------------------------------------------------------------------
// IEEE 754 binary64 arithmetic in integers: round to nearest, ties to even.
// The operands and results of the time manager are zero or normal numbers;
// anything else (infinities, NaN, subnormals, overflow) falls back to the
// native operation, which no clock of this engine reaches.

const MANT_BITS: u32 = 52;
const EXP_BIAS: i32 = 1023;

/// sign, mantissa with its implicit bit, unbiased exponent of the mantissa's
/// unit (value = mant * 2^exp); None for zero, Some(None) for what is not
/// handled.
fn unpack(x: f64) -> Option<Option<(bool, u64, i32)>> {
    let b = x.to_bits();
    let sign = b >> 63 != 0;
    let e = ((b >> MANT_BITS) & 0x7FF) as i32;
    let m = b & ((1 << MANT_BITS) - 1);
    if e == 0 && m == 0 {
        return None;
    }
    if e == 0 || e == 0x7FF {
        return Some(None);
    }
    Some(Some((sign, m | (1 << MANT_BITS), e - EXP_BIAS - MANT_BITS as i32)))
}

/// The double nearest to (-1)^sign * m * 2^e (ties to even), m > 0; None if
/// it is not a normal number.
fn round_pack(sign: bool, m: u128, e: i32) -> Option<f64> {
    debug_assert!(m != 0);
    let p = 127 - m.leading_zeros() as i32; // position of the leading bit
    let shift = p - MANT_BITS as i32;
    let (mut mant, mut exp) = if shift > 0 {
        let rest = m & ((1u128 << shift) - 1);
        let half = 1u128 << (shift - 1);
        let mut mant = (m >> shift) as u64;
        if rest > half || (rest == half && mant & 1 == 1) {
            mant += 1;
        }
        (mant, e + shift)
    } else {
        ((m << -shift) as u64, e + shift)
    };
    if mant == 1 << (MANT_BITS + 1) {
        mant >>= 1;
        exp += 1;
    }
    let biased = exp + MANT_BITS as i32 + EXP_BIAS;
    if !(1..=2046).contains(&biased) {
        return None;
    }
    Some(f64::from_bits(((sign as u64) << 63) | ((biased as u64) << MANT_BITS) | (mant & ((1 << MANT_BITS) - 1))))
}

/// `a * b`
pub fn soft_mul(a: f64, b: f64) -> f64 {
    match (unpack(a), unpack(b)) {
        (Some(Some((sa, ma, ea))), Some(Some((sb, mb, eb)))) => {
            round_pack(sa != sb, ma as u128 * mb as u128, ea + eb).unwrap_or(a * b)
        }
        (Some(None), _) | (_, Some(None)) => a * b,
        // a zero: the sign of the product
        _ => {
            if (a.to_bits() ^ b.to_bits()) >> 63 != 0 {
                -0.0
            } else {
                0.0
            }
        }
    }
}

/// `a / b`
pub fn soft_div(a: f64, b: f64) -> f64 {
    match (unpack(a), unpack(b)) {
        (Some(Some((sa, ma, ea))), Some(Some((sb, mb, eb)))) => {
            // 117 bits over 53: a quotient of 64 or 65 bits, then the
            // remainder as a sticky bit below them
            let num = (ma as u128) << 64;
            let q = num / mb as u128;
            let r = num % mb as u128;
            let m = (q << 1) | (r != 0) as u128;
            round_pack(sa != sb, m, ea - eb - 64 - 1).unwrap_or(a / b)
        }
        (Some(None), _) | (_, Some(None)) | (_, None) => a / b,
        // zero divided by a normal number
        (None, Some(Some(_))) => {
            if (a.to_bits() ^ b.to_bits()) >> 63 != 0 {
                -0.0
            } else {
                0.0
            }
        }
    }
}

/// `a + b`
pub fn soft_add(a: f64, b: f64) -> f64 {
    let (x, y) = match (unpack(a), unpack(b)) {
        (Some(Some(x)), Some(Some(y))) => (x, y),
        (Some(None), _) | (_, Some(None)) => return a + b,
        // a zero: the other operand (both zeros: +0 unless both are -0)
        (None, Some(Some(_))) => return b,
        (Some(Some(_)), None) => return a,
        (None, None) => return if a.to_bits() >> 63 != 0 && b.to_bits() >> 63 != 0 { -0.0 } else { 0.0 },
    };
    // the operand with the larger exponent first
    let ((sx, mx, ex), (sy, my, ey)) = if x.2 >= y.2 { (x, y) } else { (y, x) };
    // 64 bits of room below the mantissas; what the alignment shifts out of
    // the smaller one is kept as a sticky bit at the bottom
    let d = (ex - ey) as u32;
    let mx = (mx as u128) << 64;
    let my = (my as u128) << 64;
    let my = if d >= 120 { 1 } else { (my >> d) | ((my & ((1u128 << d) - 1)) != 0) as u128 };
    let e = ex - 64;
    if sx == sy {
        round_pack(sx, mx + my, e).unwrap_or(a + b)
    } else if mx > my {
        round_pack(sx, mx - my, e).unwrap_or(a + b)
    } else if my > mx {
        round_pack(sy, my - mx, e).unwrap_or(a + b)
    } else {
        0.0 // exact cancellation: +0 when rounding to nearest
    }
}

/// `a - b`
pub fn soft_sub(a: f64, b: f64) -> f64 {
    soft_add(a, -b)
}

/// `double(x)` of an `i64`
pub fn soft_from_i64(x: i64) -> f64 {
    if x == 0 {
        return 0.0;
    }
    round_pack(x < 0, x.unsigned_abs() as u128, 0).unwrap_or(x as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
        /// doubles of the kinds the time manager computes with: integers,
        /// short decimals, ratios of them, and arbitrary bit patterns of
        /// moderate exponent, both signs
        fn value(&mut self) -> f64 {
            let k = self.next() % 6;
            let n = (self.next() % (1 << 40)) as i64 - (1 << 30);
            match k {
                0 => n as f64,
                1 => (n % 100000) as f64 / 1000.0,
                2 => (n % 1000) as f64 / ((self.next() % 999) as f64 + 1.0),
                3 => (self.next() % (1 << 62)) as f64,
                _ => {
                    let e = 1023 - 40 + (self.next() % 80);
                    f64::from_bits((self.next() & ((1 << 52) - 1)) | (e << 52) | (self.next() & (1 << 63)))
                }
            }
        }
    }

    /// On the targets where the native operations are SSE2-like IEEE ones,
    /// the software ones must give the same bits; on the x87 targets the
    /// native ones are not the reference, and the driver of the time
    /// manager (tm_check.expected) checks the software path instead.
    #[test]
    fn soft_float_is_ieee() {
        if cfg!(all(target_arch = "x86", not(target_feature = "sse2"))) {
            return;
        }
        let mut rng = Rng(12345);
        for i in 0..2_000_000 {
            let a = rng.value();
            let b = if i % 7 == 0 { a } else { rng.value() };
            let check = |name: &str, soft: f64, native: f64| {
                assert_eq!(soft.to_bits(), native.to_bits(), "{name}({a:e}, {b:e}): {soft:e} != {native:e}");
            };
            check("add", soft_add(a, b), a + b);
            check("sub", soft_sub(a, b), a - b);
            check("mul", soft_mul(a, b), a * b);
            if b != 0.0 {
                check("div", soft_div(a, b), a / b);
            }
            let n = (rng.next() as i64) >> (rng.next() % 64);
            check("from_i64", soft_from_i64(n), n as f64);
        }
        // the case of the README: 0.88 * 6000 / 2346000 * 2346000
        let v = soft_mul(soft_div(soft_mul(0.88, 6000.0), 2346000.0), 2346000.0);
        assert_eq!(v.to_bits(), 0x40B49FFFFFFFFFFF);
        // ties to even, and zeros
        assert_eq!(soft_from_i64((1 << 53) + 1), (1u64 << 53) as f64);
        assert_eq!(soft_from_i64((1 << 53) + 3), ((1u64 << 53) + 4) as f64);
        assert_eq!(soft_add(1.0, -1.0).to_bits(), 0);
        assert_eq!(soft_mul(-0.0, 3.0).to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn to_i64_as_cvttsd2si() {
        assert_eq!(F64(5279.999).to_i64(), 5279);
        assert_eq!(F64(-1.5).to_i64(), -1);
        assert_eq!(F64(1e19).to_i64(), i64::MIN);
        assert_eq!(F64(-9.3e18).to_i64(), i64::MIN);
        assert_eq!(F64(-9_223_372_036_854_775_808.0).to_i64(), i64::MIN);
    }
}

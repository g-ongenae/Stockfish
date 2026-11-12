//! Port of `src/nnue/`: the NNUE evaluation at this commit.
//!
//! Only the portable scalar path of the C++ code is ported (its `#else`
//! branches, without `VECTOR`). All the arithmetic is on integers of fixed
//! widths: the accumulators are `i16` updated with wrapping additions of `i8`
//! weights, as the C++ scalar and SIMD paths do, so their values do not depend
//! on the order of the updates; the layers use `i32` sums and the activations
//! saturate as in C++. Nothing depends on the width of `usize` beyond the 32
//! bits needed to index the weights (about 2^27).

pub mod accumulator;
pub mod architecture;
pub mod feature_transformer;
pub mod features;
pub mod layers;
pub mod network;

pub use accumulator::{AccumulatorCaches, AccumulatorStack};
pub use network::Network;

use crate::types::*;

/// Version of the evaluation file
pub const VERSION: u32 = 0x6A44_8AFA;

// Constants used in the evaluation value calculation
pub const OUTPUT_SCALE: i32 = 16;
pub const WEIGHT_SCALE_BITS: u32 = 6;
pub const FT_MAX_VAL: i16 = 255;
pub const HIDDEN_ONE_VAL: i32 = 128;

/// Number of input feature dimensions after conversion, and of the hidden
/// layers (nnue_architecture.h).
pub const L1: usize = 1024;
pub const L2: usize = 32;
pub const L3: usize = 32;

pub const RAW_LAYER_STACKS: usize = 192;
pub const LAYER_STACKS: usize = RAW_LAYER_STACKS;

pub type IndexType = u32;

/// Orients a square according to perspective: kings on files a..d are
/// mirrored so that they land on e..h.
#[rustfmt::skip]
pub const ORIENT_TBL: [u8; SQUARE_NB] = {
    const H: u8 = SQ_H1;
    const A: u8 = SQ_A1;
    [
        H, H, H, H, A, A, A, A,
        H, H, H, H, A, A, A, A,
        H, H, H, H, A, A, A, A,
        H, H, H, H, A, A, A, A,
        H, H, H, H, A, A, A, A,
        H, H, H, H, A, A, A, A,
        H, H, H, H, A, A, A, A,
        H, H, H, H, A, A, A, A,
    ]
};

/// Reads little-endian integers from a stream, with the sticky failure of a
/// C++ stream: after a short read, every later read gives zeros and `fail` is
/// set. The byte order of the machine does not matter.
pub struct LeReader<R: std::io::Read> {
    inner: R,
    pub fail: bool,
    buf: Vec<u8>,
}

impl<R: std::io::Read> LeReader<R> {
    pub fn new(inner: R) -> Self {
        LeReader { inner, fail: false, buf: vec![0; 1 << 16] }
    }

    fn fill(&mut self, n: usize) -> &[u8] {
        if !self.fail && self.inner.read_exact(&mut self.buf[..n]).is_err() {
            self.fail = true;
        }
        if self.fail {
            self.buf[..n].fill(0);
        }
        &self.buf[..n]
    }

    pub fn read_bytes(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(1 << 16) {
            let n = chunk.len();
            chunk.copy_from_slice(self.fill(n));
        }
    }

    pub fn u16(&mut self) -> u16 {
        let b = self.fill(2);
        u16::from_le_bytes([b[0], b[1]])
    }

    pub fn u32(&mut self) -> u32 {
        let b = self.fill(4);
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    pub fn i8s(&mut self, out: &mut [i8]) {
        for chunk in out.chunks_mut(1 << 16) {
            let n = chunk.len();
            let b = self.fill(n);
            for (d, &s) in chunk.iter_mut().zip(b) {
                *d = s as i8;
            }
        }
    }

    pub fn i16s(&mut self, out: &mut [i16]) {
        for chunk in out.chunks_mut(1 << 15) {
            let n = chunk.len();
            let b = self.fill(2 * n);
            for (d, s) in chunk.iter_mut().zip(b.chunks_exact(2)) {
                *d = i16::from_le_bytes([s[0], s[1]]);
            }
        }
    }

    pub fn i32s(&mut self, out: &mut [i32]) {
        for chunk in out.chunks_mut(1 << 14) {
            let n = chunk.len();
            let b = self.fill(4 * n);
            for (d, s) in chunk.iter_mut().zip(b.chunks_exact(4)) {
                *d = i32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            }
        }
    }

    /// True if the stream is at its end, `stream.peek() == eof`.
    pub fn at_eof(&mut self) -> bool {
        let mut b = [0u8; 1];
        loop {
            match self.inner.read(&mut b) {
                Ok(0) => return true,
                Ok(_) => return false,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }
}

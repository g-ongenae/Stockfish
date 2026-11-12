//! Port of `src/nnue/layers/`: the scalar paths of the layers of the stack.
//! With the integer widths of C++ (u8 inputs, i8 weights, i32 sums that cannot
//! overflow at these sizes), the SIMD paths of C++ give the same values.

use super::IndexType;

pub const MAX_SIMD_WIDTH: usize = 32;

pub const fn ceil_to_multiple(n: usize, base: usize) -> usize {
    n.div_ceil(base) * base
}

/// `AffineTransform::get_hash_value` (also that of the sparse input variant).
pub const fn affine_hash(prev_hash: u32, output_dimensions: IndexType) -> u32 {
    let mut hash_value: u32 = 0xCC03_DAE4u32.wrapping_add(output_dimensions);
    hash_value ^= prev_hash >> 1;
    hash_value ^= prev_hash << 31;
    hash_value
}

/// `ClippedReLU::get_hash_value`.
pub const fn clipped_relu_hash(prev_hash: u32) -> u32 {
    0x538D_24C7u32.wrapping_add(prev_hash)
}

/// A fully connected layer, `AffineTransform<IN, OUT>`, its weights in the row
/// order of the file (the C++ order without SSSE3): `weights[o * PADDED + i]`.
pub struct AffineTransform<const IN: usize, const PADDED: usize, const OUT: usize> {
    pub biases: [i32; OUT],
    pub weights: Box<[i8]>,
}

impl<const IN: usize, const PADDED: usize, const OUT: usize> AffineTransform<IN, PADDED, OUT> {
    pub fn read_parameters<R: std::io::Read>(stream: &mut super::LeReader<R>) -> Self {
        assert!(PADDED == ceil_to_multiple(IN, MAX_SIMD_WIDTH));
        let mut biases = [0i32; OUT];
        stream.i32s(&mut biases);
        let mut weights = vec![0i8; OUT * PADDED].into_boxed_slice();
        stream.i8s(&mut weights);
        AffineTransform { biases, weights }
    }

    /// Dense propagation, one dot product per output.
    #[inline]
    pub fn propagate(&self, input: &[u8], output: &mut [i32]) {
        let input = &input[..IN];
        for (o, out) in output[..OUT].iter_mut().enumerate() {
            let row = &self.weights[o * PADDED..o * PADDED + IN];
            let mut sum = self.biases[o];
            for (&w, &x) in row.iter().zip(input) {
                sum += w as i32 * x as i32;
            }
            *out = sum;
        }
    }
}

/// The first layer, `AffineTransformSparseInput<IN, OUT>`, whose input is
/// mostly zeros. Its weights are stored input-major, `weights[i * OUT + o]`
/// (C++ scrambles them likewise for SSSE3), so that the outputs of one
/// non-zero input are updated from contiguous weights. The sums are exact in
/// i32, so the order does not change them.
pub struct AffineTransformSparseInput<const IN: usize, const PADDED: usize, const OUT: usize> {
    pub biases: [i32; OUT],
    pub weights: Box<[i8]>,
}

impl<const IN: usize, const PADDED: usize, const OUT: usize> AffineTransformSparseInput<IN, PADDED, OUT> {
    /// Reads the weights in the row order of the file and transposes them.
    pub fn read_parameters<R: std::io::Read>(stream: &mut super::LeReader<R>) -> Self {
        let dense = AffineTransform::<IN, PADDED, OUT>::read_parameters(stream);
        let mut weights = vec![0i8; PADDED * OUT].into_boxed_slice();
        for o in 0..OUT {
            for i in 0..PADDED {
                weights[i * OUT + o] = dense.weights[o * PADDED + i];
            }
        }
        AffineTransformSparseInput { biases: dense.biases, weights }
    }

    /// Skips the zero inputs, as `affine_transform_non_ssse3` does.
    #[inline]
    pub fn propagate(&self, input: &[u8], output: &mut [i32; OUT]) {
        *output = self.biases;
        for (i, &x) in input[..IN].iter().enumerate() {
            if x != 0 {
                let x = x as i32;
                let column: &[i8; OUT] = self.weights[i * OUT..(i + 1) * OUT].try_into().unwrap();
                for (out, &w) in output.iter_mut().zip(column) {
                    *out += w as i32 * x;
                }
            }
        }
    }
}

/// `ClippedReLU<N, SHIFT>`: clamp(x >> SHIFT, 0, 127), the shift arithmetic.
#[inline]
pub fn clipped_relu<const SHIFT: u32>(input: &[i32], output: &mut [u8]) {
    for (o, &x) in output.iter_mut().zip(input) {
        *o = (x >> SHIFT).clamp(0, 127) as u8;
    }
}

/// `SqrClippedReLU<N, SHIFT>`: min(127, x * x >> (2 * SHIFT + 7)), squared in
/// 64 bits. The SSE2 path of C++ saturates x to 16 bits first, which only
/// changes values that are clipped to 127 anyway.
#[inline]
pub fn sqr_clipped_relu<const SHIFT: u32>(input: &[i32], output: &mut [u8]) {
    for (o, &x) in output.iter_mut().zip(input) {
        *o = 127i64.min((x as i64 * x as i64) >> (2 * SHIFT + 7)) as u8;
    }
}

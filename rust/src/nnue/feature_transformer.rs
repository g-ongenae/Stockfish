//! Port of `src/nnue/nnue_feature_transformer.h`: the weights of the input
//! layer, and the conversion of the accumulators into the input of the layer
//! stack.

use super::accumulator::{AccumulatorCaches, AccumulatorStack};
use super::features::{full_threats, k32h3, pp_3wide};
use super::{IndexType, LeReader, FT_MAX_VAL, L1};
use crate::position::Position;
use crate::types::*;

pub const HALF_DIMENSIONS: usize = L1;

pub const THREAT_INPUT_DIMENSIONS: IndexType = full_threats::DIMENSIONS;
pub const PAIR_INPUT_DIMENSIONS: IndexType = pp_3wide::DIMENSIONS;
pub const PSQ_DIMENSIONS: IndexType = k32h3::DIMENSIONS;
pub const THREAT_AND_PP_DIMENSIONS: IndexType = THREAT_INPUT_DIMENSIONS + PAIR_INPUT_DIMENSIONS;
pub const INPUT_DIMENSIONS: IndexType = PSQ_DIMENSIONS + THREAT_AND_PP_DIMENSIONS;
pub const OUTPUT_DIMENSIONS: usize = HALF_DIMENSIONS;

/// Size of the output, the input of the layer stack
pub const BUFFER_SIZE: usize = OUTPUT_DIMENSIONS;

const fn combine_hash(hashes: [u32; 3]) -> u32 {
    let mut hash: u32 = 0;
    let mut i = 0;
    while i < hashes.len() {
        hash = hash.rotate_left(1);
        hash ^= hashes[i];
        i += 1;
    }
    hash
}

/// Hash value embedded in the evaluation file
pub const fn get_hash_value() -> u32 {
    combine_hash([full_threats::HASH_VALUE, pp_3wide::HASH_VALUE, k32h3::HASH_VALUE]) ^ (OUTPUT_DIMENSIONS as u32 * 2)
}

/// The weights are kept in the order of the file, the order of the C++
/// build without SIMD (`PackusEpi16Order` is the identity there): row `i` of
/// a weight array holds the 1024 weights of feature `i`.
pub struct FeatureTransformer {
    pub biases: [i16; HALF_DIMENSIONS],
    /// Piece-square weights, the blocks of K32H3 one after the other
    pub weights: Box<[i8]>,
    /// Threat then pawn pair weights, so that one index addresses either
    pub threat_and_pp_weights: Box<[i8]>,
}

impl FeatureTransformer {
    /// Reads the parameters; returns None on a short read.
    pub fn read_parameters<R: std::io::Read>(stream: &mut LeReader<R>) -> Option<FeatureTransformer> {
        let mut biases = [0i16; HALF_DIMENSIONS];
        stream.i16s(&mut biases);

        // vec![0; n] is backed by zeroed pages, so filling it does not hold
        // the network twice in memory.
        let threat_and_pp_size = THREAT_AND_PP_DIMENSIONS as usize * HALF_DIMENSIONS;
        let mut threat_and_pp_weights = vec![0i8; threat_and_pp_size].into_boxed_slice();
        let threat_size = THREAT_INPUT_DIMENSIONS as usize * HALF_DIMENSIONS;
        stream.i8s(&mut threat_and_pp_weights[..threat_size]);
        stream.i8s(&mut threat_and_pp_weights[threat_size..]);

        let mut weights = vec![0i8; PSQ_DIMENSIONS as usize * HALF_DIMENSIONS].into_boxed_slice();
        stream.i8s(&mut weights);

        if stream.fail {
            return None;
        }
        Some(FeatureTransformer { biases, weights, threat_and_pp_weights })
    }

    /// The 1024 weights of a piece-square feature of a block.
    #[inline(always)]
    pub fn psq_row(&self, block: usize, index: u16) -> &[i8; HALF_DIMENSIONS] {
        let start = (k32h3::block_offset(block) as usize + index as usize) * HALF_DIMENSIONS;
        self.weights[start..start + HALF_DIMENSIONS].try_into().unwrap()
    }

    /// The 1024 weights of a threat or pawn pair feature.
    #[inline(always)]
    pub fn threat_row(&self, index: u16) -> &[i8; HALF_DIMENSIONS] {
        let start = index as usize * HALF_DIMENSIONS;
        self.threat_and_pp_weights[start..start + HALF_DIMENSIONS].try_into().unwrap()
    }

    /// Converts the input features: updates the accumulators of the position,
    /// then multiplies the clipped halves of each perspective, side to move
    /// first.
    pub fn transform(
        &self,
        pos: &Position,
        accumulator_stack: &mut AccumulatorStack,
        cache: &mut AccumulatorCaches,
        output: &mut [u8; BUFFER_SIZE],
    ) {
        accumulator_stack.evaluate(pos, self, cache);
        let accumulation = &accumulator_stack.latest().accumulation;

        let perspectives = [pos.side_to_move(), not_color(pos.side_to_move())];
        for (p, &perspective) in perspectives.iter().enumerate() {
            transform_perspective(&accumulation[perspective as usize], output, p);
        }
    }
}

/// The scalar path of `transform_perspective`. The SIMD paths of C++ give the
/// same bytes: they skip the lower clip of the second factor, whose negative
/// products packus then turns into 0, and divide by 512 with mulhi.
#[inline]
fn transform_perspective(accumulation: &[i16; HALF_DIMENSIONS], output: &mut [u8; BUFFER_SIZE], perspective: usize) {
    let offset = (HALF_DIMENSIONS / 2) * perspective;
    let (first, second) = accumulation.split_at(HALF_DIMENSIONS / 2);
    for (j, (&a, &b)) in first.iter().zip(second).enumerate() {
        let sum0 = a.clamp(0, FT_MAX_VAL) as u32;
        let sum1 = b.clamp(0, FT_MAX_VAL) as u32;
        output[offset + j] = ((sum0 * sum1) / 512) as u8;
    }
}

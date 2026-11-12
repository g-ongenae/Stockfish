//! Port of `src/nnue/nnue_architecture.h`: one layer stack of the network.

use super::layers::*;
use super::{IndexType, LeReader, HIDDEN_ONE_VAL, L1, L2, L3, OUTPUT_SCALE, WEIGHT_SCALE_BITS};

pub const TRANSFORMED_FEATURE_DIMENSIONS: usize = L1;
pub const FC_0_OUTPUTS: usize = L2;
pub const FC_1_OUTPUTS: usize = L3;

const FC_1_INPUTS: usize = FC_0_OUTPUTS * 2;
const FC_2_INPUTS: usize = FC_0_OUTPUTS * 2 + FC_1_OUTPUTS * 2;

pub struct NetworkArchitecture {
    fc_0: AffineTransform<TRANSFORMED_FEATURE_DIMENSIONS, { ceil_to_multiple(L1, MAX_SIMD_WIDTH) }, FC_0_OUTPUTS>,
    fc_1: AffineTransform<FC_1_INPUTS, { ceil_to_multiple(FC_1_INPUTS, MAX_SIMD_WIDTH) }, FC_1_OUTPUTS>,
    fc_2: AffineTransform<FC_2_INPUTS, { ceil_to_multiple(FC_2_INPUTS, MAX_SIMD_WIDTH) }, 1>,
}

/// Hash value embedded in the evaluation file. The hash of ac_sqr_0 is left
/// out on purpose, as in C++ (the trainer does not write it).
pub const fn get_hash_value() -> u32 {
    let mut hash_value: u32 = 0xEC42_E90D;
    hash_value ^= TRANSFORMED_FEATURE_DIMENSIONS as u32 * 2;
    hash_value = affine_hash(hash_value, FC_0_OUTPUTS as IndexType);
    hash_value = clipped_relu_hash(hash_value);
    hash_value = affine_hash(hash_value, FC_1_OUTPUTS as IndexType);
    hash_value = clipped_relu_hash(hash_value);
    hash_value = affine_hash(hash_value, 1);
    hash_value
}

impl NetworkArchitecture {
    /// The activations have no parameters.
    pub fn read_parameters<R: std::io::Read>(stream: &mut LeReader<R>) -> Option<NetworkArchitecture> {
        let fc_0 = AffineTransform::read_parameters(stream);
        let fc_1 = AffineTransform::read_parameters(stream);
        let fc_2 = AffineTransform::read_parameters(stream);
        (!stream.fail).then_some(NetworkArchitecture { fc_0, fc_1, fc_2 })
    }

    pub fn propagate(&self, transformed_features: &[u8; TRANSFORMED_FEATURE_DIMENSIONS]) -> i32 {
        let mut fc_0_out = [0i32; FC_0_OUTPUTS];
        let mut concat = [0u8; ceil_to_multiple(FC_2_INPUTS, 32)];
        let mut fc_1_out = [0i32; FC_1_OUTPUTS];
        let mut fc_2_out = [0i32; 1];

        self.fc_0.propagate_sparse(transformed_features, &mut fc_0_out);
        sqr_clipped_relu::<{ WEIGHT_SCALE_BITS + 1 }>(&fc_0_out, &mut concat[..FC_0_OUTPUTS]);
        clipped_relu::<{ WEIGHT_SCALE_BITS + 1 }>(&fc_0_out, &mut concat[FC_0_OUTPUTS..FC_1_INPUTS]);

        self.fc_1.propagate(&concat, &mut fc_1_out);
        sqr_clipped_relu::<WEIGHT_SCALE_BITS>(&fc_1_out, &mut concat[FC_1_INPUTS..FC_1_INPUTS + FC_1_OUTPUTS]);
        clipped_relu::<WEIGHT_SCALE_BITS>(&fc_1_out, &mut concat[FC_1_INPUTS + FC_1_OUTPUTS..FC_2_INPUTS]);

        self.fc_2.propagate(&concat, &mut fc_2_out);

        let fwd_out = fc_2_out[0] + fc_0_out[FC_0_OUTPUTS - 2] - fc_0_out[FC_0_OUTPUTS - 1];

        // fwd_out is such that 1.0 is HIDDEN_ONE_VAL * (1 << WEIGHT_SCALE_BITS)
        // * 2 in quantized form, but 1.0 should be 600 * OUTPUT_SCALE; i64 so
        // that it cannot overflow. Division truncates toward zero, as in C++.
        const MULTIPLIER: i64 = 600 * OUTPUT_SCALE as i64;
        const DENOMINATOR: i64 = HIDDEN_ONE_VAL as i64 * (1i64 << WEIGHT_SCALE_BITS) * 2;
        ((fwd_out as i64 * MULTIPLIER) / DENOMINATOR) as i32
    }
}

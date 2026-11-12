//! Port of `src/nnue/network.{h,cpp}`: reading the network file, and the
//! evaluation of a position.

use std::io::Read;

use super::accumulator::{AccumulatorCaches, AccumulatorStack};
use super::architecture::{self, NetworkArchitecture};
use super::feature_transformer::{self, FeatureTransformer, BUFFER_SIZE};
use super::{LeReader, LAYER_STACKS, OUTPUT_SCALE, RAW_LAYER_STACKS, VERSION};
use crate::bitboard::*;
use crate::position::Position;
use crate::types::*;

const LAYER_STACK_TABLE_HASH: u32 = 0x5A7A_B1E5;

/// Hash value of the structure of the evaluation function
pub const HASH: u32 = feature_transformer::get_hash_value() ^ architecture::get_hash_value() ^ LAYER_STACK_TABLE_HASH;

/// The default network, `EvalFileDefaultName`.
pub const EVAL_FILE_DEFAULT_NAME: &str = "nn-18cd32e11a9c.nnue";

pub struct Network {
    feature_transformer: FeatureTransformer,
    /// Only the stored layer stacks; C++ reserves room for all 192.
    network: Vec<NetworkArchitecture>,
    layer_stack_fallback: [u16; RAW_LAYER_STACKS],
    layer_stack_index: [u16; RAW_LAYER_STACKS],
    description: Vec<u8>,
}

impl Network {
    pub fn feature_transformer(&self) -> &FeatureTransformer {
        &self.feature_transformer
    }

    pub fn description(&self) -> &[u8] {
        &self.description
    }

    pub fn stored_layer_stacks(&self) -> usize {
        self.network.len()
    }

    /// Reads a network file, as `Network::load` from a stream. Returns None
    /// if the file is not a network of this architecture, or does not end
    /// right after its parameters.
    pub fn load<R: Read>(stream: R) -> Option<Network> {
        let mut stream = LeReader::new(stream);

        // Header
        let version = stream.u32();
        let hash_value = stream.u32();
        let size = stream.u32();
        if stream.fail || version != VERSION {
            return None;
        }
        let mut description = Vec::new();
        let mut remaining = size as usize;
        while remaining > 0 && !stream.fail {
            let want = remaining.min(4096);
            let start = description.len();
            description.resize(start + want, 0);
            stream.read_bytes(&mut description[start..]);
            remaining -= want;
        }
        if stream.fail || hash_value != HASH {
            return None;
        }

        // Table of the layer stacks: which ones are stored, the others fall
        // back on one of them.
        if stream.u32() != LAYER_STACK_TABLE_HASH || stream.u32() != RAW_LAYER_STACKS as u32 {
            return None;
        }
        let stored_layer_stacks = stream.u32() as usize;
        if stored_layer_stacks == 0 || stored_layer_stacks > LAYER_STACKS {
            return None;
        }
        let mut layer_stack_fallback = [0u16; RAW_LAYER_STACKS];
        for f in layer_stack_fallback.iter_mut() {
            *f = stream.u16();
            if *f as usize >= RAW_LAYER_STACKS {
                return None;
            }
        }
        let mut layer_stack_index = [0u16; RAW_LAYER_STACKS];
        let mut stored = 0;
        for s in 0..RAW_LAYER_STACKS {
            if layer_stack_fallback[s] as usize == s {
                layer_stack_index[s] = stored;
                stored += 1;
            }
        }
        if stored as usize != stored_layer_stacks {
            return None;
        }
        for s in 0..RAW_LAYER_STACKS {
            let f = layer_stack_fallback[s] as usize;
            if layer_stack_fallback[f] as usize != f {
                return None;
            }
            layer_stack_index[s] = layer_stack_index[f];
        }

        if stream.u32() != feature_transformer::get_hash_value() {
            return None;
        }
        let feature_transformer = FeatureTransformer::read_parameters(&mut stream)?;

        let mut network = Vec::with_capacity(stored_layer_stacks);
        for _ in 0..stored_layer_stacks {
            if stream.u32() != architecture::get_hash_value() {
                return None;
            }
            network.push(NetworkArchitecture::read_parameters(&mut stream)?);
        }

        if stream.fail || !stream.at_eof() {
            return None;
        }
        Some(Network { feature_transformer, network, layer_stack_fallback, layer_stack_index, description })
    }

    /// The layer stack used for a position: by the number of pieces, the
    /// queens, the layout of the pawns and the number of officers.
    pub fn layer_stack(&self, pos: &Position) -> usize {
        let stm = pos.side_to_move();
        let band = (pos.count_both(ALL_PIECES) - 1) / 4;
        let queens = 2 * (pos.pieces_cp(stm, QUEEN) != 0) as i32 + (pos.pieces_cp(not_color(stm), QUEEN) != 0) as i32;

        // Pawn layout: 0 if pawns stand on both board halves, 1 if on one
        // half only, 2 if there are no pawns
        const QUEEN_SIDE: Bitboard = FILE_A_BB | (FILE_A_BB << 1) | (FILE_A_BB << 2) | (FILE_A_BB << 3);
        let pawns = pos.pieces_pt(PAWN);
        let layout = if pawns == 0 {
            2
        } else if pawns & QUEEN_SIDE != 0 && pawns & !QUEEN_SIDE != 0 {
            0
        } else {
            1
        };

        // Officers are the pieces other than pawns and kings
        let officers = |c: Color| popcount(pos.pieces_c(c) & !pos.pieces_pt2(PAWN, KING));
        let many = (officers(WHITE).max(officers(BLACK)) >= 2) as i32;

        self.layer_stack_index[(((band * 4 + queens) * 3 + layout) * 2 + many) as usize] as usize
    }

    /// The output of the network, from the point of view of the side to move.
    pub fn evaluate(&self, pos: &Position, accumulator_stack: &mut AccumulatorStack, cache: &mut AccumulatorCaches) -> Value {
        let mut transformed_features = [0u8; BUFFER_SIZE];

        let bucket = self.layer_stack(pos);

        self.feature_transformer.transform(pos, accumulator_stack, cache, &mut transformed_features);
        let positional = self.network[bucket].propagate(&transformed_features);
        positional / OUTPUT_SCALE
    }

    /// The size the C++ engine reports, `sizeof` of its structures.
    pub fn reported_size_mib(&self) -> usize {
        let ft = 2 * feature_transformer::HALF_DIMENSIONS
            + (feature_transformer::INPUT_DIMENSIONS as usize) * feature_transformer::HALF_DIMENSIONS;
        let arch = 32 * 4 + 32 * 1024 + 32 * 4 + 32 * 64 + 64 + 128;
        (ft + arch * self.network.len()) / (1024 * 1024)
    }

    pub fn layer_stack_fallback(&self) -> &[u16; RAW_LAYER_STACKS] {
        &self.layer_stack_fallback
    }
}

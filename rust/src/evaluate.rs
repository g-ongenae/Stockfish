//! Port of `src/evaluate.{h,cpp}`: the static evaluation on top of the
//! network.

use crate::nnue::{AccumulatorCaches, AccumulatorStack, Network};
use crate::position::Position;
use crate::types::*;

fn simple_eval(pos: &Position) -> i32 {
    let c = pos.side_to_move();
    PAWN_VALUE * (pos.count(c, PAWN) - pos.count(not_color(c), PAWN)) + pos.non_pawn_material(c)
        - pos.non_pawn_material(not_color(c))
}

/// The evaluation of a position not in check, from the point of view of the
/// side to move.
pub fn evaluate(
    network: &Network,
    pos: &Position,
    accumulators: &mut AccumulatorStack,
    caches: &mut AccumulatorCaches,
    optimism: i32,
) -> Value {
    debug_assert!(pos.checkers() == 0);
    let nnue = network.evaluate(pos, accumulators, caches);
    scale_evaluation(nnue, optimism, pos)
}

/// Applies the scaling by optimism, material and rule50 to the output of the
/// network. Integer divisions truncate toward zero, as in C++.
pub fn scale_evaluation(nnue: Value, optimism: i32, pos: &Position) -> Value {
    let se = simple_eval(pos);

    // Normalize the raw evaluations to [-1024, 1024] to measure their correlation
    let se_norm = (se * 1024) / (se.abs() + 1024);
    let nnue_norm = (nnue * 1024) / (nnue.abs() + 1024);
    let correl = (se_norm * nnue_norm) / 512;

    // When winning, boost easy positions, and vice versa
    let mut v = nnue + (nnue + optimism) * correl / 65536;

    // Scale the combined evaluation by total material
    let material = 521 * pos.count_both(PAWN) + pos.non_pawn_material_both();
    v += (material as i64 * v as i64 / 90649) as i32;

    // Damp down the evaluation linearly when shuffling
    v -= v * pos.rule50_count() / 189;

    // Guarantee that the evaluation does not hit the tablebase range
    v.clamp(VALUE_TB_LOSS_IN_MAX_PLY + 1, VALUE_TB_WIN_IN_MAX_PLY - 1)
}

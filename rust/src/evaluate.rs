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

/// `UCIEngine::to_cp()` with the doubles of x86-64 on every target (`F64`):
/// the values `eval` prints are outputs of single layer stacks, which can
/// lie outside the range of a score, where the margins proven for the search
/// output (`uci.rs`, `win_rate_params`) do not hold.
pub fn to_cp_exact(v: Value, pos: &Position) -> i32 {
    use crate::fp::F64;
    let material = pos.count_both(PAWN)
        + 3 * pos.count_both(KNIGHT)
        + 3 * pos.count_both(BISHOP)
        + 5 * pos.count_both(ROOK)
        + 9 * pos.count_both(QUEEN);
    let m = F64::from_i32(material.clamp(17, 78)) / 58.0;
    const AS: [f64; 4] = [-142.72052667, 372.35176398, -340.71073572, 415.23490212];
    let a = (((AS[0] * m + AS[1]) * m + AS[2]) * m) + AS[3];
    // the quotient is a double: its rounding is exact on every target
    (F64::from_i32(100 * v) / a).0.round() as i32
}

/// `0.01 * to_cp(v)` printed with two decimals: the double is within one
/// unit in the last place of the exact hundredth, so its two decimals are
/// those of the integer.
fn pawns(cp: i32) -> String {
    format!("{}.{:02}", cp.unsigned_abs() / 100, cp.unsigned_abs() % 100)
}

/// `NNUE::trace()` (`nnue_misc.cpp`): the output of each stored layer stack,
/// in pawns, the one used marked.
fn nnue_trace(pos: &Position, network: &Network, caches: &mut AccumulatorCaches) -> String {
    let mut accumulators = AccumulatorStack::new();
    accumulators.reset();
    let (positional, correct_bucket) = network.trace_evaluate(pos, &mut accumulators, caches);

    let mut s = format!(
        "NNUE network contributions (Normalized, {}\n",
        if pos.side_to_move() == WHITE { "White to move)" } else { "Black to move)" }
    );
    s += "+------------+------------+\n|   Bucket   | Evaluation |\n+------------+------------+\n";
    for (bucket, &v) in positional.iter().enumerate() {
        // format_cp_aligned_dot(): the sign of the value, then the pawns
        // right-aligned in 6 characters
        let sign = if v < 0 {
            '-'
        } else if v > 0 {
            '+'
        } else {
            ' '
        };
        s += &format!("|  {bucket:<10}|  {sign}{:>6}   |", pawns(to_cp_exact(v, pos)));
        if bucket == correct_bucket {
            s += " <-- this bucket is used";
        }
        s.push('\n');
    }
    s += "+------------+------------+\n";
    s
}

/// `Eval::trace()`: the table of the layer stacks, the output of the
/// network and the final evaluation, from white's point of view.
pub fn trace(pos: &Position, network: &Network) -> String {
    if pos.checkers() != 0 {
        return "Final evaluation: none (in check)".to_string();
    }

    let mut accumulators = AccumulatorStack::new();
    accumulators.reset();
    let mut caches = AccumulatorCaches::new(network);

    let mut s = format!("\n{}\n", nnue_trace(pos, network, &mut caches));

    let mut nnue = network.evaluate(pos, &mut accumulators, &mut caches);
    let mut s_v = scale_evaluation(nnue, VALUE_ZERO, pos); // requires stm perspective

    // showpos: "+0" for zero
    s += &format!("NNUE evaluation          {nnue:+} (side to move, internal units)\n");

    if pos.side_to_move() != WHITE {
        nnue = -nnue;
        s_v = -s_v;
    }

    let signed = |cp: i32| format!("{}{}", if cp < 0 { '-' } else { '+' }, pawns(cp));
    s += &format!("NNUE evaluation        {} (white side)\n", signed(to_cp_exact(nnue, pos)));
    s += &format!("Final evaluation      {} (white side) [with scaled NNUE, ...]\n", signed(to_cp_exact(s_v, pos)));
    s
}

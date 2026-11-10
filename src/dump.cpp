/*
  Stockfish, a UCI chess playing engine derived from Glaurung 2.1
  Copyright (C) 2004-2026 The Stockfish developers (see AUTHORS file)

  Stockfish is free software: you can redistribute it and/or modify
  it under the terms of the GNU General Public License as published by
  the Free Software Foundation, either version 3 of the License, or
  (at your option) any later version.

  Stockfish is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU General Public License for more details.

  You should have received a copy of the GNU General Public License
  along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/

#include "dump.h"

#include <cassert>
#include <initializer_list>
#include <string_view>

#include "evaluate.h"
#include "history.h"
#include "movegen.h"
#include "movepick.h"
#include "nnue/nnue_accumulator.h"
#include "position.h"
#include "uci.h"

namespace Stockfish {

namespace {

// No exchange can win or lose more than a queen, hence the SEE value of any
// move lies strictly inside (-SeeBound, SeeBound).
constexpr int SeeBound = 4096;

static_assert(QueenValue < SeeBound);

void append_string(std::string& out, std::string_view s) {
    constexpr char Hex[] = "0123456789abcdef";

    out += '"';
    for (unsigned char c : s)
    {
        if (c == '"' || c == '\\')
            (out += '\\') += char(c);
        else if (c < 0x20)
            ((out += "\\u00") += Hex[c >> 4]) += Hex[c & 15];
        else
            out += char(c);
    }
    out += '"';
}

// Keys and bitboards are printed as strings of 16 hexadecimal digits, because
// many JSON parsers cannot represent a 64-bit integer exactly.
void append_hex(std::string& out, u64 v) {
    constexpr char Hex[] = "0123456789ABCDEF";

    out += '"';
    for (int shift = 60; shift >= 0; shift -= 4)
        out += Hex[(v >> shift) & 15];
    out += '"';
}

void append_hex_list(std::string& out, std::initializer_list<u64> values) {
    char sep = '[';
    for (u64 v : values)
    {
        out += sep;
        append_hex(out, v);
        sep = ',';
    }
    out += ']';
}

void append_key(std::string& out, std::string_view name) {
    if (out.back() != '{')
        out += ',';
    append_string(out, name);
    out += ':';
}

template<typename Container, typename Fn>
void append_list(std::string& out, std::string_view name, const Container& items, Fn&& appendItem) {
    append_key(out, name);
    out += '[';
    bool first = true;
    for (const auto& item : items)
    {
        if (!first)
            out += ',';
        first = false;
        appendItem(item);
    }
    out += ']';
}

// Returns the exact static exchange evaluation of a move, that is the largest
// threshold for which see_ge() holds. It is found by bisection, see_ge() being
// monotonic in its threshold.
int see_value(const Position& pos, Move m) {

    int lo = -SeeBound, hi = SeeBound;

    assert(pos.see_ge(m, lo) && !pos.see_ge(m, hi));

    while (hi - lo > 1)
    {
        int mid = lo + (hi - lo) / 2;
        if (pos.see_ge(m, mid))
            lo = mid;
        else
            hi = mid;
    }

    return lo;
}

}  // namespace


// The history tables given to the move picker, and the scratch state of the
// NNUE evaluation. All the history entries are zero and never updated.
struct PositionDumper::Tables {

    explicit Tables(const Eval::NNUE::Network& net) :
        sharedHistory(1),
        caches(net) {
        mainHistory.fill(0);
        lowPlyHistory.fill(0);
        captureHistory.fill(0);
        continuationHistory.fill(0);
        sharedHistory.pawnHistory.clear_range(0, 0, 1);
    }

    ButterflyHistory              mainHistory;
    LowPlyHistory                 lowPlyHistory;
    CapturePieceToHistory         captureHistory;
    PieceToHistory                continuationHistory;
    SharedHistories               sharedHistory;
    Eval::NNUE::AccumulatorStack  accumulators;
    Eval::NNUE::AccumulatorCaches caches;
};

PositionDumper::PositionDumper(const Eval::NNUE::Network& net) :
    network(net),
    tables(std::make_unique<Tables>(net)) {}

PositionDumper::~PositionDumper() = default;

std::string PositionDumper::dump(const std::string& fen, bool isChess960) {

    StateInfo   st;
    Position    pos;
    std::string out = "{";

    out.reserve(4096);

    if (auto err = pos.set(fen, isChess960, &st))
    {
        append_key(out, "error");
        append_string(out, err->what());
        append_key(out, "input");
        append_string(out, fen);
        return out += '}';
    }

    const auto appendMove = [&](Move m) { append_string(out, UCIEngine::move(m, isChess960)); };

    append_key(out, "fen");
    append_string(out, pos.fen());
    append_key(out, "chess960");
    out += isChess960 ? "true" : "false";

    // Hash keys
    append_key(out, "key");
    append_hex(out, pos.key());
    append_key(out, "pawn_key");
    append_hex(out, pos.pawn_key());
    append_key(out, "material_key");
    append_hex(out, pos.material_key());
    append_key(out, "minor_piece_key");
    append_hex(out, pos.minor_piece_key());
    append_key(out, "non_pawn_key");
    append_hex_list(out, {pos.non_pawn_key(WHITE), pos.non_pawn_key(BLACK)});

    // Bitboards, lists are indexed by color or by piece type from pawn to king
    append_key(out, "checkers");
    append_hex(out, pos.checkers());
    append_key(out, "blockers_for_king");
    append_hex_list(out, {pos.blockers_for_king(WHITE), pos.blockers_for_king(BLACK)});
    append_key(out, "pinners");
    append_hex_list(out, {pos.pinners(WHITE), pos.pinners(BLACK)});
    append_key(out, "check_squares");
    append_hex_list(out,
                    {pos.check_squares(PAWN), pos.check_squares(KNIGHT), pos.check_squares(BISHOP),
                     pos.check_squares(ROOK), pos.check_squares(QUEEN), pos.check_squares(KING)});
    append_key(out, "threats");
    append_hex_list(out, {pos.threats_by(PAWN), pos.threats_by(KNIGHT), pos.threats_by(BISHOP),
                          pos.threats_by(ROOK), pos.threats_by(QUEEN), pos.threats_by(KING)});

    append_key(out, "non_pawn_material");
    (((out += '[') += std::to_string(pos.non_pawn_material(WHITE))) += ',') +=
      std::to_string(pos.non_pawn_material(BLACK));
    out += ']';

    // Static evaluation from the point of view of the side to move, in internal
    // units: the output of the network, and the final value with no optimism.
    // As in search, a position is not evaluated when the king is in check.
    std::string nnue = "null", eval = "null";

    if (!pos.checkers())
    {
        tables->accumulators.reset();
        nnue = std::to_string(network.evaluate(pos, tables->accumulators, tables->caches));
        eval = std::to_string(
          Eval::evaluate(network, pos, tables->accumulators, tables->caches, VALUE_ZERO));
    }

    append_key(out, "nnue");
    out += nnue;
    append_key(out, "eval");
    out += eval;

    // Legal moves in generation order, with the properties of each one
    const MoveList<ALL> moves(pos);

    append_list(out, "moves", moves, appendMove);
    append_list(out, "see", moves, [&](Move m) { out += std::to_string(see_value(pos, m)); });
    append_list(out, "gives_check", moves, [&](Move m) { out += pos.gives_check(m) ? '1' : '0'; });
    append_list(out, "key_after", moves, [&](Move m) {
        StateInfo newSt;
        pos.do_move(m, newSt);
        append_hex(out, pos.key());
        pos.undo_move(m);
    });

    // Move ordering of the move picker without a TT move: in the main search
    // at two depths, and in the quiescence search.
    const PieceToHistory* contHist[] = {&tables->continuationHistory, &tables->continuationHistory,
                                        &tables->continuationHistory, &tables->continuationHistory,
                                        &tables->continuationHistory, &tables->continuationHistory};

    const auto appendOrder = [&](std::string_view name, Depth depth) {
        MovePicker mp(pos, Move::none(), depth, &tables->mainHistory, &tables->lowPlyHistory,
                      &tables->captureHistory, contHist, &tables->sharedHistory, 0);

        append_key(out, name);
        out += '[';
        for (Move m = mp.next_move(); m != Move::none(); m = mp.next_move())
        {
            if (out.back() != '[')
                out += ',';
            appendMove(m);
        }
        out += ']';
    };

    appendOrder("order_main_shallow", ShallowDepth);
    appendOrder("order_main_deep", DeepDepth);
    appendOrder("order_qsearch", DEPTH_QS);

    return out += '}';
}

}  // namespace Stockfish

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

#include <algorithm>
#include <cassert>
#include <deque>
#include <initializer_list>
#include <optional>
#include <sstream>
#include <string_view>
#include <utility>
#include <vector>

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

constexpr std::string_view PieceToChar(" PNBRQK  pnbrqk");

// The hash keys kept by Position, as in the output of 'dump'
void append_keys(std::string& out, const Position& pos) {
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
}

// The bitboards that StateInfo caches for the position, as in the output of
// 'dump'. The threats are computed lazily, on first use.
void append_cached_bitboards(std::string& out, const Position& pos) {
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
}

// Splits the arguments of a 'position' command into a FEN and moves. Returns
// false if they are malformed.
bool parse_line(const std::string& line, std::string& fen, std::vector<std::string>& moves) {
    std::istringstream is(line);
    std::string        token;

    if (!(is >> token))
        return false;

    if (token == "startpos")
    {
        fen = StartFEN;
        if (is >> token && token != "moves")
            return false;
    }
    else if (token == "fen")
        while (is >> token && token != "moves")
            fen += token + " ";
    else
        return false;

    while (is >> token)
        moves.push_back(token);

    return true;
}

// The probe move of a position: a legal move other than the one of the line,
// made and undone before the line goes on. The moves are sorted by their UCI
// notation, so that the choice does not depend on the order of move generation,
// and grouped in classes: special moves (castling, en passant, promotions),
// other captures, other king moves, and all moves. Depending on the ply, the
// first non-empty class of a list of preferences is chosen, then a move of
// that class, cycling through them along the line. The move 'excluded', the
// next one of the line, is never chosen.
Move probe_move(const Position& pos, int ply, Move excluded = Move::none()) {

    enum {
        Special,
        Capture,
        King,
        Any
    };

    std::vector<std::pair<std::string, Move>> classes[4];

    for (Move m : MoveList<ALL>(pos))
    {
        if (m == excluded)
            continue;

        auto entry = std::make_pair(UCIEngine::move(m, pos.is_chess960()), m);

        if (m.type_of() != NORMAL)
            classes[Special].push_back(entry);
        else if (pos.capture(m))
            classes[Capture].push_back(entry);
        else if (type_of(pos.moved_piece(m)) == KING)
            classes[King].push_back(entry);
        classes[Any].push_back(entry);
    }

    constexpr int Preferences[4][3] = {
      {Special, Capture, Any}, {King, Capture, Any}, {Capture, Special, Any}, {Any, Any, Any}};

    for (int c : Preferences[ply % 4])
        if (auto& moves = classes[c]; !moves.empty())
        {
            std::sort(moves.begin(), moves.end(),
                      [](const auto& a, const auto& b) { return a.first < b.first; });
            return moves[(ply / 4) % moves.size()].second;
        }

    return Move::none();
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


// The NNUE state of a line, driven as Search::Worker drives its own
struct LineDumper::Tables {

    explicit Tables(const Eval::NNUE::Network& net) :
        caches(net) {}

    Eval::NNUE::AccumulatorStack  accumulators;
    Eval::NNUE::AccumulatorCaches caches;
};

LineDumper::LineDumper(const Eval::NNUE::Network& net) :
    network(net),
    tables(std::make_unique<Tables>(net)) {}

LineDumper::~LineDumper() = default;

void LineDumper::dump(const std::string& line, bool isChess960, std::string& out) {

    std::string              fen;
    std::vector<std::string> tokens;
    std::vector<Move>        moves;

    const auto appendError = [&](const std::string& reason) {
        out += '{';
        append_key(out, "error");
        append_string(out, reason);
        append_key(out, "input");
        append_string(out, line);
        out += "}\n";
    };

    // Check the whole line first, so that a line that cannot be played gives a
    // single error object rather than a truncated dump.
    {
        std::deque<StateInfo> states(1);
        Position              pos;

        if (!parse_line(line, fen, tokens))
        {
            appendError("Malformed line, expected startpos or fen, then moves");
            return;
        }
        if (auto err = pos.set(fen, isChess960, &states.back()))
        {
            appendError(err->what());
            return;
        }
        for (const auto& token : tokens)
        {
            Move m = UCIEngine::to_move(pos, token);
            if (m == Move::none())
            {
                appendError("Illegal move " + token + " at ply "
                            + std::to_string(moves.size() + 1));
                return;
            }
            moves.push_back(m);
            states.emplace_back();
            pos.do_move(m, states.back());
        }
    }

    auto& accumulators = tables->accumulators;
    auto& caches       = tables->caches;

    // Set up once, then only do_move(), undo_move() and their null move kin
    std::deque<StateInfo> states(1);
    Position              pos;
    pos.set(fen, isChess960, &states.back());
    caches.clear(network);

    const auto appendMove = [&](Move m) { append_string(out, UCIEngine::move(m, isChess960)); };

    // The output of the network from the current accumulators, null in check
    const auto appendNnue = [&](bool evaluate) {
        if (evaluate && !pos.checkers())
            out += std::to_string(network.evaluate(pos, accumulators, caches));
        else
            out += "null";
    };

    const auto appendCaptured = [&]() {
        if (Piece pc = pos.captured_piece())
            append_string(out, std::string(1, PieceToChar[pc]));
        else
            out += "null";
    };

    // A move made, evaluated and undone with an accumulator push and pop, as
    // search does with the children of a node
    const auto appendChild = [&](Move m) {
        StateInfo childSt;
        auto&     dirties = accumulators.push(pos);
        pos.do_move(m, childSt, pos.gives_check(m), dirties);
        out += '{';
        append_key(out, "move");
        appendMove(m);
        append_keys(out, pos);
        append_key(out, "checkers");
        append_hex(out, pos.checkers());
        append_key(out, "plies_from_null");
        out += std::to_string(pos.state()->pliesFromNull);
        append_key(out, "repetition");
        out += std::to_string(pos.state()->repetition);
        append_key(out, "nnue");
        appendNnue(true);
        out += '}';
        pos.undo_move(m);
        accumulators.pop();
    };

    const int lineSize = int(moves.size());
    int       root     = 0;

    for (int ply = 0;; ++ply)
    {
        // A new search would start here
        if (ply % Window == 0)
        {
            accumulators.reset();
            root = ply;
        }

        const StateInfo* st       = pos.state();
        const bool       evaluate = !skipped(ply);

        out += '{';
        append_key(out, "ply");
        out += std::to_string(ply);
        append_key(out, "move");
        if (ply)
            appendMove(moves[ply - 1]);
        else
            out += "null";
        append_key(out, "fen");
        append_string(out, pos.fen());
        append_keys(out, pos);
        append_key(out, "non_pawn_material");
        (((out += '[') += std::to_string(pos.non_pawn_material(WHITE))) += ',') +=
          std::to_string(pos.non_pawn_material(BLACK));
        out += ']';
        append_cached_bitboards(out, pos);
        append_key(out, "captured");
        appendCaptured();

        // Repetition and rule50 bookkeeping, the ply being counted from the
        // root of the current search as in the search itself
        append_key(out, "plies_from_null");
        out += std::to_string(st->pliesFromNull);
        append_key(out, "repetition");
        out += std::to_string(st->repetition);
        append_key(out, "is_draw");
        out += pos.is_draw(ply - root) ? "true" : "false";
        append_key(out, "has_repeated");
        out += pos.has_repeated() ? "true" : "false";
        append_key(out, "upcoming_repetition");
        out += pos.upcoming_repetition(ply - root) ? "true" : "false";

        // Evaluation from the incrementally updated accumulators
        append_key(out, "nnue");
        appendNnue(evaluate);
        append_key(out, "eval");
        if (evaluate && !pos.checkers())
            out += std::to_string(Eval::evaluate(network, pos, accumulators, caches, VALUE_ZERO));
        else
            out += "null";

        // A null move, as in null move pruning: no accumulator is pushed. Then
        // a reply to it, as the null move search does.
        append_key(out, "null");
        if (evaluate && !pos.checkers())
        {
            StateInfo nullSt;
            pos.do_null_move(nullSt);
            out += '{';
            append_key(out, "key");
            append_hex(out, pos.key());
            append_key(out, "fen");
            append_string(out, pos.fen());
            append_key(out, "plies_from_null");
            out += std::to_string(pos.state()->pliesFromNull);
            append_key(out, "repetition");
            out += std::to_string(pos.state()->repetition);
            append_key(out, "captured");
            appendCaptured();
            append_key(out, "nnue");
            appendNnue(true);
            append_key(out, "reply");
            if (Move reply = probe_move(pos, ply))
                appendChild(reply);
            else
                out += "null";
            out += '}';
            pos.undo_null_move();
        }
        else
            out += "null";

        // A move on the side, made and undone as search does with its children
        append_key(out, "probe");
        const Move next = ply < lineSize ? moves[ply] : Move::none();
        if (Move probe = evaluate ? probe_move(pos, ply, next) : Move::none())
            appendChild(probe);
        else
            out += "null";

        out += "}\n";

        if (ply == lineSize)
            break;

        auto& dirties = accumulators.push(pos);
        states.emplace_back();
        pos.do_move(moves[ply], states.back(), pos.gives_check(moves[ply]), dirties);
    }

    // Walk back down the line. The positions of the current search window are
    // evaluated on the way, which reuses the accumulators computed on the way
    // up, including those computed backwards after a refresh.
    std::string keys, fens, nnues;

    for (int ply = lineSize; ply > 0; --ply)
    {
        pos.undo_move(moves[ply - 1]);
        const bool inWindow = ply > root;
        if (inWindow)
            accumulators.pop();

        keys += keys.empty() ? '[' : ',';
        append_hex(keys, pos.key());
        fens += fens.empty() ? '[' : ',';
        append_string(fens, pos.fen());
        nnues += nnues.empty() ? '[' : ',';
        if (inWindow && !pos.checkers())
            nnues += std::to_string(network.evaluate(pos, accumulators, caches));
        else
            nnues += "null";
    }

    out += '{';
    append_key(out, "undo");
    out += std::to_string(lineSize);
    append_key(out, "keys");
    out += keys.empty() ? "[]" : keys + ']';
    append_key(out, "fens");
    out += fens.empty() ? "[]" : fens + ']';
    append_key(out, "nnues");
    out += nnues.empty() ? "[]" : nnues + ']';
    append_key(out, "fen");
    append_string(out, pos.fen());
    append_keys(out, pos);
    out += "}\n";
}

}  // namespace Stockfish

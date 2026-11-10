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

#ifndef DUMP_H_INCLUDED
#define DUMP_H_INCLUDED

#include <memory>
#include <string>

#include "nnue/network.h"
#include "types.h"

namespace Stockfish {

// PositionDumper backs the 'dump' debug command. It prints the engine internals
// of a position (hash keys, check and pin bitboards, static evaluation, legal
// moves with their SEE value, and the move ordering of the move picker) as one
// line of JSON, so that another implementation can be compared against this one
// component by component. See tests/differential/README.md for the format.
//
// It owns its history tables and NNUE scratch state, and it only ever works on
// a private copy of the position, hence it cannot influence the search.
class PositionDumper {
   public:
    explicit PositionDumper(const Eval::NNUE::Network& network);
    ~PositionDumper();

    PositionDumper(const PositionDumper&)            = delete;
    PositionDumper& operator=(const PositionDumper&) = delete;

    // Returns the dump of the given position as a single line, without the
    // trailing newline. An invalid FEN yields an object with an "error" member.
    std::string dump(const std::string& fen, bool isChess960);

    // The two depths at which the main search move ordering is recorded. The
    // depth sets the limit below which quiet moves are left unsorted, so that
    // the first one leaves most of them unsorted and the second one sorts all.
    static constexpr Depth ShallowDepth = 1;
    static constexpr Depth DeepDepth    = 16;

   private:
    struct Tables;

    const Eval::NNUE::Network& network;
    std::unique_ptr<Tables>    tables;
};

// LineDumper backs the 'dumpline' debug command. Where PositionDumper sets
// every position up from scratch, it sets up the start position of a line of
// play once, and then makes the moves of the line on that same position with
// do_move(), driving the NNUE accumulator stack and refresh cache the way the
// search does. It prints one line of JSON per position of the line, with the
// state reached incrementally: hash keys, rule50 and repetition bookkeeping, the
// evaluation, and the effect of a null move and of a probe move on the side, both
// undone before the line goes on. A last line walks back down the line with
// undo_move(). See tests/differential/README.md for the format.
class LineDumper {
   public:
    explicit LineDumper(const Eval::NNUE::Network& network);
    ~LineDumper();

    LineDumper(const LineDumper&)            = delete;
    LineDumper& operator=(const LineDumper&) = delete;

    // Appends the dump of a line, given as the arguments of the UCI 'position'
    // command ("startpos moves e2e4 ..." or "fen <fen> moves ..."), to 'out':
    // one line per position of the line, then the walk back, each line followed
    // by a newline. A line that cannot be set up or played yields a single
    // object with an "error" member, and nothing else.
    void dump(const std::string& line, bool isChess960, std::string& out);

    // The accumulator stack is reset every Window plies, as at the root of
    // each search of a game; the refresh cache is only cleared between lines.
    static constexpr int Window = 64;

    // Whether the position at a given ply of the line is skipped: neither
    // evaluated nor probed, so that the accumulator updates of several moves
    // are chained, as when search does not evaluate a node.
    static constexpr bool skipped(int ply) { return ply % 8 == 3 || ply % 8 >= 6; }

   private:
    struct Tables;

    const Eval::NNUE::Network& network;
    std::unique_ptr<Tables>    tables;
};

}  // namespace Stockfish

#endif  // #ifndef DUMP_H_INCLUDED

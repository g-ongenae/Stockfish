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

#ifndef SEARCHTRACE_H_INCLUDED
#define SEARCHTRACE_H_INCLUDED

// The search trace records what a single-threaded search does, node by node,
// as a stream of events, so that another implementation of the search can be
// compared with this one decision by decision. See tests/differential/README.md.
//
// It only exists in the tracing binary, built with 'make trace-build', which
// defines SEARCH_TRACE. In any other build the TRACE macros expand to empty
// statements, so that the search compiles to exactly the same code.

#ifdef SEARCH_TRACE

    // Emits an event from a member function of Search::Worker
    #define TRACE(call) \
        do \
        { \
            if (tracer) \
                tracer->call; \
        } while (false)

    // Emits an event from elsewhere, given the worker
    #define TRACE_OF(worker, call) \
        do \
        { \
            if ((worker).tracer) \
                (worker).tracer->call; \
        } while (false)

    #include <cstdint>
    #include <iosfwd>
    #include <map>
    #include <string>
    #include <vector>

    #include "types.h"

namespace Stockfish {

class MovePicker;
class Position;
class TranspositionTable;
struct TTData;
struct TTWriter;

namespace Search {
class Worker;
struct Stack;
}

// The digest of the event stream and of the state left behind by a search:
// 64-bit FNV-1a, over bytes for the events and over 64-bit words for the tables.
struct TraceDigest {
    static constexpr u64 Offset = 0xcbf29ce484222325ULL;
    static constexpr u64 Prime  = 0x100000001b3ULL;

    u64 value = Offset;

    void bytes(const char* data, usize size) {
        for (usize i = 0; i < size; ++i)
            value = (value ^ u8(data[i])) * Prime;
    }
    void word(u64 w) { value = (value ^ w) * Prime; }
};

// The events of one search of each session to print, from the 'events' argument
// of the 'searchtrace' command: those of index [from, to) of search number
// 'search' of each session (counted from 1, 0 for none, -1 for all)
struct SearchTraceWindow {
    int  search = 0;
    u64  from = 0, to = 0;
    bool counts = false;  // print the number of events of each kind at the end
};

// One iteration of iterative deepening, as recorded at its end
struct TraceIteration {
    int               depth, selDepth;
    bool              completed;
    Value             score, uciScore;
    std::string       bound;
    u64               nodes;
    std::vector<Move> pv;
};

// SearchTracer receives the events of a search. Each event is formatted as one
// line of text, a name followed by key=value fields, and fed to a running
// digest; a checkpoint of the digest is kept every 'interval' events. The
// events whose index lies in a given window are also printed, preceded by the
// nodes open at the start of the window.
class SearchTracer {
   public:
    SearchTracer(bool chess960, u64 interval);

    // Prints the events of index [from, to) of the next searches on 'out'
    void set_window(u64 from, u64 to, std::ostream* out);

    // Counts the events by kind, for statistics on what a corpus covers
    void                              count_kinds() { counting = true; }
    const std::map<std::string, u64>& kinds() const { return kindCounts; }

    void begin_search();

    u64                     events() const { return count; }
    u64                     digest() const { return running.value; }
    const std::vector<u64>& checkpoints() const { return saved; }

    std::vector<TraceIteration> iterations;

    // Iterative deepening
    void start(u8 generation, usize rootMoves);
    void iteration(Depth rootDepth);
    void aspiration(
      Depth depth, Value alpha, Value beta, int delta, Value avg, Value optimism, bool seekMate);
    void aspiration_result(Value bestValue, Value alpha, Value beta, bool stopped);
    void iteration_end(const Search::Worker& w, Depth rootDepth, bool stopped);

    // Node entry and exit, from the wrappers of search() and qsearch()
    void enter_search(const Position&      pos,
                      const Search::Stack* ss,
                      Value                alpha,
                      Value                beta,
                      Depth                depth,
                      int                  nodeType,
                      bool                 cutNode,
                      u64                  nodes);
    void enter_qsearch(const Position&      pos,
                       const Search::Stack* ss,
                       Value                alpha,
                       Value                beta,
                       bool                 pvNode,
                       u64                  nodes);
    void leave(Value value);

    // Reads
    void repetition_draw(Value alpha);
    void immediate(const char* reason);
    void tt(const TranspositionTable& table,
            const TTWriter&           writer,
            bool                      hit,
            const TTData&             adjusted,
            bool                      ttPv,
            bool                      ttCapture);
    void tt(const TranspositionTable& table,
            const TTWriter&           writer,
            bool                      hit,
            const TTData&             adjusted,
            bool                      ttPv);
    void static_eval(
      Value raw, Value corrected, Value eval, int correction, bool improving, bool worsening);
    void qsearch_eval(Value raw, Value corrected, Value standPat, int correction);
    void qsearch_eval_in_check();

    // Decisions
    void event(const char* name);
    void event(const char* name, const char* key, i64 value);
    void event(const char* name, const char* key1, i64 value1, const char* key2, i64 value2);
    void event(const char* name,
               const char* key1,
               i64         value1,
               const char* key2,
               i64         value2,
               const char* key3,
               i64         value3);
    void tt_cutoff(bool checked);
    void null_move(Depth reduction);
    void probcut_move(Move m);
    void pick(const MovePicker& mp, Move m);
    void skip(Move m, const char* reason);
    void try_move(Move m, int moveCount, bool capture, bool givesCheck, int reduction);
    void try_qmove(Move m, int moveCount, bool capture, bool givesCheck);
    void prune(const char* reason);
    void prune(const char* reason, const char* key, i64 value);
    void prune(const char* reason, const char* key1, i64 value1, const char* key2, i64 value2);
    void move_value(Move m, Value value);
    void root_move(Move  m,
                   Value score,
                   Value uciScore,
                   Value average,
                   i64   meanSquared,
                   int   selDepth,
                   bool  lower,
                   bool  upper);

    // Writes
    void store(const char* name, const TranspositionTable& table, const TTWriter& writer);
    void quiet_update(
      const Search::Worker& w, const Position& pos, const Search::Stack* ss, Move m, int bonus);
    void continuation_update(const Search::Stack* ss, Piece pc, Square to, int bonus);
    void capture_update(const Search::Worker& w, const Position& pos, Move m, int bonus);
    void correction_update(const Search::Worker& w,
                           const Position&       pos,
                           const Search::Stack*  ss,
                           int                   bonus);
    void
    eval_diff_update(const Search::Worker& w, const Position& pos, Move m, int bonus, bool pawn);
    void
    prior_quiet_update(const Search::Worker& w, const Position& pos, Move m, int bonus, bool pawn);
    void prior_capture_update(const Search::Worker& w, const Position& pos, Square prevSq);
    void ttmove_update(const Search::Worker& w, int bonus);

    // Digests of the tables a search leaves behind, as a JSON object. The low ply
    // history is filled at the start of each search and not cleared between games:
    // it is left out (null) when no search ran, as its content then comes from
    // whatever search ran before in the process.
    static std::string
    state_digests(const Search::Worker& w, const TranspositionTable& table, bool searched);

   private:
    struct Frame {
        std::string entry, move, call;
    };

    SearchTracer& begin(const char* name);
    SearchTracer& put(const char* key, i64 value);
    SearchTracer& put(const char* key, Move m);
    SearchTracer& put(const char* key, const std::string& text);
    SearchTracer& put_key(const char* key, u64 k);
    void          commit(bool isMove = false);
    std::string   move_str(Move m) const;
    void          slot(const TranspositionTable& table, const TTWriter& writer);

    bool             chess960;
    u64              interval;
    u64              count = 0;
    TraceDigest      running;
    std::vector<u64> saved;
    std::string      line;

    // Printing of a window of events, with the stack of open nodes as context
    std::ostream*      out        = nullptr;
    u64                windowFrom = 0, windowTo = 0;
    std::vector<Frame> frames;
    std::string        last;

    bool                       counting = false;
    std::map<std::string, u64> kindCounts;
};

}  // namespace Stockfish

#else

    #define TRACE(call) \
        do \
        { \
        } while (false)
    #define TRACE_OF(worker, call) \
        do \
        { \
        } while (false)

#endif  // #ifdef SEARCH_TRACE

#endif  // #ifndef SEARCHTRACE_H_INCLUDED

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

// Only compiled in the tracing binary, see searchtrace.h

#include "searchtrace.h"

#include <charconv>
#include <cstring>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string_view>

#include "engine.h"
#include "history.h"
#include "misc.h"
#include "movepick.h"
#include "position.h"
#include "search.h"
#include "tt.h"
#include "uci.h"

namespace Stockfish {

namespace {

constexpr std::string_view PieceToChar(" PNBRQK  pnbrqk");

std::string hex64(u64 v) {
    char buf[17];
    for (int i = 15; i >= 0; --i, v >>= 4)
        buf[i] = "0123456789ABCDEF"[v & 0xF];
    buf[16] = '\0';
    return buf;
}

const char* bound_name(Bound b) {
    return b == BOUND_EXACT ? "exact"
         : b == BOUND_LOWER ? "lower"
         : b == BOUND_UPPER ? "upper"
                            : "none";
}

// Digest of a table of 16-bit entries laid out in row-major order: the entries
// are read as little-endian 64-bit words of 4 entries each.
template<typename Table>
u64 digest_i16(const Table& table) {
    static_assert(sizeof(Table) % 8 == 0);
    TraceDigest d;
    const char* bytes = reinterpret_cast<const char*>(&table);
    for (usize i = 0; i < sizeof(Table); i += 8)
    {
        u64 w = 0;
        if (IsLittleEndian)
            std::memcpy(&w, bytes + i, 8);
        else
            for (int j = 0; j < 4; ++j)
            {
                i16 v;
                std::memcpy(&v, bytes + i + 2 * j, 2);
                w |= u64(u16(v)) << (16 * j);
            }
        d.word(w);
    }
    return d.value;
}

template<typename Dyn>
u64 digest_dyn(const Dyn& table) {
    TraceDigest d;
    for (usize i = 0; i < table.get_size(); ++i)
        d.word(digest_i16(table[i]));
    return d.value;
}

}  // namespace

SearchTracer::SearchTracer(bool c960, u64 iv) :
    chess960(c960),
    interval(iv) {
    line.reserve(256);
}

void SearchTracer::set_window(u64 f, u64 t, std::ostream* o) {
    windowFrom = f;
    windowTo   = t;
    out        = o;
}

void SearchTracer::begin_search() {
    count   = 0;
    running = TraceDigest();
    saved.clear();
    frames.clear();
    last.clear();
    iterations.clear();
}

std::string SearchTracer::move_str(Move m) const {
    return m == Move::none() ? "-" : m == Move::null() ? "0000" : UCIEngine::move(m, chess960);
}

SearchTracer& SearchTracer::begin(const char* name) {
    line.assign(name);
    return *this;
}

SearchTracer& SearchTracer::put(const char* key, i64 value) {
    char buf[24];
    auto res = std::to_chars(buf, buf + sizeof(buf), value);
    line += ' ';
    line += key;
    line += '=';
    line.append(buf, res.ptr);
    return *this;
}

SearchTracer& SearchTracer::put(const char* key, Move m) { return put(key, move_str(m)); }

SearchTracer& SearchTracer::put(const char* key, const std::string& text) {
    line += ' ';
    line += key;
    line += '=';
    line += text;
    return *this;
}

SearchTracer& SearchTracer::put_key(const char* key, u64 k) { return put(key, hex64(k)); }

// Feeds the line to the digest, keeps a checkpoint every 'interval' events, and
// prints the line if it lies in the window, preceded by the open nodes the first time.
void SearchTracer::commit(bool isMove) {
    running.bytes(line.data(), line.size());
    running.bytes("\n", 1);

    // The kind of an event is its name, and for some the value of the first field
    if (counting)
    {
        usize end = line.find(' ');
        if (line.compare(0, end, "prune") == 0 || line.compare(0, end, "immediate") == 0
            || line.compare(0, end, "pick") == 0 || line.compare(0, end, "skip") == 0)
        {
            const usize field = line.compare(0, end, "pick") == 0 ? line.find(" stage=")
                              : line.compare(0, end, "skip") == 0 ? line.find(" reason=")
                                                                  : end;
            const usize eq    = line.find('=', field);
            const usize stop  = line.find(' ', eq);
            ++kindCounts[line.substr(0, end) + ":" + line.substr(eq + 1, stop - eq - 1)];
        }
        else
            ++kindCounts[line.substr(0, end)];
    }

    if (out && count < windowTo)
    {
        std::string indexed = std::to_string(count) + " " + line;
        if (count >= windowFrom)
        {
            if (count == windowFrom)
                for (const Frame& f : frames)
                    for (const std::string* s : {&f.entry, &f.move, &f.call})
                        if (!s->empty())
                            *out << "context " << *s << '\n';
            *out << indexed << '\n';
        }
        if (isMove && !frames.empty())
            frames.back().move = indexed;
        last = std::move(indexed);
    }

    if (++count % interval == 0)
        saved.push_back(running.value);
}

void SearchTracer::slot(const TranspositionTable& table, const TTWriter& writer) {
    auto e = table.trace_entry(writer);
    line += " slot=" + std::to_string(e.cluster) + ":" + std::to_string(e.slot);
    char buf[8];
    std::snprintf(buf, sizeof(buf), "%06X", unsigned(e.key21));
    put("key21", std::string(buf))
      .put("move", e.data.move)
      .put("value", e.data.value)
      .put("eval", e.data.eval)
      .put("depth", e.data.depth)
      .put("bound", std::string(bound_name(e.data.bound)))
      .put("pv", e.data.is_pv)
      .put("generation", e.generation);
}

// Iterative deepening

void SearchTracer::start(u8 generation, usize rootMoves) {
    begin("start").put("generation", generation).put("root_moves", i64(rootMoves)).commit();
}

void SearchTracer::iteration(Depth rootDepth) {
    begin("iteration").put("depth", rootDepth).commit();
}

void SearchTracer::aspiration(
  Depth depth, Value alpha, Value beta, int delta, Value avg, Value optimism, bool seekMate) {
    begin("aspiration")
      .put("depth", depth)
      .put("alpha", alpha)
      .put("beta", beta)
      .put("delta", delta)
      .put("average", avg)
      .put("optimism", optimism)
      .put("seek_mate", seekMate)
      .commit();
}

void SearchTracer::aspiration_result(Value bestValue, Value alpha, Value beta, bool stopped) {
    begin("aspiration_result")
      .put("value", bestValue)
      .put("fail", std::string(bestValue <= alpha  ? "low"
                               : bestValue >= beta ? "high"
                                                   : "none"))
      .put("stopped", stopped)
      .commit();
}

void SearchTracer::iteration_end(const Search::Worker& w, Depth rootDepth, bool stopped) {
    const auto& rm = w.rootMoves[0];

    TraceIteration it{rootDepth,   rm.selDepth,
                      !stopped,    rm.score,
                      rm.uciScore, rm.inexactLower ? "lower" : rm.inexactUpper ? "upper" : "exact",
                      w.nodes,     {rm.pv.begin(), rm.pv.end()}};

    std::string pv;
    for (Move m : it.pv)
        pv += (pv.empty() ? "" : ",") + move_str(m);

    begin("iteration_end")
      .put("depth", rootDepth)
      .put("completed", it.completed)
      .put("score", it.score)
      .put("uci_score", it.uciScore)
      .put("bound", it.bound)
      .put("seldepth", it.selDepth)
      .put("nodes", i64(it.nodes))
      .put("pv", pv)
      .commit();

    iterations.push_back(std::move(it));
}

// Nodes

void SearchTracer::enter_search(const Position&      pos,
                                const Search::Stack* ss,
                                Value                alpha,
                                Value                beta,
                                Depth                depth,
                                int                  nodeType,
                                bool                 cutNode,
                                u64                  nodes) {
    begin("search")
      .put("ply", ss->ply)
      .put_key("key", pos.key())
      .put("depth", depth)
      .put("alpha", alpha)
      .put("beta", beta)
      .put("node", std::string(nodeType == Root ? "root"
                               : nodeType == PV ? "pv"
                                                : "nonpv"))
      .put("cut", cutNode)
      .put("excluded", ss->excludedMove)
      .put("nodes", i64(nodes));

    if (out && count < windowTo && !frames.empty() && frames.back().entry != last)
        frames.back().call = last;
    commit();
    if (out && count <= windowTo)
        frames.push_back({last, "", ""});
}

void SearchTracer::enter_qsearch(
  const Position& pos, const Search::Stack* ss, Value alpha, Value beta, bool pvNode, u64 nodes) {
    begin("qsearch")
      .put("ply", ss->ply)
      .put_key("key", pos.key())
      .put("alpha", alpha)
      .put("beta", beta)
      .put("node", std::string(pvNode ? "pv" : "nonpv"))
      .put("nodes", i64(nodes));

    if (out && count < windowTo && !frames.empty() && frames.back().entry != last)
        frames.back().call = last;
    commit();
    if (out && count <= windowTo)
        frames.push_back({last, "", ""});
}

void SearchTracer::leave(Value value) {
    begin("return").put("value", value).commit();
    if (out && !frames.empty())
        frames.pop_back();
}

// Reads

void SearchTracer::repetition_draw(Value alpha) {
    begin("repetition_draw").put("alpha", alpha).commit();
}

void SearchTracer::immediate(const char* reason) {
    begin("immediate").put("reason", std::string(reason)).commit();
}

void SearchTracer::tt(const TranspositionTable& table,
                      const TTWriter&           writer,
                      bool                      hit,
                      const TTData&             adjusted,
                      bool                      ttPv,
                      bool                      ttCapture) {
    begin("tt").put("hit", hit);
    slot(table, writer);
    put("tt_move", adjusted.move)
      .put("tt_value", adjusted.value)
      .put("tt_pv", ttPv)
      .put("tt_capture", ttCapture)
      .commit();
}

void SearchTracer::tt(const TranspositionTable& table,
                      const TTWriter&           writer,
                      bool                      hit,
                      const TTData&             adjusted,
                      bool                      ttPv) {
    begin("tt").put("hit", hit);
    slot(table, writer);
    put("tt_move", adjusted.move).put("tt_value", adjusted.value).put("tt_pv", ttPv).commit();
}

void SearchTracer::static_eval(
  Value raw, Value corrected, Value eval, int correction, bool improving, bool worsening) {
    begin("static_eval")
      .put("raw", raw)
      .put("static", corrected)
      .put("eval", eval)
      .put("correction", correction)
      .put("improving", improving)
      .put("worsening", worsening)
      .commit();
}

void SearchTracer::qsearch_eval(Value raw, Value corrected, Value standPat, int correction) {
    begin("static_eval")
      .put("raw", raw)
      .put("static", corrected)
      .put("stand_pat", standPat)
      .put("correction", correction)
      .commit();
}

void SearchTracer::qsearch_eval_in_check() { begin("static_eval").put("in_check", 1).commit(); }

// Decisions

void SearchTracer::event(const char* name) { begin(name).commit(); }

void SearchTracer::event(const char* name, const char* key, i64 value) {
    begin(name).put(key, value).commit();
}

void SearchTracer::event(
  const char* name, const char* key1, i64 value1, const char* key2, i64 value2) {
    begin(name).put(key1, value1).put(key2, value2).commit();
}

void SearchTracer::event(const char* name,
                         const char* key1,
                         i64         value1,
                         const char* key2,
                         i64         value2,
                         const char* key3,
                         i64         value3) {
    begin(name).put(key1, value1).put(key2, value2).put(key3, value3).commit();
}

void SearchTracer::tt_cutoff(bool checked) { begin("tt_cutoff").put("checked", checked).commit(); }

void SearchTracer::null_move(Depth reduction) {
    begin("null_move").put("reduction", reduction).commit(true);
}

void SearchTracer::probcut_move(Move m) { begin("probcut_move").put("move", m).commit(true); }

void SearchTracer::pick(const MovePicker& mp, Move m) {
    begin("pick").put("move", m).put("stage", std::string(mp.trace_stage()));
    if (!mp.trace_is_tt_move())
        put("score", mp.trace_score());
    commit();
}

void SearchTracer::skip(Move m, const char* reason) {
    begin("skip").put("move", m).put("reason", std::string(reason)).commit();
}

void SearchTracer::try_move(Move m, int moveCount, bool capture, bool givesCheck, int reduction) {
    begin("move")
      .put("move", m)
      .put("count", moveCount)
      .put("capture", capture)
      .put("check", givesCheck)
      .put("reduction", reduction)
      .commit(true);
}

void SearchTracer::try_qmove(Move m, int moveCount, bool capture, bool givesCheck) {
    begin("move")
      .put("move", m)
      .put("count", moveCount)
      .put("capture", capture)
      .put("check", givesCheck)
      .commit(true);
}

void SearchTracer::prune(const char* reason) {
    begin("prune").put("reason", std::string(reason)).commit();
}

void SearchTracer::prune(const char* reason, const char* key, i64 value) {
    begin("prune").put("reason", std::string(reason)).put(key, value).commit();
}

void SearchTracer::prune(
  const char* reason, const char* key1, i64 value1, const char* key2, i64 value2) {
    begin("prune").put("reason", std::string(reason)).put(key1, value1).put(key2, value2).commit();
}

void SearchTracer::move_value(Move m, Value value) {
    begin("value").put("move", m).put("value", value).commit();
}

void SearchTracer::root_move(Move  m,
                             Value score,
                             Value uciScore,
                             Value average,
                             i64   meanSquared,
                             int   selDepth,
                             bool  lower,
                             bool  upper) {
    begin("root_move")
      .put("move", m)
      .put("score", score)
      .put("uci_score", uciScore)
      .put("average", average)
      .put("mean_squared", meanSquared)
      .put("seldepth", selDepth)
      .put("bound", std::string(lower   ? "lower"
                                : upper ? "upper"
                                        : "exact"))
      .commit();
}

// Writes

void SearchTracer::store(const char*               name,
                         const TranspositionTable& table,
                         const TTWriter&           writer) {
    begin(name);
    slot(table, writer);
    commit();
}

namespace {

// The continuation history entries updated by update_continuation_histories()
std::string continuation_values(const Search::Stack* ss, Piece pc, Square to) {
    std::string s;
    for (int i : {1, 2, 3, 4, 6})
    {
        if (ss->inCheck && i > 2)
            break;
        s += s.empty() ? "" : ",";
        s += ((ss - i)->currentMove).is_ok()
             ? std::to_string(int((*(ss - i)->continuationHistory)[pc][to]))
             : "-";
    }
    return s;
}

}  // namespace

void SearchTracer::quiet_update(
  const Search::Worker& w, const Position& pos, const Search::Stack* ss, Move m, int bonus) {
    const Color  us = pos.side_to_move();
    const Piece  pc = pos.moved_piece(m);
    const Square to = m.to_sq();

    begin("quiet_update")
      .put("move", m)
      .put("bonus", bonus)
      .put("main", w.mainHistory[us][m.raw()]);
    if (ss->ply < LOW_PLY_HISTORY_SIZE)
        put("low_ply", w.lowPlyHistory[ss->ply][m.raw()]);
    put("continuation", continuation_values(ss, pc, to))
      .put("pawn", w.sharedHistory.pawn_entry(pos)[pc][to])
      .commit();
}

void SearchTracer::continuation_update(const Search::Stack* ss, Piece pc, Square to, int bonus) {
    begin("continuation_update")
      .put("piece", std::string(1, PieceToChar[pc]))
      .put("to", UCIEngine::square(to))
      .put("bonus", bonus)
      .put("values", continuation_values(ss, pc, to))
      .commit();
}

void SearchTracer::capture_update(const Search::Worker& w, const Position& pos, Move m, int bonus) {
    const Piece     pc       = pos.moved_piece(m);
    const PieceType captured = type_of(pos.piece_on(m.to_sq()));
    begin("capture_update")
      .put("move", m)
      .put("bonus", bonus)
      .put("value", w.captureHistory[pc][m.to_sq()][captured])
      .commit();
}

void SearchTracer::correction_update(const Search::Worker& w,
                                     const Position&       pos,
                                     const Search::Stack*  ss,
                                     int                   bonus) {
    const Color us     = pos.side_to_move();
    const auto& shared = w.sharedHistory;
    const Move  m      = (ss - 1)->currentMove;

    begin("correction_update")
      .put("bonus", bonus)
      .put("pawn", shared.pawn_correction_entry(pos)[us].pawn)
      .put("minor", shared.minor_piece_correction_entry(pos)[us].minor)
      .put("white", shared.nonpawn_correction_entry<WHITE>(pos)[us].nonPawnWhite)
      .put("black", shared.nonpawn_correction_entry<BLACK>(pos)[us].nonPawnBlack);
    if (m.is_ok())
    {
        const Square to = m.to_sq();
        const Piece  pc = pos.piece_on(to);
        put("continuation",
            std::to_string(int((*(ss - 2)->continuationCorrectionHistory)[pc][to])) + ","
              + std::to_string(int((*(ss - 4)->continuationCorrectionHistory)[pc][to])) + ","
              + std::to_string(int((*(ss - 6)->continuationCorrectionHistory)[pc][to])));
    }
    commit();
}

void SearchTracer::eval_diff_update(
  const Search::Worker& w, const Position& pos, Move m, int bonus, bool pawn) {
    const Square prevSq = m.to_sq();
    begin("eval_diff_update")
      .put("move", m)
      .put("bonus", bonus)
      .put("main", w.mainHistory[~pos.side_to_move()][m.raw()]);
    if (pawn)
        put("pawn", w.sharedHistory.pawn_entry(pos)[pos.piece_on(prevSq)][prevSq]);
    commit();
}

void SearchTracer::prior_quiet_update(
  const Search::Worker& w, const Position& pos, Move m, int bonus, bool pawn) {
    const Square prevSq = m.to_sq();
    begin("prior_quiet_update")
      .put("move", m)
      .put("bonus", bonus)
      .put("main", w.mainHistory[~pos.side_to_move()][m.raw()]);
    if (pawn)
        put("pawn", w.sharedHistory.pawn_entry(pos)[pos.piece_on(prevSq)][prevSq]);
    commit();
}

void SearchTracer::prior_capture_update(const Search::Worker& w,
                                        const Position&       pos,
                                        Square                prevSq) {
    begin("prior_capture_update")
      .put("to", UCIEngine::square(prevSq))
      .put("value", w.captureHistory[pos.piece_on(prevSq)][prevSq][type_of(pos.captured_piece())])
      .commit();
}

void SearchTracer::ttmove_update(const Search::Worker& w, int bonus) {
    begin("ttmove_update").put("bonus", bonus).put("value", w.ttMoveHistory).commit();
}

// The tables a search leaves behind for the next one

std::string SearchTracer::state_digests(const Search::Worker&     w,
                                        const TranspositionTable& table,
                                        bool                      searched) {
    const auto& shared = w.sharedHistory;

    TraceDigest continuation;
    for (const auto& inCheck : w.continuationHistory)
        for (const auto& captures : inCheck)
            continuation.word(digest_i16(captures));

    std::string s;
    auto        add = [&s](const char* name, const std::string& value, bool quote = true) {
        s += s.empty() ? "{" : ",";
        s += std::string("\"") + name + "\":" + (quote ? "\"" + value + "\"" : value);
    };

    add("main_history", hex64(digest_i16(w.mainHistory)));
    add("low_ply_history", searched ? hex64(digest_i16(w.lowPlyHistory)) : "null", searched);
    add("capture_history", hex64(digest_i16(w.captureHistory)));
    add("continuation_history", hex64(continuation.value));
    add("pawn_history", hex64(digest_dyn(shared.pawnHistory)));
    add("correction_history", hex64(digest_dyn(shared.correctionHistory)));
    add("continuation_correction_history", hex64(digest_i16(w.continuationCorrectionHistory)));
    add("ttmove_history", std::to_string(int(w.ttMoveHistory)), false);
    add("tt_generation", std::to_string(int(table.generation())), false);
    add("tt", hex64(table.trace_digest()));
    return s + "}";
}


// The 'searchtrace' command

namespace {

constexpr u64 CheckpointInterval = 4096;

std::string json_string(std::string_view s) {
    std::string r = "\"";
    for (char c : s)
    {
        if (c == '"' || c == '\\')
            r += '\\';
        if (u8(c) < 0x20)
            r += ' ';
        else
            r += c;
    }
    return r + "\"";
}

std::string trim(const std::string& s) {
    auto b = s.find_first_not_of(" \t\r\n");
    if (b == std::string::npos)
        return "";
    auto e = s.find_last_not_of(" \t\r\n");
    return s.substr(b, e - b + 1);
}

// One command of a session: a position to set up, or a search to run
struct TraceCommand {
    bool                     isGo;
    std::string              fen;
    std::vector<std::string> moves;
    Search::LimitsType       limits;
    std::string              text;
};

// Parses a session, "position ... ; go depth 5 ; position ... ; go nodes 1000",
// returns an error message, or an empty string.
std::string parse_session(const std::string& line, std::vector<TraceCommand>& commands) {
    std::istringstream all(line);
    std::string        part;
    bool               hasPosition = false;

    while (std::getline(all, part, ';'))
    {
        std::istringstream is(part);
        std::string        token;
        TraceCommand       cmd;
        cmd.text = trim(part);

        if (!(is >> token))
            return "Empty command";

        if (token == "position")
        {
            cmd.isGo = false;
            is >> token;
            if (token == "startpos")
            {
                cmd.fen = StartFEN;
                is >> token;
            }
            else if (token == "fen")
                while (is >> token && token != "moves")
                    cmd.fen += token + " ";
            else
                return "Expected startpos or fen after position";

            if (!is.fail() && token != "moves")
                return "Expected moves after the position";

            while (is >> token)
                cmd.moves.push_back(token);
            hasPosition = true;
        }
        else if (token == "go")
        {
            if (!hasPosition)
                return "go before any position";
            cmd.isGo = true;
            while (is >> token)
            {
                i64 n;
                if ((token != "depth" && token != "nodes") || !(is >> n) || n <= 0)
                    return "Only 'go depth N' and 'go nodes N' are allowed";
                if (token == "depth")
                    cmd.limits.depth = int(std::min(n, i64(MAX_PLY)));
                else
                    cmd.limits.nodes = u64(n);
            }
            if (!cmd.limits.depth && !cmd.limits.nodes)
                return "go needs a depth or a nodes limit";

            // The limits as given back in the records, in a fixed form
            cmd.text.clear();
            if (cmd.limits.depth)
                cmd.text = "depth " + std::to_string(cmd.limits.depth);
            if (cmd.limits.nodes)
                cmd.text += (cmd.text.empty() ? "" : " ") + std::string("nodes ")
                          + std::to_string(cmd.limits.nodes);
        }
        else
            return "Unknown command " + token;

        commands.push_back(std::move(cmd));
    }

    if (commands.empty())
        return "Empty session";
    return "";
}

}  // namespace

bool Engine::search_trace(const std::filesystem::path& file, const SearchTraceWindow& window) {
    std::ifstream in(file);

    if (!in.is_open())
        return false;

    verify_network();

    // The options the trace depends on, fixed to a deterministic search
    for (const char* setting :
         {"Threads value 1", "Hash value 2", "MultiPV value 1", "Skill Level value 20",
          "UCI_LimitStrength value false", "UCI_ShowWDL value false", "SyzygyPath value <empty>",
          "nodestime value 0", "Ponder value false"})
    {
        std::istringstream is(std::string("name ") + setting);
        options.setoption(is);
    }

    const bool   isChess960 = options["UCI_Chess960"];
    SearchTracer tracer(isChess960, CheckpointInterval);
    if (window.counts)
        tracer.count_kinds();

    std::vector<std::string> infos;
    std::string              bestmove, ponder;

    const auto savedContext       = updateContext;
    updateContext.onUpdateNoMoves = [&](const InfoShort& i) {
        infos.push_back("depth " + std::to_string(i.depth) + " score "
                        + UCIEngine::format_score(i.score));
    };
    updateContext.onUpdateFull = [&](const InfoFull& i) {
        std::string s = "depth " + std::to_string(i.depth) + " seldepth "
                      + std::to_string(i.selDepth) + " multipv " + std::to_string(i.multiPV)
                      + " score " + UCIEngine::format_score(i.score);
        if (!i.bound.empty())
            s += " " + std::string(i.bound);
        s += " nodes " + std::to_string(i.nodes) + " hashfull " + std::to_string(i.hashfull)
           + " tbhits " + std::to_string(i.tbHits) + " pv " + std::string(i.pv);
        infos.push_back(std::move(s));
    };
    updateContext.onIter     = [](const InfoIter&) {};
    updateContext.onStart    = []() {};
    updateContext.onBestmove = [&](std::string_view bm, std::string_view p) {
        bestmove = bm;
        ponder   = p;
    };

    Search::Worker& worker = *threads.main_thread()->worker;
    std::string     line;
    int             session = 0;

    while (std::getline(in, line))
    {
        line = trim(line);
        if (line.empty() || line[0] == '#')
            continue;
        ++session;

        std::vector<TraceCommand> commands;
        std::string               error = parse_session(line, commands);
        if (!error.empty())
        {
            sync_cout << "{\"session\":" << session << ",\"error\":" << json_string(error)
                      << ",\"input\":" << json_string(line) << "}" << sync_endl;
            continue;
        }

        // Each session starts from a cleared state, as after 'ucinewgame'
        search_clear();

        int search = 0;
        for (auto& cmd : commands)
        {
            if (!cmd.isGo)
            {
                if (auto err = set_position(cmd.fen, cmd.moves))
                {
                    sync_cout << "{\"session\":" << session << ",\"search\":" << search + 1
                              << ",\"error\":" << json_string(err->what())
                              << ",\"input\":" << json_string(cmd.text) << "}" << sync_endl;
                    break;
                }
                continue;
            }

            ++search;
            infos.clear();
            bestmove.clear();
            ponder.clear();

            tracer.begin_search();
            if (search == window.search || window.search == -1)
                tracer.set_window(window.from, window.to, &std::cout);
            else
                tracer.set_window(0, 0, nullptr);

            const std::string rootFen = pos.fen();
            cmd.limits.startTime      = now();

            worker.tracer = &tracer;
            go(cmd.limits);
            wait_for_search_finished();
            worker.tracer = nullptr;

            std::ostringstream r;
            r << "{\"session\":" << session << ",\"search\":" << search
              << ",\"fen\":" << json_string(rootFen) << ",\"go\":" << json_string(cmd.text)
              << ",\"info\":[";
            for (usize i = 0; i < infos.size(); ++i)
                r << (i ? "," : "") << json_string(infos[i]);
            r << "],\"bestmove\":" << json_string(bestmove)
              << ",\"ponder\":" << (ponder.empty() ? "null" : json_string(ponder))
              << ",\"nodes\":" << threads.nodes_searched() << ",\"iterations\":[";
            for (usize i = 0; i < tracer.iterations.size(); ++i)
            {
                const auto& it = tracer.iterations[i];
                std::string pv;
                for (Move m : it.pv)
                    pv += (pv.empty() ? "" : " ") + UCIEngine::move(m, isChess960);
                r << (i ? "," : "") << "{\"depth\":" << it.depth
                  << ",\"completed\":" << (it.completed ? "true" : "false")
                  << ",\"score\":" << it.score << ",\"uci_score\":" << it.uciScore
                  << ",\"bound\":\"" << it.bound << "\",\"seldepth\":" << it.selDepth
                  << ",\"nodes\":" << it.nodes << ",\"pv\":" << json_string(pv) << "}";
            }
            r << "],\"state\":" << SearchTracer::state_digests(worker, tt, tracer.events() > 0)
              << ",\"events\":" << tracer.events() << ",\"digest\":\"" << hex64(tracer.digest())
              << "\",\"checkpoints\":[";
            for (usize i = 0; i < tracer.checkpoints().size(); ++i)
                r << (i ? "," : "") << "\"" << hex64(tracer.checkpoints()[i]) << "\"";
            r << "]}";
            sync_cout << r.str() << sync_endl;
        }
    }
    updateContext = savedContext;

    if (window.counts)
    {
        std::string counts;
        for (const auto& [kind, n] : tracer.kinds())
            counts += (counts.empty() ? "" : ",") + json_string(kind) + ":" + std::to_string(n);
        sync_cout << "{\"counts\":{" << counts << "}}" << sync_endl;
    }

    // A read error must not pass for the end of the file
    return !in.bad();
}

}  // namespace Stockfish

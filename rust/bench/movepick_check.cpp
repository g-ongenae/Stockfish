// Drives the C++ move picker, Position::legal() and the history tables of
// src/ where the `dump` of tests/differential cannot (TT moves, ProbCut,
// skip_quiet_moves(), plies > 0, non-zero histories, the update operator),
// and prints digests of what they do. rust/examples/movepick_check.rs does
// exactly the same with the port, and must print exactly the same lines.
// See rust/README.md for how it is built (against the object files of a
// build of src/, with movepick.cpp compiled again with SEARCH_TRACE for the
// stage and score of each move) and run. Not part of the engine.
//
// Everything random comes from one splitmix64 stream, consumed in the same
// order on both sides (never two draws in one C++ expression, whose order of
// evaluation would be unspecified); the tables are filled with values that depend on the
// position of each entry in the C++ layout, so that a wrong index reads a
// wrong value. Keep the two drivers in step: any change here must be made
// there too.

#include <climits>
#include <cstdint>
#include <cstdio>
#include <ctime>
#include <fstream>
#include <iostream>
#include <memory>
#include <string>
#include <vector>

#include "attacks.h"
#include "bitboard.h"
#include "history.h"
#include "movegen.h"
#include "movepick.h"
#include "position.h"
#include "uci.h"

using namespace Stockfish;
using namespace Stockfish::Attacks;

namespace {

struct Rng {
    u64 s;
    explicit Rng(u64 seed) :
        s(seed) {}
    u64 next() {
        u64 z = (s += 0x9E3779B97F4A7C15ULL);
        z     = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
        z     = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
        return z ^ (z >> 31);
    }
    // uniform in [0, n)
    u32 below(u32 n) { return u32(((next() >> 32) * u64(n)) >> 32); }
    // uniform in [lo, hi]
    int range(int lo, int hi) { return lo + int(below(u32(hi - lo + 1))); }
};

u64 mix(u64 z) {
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
    return z ^ (z >> 31);
}

// The value of the entry of flat index i of table t, in [-d, d]
i16 fill_value(u64 t, u64 i, int d) {
    return i16(int(mix((t << 40) ^ i ^ 0x5851F42D4C957F2DULL) % u64(2 * d + 1)) - d);
}

// The same rounded to a multiple of 1000 (toward zero), for the coarse pass:
// sums of such values often fall exactly on the thresholds of the picker
// (-14000 for good quiets), and tie often, which tests the sort on ties
i16 coarse_value(u64 t, u64 i, int d) { return i16(fill_value(t, i, d) / 1000 * 1000); }

struct Digest {
    u64  h = 0xCBF29CE484222325ULL;
    void add(i64 v) {
        h = (h ^ u64(v)) * 0x100000001B3ULL;
        h ^= h >> 32;
    }
};

// A bonus for the update operator: mostly within a few D, sometimes huge
int random_bonus(Rng& rng, int d) {
    switch (rng.below(8))
    {
    case 0 :
        return int(u32(rng.next() >> 32));  // any int
    case 1 :
        return rng.range(-d / 8, d / 8);
    default :
        return rng.range(-3 * d, 3 * d);
    }
}

struct Tables {
    std::unique_ptr<ButterflyHistory>                mainHistory;
    std::unique_ptr<LowPlyHistory>                   lowPlyHistory;
    std::unique_ptr<CapturePieceToHistory>           captureHistory;
    std::unique_ptr<CorrectionHistory<Continuation>> contCorrHistory;
    std::unique_ptr<SharedHistories>                 shared;
    TTMoveHistory                                    ttMoveHistory;

    explicit Tables(usize threads) :
        mainHistory(std::make_unique<ButterflyHistory>()),
        lowPlyHistory(std::make_unique<LowPlyHistory>()),
        captureHistory(std::make_unique<CapturePieceToHistory>()),
        contCorrHistory(std::make_unique<CorrectionHistory<Continuation>>()),
        shared(std::make_unique<SharedHistories>(threads)) {}

    PieceToHistory& cont(int ic, int cap, int pc, int s) {
        return shared->continuationHistory()[ic][cap][pc][s];
    }

    // Every entry, in the order of the C++ layout, given to f with its table
    // number, flat index and bound
    template<typename F>
    void for_each(F&& f) {
        u64 i = 0;
        for (int c = 0; c < COLOR_NB; ++c)
            for (int m = 0; m < UINT_16_HISTORY_SIZE; ++m)
                f(1, i++, 7183, (*mainHistory)[c][m]);
        i = 0;
        for (int p = 0; p < LOW_PLY_HISTORY_SIZE; ++p)
            for (int m = 0; m < UINT_16_HISTORY_SIZE; ++m)
                f(2, i++, 7183, (*lowPlyHistory)[p][m]);
        i = 0;
        for (int pc = 0; pc < PIECE_NB; ++pc)
            for (int s = 0; s < SQUARE_NB; ++s)
                for (int t = 0; t < PIECE_TYPE_NB; ++t)
                    f(3, i++, 10692, (*captureHistory)[pc][s][t]);
        i = 0;
        for (int ic = 0; ic < 2; ++ic)
            for (int cap = 0; cap < 2; ++cap)
                for (int pc = 0; pc < PIECE_NB; ++pc)
                    for (int s = 0; s < SQUARE_NB; ++s)
                        for (int pc2 = 0; pc2 < PIECE_NB; ++pc2)
                            for (int s2 = 0; s2 < SQUARE_NB; ++s2)
                                f(4, i++, 30000, cont(ic, cap, pc, s)[pc2][s2]);
        i = 0;
        for (usize k = 0; k < shared->pawnHistory.get_size(); ++k)
            for (int pc = 0; pc < PIECE_NB; ++pc)
                for (int s = 0; s < SQUARE_NB; ++s)
                    f(5, i++, 8192, shared->pawnHistory[k][pc][s]);
        i = 0;
        for (usize k = 0; k < shared->correctionHistory.get_size(); ++k)
            for (int c = 0; c < COLOR_NB; ++c)
            {
                auto& b = shared->correctionHistory[k][c];
                f(6, i++, 1024, b.pawn);
                f(6, i++, 1024, b.minor);
                f(6, i++, 1024, b.nonPawnWhite);
                f(6, i++, 1024, b.nonPawnBlack);
            }
        i = 0;
        for (int pc = 0; pc < PIECE_NB; ++pc)
            for (int s = 0; s < SQUARE_NB; ++s)
                for (int pc2 = 0; pc2 < PIECE_NB; ++pc2)
                    for (int s2 = 0; s2 < SQUARE_NB; ++s2)
                        f(7, i++, 1024, (*contCorrHistory)[pc][s][pc2][s2]);
        f(8, 0, 8192, ttMoveHistory);
    }

    void fill() {
        for_each([](u64 t, u64 i, int d, auto& e) { e = fill_value(t, i, d); });
    }

    void fill_coarse() {
        for_each([](u64 t, u64 i, int d, auto& e) { e = coarse_value(t, i, d); });
    }

    // One digest per table, in order
    std::string digests() {
        Digest      dg[9];
        std::string out;
        for_each([&](u64 t, u64, int, auto& e) { dg[t].add(int(e)); });
        for (int t = 1; t <= 8; ++t)
        {
            char buf[32];
            std::snprintf(buf, sizeof buf, " %016llX", (unsigned long long) dg[t].h);
            out += buf;
        }
        return out;
    }

    // Random updates of random entries of every table, by the C++ operator<<
    void random_updates(Rng& rng, int n) {
        for (int k = 0; k < n; ++k)
        {
            int c = rng.below(COLOR_NB), m = rng.below(UINT_16_HISTORY_SIZE);
            (*mainHistory)[c][m] << random_bonus(rng, 7183);
            int p = rng.below(LOW_PLY_HISTORY_SIZE);
            m     = rng.below(UINT_16_HISTORY_SIZE);
            (*lowPlyHistory)[p][m] << random_bonus(rng, 7183);
            int pc = rng.below(PIECE_NB), s = rng.below(SQUARE_NB), t = rng.below(PIECE_TYPE_NB);
            (*captureHistory)[pc][s][t] << random_bonus(rng, 10692);
            int ic = rng.below(2), cap = rng.below(2), pc1 = rng.below(PIECE_NB),
                s1 = rng.below(SQUARE_NB), pc2 = rng.below(PIECE_NB), s2 = rng.below(SQUARE_NB);
            cont(ic, cap, pc1, s1)[pc2][s2] << random_bonus(rng, 30000);
            usize kk = rng.below(u32(shared->pawnHistory.get_size()));
            pc       = rng.below(PIECE_NB);
            s        = rng.below(SQUARE_NB);
            shared->pawnHistory[kk][pc][s] << random_bonus(rng, 8192);
            kk    = rng.below(u32(shared->correctionHistory.get_size()));
            c     = rng.below(COLOR_NB);
            auto& b = shared->correctionHistory[kk][c];
            switch (rng.below(4))
            {
            case 0 :
                b.pawn << random_bonus(rng, 1024);
                break;
            case 1 :
                b.minor << random_bonus(rng, 1024);
                break;
            case 2 :
                b.nonPawnWhite << random_bonus(rng, 1024);
                break;
            default :
                b.nonPawnBlack << random_bonus(rng, 1024);
            }
            pc1 = rng.below(PIECE_NB), s1 = rng.below(SQUARE_NB), pc2 = rng.below(PIECE_NB),
            s2 = rng.below(SQUARE_NB);
            (*contCorrHistory)[pc1][s1][pc2][s2] << random_bonus(rng, 1024);
            ttMoveHistory << random_bonus(rng, 8192);
        }
    }
};

// The update operator on every value of [-D, D] and a set of bonuses
template<typename Entry>
u64 update_digest(int d, Rng& rng) {
    std::vector<int> bonuses = {INT_MIN, INT_MIN + 1, -1000000000, -3 * d, -d - 1, -d, -d + 1,
                                -d / 2,  -1000,       -100,        -7,     -1,     0,  1,
                                7,       100,         1000,        d / 2,  d - 1,  d,  d + 1,
                                3 * d,   1000000000,  INT_MAX};
    for (int k = 0; k < 16; ++k)
        bonuses.push_back(random_bonus(rng, d));
    Digest dg;
    Entry  e;
    for (int v = -d; v <= d; ++v)
        for (int b : bonuses)
        {
            e = i16(v);
            e << b;
            dg.add(int(e));
        }
    return dg.h;
}

const char* stage_of(const MovePicker& mp) { return mp.trace_stage(); }

// A pseudo-legal but illegal move of a piece other than a pawn (pinned, or
// not answering a check, or a king move to an attacked square), found from a
// random start; none if there is none.
Move pseudo_legal_illegal(const Position& pos, Rng& rng) {
    u32 start = rng.below(4096);
    for (u32 k = 0; k < 4096; ++k)
    {
        u32    v    = (start + k) & 4095;
        Square from = Square(v >> 6), to = Square(v & 63);
        Piece  pc   = pos.piece_on(from);
        if (pc == NO_PIECE || color_of(pc) != pos.side_to_move() || type_of(pc) == PAWN)
            continue;
        if ((pos.pieces(pos.side_to_move()) & to) || !(attacks_bb(type_of(pc), from, pos.pieces()) & to))
            continue;
        Move m(from, to);
        if (!pos.legal(m))
            return m;
    }
    return Move::none();
}

struct Checker {
    Tables& t;
    bool    chess960;
    bool    verbose = false;

    void log(const std::string& s) {
        if (verbose)
            std::cerr << s << "\n";
    }

    // Runs one picker to its end, making and unmaking each move and updating
    // the histories as search does between two calls, with skip_quiet_moves()
    // called before the move of index skipAt (never if negative).
    void run(Position&          pos,
             MovePicker&        mp,
             const PieceToHistory** contHist,
             int                ply,
             int                skipAt,
             Rng&               rng,
             Digest&            dg) {
        int n = 0;
        for (;;)
        {
            if (n == skipAt)
            {
                mp.skip_quiet_moves();
                log("  skip_quiets");
            }
            Move m = mp.next_move();
            if (m == Move::none())
                break;
            bool tt    = mp.trace_is_tt_move();
            int  score = tt ? 0 : mp.trace_score();
            dg.add(m.raw());
            dg.add(tt);
            dg.add(score);
            for (const char* c = stage_of(mp); *c; ++c)
                dg.add(*c);
            log("  " + UCIEngine::move(m, chess960) + " " + stage_of(mp) + " "
                + (tt ? std::string("tt") : std::to_string(score)));
            ++n;

            StateInfo st;
            pos.do_move(m, st);
            pos.undo_move(m);

            // History updates between two calls, as search makes them
            if (rng.below(2))
            {
                const Color us = pos.side_to_move();
                const Piece pc = pos.moved_piece(m);
                const Square to = m.to_sq();
                (*t.mainHistory)[us][m.raw()] << random_bonus(rng, 7183);
                if (ply < LOW_PLY_HISTORY_SIZE)
                    (*t.lowPlyHistory)[ply][m.raw()] << random_bonus(rng, 7183);
                (*t.captureHistory)[pc][to][type_of(pos.piece_on(to))] << random_bonus(rng, 10692);
                // search writes through the same pointers as it gives the picker
                auto* ch = const_cast<PieceToHistory*>(contHist[rng.below(6)]);
                (*ch)[pc][to] << random_bonus(rng, 30000);
                t.shared->pawn_entry(pos)[pc][to] << random_bonus(rng, 8192);
            }
        }
        dg.add(n);
    }

    std::string check(const std::string& fen, Rng& rng, bool allLegal) {
        StateInfo st;
        Position  pos;
        if (pos.set(fen, chess960, &st))
            return "error";

        Digest      dg;
        std::string out;

        // legal() on every 16-bit value
        if (allLegal)
        {
            Digest ld;
            int    count = 0;
            for (u32 v = 0; v < 65536; ++v)
                if (pos.legal(Move(u16(v))))
                    ld.add(v), ++count;
            char buf[64];
            std::snprintf(buf, sizeof buf, " legal %d %016llX", count, (unsigned long long) ld.h);
            out += buf;
        }

        // The entries of the shared histories for this position
        const Color us = pos.side_to_move();
        auto&       sh = *t.shared;
        sh.pawn_correction_entry(pos)[us].pawn << random_bonus(rng, 1024);
        sh.minor_piece_correction_entry(pos)[us].minor << random_bonus(rng, 1024);
        sh.nonpawn_correction_entry<WHITE>(pos)[us].nonPawnWhite << random_bonus(rng, 1024);
        sh.nonpawn_correction_entry<BLACK>(pos)[us].nonPawnBlack << random_bonus(rng, 1024);
        dg.add(sh.pawn_correction_entry(pos)[us].pawn);
        dg.add(sh.minor_piece_correction_entry(pos)[us].minor);
        dg.add(sh.nonpawn_correction_entry<WHITE>(pos)[us].nonPawnWhite);
        dg.add(sh.nonpawn_correction_entry<BLACK>(pos)[us].nonPawnBlack);
        const int ppc = rng.below(PIECE_NB);
        const int pto = rng.below(SQUARE_NB);
        dg.add(sh.pawn_entry(pos)[ppc][pto]);

        const MoveList<ALL> legalMoves(pos);
        const int           nLegal = int(legalMoves.size());

        // 0 none, 1 legal, 2 pseudo-legal illegal, 3 garbage
        auto ttMove = [&](int kind) -> Move {
            switch (kind)
            {
            case 1 :
                return nLegal ? Move(legalMoves.begin()[rng.below(nLegal)]) : Move::none();
            case 2 :
                return pseudo_legal_illegal(pos, rng);
            case 3 :
                return Move(u16(rng.below(65536)));
            default :
                return Move::none();
            }
        };

        for (int r = 0; r < 10; ++r)
        {
            const PieceToHistory* contHist[6];
            for (auto& ch : contHist)
            {
                if (rng.below(8) == 0)
                {
                    ch = &t.cont(0, 0, NO_PIECE, 0);  // the sentinel of search
                    continue;
                }
                const int ic  = rng.below(2);
                const int cap = rng.below(2);
                const int pc  = rng.below(PIECE_NB);
                const int s   = rng.below(SQUARE_NB);
                ch            = &t.cont(ic, cap, pc, s);
            }
            const int  kind   = r % 4;
            const Move ttm    = ttMove(kind);
            const int  ply    = rng.range(0, 7);
            const int  skipAt = rng.below(3) ? -1 : rng.range(0, 40);

            if (r < 6)
            {
                // main search, or quiescence search (depth 0) for r = 4, 5
                // depths 1 to 3 half of the time: below 4 the quiet moves
                // between -3560 * depth and -14000 are left unsorted
                const Depth depth = r >= 4 ? DEPTH_QS : rng.below(2) ? rng.range(1, 3) : rng.range(4, 20);
                log("main depth " + std::to_string(depth) + " ply " + std::to_string(ply) + " tt "
                    + UCIEngine::move(ttm, chess960) + " skip " + std::to_string(skipAt));
                MovePicker mp(pos, ttm, depth, t.mainHistory.get(), t.lowPlyHistory.get(),
                              t.captureHistory.get(), contHist, t.shared.get(), ply);
                run(pos, mp, contHist, ply, skipAt, rng, dg);
            }
            else if (!pos.checkers())
            {
                const int threshold = rng.range(-600, 1600);
                log("probcut threshold " + std::to_string(threshold) + " tt "
                    + UCIEngine::move(ttm, chess960) + " skip " + std::to_string(skipAt));
                MovePicker mp(pos, ttm, threshold, t.captureHistory.get());
                run(pos, mp, contHist, ply, skipAt, rng, dg);
            }
        }

        char buf[32];
        std::snprintf(buf, sizeof buf, " %016llX", (unsigned long long) dg.h);
        return out + buf;
    }
};

double cpu_time() { return double(std::clock()) / CLOCKS_PER_SEC; }

std::vector<std::string> read_fens(const char* path) {
    std::vector<std::string> fens;
    std::ifstream            f(path);
    for (std::string l; std::getline(f, l);)
        if (l.find_first_not_of(" \t\r") != std::string::npos)
            fens.push_back(l);
    return fens;
}

// The same work as rust/examples/movepick_check.rs bench: every move of the
// main picker at depths 1, 8 and 16 and of the quiescence picker, or only
// the first three (as a node with an early cutoff would), on each position,
// reps times per round, best of 5 rounds
int bench(Tables& t, const std::vector<std::string>& fens, bool chess960, int reps) {
    const int rounds = 5;
    std::vector<std::unique_ptr<std::pair<StateInfo, Position>>> positions;
    for (const auto& fen : fens)
    {
        auto p = std::make_unique<std::pair<StateInfo, Position>>();
        if (!p->second.set(fen, chess960, &p->first))
            positions.push_back(std::move(p));
    }
    const PieceToHistory* contHist[6];
    for (int i = 0; i < 6; ++i)
        contHist[i] = &t.cont(i & 1, (i >> 1) & 1, W_KNIGHT + i, 20 + i);

    for (int firstOnly = 0; firstOnly < 2; ++firstOnly)
    {
        double best = 1e9;
        u64    sum = 0, picks = 0;
        for (int round = 0; round < rounds; ++round)
        {
            sum = picks  = 0;
            double t0    = cpu_time();
            for (int rep = 0; rep < reps; ++rep)
                for (auto& p : positions)
                {
                    Position& pos = p->second;
                    for (Depth depth : {1, 8, 16, 0})
                    {
                        MovePicker mp(pos, Move::none(), depth, t.mainHistory.get(),
                                      t.lowPlyHistory.get(), t.captureHistory.get(), contHist,
                                      t.shared.get(), 2);
                        int n = 0;
                        for (Move m; (m = mp.next_move()) != Move::none();)
                        {
                            sum = sum * 31 + m.raw();
                            ++picks;
                            if (firstOnly && ++n == 3)
                                break;
                        }
                    }
                }
            double el = cpu_time() - t0;
            if (el < best)
                best = el;
        }
        const u64 pickers = u64(positions.size()) * 4 * reps;
        std::printf("%s: %llu pickers, %llu moves in %.3f s (best of %d), %.0f pickers/s, checksum %016llX\n",
                    firstOnly ? "first 3 moves" : "all moves", (unsigned long long) pickers,
                    (unsigned long long) picks, best, rounds, pickers / best, (unsigned long long) sum);
    }
    return 0;
}

}  // namespace

int main(int argc, char* argv[]) {
    // movepick_check check <fens> <chess960 0|1> <legal every N> [verbose index]
    // (verbose index -2 - i: position i of the coarse pass)
    // movepick_check bench <fens> <chess960 0|1> <repetitions>
    if (argc < 5)
    {
        std::cerr << "usage: movepick_check check|bench <fens> <chess960> <n> [verbose index]\n";
        return 2;
    }
    Attacks::init();
    Position::init();

    const std::string mode     = argv[1];
    const auto        fens     = read_fens(argv[2]);
    const bool        chess960 = std::string(argv[3]) == "1";
    const int         n        = std::stoi(argv[4]);

    Tables t(1);
    t.fill();

    if (mode == "bench")
        return bench(t, fens, chess960, n);

    Rng rng(0x2545F4914F6CDD1DULL);

    std::cout << "fill" << t.digests() << "\n";

    std::cout << "update";
    std::cout << " " << std::hex << update_digest<StatsEntry<i16, 7183>>(7183, rng);
    std::cout << " " << update_digest<StatsEntry<i16, 10692>>(10692, rng);
    std::cout << " " << update_digest<StatsEntry<i16, 30000, true>>(30000, rng);
    std::cout << " " << update_digest<StatsEntry<i16, 8192, true>>(8192, rng);
    std::cout << " " << update_digest<StatsEntry<i16, 1024, true>>(1024, rng);
    std::cout << " " << update_digest<StatsEntry<i16, 1024>>(1024, rng);
    std::cout << " " << update_digest<StatsEntry<i16, 8192>>(8192, rng) << std::dec << "\n";

    t.random_updates(rng, 200000);
    std::cout << "updated" << t.digests() << "\n";

    Checker   checker{t, chess960};
    const int verboseIndex = argc > 5 ? std::stoi(argv[5]) : -1;
    for (usize i = 0; i < fens.size(); ++i)
    {
        checker.verbose = int(i) == verboseIndex;
        std::cout << i << checker.check(fens[i], rng, n > 0 && i % n == 0) << "\n";
    }

    std::cout << "end" << t.digests() << "\n";

    // The coarse pass: the same on every position again, with coarse tables
    t.fill_coarse();
    std::cout << "coarse" << t.digests() << "\n";
    for (usize i = 0; i < fens.size(); ++i)
    {
        checker.verbose = int(i) == -2 - verboseIndex;
        std::cout << "c" << i << checker.check(fens[i], rng, false) << "\n";
    }
    std::cout << "end" << t.digests() << "\n";
    return 0;
}

// Drives the C++ time manager of src/ (TimeManagement of timeman.cpp, with the
// OptionsMap of ucioption.cpp) over a large set of clocks and prints what it
// computes: optimum(), maximum(), the limits that init() rewrites in 'nodes as
// time' mode and the nodes left. rust/examples/tm_check.rs does exactly the
// same with the port, and must print exactly the same lines (README.md, "Time
// management"). Built by bench/build_tm_check.sh; not part of the engine.
//
//   tm_check [lines|digest] [doubles]
//
// lines prints one line per call of init(); digest one digest per block of
// 4096 lines and one per section, for the recorded expected output of the
// emulated targets (tm_check.expected). doubles appends to each line the bits
// of originalTimeAdjust, the one double init() keeps between calls.
//
// Four sections, every value drawn from one splitmix64 stream consumed in the
// same order on both sides (never two draws in one C++ expression):
//   grid    the product of short lists of times, increments, movestogo, plies,
//           Move Overhead, nodestime and Ponder, each call on a fresh manager
//   random  calls with values drawn over wide ranges and a random previous
//           originalTimeAdjust
//   games   sequences of calls as in a game: clocks that run down and get
//           their increment, movestogo cycles, plies that advance, and in
//           'nodes as time' mode advance_nodes_time() between calls as
//           Worker::start_searching() does; ucinewgame (clear()) at times
//   edges   time below the overhead, 1 ms, 1 move to go, a negative
//           increment, a clock for the side not to move only, movetime with
//           nodestime, clocks of millions of years

#include <cinttypes>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <sstream>
#include <string>

#include <limits>

#include "misc.h"
#include "types.h"

// The state of 'nodes as time' mode is private: the driver prints and sets
// it. timeman.h is included first (search.h includes it), its own includes
// before it, so that nothing else is affected.
#define private public
#include "timeman.h"
#undef private

#include "search.h"
#include "ucioption.h"

using namespace Stockfish;

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
    u64 below(u64 n) { return next() % n; }
    // about log-uniform in [lo, hi], lo >= 1
    i64 logu(i64 lo, i64 hi) {
        // a number of bits, then a value with that many bits, clamped
        int bitsLo = 64 - __builtin_clzll(u64(lo));
        int bitsHi = 64 - __builtin_clzll(u64(hi));
        int bits   = bitsLo + int(below(u64(bitsHi - bitsLo + 1)));
        i64 v      = i64(below(u64(1) << bits));
        return v < lo ? lo : v > hi ? hi : v;
    }
};

bool doubles = false;
bool digest  = false;

struct Digest {
    u64  h = 0xCBF29CE484222325ULL;
    void add(const char* s) {
        for (; *s; ++s)
            h = (h ^ u64(u8(*s))) * 0x100000001B3ULL;
    }
};

struct Out {
    Digest block, section;
    u64    lines = 0, sectionLines = 0;
    void   line(const char* s) {
        if (!digest)
        {
            fputs(s, stdout);
            return;
        }
        block.add(s);
        section.add(s);
        ++lines;
        ++sectionLines;
        if (lines % 4096 == 0)
        {
            printf("block %" PRIu64 " %016" PRIX64 "\n", lines / 4096, block.h);
            block = Digest();
        }
    }
    void end_section(const char* name) {
        if (digest)
            printf("section %s %" PRIu64 " lines %016" PRIX64 "\n", name, sectionLines, section.h);
        section      = Digest();
        sectionLines = 0;
    }
} out;

OptionsMap options;

void set_option(const std::string& name, i64 value) {
    std::istringstream is("name " + name + " value " + std::to_string(value));
    options.setoption(is);
}

void set_ponder(bool b) {
    std::istringstream is(std::string("name Ponder value ") + (b ? "true" : "false"));
    options.setoption(is);
}

u64 bits(double d) {
    u64 u;
    std::memcpy(&u, &d, sizeof(u));
    return u;
}

// One call of init(), printed: the inputs, then what it computes.
void call(TimeManagement&      tm,
          Search::LimitsType&  limits,
          Color                us,
          int                  ply,
          int                  npmsec,
          int                  overhead,
          bool                 ponder,
          double&              adjust) {
    set_option("nodestime", npmsec);
    set_option("Move Overhead", overhead);
    set_ponder(ponder);

    char in[256];
    snprintf(in, sizeof in, "%d %d %" PRId64 " %" PRId64 " %" PRId64 " %" PRId64 " %d %" PRId64 " %d %d %d",
             int(us), ply, limits.time[WHITE], limits.time[BLACK], limits.inc[WHITE], limits.inc[BLACK],
             limits.movestogo, limits.movetime, npmsec, overhead, int(ponder));

    tm.init(limits, us, ply, options, adjust);

    char line[512];
    int  n = snprintf(line, sizeof line, "%s : %" PRId64 " %" PRId64 " %" PRId64 " %" PRId64 " %" PRId64
                                         " %" PRId64 " %" PRId64,
                      in, tm.optimum(), tm.maximum(), limits.time[us], limits.inc[us], limits.npmsec,
                      limits.movetime, tm.availableNodes);
    if (doubles)
        n += snprintf(line + n, sizeof line - n, " %016" PRIX64, bits(adjust));
    snprintf(line + n, sizeof line - n, "\n");
    out.line(line);
}

Search::LimitsType make_limits(i64 wtime, i64 btime, i64 winc, i64 binc, int movestogo, i64 movetime) {
    Search::LimitsType limits;
    limits.time[WHITE] = wtime;
    limits.time[BLACK] = btime;
    limits.inc[WHITE]  = winc;
    limits.inc[BLACK]  = binc;
    limits.movestogo   = movestogo;
    limits.movetime    = movetime;
    limits.startTime   = 0;
    return limits;
}

void grid() {
    const i64 times[]     = {1,    2,    5,    10,    30,    99,    100,   250,    500,    999,
                             1000, 1500, 3000, 10000, 30000, 60000, 180000, 600000, 3600000, 36000000};
    const i64 incs[]      = {0, 1, 100, 1000, 10000};
    const int movestogo[] = {0, 1, 2, 10, 40, 60};
    const int plies[]     = {0, 1, 10, 40, 100, 300};
    const int overheads[] = {0, 10, 1000};
    const int npmsecs[]   = {0, 1, 600};
    int       k           = 0;

    for (i64 t : times)
        for (i64 inc : incs)
            for (int mtg : movestogo)
                for (int ply : plies)
                    for (int ov : overheads)
                        for (int np : npmsecs)
                            for (int pd = 0; pd < 2; ++pd)
                            {
                                // the opponent's clock: equal, half, double, none
                                const i64 opp[] = {t, t / 2, 2 * t, 0};
                                Color     us    = Color(k % 2);
                                i64       o     = opp[(k / 2) % 4];
                                ++k;
                                TimeManagement     tm;
                                double             adjust = -1;
                                Search::LimitsType limits =
                                  us == WHITE ? make_limits(t, o, inc, inc, mtg, 0)
                                              : make_limits(o, t, inc, inc, mtg, 0);
                                call(tm, limits, us, ply, np, ov, pd, adjust);
                            }
    out.end_section("grid");
}

void random_calls(Rng& rng, int count) {
    const int npmsecs[] = {0, 0, 0, 1, 7, 100, 1000, 10000};
    for (int i = 0; i < count; ++i)
    {
        Color us    = Color(rng.below(2));
        i64   t     = rng.logu(1, 100000000);
        i64   o     = rng.below(8) == 0 ? 0 : rng.logu(1, 100000000);
        i64   inc   = rng.below(3) == 0 ? 0 : rng.logu(1, 100000);
        i64   oinc  = rng.below(2) == 0 ? inc : rng.logu(1, 100000);
        int   mtg   = rng.below(2) == 0 ? 0 : int(rng.logu(1, 200));
        int   ply   = int(rng.below(600));
        int   ov    = rng.below(4) == 0 ? 10 : int(rng.below(5001));
        int   np    = npmsecs[rng.below(8)];
        bool  pd    = rng.below(2) == 1;
        // the previous originalTimeAdjust: none yet, or one of a game
        double adjust = rng.below(2) == 0 ? -1.0 : double(i64(rng.below(4000)) - 500) / 1000.0;
        // the nodes left from an earlier move, in 'nodes as time' mode
        i64 avail = rng.below(2) == 0 ? -1 : rng.logu(1, i64(1) << 40);
        int prevMtg = int(rng.below(60));

        TimeManagement tm;
        tm.availableNodes    = avail;
        tm.previousMovesToGo = prevMtg;
        tm.cyclicBudget      = rng.below(2) == 0 ? 0 : rng.logu(1, i64(1) << 36);
        Search::LimitsType limits =
          us == WHITE ? make_limits(t, o, inc, oinc, mtg, 0) : make_limits(o, t, oinc, inc, mtg, 0);
        call(tm, limits, us, ply, np, ov, pd, adjust);
    }
    out.end_section("random");
}

void games(Rng& rng, int count) {
    const int npmsecs[] = {0, 0, 1, 50, 1000};
    TimeManagement tm;
    for (int g = 0; g < count; ++g)
    {
        // ucinewgame: ThreadPool::clear()
        tm.clear();
        double adjust = -1;

        Color us      = Color(rng.below(2));
        i64   base    = rng.logu(50, 10800000);
        i64   inc     = rng.below(3) == 0 ? 0 : rng.logu(1, base / 10 + 1);
        int   cycle   = rng.below(3) == 0 ? int(rng.logu(1, 60)) : 0;
        int   ov      = rng.below(3) == 0 ? 10 : int(rng.below(1001));
        int   np      = npmsecs[rng.below(5)];
        bool  pd      = rng.below(4) == 0;
        int   moves   = 1 + int(rng.below(120));
        int   ply     = int(rng.below(2)) + (us == BLACK ? 1 : 0);
        i64   mine    = base;
        i64   theirs  = base;
        int   togo    = cycle;

        for (int m = 0; m < moves; ++m)
        {
            Search::LimitsType limits = us == WHITE ? make_limits(mine, theirs, inc, inc, togo, 0)
                                                    : make_limits(theirs, mine, inc, inc, togo, 0);
            call(tm, limits, us, ply, np, ov, pd, adjust);

            // the time (or nodes) used by this move, up to the maximum
            i64 cap  = tm.maximum() < (i64(1) << 40) ? tm.maximum() : (i64(1) << 40);
            i64 used = cap > 0 ? i64(rng.below(u64(cap) + 1)) : 0;
            if (np)
            {
                // Worker::start_searching(): the nodes searched minus the
                // increment, in nodes (init() has converted it)
                tm.advance_nodes_time(used - limits.inc[us]);
                used /= np;
            }
            mine -= used;
            if (mine <= 0)
                mine = 1;  // the GUI's clock, as parse_limits() sanitizes it
            mine += inc;
            i64 theirUse = i64(rng.below(u64(theirs / 20 + 1)));
            theirs       = theirs - theirUse > 0 ? theirs - theirUse + inc : 1 + inc;
            ply += 2;
            if (cycle && --togo == 0)
            {
                togo = cycle;
                mine += base;
                theirs += base;
            }
            if (rng.below(50) == 0)
            {
                tm.clear();
                adjust = -1;
            }
        }
    }
    out.end_section("games");
}

void edges() {
    struct E {
        i64  wtime, btime, winc, binc;
        int  movestogo;
        i64  movetime;
        int  ply, npmsec, overhead;
        bool ponder;
    };
    const i64 big   = INT64_MAX;
    const E   cases[] = {
      // time below the overhead, and at it
      {5, 5, 0, 0, 0, 0, 0, 0, 10, false},
      {10, 10, 0, 0, 0, 0, 0, 0, 10, false},
      {11, 11, 0, 0, 0, 0, 20, 0, 10, false},
      {100, 100, 0, 0, 0, 0, 20, 0, 5000, false},
      {100, 100, 50, 50, 0, 0, 20, 0, 5000, true},
      {4999, 4999, 0, 0, 0, 0, 20, 0, 5000, false},
      {5000, 5000, 0, 0, 0, 0, 20, 0, 5000, false},
      {5001, 5001, 0, 0, 0, 0, 20, 0, 5000, false},
      {100, 100, 0, 0, 3, 0, 20, 0, 1000, false},
      {100, 100, 0, 0, 3, 0, 20, 10, 1000, false},
      // 1 ms, the sanitized negative time
      {1, 1, 0, 0, 0, 0, 0, 0, 10, false},
      {1, 1, 0, 0, 0, 0, 0, 0, 0, false},
      {1, 1, 0, 0, 1, 0, 0, 0, 0, false},
      {1, 1000, 0, 0, 0, 0, 0, 0, 0, false},
      {1, 1, 0, 0, 0, 0, 0, 10000, 5000, true},
      // 1 move to go, and more
      {1000, 1000, 0, 0, 1, 0, 30, 0, 10, false},
      {60000, 1000, 0, 0, 1, 0, 30, 0, 10, false},
      {60000, 120000, 0, 0, 2, 0, 30, 0, 10, false},
      {60000, 60000, 1000, 1000, 1, 0, 30, 0, 10, false},
      {60000, 60000, 0, 0, 50, 0, 30, 0, 10, false},
      {60000, 60000, 0, 0, 51, 0, 30, 0, 10, false},
      {60000, 60000, 0, 0, 1000000, 0, 30, 0, 10, false},
      // negative increments
      {60000, 60000, -100, -100, 0, 0, 30, 0, 10, false},
      {600, 600, -1000, -1000, 0, 0, 30, 0, 10, false},
      {60000, 60000, -100, -100, 10, 0, 30, 100, 10, false},
      // a clock for the side not to move only (time[us] == 0)
      {0, 60000, 0, 1000, 0, 0, 30, 0, 10, false},
      {0, 60000, 0, 1000, 0, 5000, 30, 100, 10, false},
      // movetime with nodestime, and no clock
      {0, 0, 0, 0, 0, 1000, 0, 1000, 10, false},
      {0, 0, 0, 0, 0, 7, 0, 10000, 10, false},
      // plies far into a game
      {60000, 60000, 0, 0, 0, 0, 1000, 0, 10, false},
      {60000, 60000, 0, 0, 0, 0, 5000, 0, 10, false},
      {60000, 60000, 0, 0, 10, 0, 5000, 0, 10, false},
      // clocks of years, and of millions of years: out of the range of a
      // TimePoint once scaled (undefined in C++, INT64_MIN on x86-64)
      {INT64_C(31536000000), INT64_C(31536000000), 0, 0, 0, 0, 10, 0, 10, false},
      {INT64_C(31536000000), INT64_C(31536000000), 0, 0, 0, 0, 10, 10000, 10, false},
      {INT64_C(4000000000000000000), INT64_C(4000000000000000000), 0, 0, 0, 0, 0, 0, 10, false},
      {INT64_C(4000000000000000000), INT64_C(4000000000000000000), 0, 0, 0, 0, 0, 0, 10, true},
      {big, big, 0, 0, 0, 0, 0, 0, 10, false},
      {big, big, 0, 0, 0, 0, 0, 0, 0, true},
      {big, 1, 0, 0, 0, 0, 0, 0, 0, false},
      {big, big, 0, 0, 40, 0, 0, 0, 10, false},
      // with a ply of thousands (a FEN may say so) the optimum exceeds it
      {big, big, 0, 0, 0, 0, 5000, 0, 0, false},
      {big, big, 0, 0, 0, 0, 5000, 0, 10, true},
      {INT64_C(8000000000000000000), 1, 0, 0, 0, 0, 3000, 0, 10, false},
    };
    for (const E& e : cases)
        for (int c = 0; c < 2; ++c)
        {
            Color              us = Color(c);
            TimeManagement     tm;
            double             adjust = -1;
            Search::LimitsType limits =
              us == WHITE ? make_limits(e.wtime, e.btime, e.winc, e.binc, e.movestogo, e.movetime)
                          : make_limits(e.btime, e.wtime, e.binc, e.winc, e.movestogo, e.movetime);
            call(tm, limits, us, e.ply + c, e.npmsec, e.overhead, e.ponder, adjust);
        }
    out.end_section("edges");
}

}  // namespace

int main(int argc, char** argv) {
    for (int i = 1; i < argc; ++i)
    {
        if (!strcmp(argv[i], "digest"))
            digest = true;
        else if (!strcmp(argv[i], "doubles"))
            doubles = true;
        else if (strcmp(argv[i], "lines"))
        {
            fprintf(stderr, "usage: tm_check [lines|digest] [doubles]\n");
            return 2;
        }
    }

    options.add("nodestime", Option(0, 0, 10000));
    options.add("Move Overhead", Option(10, 0, 5000));
    options.add("Ponder", Option(false));

    Rng rng(0x7130E5);
    grid();
    random_calls(rng, 100000);
    games(rng, 3000);
    edges();
    return 0;
}

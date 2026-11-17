// What the commands outside a search compute without printing it, against
// the C++ functions themselves: the parts of the skill level (Skill of
// search.h, Skill::pick_best of search.cpp) that do not depend on chance
// and pick_best() itself given the random numbers (the conversion of each
// UCI_Elo to a level, the levels that enable it, the depth at which the move
// is picked, pick_best() on synthetic root moves with the generator seeded
// by SF_SKILL_SEED), and the commands speedtest runs
// (Benchmark::setup_benchmark) for several arguments.
// rust/examples/surface_check.rs does the same with the port and must print
// the same lines (README.md, "Skill levels", "speedtest"). Built by
// bench/build_surface_check.sh against a build of src/ patched with
// bench/skill_seed.patch; not part of the engine.
//
//   surface_check [lines|digest]
//
// digest prints one FNV-1a digest per section, for surface_check.expected.

#include <cinttypes>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

#include <sstream>

#include "benchmark.h"
#include "search.h"
#include "types.h"

using namespace Stockfish;

namespace {

struct Out {
    bool     digest;
    uint64_t h = 0xcbf29ce484222325ULL;
    uint64_t n = 0;

    void line(const std::string& s) {
        if (!digest)
        {
            std::fputs(s.c_str(), stdout);
            std::fputc('\n', stdout);
        }
        for (char c : s + "\n")
            h = (h ^ uint8_t(c)) * 0x100000001b3ULL;
        n++;
    }
    void section(const char* name) {
        if (digest)
            std::printf("%s lines %" PRIu64 " fnv %016" PRIx64 "\n", name, n, h);
        h = 0xcbf29ce484222325ULL;
        n = 0;
    }
};

uint64_t sm = 0x9e3779b97f4a7c15ULL;
uint64_t next() {
    uint64_t z = (sm += 0x9e3779b97f4a7c15ULL);
    z          = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z          = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}
int below(int n) { return int(next() % uint64_t(n)); }

std::string skill_line(const char* what, int value, const Search::Skill& s) {
    uint64_t bits;
    std::memcpy(&bits, &s.level, 8);
    char buf[200];
    std::snprintf(buf, sizeof buf, "%s %d level %016" PRIx64 " enabled %d pick_depth %d weakness %d", what,
                  value, bits, int(s.enabled()), 1 + int(s.level), int(120 - 2 * s.level));
    return buf;
}

}  // namespace

int main(int argc, char* argv[]) {
    Out out{argc > 1 && std::strcmp(argv[1], "digest") == 0};
    setenv("SF_SKILL_SEED", "1070372", 1);

    // UCI_LimitStrength with every UCI_Elo; Skill Level alone
    for (int elo = Search::Skill::LowestElo; elo <= Search::Skill::HighestElo; ++elo)
        out.line(skill_line("elo", elo, Search::Skill(20, elo)));
    for (int level = 0; level <= 20; ++level)
        out.line(skill_line("level", level, Search::Skill(level, 0)));
    out.section("conversion");

    // pick_best() on synthetic root moves, sorted as search leaves them
    for (int c = 0; c < 30000; ++c)
    {
        const bool       fromElo = below(2);
        const int        param   = fromElo ? Search::Skill::LowestElo + below(1871) : below(20);
        Search::Skill    skill(fromElo ? 20 : param, fromElo ? param : 0);
        const int        n       = 1 + below(30);
        Search::RootMoves rootMoves;
        int              score = below(8) == 0 ? VALUE_MATE - 1 - below(40) : below(6001) - 3000;
        for (int i = 0; i < n; ++i)
        {
            rootMoves.emplace_back(Move(Square(i % 64), Square((i * 7 + 13) % 64)));
            const int kind = below(4);
            score -= kind == 0 ? 0 : kind == 1 ? below(8) : kind == 2 ? below(100) : below(800);
            rootMoves.back().score = std::max(score, -VALUE_INFINITE + 1);
        }
        const usize multiPV = usize(std::min(n, 1 + below(8)));
        const Move  m       = skill.pick_best(rootMoves, multiPV);
        int         idx     = 0;
        while (rootMoves[idx].pv[0] != m)
            idx++;
        char buf[160];
        std::snprintf(buf, sizeof buf, "case %d %s %d moves %d multipv %zu pick %d", c, fromElo ? "elo" : "level",
                      param, n, multiPV, idx);
        out.line(buf);
    }
    out.section("pick_best");

    // The commands of speedtest (always with a thread count: the default is
    // the machine's)
    for (const char* a : {"1 16 1", "1 16 2", "1 16 3", "1 16 7", "1 16 10", "1 16 60", "1 16 150", "1 16 151",
                          "1 16 999", "1 16 3600", "1 16 86400", "1 16 2147483", "2 1 5", "0 0 0", "3 64",
                          "4", "1 16 -5", "1 16 99999999999", "1 x 5", "2 3 4x"})
    {
        std::istringstream is(a);
        auto               setup = Benchmark::setup_benchmark(is);
        out.line(std::string("args '") + a + "' threads " + std::to_string(setup.threads) + " hash "
                 + std::to_string(setup.ttSize) + " original '" + setup.originalInvocation + "' filled '"
                 + setup.filledInvocation + "' commands "
                 + std::to_string(setup.commands.size()));
        for (const auto& c : setup.commands)
            out.line(c);
    }
    out.section("speedtest");
    return 0;
}

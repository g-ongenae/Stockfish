// The deterministic parts of the thread pool, on the C++ engine of src/: the
// sizes and index masks of the shared histories for each thread count, the
// part of them each thread clears, what every worker's tables, the shared
// histories and the transposition table hold after the pool clears them with
// N threads (after a resize, and after a ucinewgame that follows tables
// filled with garbage), the main manager's data after a clear, and
// get_best_thread() on synthetic results of N threads (scores, mates, losses,
// inexact scores, PV lengths, ties of votes). rust/examples/threads_check.rs
// does the same with the port and must print exactly the same lines. Built by
// bench/build_threads_check.sh against the objects of a build of src/; not
// part of the engine.
//
// Everything random comes from one splitmix64 stream, consumed in the same
// order on both sides (never two draws in one C++ expression).

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

#include "misc.h"
#include "types.h"

// The pool, the workers' tables and indices, the shared histories' masks and
// the table's storage are private: the driver reads (and dirties) them.
#define private public
#include "engine.h"
#include "history.h"
#include "search.h"
#include "thread.h"
#include "tt.h"
#undef private

using namespace Stockfish;
using Search::RootMove;
using Search::RootMoves;

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

std::string hex64(u64 v) {
    char buf[17];
    std::snprintf(buf, sizeof buf, "%016llX", (unsigned long long) v);
    return buf;
}

// The digests of the search trace (searchtrace.cpp): 64-bit FNV-1a over
// little-endian words of 4 entries
struct TraceDigest {
    u64  value = 0xcbf29ce484222325ULL;
    void word(u64 w) { value = (value ^ w) * 0x100000001b3ULL; }
};

u64 digest_raw(const void* p, usize size) {
    TraceDigest d;
    const char* bytes = static_cast<const char*>(p);
    for (usize i = 0; i < size; i += 8)
    {
        u64 w = 0;
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

template<typename Table>
u64 digest_i16(const Table& t) {
    static_assert(sizeof(Table) % 8 == 0);
    return digest_raw(&t, sizeof(Table));
}

template<typename Dyn>
u64 digest_dyn(const Dyn& table) {
    TraceDigest d;
    for (usize i = 0; i < table.get_size(); ++i)
        d.word(digest_i16(table[i]));
    return d.value;
}

// Every 16-bit entry of a table to a value of its flat index
void dirty(void* p, usize size, int salt) {
    i16* e = static_cast<i16*>(p);
    for (usize i = 0; i < size / 2; ++i)
        e[i] = i16(int((i * 7 + usize(salt)) % 1999) - 999);
}

template<typename Dyn>
void dirty_dyn(Dyn& table, int salt) {
    for (usize i = 0; i < table.get_size(); ++i)
        dirty(&table[i], sizeof(table[i]), salt + int(i));
}

void setoption(Engine& engine, const std::string& name, const std::string& value) {
    std::istringstream is("name " + name + " value " + value);
    engine.get_options().setoption(is);
}

// The size of the shared histories for N threads, and the part of them each
// thread of N clears: thread t writes 100 + t over its part, then the first
// and last entry of each value are printed
void sizes() {
    for (usize n : {1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 16, 17, 31, 32, 33, 64, 100, 255, 256, 257, 1024, 4096})
    {
        // next_power_of_two() is static in thread.cpp: what set() passes is
        // the size of the shared histories divided by the size for 1 thread
        std::cout << "next_power_of_two " << n << " ";
        usize p = n > 1 ? (2ULL << msb(n - 1)) : 1;
        std::cout << p << "\n";
    }

    for (usize n = 1; n <= 9; ++n)
    {
        usize           p = n > 1 ? (2ULL << msb(n - 1)) : 1;
        SharedHistories sh(p);
        std::cout << "shared " << n << " size " << sh.get_size() << " pawn " << sh.pawnHistory.get_size()
                  << " mask " << sh.sizeMinus1 << " pawn_mask " << sh.pawnHistSizeMinus1 << "\n";
        for (usize t = 0; t < n; ++t)
        {
            sh.correctionHistory.clear_range(int(100 + t), t, n);
            sh.pawnHistory.clear_range(int(100 + t), t, n);
        }
        auto ranges = [&](const char* name, auto& table, auto first) {
            std::cout << "  " << name;
            usize start = 0;
            for (usize i = 1; i <= table.get_size(); ++i)
                if (i == table.get_size() || first(table[i]) != first(table[start]))
                {
                    std::cout << " " << first(table[start]) << ":" << start << "-" << i;
                    start = i;
                }
            std::cout << "\n";
        };
        ranges("correction", sh.correctionHistory, [](const auto& e) { return int(e[WHITE].pawn); });
        ranges("pawn", sh.pawnHistory, [](const auto& e) { return int(e[W_PAWN][SQ_A1]); });
    }
}

void print_state(Engine& engine, const char* when) {
    auto& threads = engine.threads;
    std::cout << when << " threads " << threads.size() << "\n";

    auto& shared = engine.sharedHists.at(0);
    std::cout << "  shared size " << shared.get_size() << " pawn " << shared.pawnHistory.get_size()
              << " pawn_history " << hex64(digest_dyn(shared.pawnHistory)) << " correction_history "
              << hex64(digest_dyn(shared.correctionHistory));
    TraceDigest continuation;
    for (const auto& inCheck : shared.continuationHistory())
        for (const auto& captures : inCheck)
            continuation.word(digest_i16(captures));
    std::cout << " continuation_history " << hex64(continuation.value) << "\n";

    for (auto&& th : threads)
    {
        auto& w = *th->worker;
        std::cout << "  worker " << w.threadIdx << " numa " << w.numaThreadIdx << "/" << w.numaTotal
                  << " main " << (w.manager != nullptr) << " shared " << (&w.sharedHistory == &shared)
                  << " main_history " << hex64(digest_i16(w.mainHistory)) << " capture_history "
                  << hex64(digest_i16(w.captureHistory)) << " continuation_correction_history "
                  << hex64(digest_i16(w.continuationCorrectionHistory)) << " ttmove_history "
                  << int(w.ttMoveHistory) << "\n";
    }

    auto* mm = threads.main_manager();
    std::cout << "  manager " << mm->bestPreviousAverageScore << " " << mm->previousTimeReduction << " "
              << mm->callsCnt << " " << mm->bestPreviousScore << " " << mm->originalTimeAdjust << " "
              << mm->tm.availableNodes << "\n";

    auto&       tt    = engine.tt;
    const char* bytes = reinterpret_cast<const char*>(tt.table);
    usize       nonzero = 0;
    for (usize i = 0; i < tt.clusterCount * 32; ++i)
        nonzero += bytes[i] != 0;
    std::cout << "  tt clusters " << tt.clusterCount << " generation " << int(tt.generation8) << " nonzero "
              << nonzero << " hashfull " << tt.hashfull() << "\n";
}

// Garbage in every table the clear of the pool writes, and in the
// transposition table, then a ucinewgame
void dirty_and_clear(Engine& engine, Rng& rng) {
    auto& threads = engine.threads;
    auto& shared  = engine.sharedHists.at(0);
    dirty_dyn(shared.pawnHistory, 1);
    dirty_dyn(shared.correctionHistory, 2);
    dirty(&shared.continuationHistory(), sizeof(ContinuationHistory) * 4, 3);
    for (auto&& th : threads)
    {
        auto& w = *th->worker;
        dirty(&w.mainHistory, sizeof(w.mainHistory), 4 + int(w.threadIdx));
        dirty(&w.captureHistory, sizeof(w.captureHistory), 5 + int(w.threadIdx));
        dirty(&w.continuationCorrectionHistory, sizeof(w.continuationCorrectionHistory), 6 + int(w.threadIdx));
        w.ttMoveHistory = int(w.threadIdx) + 7;
    }
    auto* mm                     = threads.main_manager();
    mm->bestPreviousAverageScore = 11;
    mm->previousTimeReduction    = 1.5;
    mm->callsCnt                 = 12;
    mm->bestPreviousScore        = 13;
    mm->originalTimeAdjust       = 2.5;

    auto& tt = engine.tt;
    for (int i = 0; i < 3; ++i)
        tt.new_search();
    for (int i = 0; i < 5000; ++i)
    {
        Key key                 = rng.next();
        auto [hit, data, write] = tt.probe(key);
        (void) hit;
        (void) data;
        Value v  = Value(rng.range(-500, 500));
        bool  pv = rng.below(2);
        Bound b  = Bound(1 + rng.below(3));
        Depth d  = Depth(rng.range(-6, 60));
        Move  m  = Move(u16(rng.below(65536)));
        Value ev = Value(rng.range(-500, 500));
        write.write(key, v, pv, b, d, m, ev, tt.generation());
    }
    print_state(engine, "dirty");
    engine.search_clear();
    print_state(engine, "ucinewgame");
}

void engine_states() {
    Engine engine;
    Rng    rng(42);
    print_state(engine, "start");
    dirty_and_clear(engine, rng);
    for (auto [threads, hash] : std::vector<std::pair<int, int>>{
           {2, 16}, {3, 5}, {4, 1}, {8, 3}, {5, 2}, {1, 16}, {7, 1}, {1, 2}})
    {
        setoption(engine, "Threads", std::to_string(threads));
        print_state(engine, "threads");
        setoption(engine, "Hash", std::to_string(hash));
        print_state(engine, "hash");
        dirty_and_clear(engine, rng);
        setoption(engine, "Clear Hash", "");
        print_state(engine, "clear_hash");
    }
}

// get_best_thread() on synthetic root moves: the real one, of the pool of
// an engine with N threads
void best_thread() {
    Engine engine;
    Rng    rng(7);
    // A few moves for the first move of the PVs, so that votes gather
    const Move moves[] = {Move(SQ_E2, SQ_E4), Move(SQ_D2, SQ_D4), Move(SQ_G1, SQ_F3), Move(SQ_C2, SQ_C4),
                          Move(SQ_B1, SQ_C3)};
    u64        digest  = 0;
    int        cases   = 0;
    for (int n : {1, 2, 3, 4, 5, 6, 7, 8, 13, 32})
    {
        setoption(engine, "Threads", std::to_string(n));
        for (int c = 0; c < 4000; ++c)
        {
            std::string line = "best " + std::to_string(n);
            int         kind = rng.range(0, 3);  // how scores are drawn
            for (auto&& th : engine.threads)
            {
                RootMove rm(moves[rng.below(kind == 3 ? 2 : 5)]);
                int      len = rng.range(1, 6);
                for (int i = 1; i < len; ++i)
                    rm.pv.push_back(moves[rng.below(5)]);
                int s = rng.range(0, 9);
                rm.score = s == 0                ? -VALUE_INFINITE
                         : s == 1                ? VALUE_MATE - rng.range(1, 30)
                         : s == 2                ? -VALUE_MATE + rng.range(1, 30)
                         : s == 3                ? VALUE_TB - rng.range(1, 60)
                         : s == 4                ? -VALUE_TB + rng.range(1, 60)
                         : s == 5 || kind == 1   ? rng.range(-20, 20)
                                                 : rng.range(-2000, 2000);
                int x = rng.range(0, 5);
                rm.inexactLower = x == 0;
                rm.inexactUpper = x == 1;
                line += " " + std::to_string(rm.pv[0].raw()) + "/" + std::to_string(rm.score) + "/"
                      + std::to_string(rm.pv.size()) + "/" + std::to_string(x);
                th->worker->rootMoves = RootMoves{rm};
            }
            usize best = engine.threads.get_best_thread()->id();
            line += " -> " + std::to_string(best);
            for (char ch : line)
                digest = (digest ^ u8(ch)) * 0x100000001b3ULL;
            if (c < 25)
                std::cout << line << "\n";
            ++cases;
        }
    }
    std::cout << "best cases " << cases << " digest " << hex64(digest) << "\n";
}

}  // namespace

int main(int argc, char* argv[]) {
    Attacks::init();
    Position::init();

    std::string what = argc > 1 ? argv[1] : "all";
    if (what == "sizes" || what == "all")
        sizes();
    if (what == "engine" || what == "all")
        engine_states();
    if (what == "best" || what == "all")
        best_thread();
    return 0;
}

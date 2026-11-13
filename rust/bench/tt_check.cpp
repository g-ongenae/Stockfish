// Drives the C++ transposition table of src/ (tt.cpp, compiled with
// SEARCH_TRACE for TranspositionTable::trace_entry and trace_digest) through
// random probes, writes, penalties, new searches and hashfull counts, at
// several sizes (some not powers of two), and prints digests of what it
// returns and of the table. rust/examples/tt_check.rs does exactly the same
// with the port, and must print exactly the same lines. This checks the
// packing of the entries, the replacement and ageing rules and the index of
// the clusters (mul_hi64) beyond the 2 MB of the search trace, on every
// target. Built by bench/build_tt_check.sh; not part of the engine.
//
// Everything random comes from one splitmix64 stream, consumed in the same
// order on both sides (never two draws in one C++ expression).

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

#include "misc.h"
#include "memory.h"
#include "thread.h"
#include "types.h"

// The table's storage and size are private: the driver zeroes the storage
// itself, as clear() would with search threads (an empty pool has none).
#define private public
#include "tt.h"
#undef private

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
    u32 below(u32 n) { return u32(((next() >> 32) * u64(n)) >> 32); }
    // uniform in [lo, hi]
    int range(int lo, int hi) { return lo + int(below(u32(hi - lo + 1))); }
};

struct Digest {
    u64  h = 0xCBF29CE484222325ULL;
    void add(i64 v) {
        h = (h ^ u64(v)) * 0x100000001B3ULL;
        h ^= h >> 32;
    }
};

void add_data(Digest& d, const TTData& t) {
    d.add(t.move.raw());
    d.add(t.value);
    d.add(t.eval);
    d.add(t.depth);
    d.add(t.bound);
    d.add(t.is_pv);
}

Value random_value(Rng& rng) {
    switch (rng.below(10))
    {
    case 0 :
        return VALUE_NONE;
    case 1 : {
        int plies = rng.range(0, 300);
        return rng.below(2) ? VALUE_MATE - plies : -VALUE_MATE + plies;
    }
    default :
        return rng.range(-3000, 3000);
    }
}

void run(usize mb, int ops, Rng& rng) {
    ThreadPool         pool;
    TranspositionTable tt;
    tt.resize(mb, pool);
    std::memset(static_cast<void*>(tt.table), 0, tt.clusterCount * 32);
    tt.generation8 = 0;

    // Keys: a few hundred "bases" whose high 32 bits pick a cluster, each with
    // many low halves, so that clusters fill up and entries get replaced;
    // some keys share their low 21 bits with another key of the same cluster
    std::vector<u64> bases(300);
    for (auto& b : bases)
        b = rng.next() & 0xFFFFFFFF00000000ULL;

    Digest d;
    for (int op = 1; op <= ops; ++op)
    {
        u64 key = bases[rng.below(u32(bases.size()))];
        u64 low = rng.next();
        key |= rng.below(4) ? (low & 0x3F) : (low & 0xFFFFFFFF);
        if (rng.below(16) == 0)
            key = rng.next();

        auto [hit, data, writer] = tt.probe(key);
        auto e                   = tt.trace_entry(writer);
        d.add(hit);
        add_data(d, data);
        d.add(i64(e.cluster));
        d.add(e.slot);
        d.add(e.key21);
        add_data(d, e.data);
        d.add(e.generation);

        u32 r = rng.below(100);
        if (r < 70)
        {
            Value v  = random_value(rng);
            bool  pv = rng.below(2);
            Bound b  = Bound(rng.below(4));
            Depth dp = rng.range(-2, 245);
            Move  m  = rng.below(3) ? Move(u16(rng.next())) : Move::none();
            Value ev = rng.below(8) ? rng.range(-2000, 2000) : VALUE_NONE;
            writer.write(key, v, pv, b, dp, m, ev, tt.generation());
        }
        else if (r < 75)
            writer.penalize(rng.range(1, 3));
        else if (r < 77)
            tt.new_search();
        else if (r < 78)
            d.add(tt.hashfull(rng.range(0, 31)));

        if (op % 20000 == 0)
            std::printf("size %zu op %d digest %016llX tt %016llX generation %d hashfull %d\n", mb, op,
                        (unsigned long long) d.h, (unsigned long long) tt.trace_digest(),
                        int(tt.generation()), tt.hashfull());
    }
}

}  // namespace

int main(int argc, char* argv[]) {
    // tt_check <ops per size> <size in MB>...
    if (argc < 3)
    {
        std::cerr << "usage: tt_check <ops> <MB>...\n";
        return 2;
    }
    const int ops = std::stoi(argv[1]);
    Rng       rng(0x9E3779B97F4A7C15ULL);
    for (int i = 2; i < argc; ++i)
        run(usize(std::stoul(argv[i])), ops, rng);
    return 0;
}

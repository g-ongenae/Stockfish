//! The port's side of `bench/tt_check.cpp`: drives the transposition table
//! through the same random stream of probes, writes, penalties, new searches
//! and hashfull counts, at the same sizes, and prints the same lines. See
//! that file, and README.md ("The transposition table").
//!
//!     tt_check <ops per size> <size in MB>...

use stockfish::searchtrace::TraceDigest;
use stockfish::tt::*;
use stockfish::types::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// uniform in [0, n)
    fn below(&mut self, n: u32) -> u32 {
        (((self.next() >> 32) * n as u64) >> 32) as u32
    }

    /// uniform in [lo, hi]
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u32) as i32
    }
}

struct Digest(u64);

impl Digest {
    fn add(&mut self, v: i64) {
        self.0 = (self.0 ^ v as u64).wrapping_mul(0x100000001B3);
        self.0 ^= self.0 >> 32;
    }

    fn add_data(&mut self, t: &TTData) {
        self.add(t.mv.raw() as i64);
        self.add(t.value as i64);
        self.add(t.eval as i64);
        self.add(t.depth as i64);
        self.add(t.bound as i64);
        self.add(t.is_pv as i64);
    }
}

fn random_value(rng: &mut Rng) -> Value {
    match rng.below(10) {
        0 => VALUE_NONE,
        1 => {
            let plies = rng.range(0, 300);
            if rng.below(2) != 0 {
                VALUE_MATE - plies
            } else {
                -VALUE_MATE + plies
            }
        }
        _ => rng.range(-3000, 3000),
    }
}

fn run(mb: usize, ops: u32, rng: &mut Rng) {
    let tt = TranspositionTable::new(mb);

    let bases: Vec<u64> = (0..300).map(|_| rng.next() & 0xFFFF_FFFF_0000_0000).collect();

    let mut d = Digest(TraceDigest::new().value);
    for op in 1..=ops {
        let mut key = bases[rng.below(bases.len() as u32) as usize];
        let low = rng.next();
        match rng.below(4) {
            // 32 hot keys, rewritten often within a generation
            1 => key = bases[(low & 7) as usize] | ((low >> 3) & 0x3),
            0 => key |= low & 0xFFFF_FFFF,
            _ => key |= low & 0x3F,
        }
        if rng.below(16) == 0 {
            key = rng.next();
        }

        let (hit, data, writer) = tt.probe(key);
        let e = tt.trace_entry(writer);
        d.add(hit as i64);
        d.add_data(&data);
        d.add(e.cluster as i64);
        d.add(e.slot as i64);
        d.add(e.key21 as i64);
        d.add_data(&e.data);
        d.add(e.generation as i64);

        let r = rng.below(1000);
        if r < 700 {
            let v = random_value(rng);
            let pv = rng.below(2) != 0;
            let b = rng.below(4) as Bound;
            let dp = if rng.below(2) != 0 { rng.range(-2, 10) } else { rng.range(-2, 245) };
            let m = if rng.below(3) != 0 { Move(rng.next() as u16) } else { Move::none() };
            let ev = if rng.below(8) != 0 { rng.range(-2000, 2000) } else { VALUE_NONE };
            tt.write(writer, key, v, pv, b, dp, m, ev, tt.generation());
        } else if r < 750 {
            let p = rng.range(1, 3);
            tt.penalize(writer, p);
        } else if r < 752 {
            tt.new_search();
        } else if r < 760 {
            let max_age = rng.range(0, 31);
            d.add(tt.hashfull(max_age) as i64);
        }

        if op % 20000 == 0 {
            println!(
                "size {mb} op {op} digest {:016X} tt {:016X} generation {} hashfull {}",
                d.0,
                tt.trace_digest(),
                tt.generation(),
                tt.hashfull(0)
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: tt_check <ops> <MB>...");
        std::process::exit(2);
    }
    let ops: u32 = args[0].parse().expect("a count");
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for a in &args[1..] {
        run(a.parse().expect("a size in MB"), ops, &mut rng);
    }
}

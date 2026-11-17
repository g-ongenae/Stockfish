//! The port's side of bench/surface_check.cpp (see there): the conversion
//! of each UCI_Elo to a level, the levels that enable the skill, the depth
//! at which the move is picked, `pick_best()` on synthetic root moves with
//! the generator seeded as the C++ driver seeds it, and the commands of
//! `speedtest`. Must print the same lines as the C++ driver.
//!
//!   surface_check [lines|digest]

use stockfish::misc::Prng;
use stockfish::search::{RootMove, Skill};
use stockfish::types::*;

struct Out {
    digest: bool,
    h: u64,
    n: u64,
}

impl Out {
    fn line(&mut self, s: &str) {
        if !self.digest {
            println!("{s}");
        }
        for &c in s.as_bytes().iter().chain(b"\n") {
            self.h = (self.h ^ c as u64).wrapping_mul(0x100000001b3);
        }
        self.n += 1;
    }

    fn section(&mut self, name: &str) {
        if self.digest {
            println!("{name} lines {} fnv {:016x}", self.n, self.h);
        }
        self.h = 0xcbf29ce484222325;
        self.n = 0;
    }
}

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: i32) -> i32 {
        (self.next() % n as u64) as i32
    }
}

fn skill_line(what: &str, value: i32, s: &Skill) -> String {
    format!(
        "{what} {value} level {:016x} enabled {} pick_depth {} weakness {}",
        s.level.0.to_bits(),
        s.enabled() as i32,
        1 + s.level.to_i32(),
        (stockfish::fp::F64(120.0) - 2.0 * s.level).to_i32()
    )
}

fn main() {
    let mut out = Out { digest: std::env::args().nth(1).as_deref() == Some("digest"), h: 0xcbf29ce484222325, n: 0 };
    let mut rng = Prng::new(1070372);
    let mut sm = SplitMix(0x9e3779b97f4a7c15);

    for elo in Skill::LOWEST_ELO..=Skill::HIGHEST_ELO {
        out.line(&skill_line("elo", elo, &Skill::new(20, elo)));
    }
    for level in 0..=20 {
        out.line(&skill_line("level", level, &Skill::new(level, 0)));
    }
    out.section("conversion");

    for c in 0..30000 {
        let from_elo = sm.below(2) != 0;
        let param = if from_elo { Skill::LOWEST_ELO + sm.below(1871) } else { sm.below(20) };
        let mut skill = if from_elo { Skill::new(20, param) } else { Skill::new(param, 0) };
        let n = 1 + sm.below(30);
        let mut root_moves = Vec::new();
        let mut score = if sm.below(8) == 0 { VALUE_MATE - 1 - sm.below(40) } else { sm.below(6001) - 3000 };
        for i in 0..n {
            root_moves.push(RootMove::new(Move::new((i % 64) as Square, ((i * 7 + 13) % 64) as Square)));
            let kind = sm.below(4);
            score -= match kind {
                0 => 0,
                1 => sm.below(8),
                2 => sm.below(100),
                _ => sm.below(800),
            };
            root_moves.last_mut().unwrap().score = score.max(-VALUE_INFINITE + 1);
        }
        let multi_pv = n.min(1 + sm.below(8)) as usize;
        let m = skill.pick_best(&root_moves, multi_pv, &mut rng);
        let idx = root_moves.iter().position(|rm| rm.pv[0] == m).unwrap();
        out.line(&format!(
            "case {c} {} {param} moves {n} multipv {multi_pv} pick {idx}",
            if from_elo { "elo" } else { "level" }
        ));
    }
    out.section("pick_best");

    // The commands of speedtest (always with a thread count: the default is
    // the machine's)
    for a in [
        "1 16 1",
        "1 16 2",
        "1 16 3",
        "1 16 7",
        "1 16 10",
        "1 16 60",
        "1 16 150",
        "1 16 151",
        "1 16 999",
        "1 16 3600",
        "1 16 86400",
        "1 16 2147483",
        "2 1 5",
        "0 0 0",
        "3 64",
        "4",
        "1 16 -5",
        "1 16 99999999999",
        "1 x 5",
        "2 3 4x",
    ] {
        let args: Vec<&[u8]> = a.split_whitespace().map(|t| t.as_bytes()).collect();
        let hw = stockfish::numa::system_threads_nb() as i64;
        let setup = stockfish::benchmark::setup_benchmark(
            &args,
            hw,
            stockfish::engine::max_threads(),
            stockfish::engine::MAX_HASH_MB,
        );
        out.line(&format!(
            "args '{a}' threads {} hash {} original '{}' filled '{}' commands {}",
            setup.threads,
            setup.tt_size,
            setup.original_invocation,
            setup.filled_invocation,
            setup.commands.len()
        ));
        for c in &setup.commands {
            out.line(c);
        }
    }
    out.section("speedtest");
}

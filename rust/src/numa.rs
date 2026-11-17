//! The part of `src/numa.h` that `NumaPolicy` prints: the configuration of
//! processors in nodes, parsed from the option or taken from the system,
//! printed as C++ prints it, and which threads C++ would bind to which node.
//!
//! Nothing is bound and nothing is replicated: the port runs every thread
//! unbound with one network and one set of shared histories, whatever the
//! configuration says (README.md, "Threads", and INVENTORY.md, section 3).
//! The system configuration is that of a machine with one NUMA node: on
//! Linux the processors the process may use (or, for `hardware`, every
//! processor online), elsewhere processors 0 to N-1. C++ also splits a node
//! into L3 domains in bundles of 32 threads; on a machine where that gives
//! more than one domain, or with several NUMA nodes, the port prints one node
//! where C++ prints several.

use std::collections::BTreeSet;

/// `std::thread::hardware_concurrency()` (libstdc++: `get_nprocs()`, the
/// processors online), at least 1: C++'s `SYSTEM_THREADS_NB`.
pub fn system_threads_nb() -> usize {
    if let Some(cpus) = read_cpu_list("/sys/devices/system/cpu/online") {
        if !cpus.is_empty() {
            return cpus.len();
        }
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

fn read_cpu_list(path: &str) -> Option<Vec<usize>> {
    let s = std::fs::read_to_string(path).ok()?;
    let s: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    Some(indices_from_shortened_string(&s))
}

/// The processors this process may use: `Cpus_allowed_list` of
/// `/proc/self/status` on Linux (the affinity C++ reads at start-up with
/// `sched_getaffinity`).
fn allowed_cpus() -> Option<Vec<usize>> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find_map(|l| l.strip_prefix("Cpus_allowed_list:"))?;
    let l: String = line.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    Some(indices_from_shortened_string(&l))
}

/// `str_to_size_t()`: a decimal number without sign, nothing after it but
/// white space.
fn str_to_size_t(s: &str) -> Option<usize> {
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    // strtoull skips leading white space and takes a '+'
    let t = s.trim_start_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r']);
    let t = t.strip_prefix('+').unwrap_or(t);
    let digits = t.bytes().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let rest = &t[digits..];
    if let Some(c) = rest.bytes().next() {
        if !matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r') {
            return None;
        }
    }
    t[..digits].parse::<u64>().ok().and_then(|v| usize::try_from(v).ok())
}

/// `split()` of `misc.h`: the pieces between the delimiters, the last one
/// included even if empty; an empty string gives no piece.
fn split<'a>(s: &'a str, delim: &str) -> Vec<&'a str> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut res = Vec::new();
    let mut begin = 0;
    while let Some(i) = s[begin..].find(delim) {
        res.push(&s[begin..begin + i]);
        begin += i + delim.len();
    }
    res.push(&s[begin..]);
    res
}

/// `indices_from_shortened_string()`: "0-3,8,10-11".
fn indices_from_shortened_string(s: &str) -> Vec<usize> {
    let mut indices = Vec::new();
    if s.is_empty() {
        return indices;
    }
    for ss in split(s, ",") {
        if ss.is_empty() {
            continue;
        }
        let parts = split(ss, "-");
        if parts.len() == 1 {
            if let Some(c) = str_to_size_t(parts[0]) {
                indices.push(c);
            }
        } else if parts.len() == 2 {
            const MAX_INDICES: usize = 1 << 20; // prevent oom
            if let (Some(first), Some(last)) = (str_to_size_t(parts[0]), str_to_size_t(parts[1])) {
                // (unsigned subtraction, as in C++: a reversed range wraps
                // and is refused)
                if last.wrapping_sub(first) < MAX_INDICES {
                    indices.extend(first..=last);
                }
            }
        }
    }
    indices
}

pub struct NumaConfig {
    nodes: Vec<BTreeSet<usize>>,
    assigned: BTreeSet<usize>,
    custom_affinity: bool,
}

impl NumaConfig {
    fn empty() -> NumaConfig {
        NumaConfig { nodes: Vec::new(), assigned: BTreeSet::new(), custom_affinity: false }
    }

    /// `NumaConfig()`: one node of processors 0 to `SYSTEM_THREADS_NB - 1`
    /// (`NumaPolicy none`).
    pub fn single() -> NumaConfig {
        let mut cfg = NumaConfig::empty();
        for c in 0..system_threads_nb() {
            cfg.add_cpu_to_node(0, c);
        }
        cfg
    }

    /// `from_system()`, for a machine with one node: the processors the
    /// process may use, or with `respect_affinity` false every processor
    /// online (`hardware`, then a custom configuration).
    pub fn from_system(respect_affinity: bool) -> NumaConfig {
        let cpus = if respect_affinity {
            allowed_cpus()
        } else {
            read_cpu_list("/sys/devices/system/node/node0/cpulist")
                .or_else(|| read_cpu_list("/sys/devices/system/cpu/online"))
        };
        let mut cfg = NumaConfig::empty();
        match cpus {
            Some(cpus) if !cpus.is_empty() => {
                for c in cpus {
                    cfg.add_cpu_to_node(0, c);
                }
            }
            _ => {
                for c in 0..system_threads_nb() {
                    cfg.add_cpu_to_node(0, c);
                }
            }
        }
        cfg.nodes.retain(|n| !n.is_empty());
        if !respect_affinity {
            cfg.custom_affinity = true;
        }
        cfg
    }

    /// `from_string()`: nodes separated by ':', processors by ',', ranges
    /// "first-last". None when no node is given, or a processor repeats.
    pub fn from_string(s: &str) -> Option<NumaConfig> {
        let mut cfg = NumaConfig::empty();
        let mut n = 0;
        for node_str in split(s, ":") {
            let indices = indices_from_shortened_string(node_str);
            if !indices.is_empty() {
                for idx in indices {
                    if !cfg.add_cpu_to_node(n, idx) {
                        return None;
                    }
                }
                n += 1;
            }
        }
        if n == 0 {
            return None; // failed to parse any nodes
        }
        cfg.custom_affinity = true;
        Some(cfg)
    }

    fn add_cpu_to_node(&mut self, n: usize, c: usize) -> bool {
        if !self.assigned.insert(c) {
            return false;
        }
        while self.nodes.len() <= n {
            self.nodes.push(BTreeSet::new());
        }
        self.nodes[n].insert(c);
        true
    }

    pub fn num_numa_nodes(&self) -> usize {
        self.nodes.len()
    }

    pub fn num_cpus_in_numa_node(&self, n: usize) -> usize {
        self.nodes[n].len()
    }

    pub fn requires_memory_replication(&self) -> bool {
        self.custom_affinity || self.nodes.len() > 1
    }

    /// `to_string()`: "0-15,128-143:16-31".
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        for (i, cpus) in self.nodes.iter().enumerate() {
            if i > 0 {
                s.push(':');
            }
            let v: Vec<usize> = cpus.iter().copied().collect();
            let mut first_set = true;
            let mut start = 0;
            for j in 0..v.len() {
                if j + 1 == v.len() || v[j + 1] != v[j] + 1 {
                    if !first_set {
                        s.push(',');
                    }
                    if j != start {
                        s += &format!("{}-{}", v[start], v[j]);
                    } else {
                        s += &v[j].to_string();
                    }
                    start = j + 1;
                    first_set = false;
                }
            }
        }
        s
    }

    /// `suggests_binding_threads()`.
    pub fn suggests_binding_threads(&self, num_threads: usize) -> bool {
        if self.custom_affinity {
            return true;
        }
        if num_threads <= 1 {
            return false;
        }
        let largest = self.nodes.iter().map(|n| n.len()).max().unwrap_or(0);
        let is_small = |n: &BTreeSet<usize>| n.len() as f64 / largest as f64 <= 0.6;
        let not_small = self.nodes.iter().filter(|n| !is_small(n)).count();
        (num_threads > largest / 2 || num_threads >= not_small * 4) && self.nodes.len() > 1
    }

    /// `distribute_threads_among_numa_nodes()`: the node of each thread,
    /// each in turn to the least filled node (`float` fill ratios, the first
    /// node on a tie).
    pub fn distribute_threads_among_numa_nodes(&self, num_threads: usize) -> Vec<usize> {
        if self.nodes.len() == 1 {
            return vec![0; num_threads];
        }
        let mut occupation = vec![0usize; self.nodes.len()];
        let mut ns = Vec::with_capacity(num_threads);
        for _ in 0..num_threads {
            let mut best = 0;
            let mut best_fill = f32::MAX;
            for (n, cpus) in self.nodes.iter().enumerate() {
                // two integers below 2^24 and one IEEE division: the same
                // on every target
                let fill = (occupation[n] + 1) as f32 / cpus.len() as f32;
                if fill < best_fill {
                    best = n;
                    best_fill = fill;
                }
            }
            ns.push(best);
            occupation[best] += 1;
        }
        ns
    }
}

/// The NUMA state of the engine that its messages depend on: the
/// configuration, the node C++ would bind each thread to (empty: none bound,
/// `boundThreadToNumaNode`), and which replicas of the network C++ would
/// have allocated (`LazyNumaReplicatedSystemWide`).
pub struct NumaState {
    pub config: NumaConfig,
    pub bound_thread_to_numa_node: Vec<usize>,
    pub replicas: Vec<bool>,
}

impl NumaState {
    pub fn new() -> NumaState {
        let mut s =
            NumaState { config: NumaConfig::from_system(true), bound_thread_to_numa_node: Vec::new(), replicas: Vec::new() };
        s.prepare_replicas();
        s
    }

    /// `set_numa_config_from_option()`: false for a value that is not
    /// understood (compared with case, as C++ compares the strings).
    pub fn set_config_from_option(&mut self, o: &str) -> bool {
        let cfg = match o {
            "auto" | "system" => NumaConfig::from_system(true),
            "hardware" => NumaConfig::from_system(false),
            "none" => NumaConfig::single(),
            _ => match NumaConfig::from_string(o) {
                Some(c) => c,
                None => return false,
            },
        };
        self.config = cfg;
        self.prepare_replicas();
        true
    }

    /// `ThreadPool::set()`: whether C++ binds the threads, and where. The
    /// policy is the value of the option as stored, refused values included.
    pub fn set_threads(&mut self, policy: &str, requested: usize) {
        let bind = match policy {
            "none" => false,
            "auto" => self.config.suggests_binding_threads(requested),
            _ => true,
        };
        self.bound_thread_to_numa_node =
            if bind { self.config.distribute_threads_among_numa_nodes(requested) } else { Vec::new() };
    }

    /// Whether C++ could bind its threads where `set_threads()` says. On
    /// Linux `sched_setaffinity` refuses a set of processors none of which
    /// is online, and C++ then exits with 1 without a message (a list naming
    /// processors the machine lacks, `NumaPolicy 5` on a machine with 2).
    /// The port binds nothing but exits likewise; elsewhere C++ binds
    /// nothing either.
    pub fn binding_possible(&self) -> bool {
        if !cfg!(target_os = "linux") || self.bound_thread_to_numa_node.is_empty() {
            return true;
        }
        let Some(online) = read_cpu_list("/sys/devices/system/cpu/online") else { return true };
        self.bound_thread_to_numa_node
            .iter()
            .all(|&n| self.config.nodes[n].iter().any(|c| online.contains(c)))
    }

    /// `prepare_replicate_from()`: the first replica only, on a new
    /// configuration or network.
    pub fn prepare_replicas(&mut self) {
        let n = if self.config.requires_memory_replication() { self.config.num_numa_nodes() } else { 1 };
        self.replicas = vec![false; n];
        self.replicas[0] = true;
    }

    /// `ensure_network_replicated()`: the replica of the node of each
    /// thread (node 0 for threads not bound).
    pub fn ensure_replicated(&mut self, num_threads: usize) {
        for i in 0..num_threads {
            let n = self.bound_thread_to_numa_node.get(i).copied().unwrap_or(0);
            if n < self.replicas.len() {
                self.replicas[n] = true;
            }
        }
    }

    /// `get_bound_thread_count_by_numa_node()` with the processors of each
    /// node: "bound/cpus" separated by ':', empty when nothing is bound.
    pub fn thread_binding_information(&self) -> String {
        if self.bound_thread_to_numa_node.is_empty() {
            return String::new();
        }
        let highest = *self.bound_thread_to_numa_node.iter().max().unwrap();
        let mut counts = vec![0usize; highest + 1];
        for &n in &self.bound_thread_to_numa_node {
            counts[n] += 1;
        }
        let mut parts = Vec::new();
        for n in 0..self.config.num_numa_nodes().max(counts.len()) {
            let current = counts.get(n).copied().unwrap_or(0);
            parts.push(format!("{current}/{}", self.config.num_cpus_in_numa_node(n)));
        }
        parts.join(":")
    }

    /// The status lines of the replicas, `verify_network()`. The port holds
    /// one network in the memory of its process, never shared: what C++
    /// prints where it has no shared memory.
    pub fn replica_statuses(&self) -> Vec<String> {
        self.replicas
            .iter()
            .enumerate()
            .map(|(i, &allocated)| {
                let status = if allocated {
                    "Local memory. Shared memory not supported by the OS. Local allocation fallback."
                } else {
                    "No allocation."
                };
                format!("Network replica {}: {status}", i + 1)
            })
            .collect()
    }
}

impl Default for NumaState {
    fn default() -> Self {
        NumaState::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_print_like_cpp() {
        // (value, C++'s configuration on a machine with CPUs 0-1, or None
        // for a refused value), as the reference binary prints them
        for (v, expected) in [
            ("0", Some("0")),
            ("1", Some("1")),
            ("0-1", Some("0-1")),
            ("0:1", Some("0:1")),
            ("1:0", Some("1:0")),
            ("5", Some("5")),
            ("0,0", None),
            ("0:0", None),
            ("garbage", None),
            ("0-", None),
            ("-1", None),
            ("99999999999999999999", None),
            ("0-1048577", None),
            ("0-1048575", Some("0-1048575")),
            (",", None),
            (":", None),
            ("3-1", None),
            ("0;1", None),
            ("0,2-4,7:9", Some("0,2-4,7:9")),
            ("::0::", Some("0")),
            ("0-1-2", None),
        ] {
            assert_eq!(NumaConfig::from_string(v).map(|c| c.to_string()).as_deref(), expected, "{v}");
        }
    }

    #[test]
    fn binding_messages_like_cpp() {
        let mut s = NumaState::new();
        assert!(s.set_config_from_option("0:1"));
        s.set_threads("0:1", 3);
        assert_eq!(s.thread_binding_information(), "2/1:1/1");
        s.set_threads("0:1", 1);
        assert_eq!(s.thread_binding_information(), "1/1:0/1");
        s.ensure_replicated(1);
        assert_eq!(s.replicas, vec![true, false]);
        assert!(s.set_config_from_option("none"));
        s.set_threads("none", 3);
        assert_eq!(s.thread_binding_information(), "");
        assert!(!s.set_config_from_option("NONE"));
    }
}

//! Port of `src/engine.{h,cpp}`: the engine behind the UCI loop. It owns
//! the position, the options, the transposition table, the shared histories,
//! the network and the thread pool, as in C++.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

use crate::history::SharedHistories;
use crate::nnue::network::EVAL_FILE_DEFAULT_NAME;
use crate::nnue::Network;
use crate::numa::NumaState;
use crate::sync_println;
use crate::position::{Position, PositionSetError};
use crate::search::*;
use crate::searchtrace::{self, json_string, parse_session, trim, SearchTraceWindow, SearchTracer};
use crate::thread::ThreadPool;
use crate::timeman::now;
use crate::tt::TranspositionTable;
use crate::types::*;
use crate::uci::{self, START_FEN};
use crate::ucioption::{OptionsMap, UciOption};

/// `MaxHashMB`: the largest hash the option accepts.
pub const MAX_HASH_MB: i64 = if cfg!(target_pointer_width = "64") { 33554432 } else { 2048 };

/// `MaxThreads`: max(1024, 4 × `hardware_concurrency()`).
pub fn max_threads() -> i64 {
    1024.max(4 * crate::numa::system_threads_nb() as i64)
}

pub struct Engine {
    pub pos: Position,
    pub options: OptionsMap,
    pub threads: ThreadPool,
    tt: Arc<TranspositionTable>,
    /// The network and the name it was loaded for (C++'s
    /// `EvalFile::current`). Read from a file rather than embedded (see
    /// README.md, "The network"), on first use rather than at start-up.
    network: Option<(Vec<u8>, Arc<Network>)>,
    /// Whether the load C++ makes at start-up (the default network) has
    /// been made.
    initial_network_tried: bool,
    /// Where the network is looked for after the working directory: the
    /// directory of `argv[0]` (`CommandLine::get_binary_directory`).
    binary_directory: std::path::PathBuf,
    /// Whether verify_network() prints (C++'s `onVerifyNetwork` listener,
    /// silenced by `speedtest`).
    pub verify_output: bool,
    /// What NumaPolicy prints (no thread is bound: see numa.rs).
    pub numa: NumaState,
    update_context: Arc<UpdateContext>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.wait_for_search_finished();
    }
}

impl Engine {
    pub fn new() -> Engine {
        let mut pos = Position::new();
        pos.set(START_FEN.as_bytes(), false).expect("start position");

        let max_threads = max_threads();

        let mut options = OptionsMap::default();
        options.add(UciOption::string("Debug Log File", ""));
        options.add(UciOption::string("NumaPolicy", "auto"));
        options.add(UciOption::spin("Threads", 1, 1, max_threads));
        options.add(UciOption::spin("Hash", 16, 1, MAX_HASH_MB));
        options.add(UciOption::button("Clear Hash"));
        options.add(UciOption::check("Ponder", false));
        options.add(UciOption::spin("MultiPV", 1, 1, MAX_MOVES as i64));
        options.add(UciOption::spin("Skill Level", 20, 0, 20));
        options.add(UciOption::spin("Move Overhead", 10, 0, 5000));
        options.add(UciOption::spin("nodestime", 0, 0, 10000));
        options.add(UciOption::check("UCI_Chess960", false));
        options.add(UciOption::check("UCI_LimitStrength", false));
        options.add(UciOption::spin(
            "UCI_Elo",
            Skill::LOWEST_ELO as i64,
            Skill::LOWEST_ELO as i64,
            Skill::HIGHEST_ELO as i64,
        ));
        options.add(UciOption::check("UCI_ShowWDL", false));
        options.add(UciOption::string("SyzygyPath", ""));
        options.add(UciOption::spin("SyzygyProbeDepth", 1, 1, 100));
        options.add(UciOption::check("Syzygy50MoveRule", true));
        options.add(UciOption::spin("SyzygyProbeLimit", 7, 0, 7));
        options.add(UciOption::string("EvalFile", EVAL_FILE_DEFAULT_NAME));

        let mut e = Engine {
            pos,
            options,
            threads: ThreadPool::new(),
            // (made by set_tt_size(), from resize_threads(), as in C++)
            tt: Arc::new(TranspositionTable::new(0)),
            network: None,
            initial_network_tried: false,
            binary_directory: binary_directory(std::env::args_os().next()),
            verify_output: true,
            numa: NumaState::new(),
            update_context: Arc::new(default_update_context()),
        };
        e.resize_threads();
        e
    }

    pub fn chess960(&self) -> bool {
        self.options.int("UCI_Chess960") != 0
    }

    /// `setoption`: sets an option and does what C++ does on its change.
    /// Returns what C++'s `on_change` returns, which the UCI loop prints as
    /// an `info string` (C++'s info listener).
    pub fn setoption(&mut self, name: &str, value: &str) -> Option<String> {
        self.wait_for_search_finished();
        match self.options.setoption(name, value) {
            Some("Debug Log File") => crate::io::start_logger(self.options.string("Debug Log File").as_bytes()),
            Some("NumaPolicy") => {
                let o = self.options.string("NumaPolicy").to_string();
                if !self.numa.set_config_from_option(&o) {
                    return Some(format!("NumaPolicy: invalid value '{o}', keeping previous config."));
                }
                // Force reallocation of threads in case affinities need to
                // change
                self.resize_threads();
                return Some(format!(
                    "{}\n{}",
                    self.numa_config_information_as_string(),
                    self.thread_allocation_information_as_string()
                ));
            }
            Some("Threads") => {
                self.resize_threads();
                return Some(self.thread_allocation_information_as_string());
            }
            Some("Hash") => self.set_tt_size(self.options.int("Hash") as usize),
            Some("Clear Hash") => self.search_clear(),
            // C++ loads the network at once and clears the workers
            // (load_network()), on every accepted value, the same one
            // included: so does the port, so that the next go does not spend
            // its clock on it. The load prints nothing; a failure is reported
            // by the next verify_network(), as before.
            Some("EvalFile") => {
                self.threads.clear();
                self.load_network();
                self.numa.prepare_replicas();
                self.numa.ensure_replicated(self.threads.size());
            }
            Some("SyzygyPath") => {
                if !self.options.string("SyzygyPath").is_empty() {
                    return Some("Tablebases are not supported by this port".into());
                }
            }
            _ => {}
        }
        None
    }

    /// `resize_threads()`: recreates the threads, their shared histories
    /// and their workers, cleared, then reallocates the hash.
    pub fn resize_threads(&mut self) {
        self.wait_for_search_finished();
        let network = self.network.as_ref().map(|(_, n)| n.clone());
        let requested = self.options.int("Threads") as usize;
        self.threads.set(requested, &self.tt, network, self.update_context.clone());
        self.numa.set_threads(self.options.string("NumaPolicy"), requested);
        if !self.numa.binding_possible() {
            std::process::exit(1);
        }

        // Reallocate the hash with the new threadpool size
        self.set_tt_size(self.options.int("Hash") as usize);
        self.numa.ensure_replicated(self.threads.size());
    }

    /// `thread_allocation_information_as_string()`, with the binding C++
    /// would make (the port binds nothing: see numa.rs).
    pub fn thread_allocation_information_as_string(&self) -> String {
        let n = self.threads.size();
        let mut s = format!("Using {n} {}", if n > 1 { "threads" } else { "thread" });
        let bound = self.numa.thread_binding_information();
        if !bound.is_empty() {
            s += " with NUMA node thread binding: ";
            s += &bound;
        }
        s
    }

    /// `numa_config_information_as_string()`.
    pub fn numa_config_information_as_string(&self) -> String {
        format!("Available processors: {}", self.numa.config.to_string())
    }

    /// `thread_binding_information_as_string()`.
    pub fn thread_binding_information_as_string(&self) -> String {
        self.numa.thread_binding_information()
    }

    /// `set_tt_size()`: a new table of `mb` MiB, cleared.
    pub fn set_tt_size(&mut self, mb: usize) {
        self.wait_for_search_finished();
        // C++ frees the old table before allocating the new one
        self.tt = Arc::new(TranspositionTable::new(0));
        let tt = self.tt.clone();
        self.threads.for_each_worker(|w| w.tt = tt.clone());
        // Allocated zeroed, which is what C++'s clear() then leaves
        self.tt = Arc::new(TranspositionTable::new(mb));
        let tt = self.tt.clone();
        self.threads.for_each_worker(|w| w.tt = tt.clone());
    }

    pub fn wait_for_search_finished(&mut self) {
        self.threads.wait_for_search_finished();
    }

    pub fn stop(&self) {
        self.threads.flags.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_ponderhit(&self, b: bool) {
        self.threads.flags.ponder.store(b, std::sync::atomic::Ordering::Relaxed);
    }

    /// `search_clear()`: what `ucinewgame` does.
    pub fn search_clear(&mut self) {
        self.wait_for_search_finished();
        self.tt.clear_with(&self.threads);
        self.threads.clear();
    }

    /// `set_position()`: the position of a FEN and moves, with the history
    /// of the moves (for repetitions). Left unchanged on an error.
    pub fn set_position(&mut self, fen: &[u8], moves: &[&[u8]]) -> Result<(), PositionSetError> {
        let mut pos = Position::new();
        pos.set(fen, self.chess960())?;
        for &t in moves {
            let m = uci::to_move(&pos, t);
            if m == Move::none() {
                let mut msg = b"Illegal move: ".to_vec();
                msg.extend_from_slice(t);
                return Err(PositionSetError(msg));
            }
            pos.do_move(m);
        }
        self.pos = pos;
        Ok(())
    }

    /// The options the search reads.
    fn search_options(&self) -> SearchOptions {
        SearchOptions {
            multi_pv: self.options.int("MultiPV") as usize,
            skill_level: self.options.int("Skill Level") as i32,
            limit_strength: self.options.int("UCI_LimitStrength") != 0,
            uci_elo: self.options.int("UCI_Elo") as i32,
            show_wdl: self.options.int("UCI_ShowWDL") != 0,
            move_overhead: self.options.int("Move Overhead") as i32,
            nodestime: self.options.int("nodestime") as i32,
            ponder: self.options.int("Ponder") != 0,
            syzygy_50_move_rule: self.options.int("Syzygy50MoveRule") != 0,
        }
    }

    /// `go()`: starts a search of the current position; returns at once.
    pub fn go(&mut self, limits: LimitsType) {
        debug_assert!(limits.perft == 0);
        self.verify_network();
        // the threads reach the replica of their node in the search
        self.numa.ensure_replicated(self.threads.size());
        let options = self.search_options();
        self.threads.start_thinking(&self.pos, limits, options, false);
    }

    /// Replaces the callbacks of the search output.
    pub fn set_update_context(&mut self, ctx: UpdateContext) {
        self.set_update_context_arc(Arc::new(ctx));
    }

    /// The file of the `EvalFile` option, the default name if empty.
    fn eval_file(&self) -> Vec<u8> {
        let path = self.options.string("EvalFile").as_bytes().to_vec();
        if path.is_empty() {
            EVAL_FILE_DEFAULT_NAME.as_bytes().to_vec()
        } else {
            path
        }
    }

    /// `Network::load()` of a name: unless it is the one loaded, the file is
    /// looked for as given (relative to the working directory), then in the
    /// directory of `argv[0]`, then in `DEFAULT_NNUE_DIRECTORY` if the build
    /// environment named one. A network that cannot be read leaves the one
    /// loaded in place, as in C++. When a network is loaded the workers get
    /// it and are cleared. Prints nothing. (C++'s default name means its
    /// embedded network first; the port has none, as a C++ build with
    /// `NNUE_EMBEDDING_OFF`.)
    fn load_network_named(&mut self, path: Vec<u8>) {
        if self.network.as_ref().map(|(p, _)| p) == Some(&path) {
            return;
        }
        let mut dirs = vec![std::path::PathBuf::new(), self.binary_directory.clone()];
        if let Some(d) = option_env!("DEFAULT_NNUE_DIRECTORY") {
            dirs.push(std::path::PathBuf::from(d));
        }
        for dir in dirs {
            let Ok(file) = File::open(dir.join(uci::path_from_bytes(&path))) else { continue };
            if let Some(net) = Network::load(BufReader::with_capacity(1 << 16, file)) {
                let net = Arc::new(net);
                self.network = Some((path, net.clone()));
                self.threads.for_each_worker(|w| w.set_network(net.clone()));
                self.threads.clear();
                return;
            }
        }
    }

    /// The load of the default network that C++ makes when the engine is
    /// created, made here on first need.
    fn ensure_initial_network(&mut self) {
        if !self.initial_network_tried {
            self.initial_network_tried = true;
            self.load_network_named(EVAL_FILE_DEFAULT_NAME.as_bytes().to_vec());
        }
    }

    /// `load_network()`: the network of the `EvalFile` option.
    fn load_network(&mut self) {
        self.ensure_initial_network();
        let path = self.eval_file();
        self.load_network_named(path);
    }

    /// At `isready`, loads the network if none is loaded yet, so that the
    /// first `go` does not spend its clock on it: C++ loads it when the
    /// engine is created, and the UCI protocol sends `isready` before a
    /// search for such initialisation. Prints nothing; a failure is reported
    /// by the next verify_network(). Never while a network is loaded, so
    /// never while a search runs.
    pub fn preload_network(&mut self) {
        if self.network.is_none() {
            self.load_network();
        }
    }

    /// The network for a search or a dump, loaded if needed, without the
    /// messages of verify_network().
    pub fn network(&mut self) -> Option<Arc<Network>> {
        self.load_network();
        self.network.as_ref().map(|(_, n)| n.clone())
    }

    /// `Engine::verify_network()`: unless the network of the `EvalFile`
    /// option is the one loaded, terminates as C++ does; otherwise announces
    /// it, then the status of each replica, at every call, as C++ does.
    pub fn verify_network(&mut self) -> Arc<Network> {
        self.load_network();
        let path = self.eval_file();
        let name = String::from_utf8_lossy(&path).into_owned();
        match &self.network {
            Some((current, net)) if *current == path => {
                if self.verify_output {
                    sync_println!(
                        "info string NNUE evaluation using {name} ({}MiB, ({}, {}, {}, {}, 1))",
                        net.reported_size_mib(),
                        crate::nnue::feature_transformer::INPUT_DIMENSIONS,
                        crate::nnue::L1,
                        crate::nnue::L2,
                        crate::nnue::L3
                    );
                }
            }
            _ => {
                if self.verify_output {
                    let msg = [
                        "Network evaluation parameters compatible with the engine must be available.".to_string(),
                        format!("The network file {name} was not loaded successfully."),
                        "The UCI option EvalFile might need to specify the full path, including the directory name, to the network file.".to_string(),
                        format!("The default net can be downloaded from: https://tests.stockfishchess.org/api/nn/{EVAL_FILE_DEFAULT_NAME}"),
                        "The engine will be terminated now.".to_string(),
                    ];
                    let mut out = String::new();
                    for line in msg {
                        out += &format!("info string ERROR: {line}\n");
                    }
                    crate::io::write_out(out.as_bytes());
                }
                std::process::exit(1);
            }
        }
        if self.verify_output {
            for line in self.numa.replica_statuses() {
                sync_println!("info string {line}");
            }
        }
        self.network.as_ref().unwrap().1.clone()
    }

    /// `Engine::save_network()` (`export_net`): writes the network to the
    /// file, by default (only for the default network) to the default name
    /// in the working directory, byte for byte the file it was read from.
    pub fn save_network(&mut self, file: Option<&[u8]>) {
        self.ensure_initial_network();
        match &self.network {
            None => sync_println!(
                "Failed to export a net. No network file is currently loaded. Please load a network file first."
            ),
            Some((current, _)) if file.is_none() && current.as_slice() != EVAL_FILE_DEFAULT_NAME.as_bytes() => {
                sync_println!("Failed to export a net. A non-embedded net can only be saved if the filename is specified")
            }
            Some((_, net)) => {
                let name = file.unwrap_or(EVAL_FILE_DEFAULT_NAME.as_bytes());
                let saved = File::create(uci::path_from_bytes(name)).and_then(|f| {
                    let mut w = io::BufWriter::with_capacity(1 << 16, f);
                    net.save(&mut w)?;
                    w.flush()
                });
                if saved.is_ok() {
                    sync_println!("Network saved successfully to {}", String::from_utf8_lossy(name));
                } else {
                    sync_println!("Failed to export a net");
                }
            }
        }
        // modify_and_replicate()
        self.numa.prepare_replicas();
    }

    /// `Engine::trace_eval()` (`eval`): the evaluation of the position set
    /// again from its FEN (its history dropped, as in C++).
    pub fn trace_eval(&mut self) {
        let mut p = Position::new();
        let _ = p.set(self.pos.fen().as_bytes(), self.chess960());
        let network = self.verify_network();
        sync_println!("\n{}", crate::evaluate::trace(&p, &network));
    }

    /// `Engine::visualize()` (`d`).
    pub fn visualize(&self) -> String {
        self.pos.visualize()
    }

    pub fn tt(&self) -> &TranspositionTable {
        &self.tt
    }

    pub fn shared_histories(&self) -> &SharedHistories {
        self.threads.shared_histories()
    }

    // -----------------------------------------------------------------------
    // searchtrace

    /// `Engine::search_trace()`: runs the sessions of a file with the trace
    /// (see `searchtrace.rs` and the README of `tests/differential`). Returns
    /// false if the file cannot be read.
    pub fn search_trace(&mut self, file: &[u8], window: SearchTraceWindow) -> bool {
        let Ok(f) = File::open(uci::path_from_bytes(file)) else { return false };
        let mut input = BufReader::new(f);

        self.verify_network();

        // The options the trace depends on, fixed to a deterministic search
        for (name, value) in [
            ("Threads", "1"),
            ("Hash", "2"),
            ("MultiPV", "1"),
            ("Skill Level", "20"),
            ("UCI_LimitStrength", "false"),
            ("UCI_ShowWDL", "false"),
            ("SyzygyPath", "<empty>"),
            ("nodestime", "0"),
            ("Ponder", "false"),
        ] {
            if let Some(info) = self.setoption(name, value) {
                uci::print_info_string(&info);
            }
        }

        let chess960 = self.chess960();
        let mut tracer = Box::new(SearchTracer::new(chess960, searchtrace::CHECKPOINT_INTERVAL));
        if window.counts {
            tracer.count_kinds();
        }

        #[derive(Default)]
        struct Collected {
            infos: Vec<String>,
            bestmove: String,
            ponder: String,
        }
        let collected = Arc::new(Mutex::new(Collected::default()));

        let saved_context = self.update_context.clone();
        let (c1, c2, c3) = (collected.clone(), collected.clone(), collected.clone());
        self.set_update_context(UpdateContext {
            on_update_no_moves: Box::new(move |i: &InfoShort| {
                c1.lock().unwrap().infos.push(format!("depth {} score {}", i.depth, uci::format_score(&i.score)));
            }),
            on_update_full: Box::new(move |i: &InfoFull| {
                let mut s = format!(
                    "depth {} seldepth {} multipv {} score {}",
                    i.depth,
                    i.sel_depth,
                    i.multi_pv,
                    uci::format_score(&i.score)
                );
                if !i.bound.is_empty() {
                    s.push(' ');
                    s += i.bound;
                }
                s += &format!(" nodes {} hashfull {} tbhits {} pv {}", i.nodes, i.hashfull, i.tb_hits, i.pv);
                c2.lock().unwrap().infos.push(s);
            }),
            on_iter: Box::new(|_| {}),
            on_bestmove: Box::new(move |bm: &str, p: &str| {
                let mut c = c3.lock().unwrap();
                c.bestmove = bm.to_string();
                c.ponder = p.to_string();
            }),
            on_start: Box::new(|| {}),
        });

        let mut line = Vec::new();
        let mut session = 0;
        let mut read_error = false;

        loop {
            line.clear();
            match input.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => {
                    read_error = true;
                    break;
                }
            }
            let l = trim(&line).to_vec();
            if l.is_empty() || l[0] == b'#' {
                continue;
            }
            session += 1;

            let mut commands = Vec::new();
            if let Some(error) = parse_session(&l, &mut commands) {
                sync_println!("{{\"session\":{session},\"error\":{},\"input\":{}}}",
                    json_string(error.as_bytes()),
                    json_string(&l)
                );
                continue;
            }

            // Each session starts from a cleared state, as after 'ucinewgame'
            self.search_clear();

            let mut search = 0;
            for cmd in &commands {
                if !cmd.is_go {
                    let moves: Vec<&[u8]> = cmd.moves.iter().map(|m| m.as_slice()).collect();
                    if let Err(e) = self.set_position(&cmd.fen, &moves) {
                        sync_println!("{{\"session\":{session},\"search\":{},\"error\":{},\"input\":{}}}",
                            search + 1,
                            json_string(e.what()),
                            json_string(&cmd.text)
                        );
                        break;
                    }
                    continue;
                }

                search += 1;
                *collected.lock().unwrap() = Collected::default();

                tracer.begin_search();
                if search == window.search || window.search == -1 {
                    tracer.set_window(window.from, window.to, true);
                } else {
                    tracer.set_window(0, 0, false);
                }

                let root_fen = self.pos.fen();
                let limits =
                    LimitsType { depth: cmd.depth, nodes: cmd.nodes, start_time: now(), ..LimitsType::default() };

                self.network();
                self.threads.main_worker().tracer = Some(tracer);
                let options = self.search_options();
                self.threads.start_thinking(&self.pos, limits, options, true);
                self.wait_for_search_finished();
                tracer = self.threads.main_worker().tracer.take().unwrap();

                let c = collected.lock().unwrap();
                let mut r = format!(
                    "{{\"session\":{session},\"search\":{search},\"fen\":{},\"go\":{},\"info\":[",
                    json_string(root_fen.as_bytes()),
                    json_string(&cmd.text)
                );
                for (i, info) in c.infos.iter().enumerate() {
                    if i > 0 {
                        r.push(',');
                    }
                    r += &json_string(info.as_bytes());
                }
                r += &format!(
                    "],\"bestmove\":{},\"ponder\":{},\"nodes\":{},\"iterations\":[",
                    json_string(c.bestmove.as_bytes()),
                    if c.ponder.is_empty() { "null".to_string() } else { json_string(c.ponder.as_bytes()) },
                    self.threads.nodes_searched()
                );
                for (i, it) in tracer.iterations.iter().enumerate() {
                    let pv = it.pv.iter().map(|&m| uci::move_to_string(m, chess960)).collect::<Vec<_>>().join(" ");
                    if i > 0 {
                        r.push(',');
                    }
                    r += &format!(
                        "{{\"depth\":{},\"completed\":{},\"score\":{},\"uci_score\":{},\"bound\":\"{}\",\"seldepth\":{},\"nodes\":{},\"pv\":{}}}",
                        it.depth,
                        it.completed,
                        it.score,
                        it.uci_score,
                        it.bound,
                        it.sel_depth,
                        it.nodes,
                        json_string(pv.as_bytes())
                    );
                }
                let tt = self.tt.clone();
                let state = searchtrace::state_digests(&self.threads.main_worker(), &tt, tracer.events() > 0);
                r += &format!(
                    "],\"state\":{state},\"events\":{},\"digest\":\"{:016X}\",\"checkpoints\":[",
                    tracer.events(),
                    tracer.digest()
                );
                for (i, cp) in tracer.checkpoints().iter().enumerate() {
                    if i > 0 {
                        r.push(',');
                    }
                    r += &format!("\"{cp:016X}\"");
                }
                r += "]}";
                sync_println!("{r}");
            }
        }

        self.set_update_context_arc(saved_context);

        if window.counts {
            let counts = tracer
                .kinds()
                .iter()
                .map(|(k, n)| format!("{}:{n}", json_string(k.as_bytes())))
                .collect::<Vec<_>>()
                .join(",");
            sync_println!("{{\"counts\":{{{counts}}}}}");
        }

        // A read error must not pass for the end of the file
        !read_error
    }

    fn set_update_context_arc(&mut self, ctx: Arc<UpdateContext>) {
        self.update_context = ctx.clone();
        self.threads.main_worker().manager.as_deref_mut().unwrap().updates = ctx;
    }

    pub fn update_context(&self) -> Arc<UpdateContext> {
        self.update_context.clone()
    }

    pub fn restore_update_context(&mut self, ctx: Arc<UpdateContext>) {
        self.set_update_context_arc(ctx);
    }
}

/// The UCI output of a search, as `UCIEngine::init_search_update_listeners`
/// sets it up.
pub fn default_update_context() -> UpdateContext {
    UpdateContext {
        on_update_no_moves: Box::new(uci::on_update_no_moves),
        on_update_full: Box::new(uci::on_update_full),
        on_iter: Box::new(uci::on_iter),
        on_bestmove: Box::new(uci::on_bestmove),
        on_start: Box::new(|| {}),
    }
}

/// `CommandLine::get_binary_directory()`: the directory of `argv[0]`, `.`
/// if it has none.
fn binary_directory(argv0: Option<std::ffi::OsString>) -> std::path::PathBuf {
    let dir = argv0.map(std::path::PathBuf::from).and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
    if dir.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        dir
    }
}

use stockfish::uci::UciEngine;

fn main() {
    println!("{}", stockfish::misc::engine_info(false));
    stockfish::attacks::init();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    UciEngine::new().run(&args);
}

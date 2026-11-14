use stockfish::uci::UciEngine;

fn main() {
    println!("Stockfish Rust port (layers 1-7 of the C++ engine at 43e01e07)");
    stockfish::attacks::init();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    UciEngine::new().run(&args);
}

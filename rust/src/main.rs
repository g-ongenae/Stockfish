use stockfish::uci::UciEngine;

fn main() {
    // f64 arithmetic of the x87 rounded as on x86-64 (see the function)
    stockfish::misc::set_double_precision();
    println!("Stockfish Rust port (layers 1-7 of the C++ engine at 43e01e07)");
    stockfish::attacks::init();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    UciEngine::new().run(&args);
}

use stockfish::uci::UciEngine;

fn main() {
    println!("Stockfish Rust port (layers 1-4 of the C++ engine at 43e01e07)");
    stockfish::attacks::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    UciEngine::new().run(&args);
}

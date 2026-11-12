// Evaluation throughput of the C++ engine, the same measurement as
// rust/examples/nnue_bench.rs (see rust/README.md for how to build it against
// the object files of a build of src/). Not part of the engine.

#include <ctime>
#include <deque>
#include <fstream>
#include <iostream>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include "attacks.h"
#include "nnue/network.h"
#include "nnue/nnue_accumulator.h"
#include "position.h"
#include "uci.h"

using namespace Stockfish;

static double cpu_time() { return double(std::clock()) / CLOCKS_PER_SEC; }

int main(int argc, char* argv[]) {
    if (argc != 4)
    {
        std::cerr << "usage: nnue_bench <network> <fens> <lines>\n";
        return 2;
    }
    Attacks::init();
    Position::init();

    double t0  = cpu_time();
    auto   net = std::make_unique<Eval::NNUE::Network>();
    Eval::NNUE::EvalFile evalFile;
    net->load("", argv[1], evalFile);
    if (!evalFile.current)
    {
        std::cerr << "cannot load " << argv[1] << "\n";
        return 1;
    }
    std::cout << "load: " << cpu_time() - t0 << " s\n";

    auto stack  = std::make_unique<Eval::NNUE::AccumulatorStack>();
    auto caches = std::make_unique<Eval::NNUE::AccumulatorCaches>(*net);

    // From scratch
    std::vector<std::string> fens;
    {
        std::ifstream f(argv[2]);
        for (std::string l; std::getline(f, l);)
            if (l.find_first_not_of(" \t\r") != std::string::npos)
                fens.push_back(l);
    }
    long long n = 0, sum = 0;
    t0          = cpu_time();
    for (const auto& fen : fens)
    {
        StateInfo st;
        Position  pos;
        if (pos.set(fen, false, &st) || pos.checkers())
            continue;
        stack->reset();
        sum += net->evaluate(pos, *stack, *caches);
        n++;
    }
    double t = cpu_time() - t0;
    std::cout << "scratch: " << n << " evals in " << t << " s, " << long(n / t)
              << " evals/s, checksum " << sum << "\n";

    // Incremental
    std::vector<std::string> lines;
    {
        std::ifstream f(argv[3]);
        for (std::string l; std::getline(f, l);)
            if (l.find_first_not_of(" \t\r") != std::string::npos)
                lines.push_back(l);
    }
    n = sum = 0;
    t0      = cpu_time();
    for (const auto& line : lines)
    {
        std::istringstream       is(line);
        std::string              token, fen;
        std::vector<std::string> moves;
        is >> token;
        if (token == "startpos")
        {
            fen = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
            is >> token;
        }
        else
            while (is >> token && token != "moves")
                fen += token + " ";
        while (is >> token)
            moves.push_back(token);

        std::deque<StateInfo> states(1);
        Position              pos;
        if (pos.set(fen, false, &states.back()))
            continue;
        caches->clear(*net);
        stack->reset();
        int size = 1;
        for (const auto& mv : moves)
        {
            Move m = UCIEngine::to_move(pos, mv);
            if (m == Move::none())
                break;
            if (size == int(Eval::NNUE::AccumulatorStack::MaxSize))
            {
                stack->reset();
                size = 1;
            }
            bool givesCheck = pos.gives_check(m);
            auto& dirties   = stack->push(pos);
            size++;
            states.emplace_back();
            pos.do_move(m, states.back(), givesCheck, dirties);
            if (!pos.checkers())
            {
                sum += net->evaluate(pos, *stack, *caches);
                n++;
            }
        }
    }
    t = cpu_time() - t0;
    std::cout << "incremental: " << n << " evals in " << t << " s, " << long(n / t)
              << " evals/s, checksum " << sum << "\n";
    return 0;
}

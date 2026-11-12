import argparse
import gzip
import json
import re
import sys
import subprocess
import pathlib
import os
import fnmatch

from testing import (
    EPD,
    Stockfish as Engine,
    MiniTestFramework,
    OrderedClassMembers,
    Valgrind,
    Syzygy,
)

PATH = pathlib.Path(__file__).parent.resolve()
CWD = os.getcwd()


def get_prefix(expect_failure=False):
    if args.valgrind:
        return Valgrind.get_valgrind_command(expect_failure)
    if args.valgrind_thread:
        return Valgrind.get_valgrind_thread_command()

    return []


def get_threads():
    if args.valgrind_thread or args.sanitizer_thread:
        return 4
    return 1


def get_path():
    return os.path.abspath(os.path.join(CWD, args.stockfish_path))


def postfix_check(output):
    if args.sanitizer_undefined:
        for idx, line in enumerate(output):
            if "runtime error:" in line:
                # print next possible 50 lines
                for i in range(50):
                    debug_idx = idx + i
                    if debug_idx < len(output):
                        print(output[debug_idx])
                return False

    if args.sanitizer_thread:
        for idx, line in enumerate(output):
            if "WARNING: ThreadSanitizer:" in line:
                # print next possible 50 lines
                for i in range(50):
                    debug_idx = idx + i
                    if debug_idx < len(output):
                        print(output[debug_idx])
                return False

    if args.valgrind or args.valgrind_thread:
        for line in output:
            match = re.search(r"ERROR SUMMARY:\s*(\d+) errors", line)
            if match and int(match.group(1)) > 0:
                return False

    return True


def Stockfish(*args, expect_failure=False, **kwargs):
    return Engine(get_prefix(expect_failure), get_path(), *args, expect_failure=expect_failure, **kwargs)


class TestCLI(metaclass=OrderedClassMembers):
    def beforeAll(self):
        pass

    def afterAll(self):
        pass

    def beforeEach(self):
        self.stockfish = None

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def test_eval(self):
        self.stockfish = Stockfish("eval".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_go_nodes_1000(self):
        self.stockfish = Stockfish("go nodes 1000".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_go_depth_10(self):
        self.stockfish = Stockfish("go depth 10".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_go_perft_4(self):
        self.stockfish = Stockfish("go perft 4".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_go_movetime_1000(self):
        self.stockfish = Stockfish("go movetime 1000".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_go_wtime_8000_btime_8000_winc_500_binc_500(self):
        self.stockfish = Stockfish(
            "go wtime 8000 btime 8000 winc 500 binc 500".split(" "),
            True,
        )
        assert self.stockfish.process.returncode == 0

    def test_go_wtime_1000_btime_1000_winc_0_binc_0(self):
        self.stockfish = Stockfish(
            "go wtime 1000 btime 1000 winc 0 binc 0".split(" "),
            True,
        )
        assert self.stockfish.process.returncode == 0

    def test_go_wtime_1000_btime_1000_winc_0_binc_0_movestogo_5(self):
        self.stockfish = Stockfish(
            "go wtime 1000 btime 1000 winc 0 binc 0 movestogo 5".split(" "),
            True,
        )
        assert self.stockfish.process.returncode == 0

    def test_go_movetime_200(self):
        self.stockfish = Stockfish("go movetime 200".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_go_nodes_20000_searchmoves_e2e4_d2d4(self):
        self.stockfish = Stockfish(
            "go nodes 20000 searchmoves e2e4 d2d4".split(" "), True
        )
        assert self.stockfish.process.returncode == 0

    def test_bench_128_threads_8_default_depth(self):
        self.stockfish = Stockfish(
            f"bench 128 {get_threads()} 8 default depth".split(" "),
            True,
        )
        assert self.stockfish.process.returncode == 0

    def test_bench_128_threads_3_bench_tmp_epd_depth(self):
        self.stockfish = Stockfish(
            f"bench 128 {get_threads()} 3 {os.path.join(PATH, 'bench_tmp.epd')} depth".split(
                " "
            ),
            True,
        )
        assert self.stockfish.process.returncode == 0

    def test_d(self):
        self.stockfish = Stockfish("d".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_compiler(self):
        self.stockfish = Stockfish("compiler".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_license(self):
        self.stockfish = Stockfish("license".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_uci(self):
        self.stockfish = Stockfish("uci".split(" "), True)
        assert self.stockfish.process.returncode == 0

    def test_export_net_verify_nnue(self):
        current_path = os.path.abspath(os.getcwd())
        self.stockfish = Stockfish(
            f"export_net {os.path.join(current_path, 'verify.nnue')}".split(" "), True
        )
        assert self.stockfish.process.returncode == 0

    # verify the generated net equals the base net

    def test_network_equals_base(self):
        self.stockfish = Stockfish(
            ["uci"],
            True,
        )

        output = self.stockfish.process.stdout

        # find line
        for line in output.split("\n"):
            if "option name EvalFile type string default" in line:
                network = line.split(" ")[-1]
                break

        # find network file in src dir
        network = os.path.join(PATH.parent.resolve(), "src", network)

        if not os.path.exists(network):
            print(
                f"Network file {network} not found, please download the network file over the make command."
            )
            assert False

        diff = subprocess.run(["diff", network, f"verify.nnue"])

        assert diff.returncode == 0


class TestInteractive(metaclass=OrderedClassMembers):
    def beforeAll(self):
        self.stockfish = Stockfish()

    def afterAll(self):
        self.stockfish.quit()
        assert self.stockfish.close() == 0

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def test_startup_output(self):
        self.stockfish.starts_with("Stockfish")

    def test_uci_command(self):
        self.stockfish.send_command("uci")
        self.stockfish.equals("uciok")

    def test_set_threads_option(self):
        self.stockfish.send_command(f"setoption name Threads value {get_threads()}")

    def test_ucinewgame_and_startpos_nodes_1000(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position startpos")
        self.stockfish.send_command("go nodes 1000")
        self.stockfish.starts_with("bestmove")

    def test_ucinewgame_and_startpos_moves(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position startpos moves e2e4 e7e6")
        self.stockfish.send_command("go nodes 1000")
        self.stockfish.starts_with("bestmove")

    def test_fen_position_1(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position fen 5rk1/1K4p1/8/8/3B4/8/8/8 b - - 0 1")
        self.stockfish.send_command("go nodes 1000")
        self.stockfish.starts_with("bestmove")

    def test_fen_position_2_flip(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position fen 5rk1/1K4p1/8/8/3B4/8/8/8 b - - 0 1")
        self.stockfish.send_command("flip")
        self.stockfish.send_command("go nodes 1000")
        self.stockfish.starts_with("bestmove")

    def test_depth_5_with_callback(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position startpos")
        self.stockfish.send_command("go depth 5")

        def callback(output):
            regex = r"info depth \d+ seldepth \d+ multipv \d+ score cp -?\d+ nodes \d+ nps \d+ hashfull \d+ tbhits \d+ time \d+ pv"
            if output.startswith("info depth") and not re.match(regex, output):
                assert False
            if output.startswith("bestmove"):
                return True
            return False

        self.stockfish.check_output(callback)

    def test_ucinewgame_and_go_depth_9(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("setoption name UCI_ShowWDL value true")
        self.stockfish.send_command("position startpos")
        self.stockfish.send_command("go depth 9")

        depth = 1
        resent = False

        def callback(output):
            nonlocal depth, resent

            if output.startswith("info depth"):
                # The PV of the last depth may be sent once more before bestmove,
                # when its PV had a single move and a ponder move is then found in
                # the TT (Search::Worker::start_searching). With several threads
                # sharing the TT this depends on scheduling.
                if depth == 10 and not resent:
                    depth, resent = 9, True

                regex = rf"info depth {depth} seldepth \d+ multipv \d+ score cp -?\d+ wdl \d+ \d+ \d+ nodes \d+ nps \d+ hashfull \d+ tbhits \d+ time \d+ pv"
                if not re.match(regex, output):
                    assert False
                depth += 1

            if output.startswith("bestmove"):
                assert depth == 10
                return True

            return False

        self.stockfish.check_output(callback)

    def test_go_depth_3_with_mismatched_clock(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position startpos")
        self.stockfish.send_command("go depth 3 btime 1000")

        max_depth = 0

        def callback(output):
            nonlocal max_depth
            if output.startswith("info depth"):
                match = re.search(r"info depth (\d+)", output)
                if match:
                    max_depth = max(max_depth, int(match.group(1)))

            if output.startswith("bestmove"):
                assert max_depth == 3
                return True

            return False

        self.stockfish.check_output(callback)

    def test_clear_hash(self):
        self.stockfish.send_command("setoption name Clear Hash")

    def test_fen_position_mate_1(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 5K2/8/2qk4/2nPp3/3r4/6B1/B7/3R4 w - e6"
        )
        self.stockfish.send_command("go depth 18")

        self.stockfish.expect("* score mate 1 * pv d5e6")
        self.stockfish.equals("bestmove d5e6")

    def test_fen_position_mate_minus_1(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 2brrb2/8/p7/Q7/1p1kpPp1/1P1pN1K1/3P4/8 b - -"
        )
        self.stockfish.send_command("go depth 18")
        self.stockfish.expect("* score mate -1 *")
        self.stockfish.starts_with("bestmove")

    def test_fen_position_fixed_node(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 5K2/8/2P1P1Pk/6pP/3p2P1/1P6/3P4/8 w - - 0 1"
        )
        self.stockfish.send_command("go nodes 500000")
        self.stockfish.starts_with("bestmove")

    def test_fen_position_with_mate_go_depth(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - -"
        )
        self.stockfish.send_command("go depth 18 searchmoves c6d7")
        self.stockfish.expect("* score mate 2 * pv c6d7 * f7f5")

        self.stockfish.starts_with("bestmove")

    def test_fen_position_with_mate_go_mate(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - -"
        )
        self.stockfish.send_command("go mate 2 searchmoves c6d7")
        self.stockfish.expect("* score mate 2 * pv c6d7 *")

        self.stockfish.starts_with("bestmove")

    def test_fen_position_with_mate_go_nodes(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - -"
        )
        self.stockfish.send_command("go nodes 500000 searchmoves c6d7")
        self.stockfish.expect("* score mate 2 * pv c6d7 * f7f5")

        self.stockfish.starts_with("bestmove")

    def test_fen_position_depth_27(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen r1b2r1k/pp1p2pp/2p5/2B1q3/8/8/P1PN2PP/R4RK1 w - - 0 18"
        )
        self.stockfish.send_command("go")
        self.stockfish.contains("score mate 1")

        self.stockfish.starts_with("bestmove")

    def test_fen_position_with_mate_go_depth_and_promotion(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - - moves c6d7 f2f1q"
        )
        self.stockfish.send_command("go depth 18")
        self.stockfish.expect("* score mate 1 * pv f7f5")
        self.stockfish.starts_with("bestmove f7f5")

    def test_fen_position_with_mate_go_depth_and_searchmoves(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - -"
        )
        self.stockfish.send_command("go depth 18 searchmoves c6d7")
        self.stockfish.expect("* score mate 2 * pv c6d7 * f7f5")

        self.stockfish.starts_with("bestmove c6d7")

    def test_fen_position_with_moves_with_mate_go_depth_and_searchmoves(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command(
            "position fen 8/5R2/2K1P3/4k3/8/b1PPpp1B/5p2/8 w - - moves c6d7"
        )
        self.stockfish.send_command("go depth 18 searchmoves e3e2")
        self.stockfish.expect("* score mate -1 * pv e3e2 f7f5")
        self.stockfish.starts_with("bestmove e3e2")

    def test_verify_nnue_network(self):
        current_path = os.path.abspath(os.getcwd())
        Stockfish(
            f"export_net {os.path.join(current_path, 'verify.nnue')}".split(" "), True
        )

        self.stockfish.send_command("setoption name EvalFile value verify.nnue")
        self.stockfish.send_command("position startpos")
        self.stockfish.send_command("go depth 5")
        self.stockfish.starts_with("bestmove")

    def test_multipv_setting(self):
        self.stockfish.send_command("setoption name MultiPV value 4")
        self.stockfish.send_command("position startpos")
        self.stockfish.send_command("go depth 5")
        self.stockfish.starts_with("bestmove")

    def test_fen_position_with_skill_level(self):
        self.stockfish.send_command("setoption name Skill Level value 10")
        self.stockfish.send_command("position startpos")
        self.stockfish.send_command("go depth 5")
        self.stockfish.starts_with("bestmove")

        self.stockfish.send_command("setoption name Skill Level value 20")


class TestSyzygy(metaclass=OrderedClassMembers):
    def beforeAll(self):
        self.stockfish = Stockfish()

    def afterAll(self):
        self.stockfish.quit()
        assert self.stockfish.close() == 0

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def test_syzygy_setup(self):
        self.stockfish.starts_with("Stockfish")
        self.stockfish.send_command("uci")
        self.stockfish.send_command(
            f"setoption name SyzygyPath value {os.path.join(PATH, 'syzygy')}"
        )
        self.stockfish.expect(
            "info string Found 35 WDL and 35 DTZ tablebase files (up to 4-man)."
        )

    def test_syzygy_bench(self):
        self.stockfish.send_command("bench 128 1 8 default depth")
        self.stockfish.expect("Nodes searched  :*")

    def test_syzygy_position(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position fen 4k3/PP6/8/8/8/8/8/4K3 w - - 0 1")
        self.stockfish.send_command("go depth 5")

        def check_output(output):
            if "score cp 20000" in output or "score mate" in output:
                return True

        self.stockfish.check_output(check_output)
        self.stockfish.expect("bestmove *")

    def test_syzygy_position_2(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position fen 8/1P6/2B5/8/4K3/8/6k1/8 w - - 0 1")
        self.stockfish.send_command("go depth 5")

        def check_output(output):
            if "score cp 20000" in output or "score mate" in output:
                return True

        self.stockfish.check_output(check_output)
        self.stockfish.expect("bestmove *")

    def test_syzygy_position_3(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position fen 8/1P6/2B5/8/4K3/8/6k1/8 b - - 0 1")
        self.stockfish.send_command("go depth 5")

        def check_output(output):
            if "score cp -20000" in output or "score mate -" in output:
                return True

        self.stockfish.check_output(check_output)
        self.stockfish.expect("bestmove *")

    def test_syzygy_position_4(self):
        self.stockfish.send_command("ucinewgame")
        self.stockfish.send_command("position fen 8/8/7B/3B3P/7k/8/5K2/3r4 w - - 0 1")
        self.stockfish.send_command("go depth 1")
        self.stockfish.expect("bestmove *")

class TestEnPassantSanitization(metaclass=OrderedClassMembers):
    def beforeAll(self):
        self.stockfish = Stockfish()

    def afterAll(self):
        self.stockfish.quit()
        assert self.stockfish.close() == 0

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def test_position_1(self):
        self.stockfish.send_command("position fen rnbqkbnr/ppp1p1pp/5p2/3pP3/8/8/PPPP1PPP/RNBQKBNR w kq d6 0 3")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*rnbqkbnr/ppp1p1pp/5p2/3pP3/8/8/PPPP1PPP/RNBQKBNR w kq d6 0 3*")

    def test_position_2(self):
        self.stockfish.send_command("position fen k7/8/8/1pP5/2K5/8/8/8 w - b6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k7/8/8/1pP5/2K5/8/8/8 w - b6 0 1*")

    def test_position_3(self):
        self.stockfish.send_command("position fen k1r5/8/8/1pP5/2K5/8/8/8 w - b6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k1r5/8/8/1pP5/2K5/8/8/8 w - - 0 1*")

    def test_position_4(self):
        self.stockfish.send_command("position fen k1r5/8/8/1pP5/8/2K5/8/8 w - b6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k1r5/8/8/1pP5/8/2K5/8/8 w - - 0 1*")

    def test_position_5(self):
        self.stockfish.send_command("position fen k1r5/8/8/PpP5/8/2K5/8/8 w - b6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k1r5/8/8/PpP5/8/2K5/8/8 w - b6 0 1*")

    def test_position_6(self):
        self.stockfish.send_command("position fen k1r5/8/8/PpP5/2K5/8/8/8 w - b6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k1r5/8/8/PpP5/2K5/8/8/8 w - b6 0 1*")

    def test_position_7(self):
        self.stockfish.send_command("position fen k7/4b3/8/PpP5/1K6/8/8/8 w - b6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k7/4b3/8/PpP5/1K6/8/8/8 w - b6 0 1*")

    def test_position_8(self):
        self.stockfish.send_command("position fen k7/b5b1/8/2PpP3/3K4/8/8/8 w - d6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k7/b5b1/8/2PpP3/3K4/8/8/8 w - - 0 1*")

    def test_position_9(self):
        self.stockfish.send_command("position fen k7/8/8/r2pPK2/8/8/8/8 w - d6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k7/8/8/r2pPK2/8/8/8/8 w - - 0 1*")

    def test_position_10(self):
        self.stockfish.send_command("position fen k7/8/8/r1PpPK2/8/8/8/8 w - d6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*k7/8/8/r1PpPK2/8/8/8/8 w - d6 0 1*")

    def test_position_11(self):
        self.stockfish.send_command("position fen kb6/8/8/3pP3/5K2/8/8/8 w - d6 0 1")
        self.stockfish.send_command("d")

        self.stockfish.expect_for_line_matching("Fen*", "*kb6/8/8/3pP3/5K2/8/8/8 w - d6 0 1*")

    def test_position_find_draw(self):
        self.stockfish.send_command("position fen q4kb1/3Q2nq/8/r3PpK1/2n5/7q/8/q7 w - f6 0 1 moves d7c8 f8f7 c8d7 f7f8 d7d8 f8f7")
        self.stockfish.send_command("go nodes 10000")

        def check_output(output):
            if fnmatch.fnmatch(output, "* score cp 0 * pv d8d7*"):
                return True

        self.stockfish.check_output(check_output)
        self.stockfish.expect("bestmove d8d7*")

class TestInvalidFEN(metaclass=OrderedClassMembers):
    def beforeEach(self):
        self.stockfish = None

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def _expect_critical(self, fen):
        self.stockfish = Stockfish(f"position fen {fen}".split(" "), True, expect_failure=True)
        assert self.stockfish.process.returncode != 0
        assert "CRITICAL ERROR" in self.stockfish.process.stdout

    def test_no_kings(self):
        self._expect_critical("8/8/8/8/8/8/8/8 w - - 0 1")

    def test_invalid_piece(self):
        self._expect_critical("rnbqkbnr/pppXpppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1")

    def test_invalid_side_to_move(self):
        self._expect_critical("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x KQkq - 0 1")

    def test_pawns_on_back_rank(self):
        self._expect_critical("pppppppp/8/8/8/8/8/8/4K2k w - - 0 1")

    def test_invalid_skip_count(self):
        self._expect_critical("9/8/8/8/8/8/8/8 w - - 0 1")


class TestInvalidOptions(metaclass=OrderedClassMembers):
    def beforeAll(self):
        self.stockfish = Stockfish()

    def afterAll(self):
        self.stockfish.quit()
        assert self.stockfish.close() == 0

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    # Ignore bogus spin values
    def test_spin_non_numeric(self):
        self.stockfish.send_command("setoption name Threads value abc")
        self.stockfish.send_command("isready")
        self.stockfish.equals("readyok")

    def test_spin_out_of_range(self):
        self.stockfish.send_command("setoption name Threads value 999999999999")
        self.stockfish.send_command("isready")
        self.stockfish.equals("readyok")

    def test_spin_negative(self):
        self.stockfish.send_command("setoption name Threads value -5")
        self.stockfish.send_command("isready")
        self.stockfish.equals("readyok")

    # Warn on bogus NUMA configs
    def test_numa_garbage(self):
        self.stockfish.send_command("setoption name NumaPolicy value zzz")
        self.stockfish.expect("*NumaPolicy: invalid value 'zzz', keeping previous config.*")

    def test_numa_malformed_range(self):
        self.stockfish.send_command("setoption name NumaPolicy value 0-")
        self.stockfish.expect("*NumaPolicy: invalid value '0-', keeping previous config.*")

    def test_numa_overflow(self):
        self.stockfish.send_command(
            "setoption name NumaPolicy value 99999999999999999999999"
        )
        self.stockfish.expect("*NumaPolicy: invalid value*keeping previous config.*")
        self.stockfish.send_command("isready")
        self.stockfish.equals("readyok")


class TestBenchFile(metaclass=OrderedClassMembers):
    def beforeEach(self):
        self.stockfish = None

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def _bench(self, name, content, expect_failure=False):
        with open(name, "w") as f:
            f.write(content)
        self.stockfish = Stockfish(f"bench 16 1 4 {name} depth".split(" "), True, expect_failure=expect_failure)

    def test_valid_file(self):
        self._bench("good.epd", "4k3/8/4K3/8/8/8/8/8 w - - 0 1\n")
        assert self.stockfish.process.returncode == 0
        assert "Nodes searched" in self.stockfish.process.stderr

    def test_empty_file(self):
        self._bench("empty.epd", "")
        assert self.stockfish.process.returncode == 0

    def test_malformed_fen(self):
        self._bench("bad.epd", "not a valid fen\n", expect_failure=True)
        assert self.stockfish.process.returncode != 0
        assert "CRITICAL ERROR" in self.stockfish.process.stdout

    def test_missing_file(self):
        self.stockfish = Stockfish(
            "bench 16 1 4 does_not_exist.epd depth".split(" "), True, expect_failure=True
        )
        assert self.stockfish.process.returncode != 0


class TestDump(metaclass=OrderedClassMembers):
    def beforeAll(self):
        self.stockfish = Stockfish()

    def afterAll(self):
        self.stockfish.quit()
        assert self.stockfish.close() == 0

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def _next_dump_line(self):
        for line in self.stockfish.readline():
            if line.startswith("{"):
                return line

    def _dump(self, position):
        self.stockfish.send_command(f"position {position}")
        self.stockfish.send_command("dump")
        return json.loads(self._next_dump_line())

    def test_startpos(self):
        dump = self._dump("startpos")

        assert dump["fen"] == "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"
        assert dump["chess960"] == False
        # the key printed by the 'd' command
        assert dump["key"] == "8F8F01D4562F59FB"
        assert dump["checkers"] == "0000000000000000"
        assert dump["non_pawn_material"] == [8302, 8302]
        assert isinstance(dump["nnue"], int) and isinstance(dump["eval"], int)
        assert len(dump["moves"]) == 20
        assert dump["moves"][0] == "a2a3" and dump["moves"][-1] == "g1h3"
        assert dump["see"] == [0] * 20
        assert dump["gives_check"] == [0] * 20
        assert sorted(dump["order_main_shallow"]) == sorted(dump["moves"])
        assert sorted(dump["order_main_deep"]) == sorted(dump["moves"])
        assert dump["order_qsearch"] == []

    def test_key_after_is_key_of_child(self):
        parent = self._dump("startpos")

        for move, key in zip(parent["moves"], parent["key_after"]):
            assert self._dump(f"startpos moves {move}")["key"] == key

    def test_see_winning_capture(self):
        dump = self._dump("fen 4k3/8/3p4/8/8/8/3R4/4K3 w - - 0 1")

        # the rook wins a pawn
        assert dump["see"][dump["moves"].index("d2d6")] == 208
        assert dump["order_main_shallow"][0] == "d2d6"
        assert dump["order_qsearch"] == ["d2d6"]

    def test_see_losing_capture(self):
        dump = self._dump("fen 4k3/2p5/3p4/8/8/8/3R4/4K3 w - - 0 1")

        # the rook wins a pawn and is taken back
        assert dump["see"][dump["moves"].index("d2d6")] == 208 - 1276
        # a losing capture comes after the quiet moves, the first of which gives check
        assert dump["order_main_shallow"][0] == "d2e2"
        assert dump["order_main_shallow"][-1] == "d2d6"
        assert dump["order_main_deep"][-1] == "d2d6"
        assert dump["order_qsearch"] == ["d2d6"]
        assert dump["gives_check"] == [int(m == "d2e2") for m in dump["moves"]]

    def test_pinned_piece(self):
        dump = self._dump("fen 4k3/4r3/8/8/8/8/4B3/4K3 w - - 0 1")

        # the bishop on e2 is pinned by the rook on e7
        assert dump["blockers_for_king"] == ["0000000000001000", "0000000000000000"]
        assert dump["pinners"] == ["0000000000000000", "0010000000000000"]
        assert dump["moves"] == ["e1d1", "e1f1", "e1d2", "e1f2"]

    def test_check(self):
        dump = self._dump("fen 4k3/8/8/8/7b/8/8/R3K2R w KQ - 3 1")

        # checked by the bishop on h4: no evaluation, evasions in every stage
        assert dump["checkers"] == "0000000080000000"
        assert dump["nnue"] is None and dump["eval"] is None
        assert dump["moves"] == ["h1h4", "e1d1", "e1f1", "e1d2", "e1e2"]
        assert dump["see"] == [825, 0, 0, 0, 0]
        assert dump["order_main_shallow"] == dump["moves"]
        assert dump["order_qsearch"] == dump["moves"]

    def test_double_check(self):
        dump = self._dump("fen 4k3/8/8/8/8/5n2/4r3/4K3 w - - 0 1")

        # only the king can move, and taking the rook comes first
        assert dump["checkers"] == "0000000000201000"
        assert dump["moves"] == ["e1d1", "e1f1", "e1e2"]
        assert dump["see"] == [0, 0, 1276]
        assert dump["order_main_shallow"] == ["e1e2", "e1d1", "e1f1"]

    def test_checkmate(self):
        dump = self._dump("fen R5k1/5ppp/8/8/8/8/8/4K3 b - - 0 1")

        assert dump["checkers"] == "0100000000000000"
        assert dump["moves"] == [] and dump["see"] == [] and dump["key_after"] == []
        assert dump["order_main_shallow"] == [] and dump["order_qsearch"] == []

    def test_en_passant_and_promotion(self):
        dump = self._dump("fen 8/P6k/8/8/5Pp1/8/8/4K3 b - f3 0 1")
        assert dump["fen"] == "8/P6k/8/8/5Pp1/8/8/4K3 b - f3 0 1"
        assert dump["moves"][0] == "g4f3"
        assert dump["order_qsearch"] == ["g4f3"]

        dump = self._dump("fen 7k/P7/8/8/8/8/8/4K3 w - - 20 40")
        assert dump["moves"][:4] == ["a7a8q", "a7a8r", "a7a8b", "a7a8n"]
        assert dump["gives_check"][:4] == [1, 1, 0, 0]
        # only the queen promotion belongs to the quiescence search
        assert dump["order_qsearch"] == ["a7a8q"]

    def test_chess960(self):
        self.stockfish.send_command("setoption name UCI_Chess960 value true")
        dump = self._dump("fen bnrqkr2/ppppppbp/5np1/8/8/5NP1/PPPPPPBP/BNRQKR2 w FCfc - 2 4")
        self.stockfish.send_command("setoption name UCI_Chess960 value false")

        assert dump["chess960"] == True
        assert dump["fen"] == "bnrqkr2/ppppppbp/5np1/8/8/5NP1/PPPPPPBP/BNRQKR2 w FCfc - 2 4"
        # castling is the king taking its own rook
        assert dump["moves"][-1] == "e1f1"

    def _check_sample(self, name, count):
        # compare with the golden files, see tests/differential
        sample = PATH / "differential" / "sample"
        with gzip.open(sample / f"{name}.fen.gz", "rt") as f:
            fens = f.readlines()[:count]
        with gzip.open(sample / f"{name}.jsonl.gz", "rt") as f:
            golden = f.readlines()[:count]
        with open(f"{name}.fen", "w") as f:
            f.writelines(fens)

        self.stockfish.send_command(f"dump {name}.fen")
        for expected in golden:
            assert self._next_dump_line() == expected.strip()

    def test_golden_sample(self):
        self._check_sample("standard", 500)

    def test_golden_sample_chess960(self):
        self.stockfish.send_command("setoption name UCI_Chess960 value true")
        self._check_sample("chess960", 250)
        self.stockfish.send_command("setoption name UCI_Chess960 value false")


class TestDumpFile(metaclass=OrderedClassMembers):
    def beforeEach(self):
        self.stockfish = None

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def _dump(self, name, content):
        with open(name, "w") as f:
            f.write(content)
        self.stockfish = Stockfish(f"dump {name}".split(" "), True)
        return [json.loads(l) for l in self.stockfish.process.stdout.splitlines() if l.startswith("{")]

    def test_valid_file(self):
        fens = ["4k3/8/4K3/8/8/8/8/8 w - - 0 1", "4k3/8/4K3/8/8/8/8/8 b - - 0 1"]
        dumps = self._dump("good.epd", fens[0] + "\n\n" + fens[1] + "\n")
        assert self.stockfish.process.returncode == 0
        assert [d["fen"] for d in dumps] == fens

    def test_empty_file(self):
        assert self._dump("empty.epd", "") == []
        assert self.stockfish.process.returncode == 0

    def test_malformed_fen(self):
        # a position that cannot be set up is reported, the next ones are still dumped
        dumps = self._dump("bad.epd", "not a valid fen\n4k3/8/4K3/8/8/8/8/8 w - - 0 1\n")
        assert self.stockfish.process.returncode == 0
        assert len(dumps) == 2
        assert "error" in dumps[0] and dumps[0]["input"] == "not a valid fen"
        assert dumps[1]["fen"] == "4k3/8/4K3/8/8/8/8/8 w - - 0 1"

    def test_missing_file(self):
        self.stockfish = Stockfish("dump does_not_exist.epd".split(" "), True, expect_failure=True)
        assert self.stockfish.process.returncode != 0
        assert "CRITICAL ERROR" in self.stockfish.process.stdout


class TestDumpLine(metaclass=OrderedClassMembers):
    def beforeAll(self):
        self.stockfish = Stockfish()

    def afterAll(self):
        self.stockfish.quit()
        assert self.stockfish.close() == 0

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def _next_dump_line(self):
        for line in self.stockfish.readline():
            if line.startswith("{"):
                return line

    def _dump(self, position):
        self.stockfish.send_command(f"position {position}")
        self.stockfish.send_command("dump")
        return json.loads(self._next_dump_line())

    def _dump_line(self, line):
        # one record per position of the line, then the walk back
        self.stockfish.send_command(f"dumpline {line}")
        records = [json.loads(self._next_dump_line())]
        if "error" in records[0]:
            return records
        while "undo" not in records[-1]:
            records.append(json.loads(self._next_dump_line()))
        return records

    def test_line_matches_positions(self):
        line = "startpos moves e2e4 d7d5 e4d5 g8f6 f1b5 c7c6 d5c6 d8a5 c6b7 a5b5 b7a8n"
        moves = line.split()[2:]
        records = self._dump_line(line)

        assert len(records) == len(moves) + 2
        for ply, record in enumerate(records[:-1]):
            assert record["ply"] == ply
            assert record["move"] == (moves[ply - 1] if ply else None)
            # what the moves maintain is what a fresh set up computes
            dump = self._dump(f"startpos moves {' '.join(moves[:ply])}")
            for member in ("fen", "key", "pawn_key", "material_key", "minor_piece_key",
                           "non_pawn_key", "non_pawn_material", "checkers", "blockers_for_king",
                           "pinners", "check_squares", "threats"):
                assert record[member] == dump[member]
            if record["nnue"] is not None:
                assert (record["nnue"], record["eval"]) == (dump["nnue"], dump["eval"])
            if record["probe"] is not None:
                index = dump["moves"].index(record["probe"]["move"])
                assert record["probe"]["key"] == dump["key_after"][index]
                # the probe is never the next move of the line
                assert ply == len(moves) or record["probe"]["move"] != moves[ply]

        # plies 3, 6 and 7 of every 8 are not evaluated, nor probed
        assert [r["nnue"] is None for r in records[:-1]] == [
            p % 8 in (3, 6, 7) or r["checkers"] != "0" * 16 for p, r in enumerate(records[:-1])
        ]
        assert records[3]["captured"] == "p" and records[11]["captured"] == "r"
        assert records[11]["material_key"] != records[10]["material_key"]

    def test_walk_back(self):
        records = self._dump_line("startpos moves g1f3 d7d5 g2g3 c8g4 f1g2 b8c6 e1g1")
        undo = records[-1]

        assert undo["undo"] == 7
        # back down the line, the keys are those of the way up
        assert undo["keys"] == [r["key"] for r in reversed(records[:-2])]
        assert undo["fens"] == [r["fen"] for r in reversed(records[:-2])]
        assert undo["fen"] == records[0]["fen"] and undo["key"] == records[0]["key"]
        # every position of the search window is evaluated on the way back
        assert all(v is not None for v in undo["nnues"])
        for record, nnue in zip(reversed(records[:-2]), undo["nnues"]):
            assert record["nnue"] in (None, nnue)

    def test_null_move(self):
        records = self._dump_line("startpos moves e2e4 c7c5 g1f3")

        null = records[1]["null"]
        assert null["fen"] == "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 1"
        dump = self._dump(f"fen {null['fen']}")
        assert (null["key"], null["nnue"]) == (dump["key"], dump["nnue"])
        assert (null["plies_from_null"], null["repetition"], null["captured"]) == (0, 0, None)
        # a reply to the null move, made and undone
        reply = null["reply"]
        assert reply["key"] == dump["key_after"][dump["moves"].index(reply["move"])]
        assert (reply["plies_from_null"], reply["repetition"]) == (1, 0)
        assert records[2]["plies_from_null"] == 2

    def test_probe_is_not_the_next_move(self):
        records = self._dump_line("startpos moves a2a3")
        assert records[0]["probe"]["move"] != "a2a3"

    def test_repetition(self):
        records = self._dump_line("startpos moves g1f3 g8f6 f3g1 f6g8 g1f3 g8f6 f3g1 f6g8")

        assert [r["repetition"] for r in records[:-1]] == [0, 0, 0, 0, 4, 4, 4, 4, -4]
        assert [r["has_repeated"] for r in records[:-1]] == [False] * 4 + [True] * 5
        # a repetition draws when it lies strictly after the root, here ply 0
        assert [r["is_draw"] for r in records[:-1]] == [False] * 5 + [True] * 4
        assert [r["upcoming_repetition"] for r in records[:-1]] == [False] * 4 + [True] * 5

    def test_fifty_moves(self):
        records = self._dump_line("fen 8/8/8/4k3/8/8/8/R3K3 w - - 98 80 moves a1a2 e5e4 a2a3")

        assert [r["fen"].split()[4] for r in records[:-1]] == ["98", "99", "100", "101"]
        assert [r["is_draw"] for r in records[:-1]] == [False, False, True, True]
        # the key changes every 8 plies of rule50 from 14 on
        assert records[0]["key"] != self._dump("fen 8/8/8/4k3/8/8/8/R3K3 w - - 0 80")["key"]

    def test_chess960(self):
        self.stockfish.send_command("setoption name UCI_Chess960 value true")
        records = self._dump_line(
            "fen bnrqkr2/ppppppbp/5np1/8/8/5NP1/PPPPPPBP/BNRQKR2 w FCfc - 2 4 moves e1f1 e8f8"
        )
        self.stockfish.send_command("setoption name UCI_Chess960 value false")

        assert records[1]["fen"] == "bnrqkr2/ppppppbp/5np1/8/8/5NP1/PPPPPPBP/BNRQ1RK1 b fc - 3 4"
        assert records[2]["fen"] == "bnrq1rk1/ppppppbp/5np1/8/8/5NP1/PPPPPPBP/BNRQ1RK1 w - - 4 5"

    def test_errors(self):
        for line, error in (
            ("startpos moves e2e4 e7e5 e4e5", "Illegal move e4e5 at ply 3"),
            ("startpos e2e4", "Malformed line, expected startpos or fen, then moves"),
            ("", "Malformed line, expected startpos or fen, then moves"),
        ):
            records = self._dump_line(line)
            assert records == [{"error": error, "input": line}]

        # an invalid FEN, and the next line is still dumped
        assert "error" in self._dump_line("fen not a valid fen moves e2e4")[0]
        records = self._dump_line("startpos")
        assert len(records) == 2 and records[1]["undo"] == 0

    def _check_sample(self, name, count):
        # compare with the golden files, see tests/differential
        sample = PATH / "differential" / "sample"
        with gzip.open(sample / f"{name}.lines.gz", "rt") as f:
            lines = f.readlines()[:count]
        assert len(lines) == count
        expected = []
        with gzip.open(sample / f"{name}.lines.jsonl.gz", "rt") as f:
            # the records of the first lines, each ending with its walk back
            for record in f:
                expected.append(record.strip())
                if record.startswith('{"undo"'):
                    count -= 1
                    if count == 0:
                        break
        with open(f"{name}.lines", "w") as f:
            f.writelines(lines)

        # the sample holds the dumps of all these lines, complete
        assert count == 0

        self.stockfish.send_command(f"dumpline {name}.lines")
        for record in expected:
            assert self._next_dump_line() == record

        # and nothing more is printed
        self.stockfish.send_command("isready")
        for line in self.stockfish.readline():
            assert not line.startswith("{")
            if line == "readyok":
                break

    def test_golden_sample(self):
        self._check_sample("standard", 10)

    def test_golden_sample_chess960(self):
        self.stockfish.send_command("setoption name UCI_Chess960 value true")
        self._check_sample("chess960", 5)
        self.stockfish.send_command("setoption name UCI_Chess960 value false")


class TestDumpLineFile(metaclass=OrderedClassMembers):
    def beforeEach(self):
        self.stockfish = None

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def _dump(self, name, content):
        with open(name, "w") as f:
            f.write(content)
        self.stockfish = Stockfish(f"dumpline {name}".split(" "), True)
        return [json.loads(l) for l in self.stockfish.process.stdout.splitlines() if l.startswith("{")]

    def test_valid_file(self):
        records = self._dump("good.lines", "startpos moves e2e4\n\nstartpos\n")
        assert self.stockfish.process.returncode == 0
        assert [("ply" in r, "undo" in r) for r in records] == [
            (True, False), (True, False), (False, True), (True, False), (False, True)
        ]

    def test_malformed_lines(self):
        # a line that cannot be played is reported, the next ones are still dumped
        records = self._dump("bad.lines", "startpos moves e2e5\nnonsense\nstartpos moves e2e4\n")
        assert self.stockfish.process.returncode == 0
        assert records[0] == {"error": "Illegal move e2e5 at ply 1", "input": "startpos moves e2e5"}
        assert records[1]["error"].startswith("Malformed line") and records[1]["input"] == "nonsense"
        assert records[2]["ply"] == 0 and records[-1]["undo"] == 1

    def test_missing_file(self):
        self.stockfish = Stockfish("dumpline does_not_exist.lines".split(" "), True, expect_failure=True)
        assert self.stockfish.process.returncode != 0
        assert "CRITICAL ERROR" in self.stockfish.process.stdout


def read_sessions(name, count):
    """The first sessions of a set of the sample of the search trace, with their records."""
    sample = PATH / "differential" / "sample"
    with gzip.open(sample / f"{name}.trace.gz", "rt") as f:
        sessions = [line.strip() for line in f if line.strip()][:count]
    records = []
    with gzip.open(sample / f"{name}.trace.jsonl.gz", "rt") as f:
        for line in f:
            record = json.loads(line)
            if record["session"] > count:
                break
            records.append(record)
    assert len(sessions) == count and records
    return sessions, records


class TestSearchTraceUCI(metaclass=OrderedClassMembers):
    """The searches of the search trace, played through plain UCI to any build:
    what it prints must be what the tracing binary recorded."""

    def beforeAll(self):
        self.stockfish = Stockfish()

    def afterAll(self):
        self.stockfish.quit()
        assert self.stockfish.close() == 0

    def afterEach(self):
        assert postfix_check(self.stockfish.get_output()) == True
        self.stockfish.clear_output()

    def _play(self, name, count, chess960):
        sessions, records = read_sessions(name, count)
        # the options the trace fixes, see tests/differential/README.md
        for option, value in (("Threads", "1"), ("Hash", "2"), ("MultiPV", "1"),
                              ("UCI_Chess960", "true" if chess960 else "false")):
            self.stockfish.setoption(option, value)

        expected = iter(records)
        for session in sessions:
            self.stockfish.send_command("ucinewgame")
            for command in (c.strip() for c in session.split(";")):
                if command.startswith("position"):
                    self.stockfish.send_command(command)
                    continue
                self.stockfish.send_command(command)
                info = []
                for line in self.stockfish.readline():
                    if line.startswith("info depth") and " score " in line:
                        info.append(re.sub(r" (nps|time) \d+", "", line[len("info "):]))
                    elif line.startswith("bestmove"):
                        break
                record = next(expected)
                assert info == record["info"]
                assert line.split()[1] == record["bestmove"]
        self.stockfish.setoption("UCI_Chess960", "false")

    def test_golden_sample(self):
        self._play("standard", 25, False)

    def test_golden_sample_chess960(self):
        self._play("chess960", 8, True)


class TestSearchTrace(metaclass=OrderedClassMembers):
    """The 'searchtrace' command of the tracing binary, built next to the binary
    under test with 'make trace-build'. Skipped when there is none."""

    def beforeAll(self):
        self.path = os.path.join(os.path.dirname(get_path()), "stockfish-trace")
        self.available = os.path.exists(self.path)
        # set in CI, where a missing tracing binary must fail rather than skip
        if os.environ.get("SEARCH_TRACE_REQUIRED") == "1":
            assert self.available, f"no tracing binary at {self.path}"

    def beforeEach(self):
        self.stockfish = None

    def afterEach(self):
        if self.stockfish:
            assert postfix_check(self.stockfish.get_output()) == True
            self.stockfish.clear_output()

    def _trace(self, sessions, *options, chess960=False):
        """Runs 'searchtrace' on sessions, returns the records and the other lines."""
        with open("sessions.txt", "w") as f:
            f.write("".join(s + "\n" for s in sessions))
        self.stockfish = Engine(get_prefix(), self.path)
        if chess960:
            self.stockfish.setoption("UCI_Chess960", "true")
        self.stockfish.send_command(" ".join(["searchtrace", "sessions.txt", *options]))
        # the command runs in the UCI loop: once it is done, the next one is answered
        self.stockfish.send_command("isready")
        lines = []
        for line in self.stockfish.readline():
            if line == "readyok":
                break
            lines.append(line)
        self.stockfish.quit()
        assert self.stockfish.close() == 0
        return [json.loads(l) for l in lines if l.startswith("{")], [l for l in lines if not l.startswith("{")]

    def test_record(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        records, _ = self._trace(["position startpos moves e2e4 ; go depth 4 ; position startpos moves e2e4 e7e5 ; go nodes 500"])

        assert [(r["session"], r["search"], r["go"]) for r in records] == [(1, 1, "depth 4"), (1, 2, "nodes 500")]
        first, second = records
        assert first["fen"] == "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1"
        assert [it["depth"] for it in first["iterations"]] == [1, 2, 3, 4]
        assert all(it["completed"] for it in first["iterations"])
        assert first["info"][-1].startswith("depth 4 ") and first["nodes"] == first["iterations"][-1]["nodes"]
        assert first["info"][-1].endswith(" pv " + first["iterations"][-1]["pv"])
        assert first["bestmove"] == first["iterations"][-1]["pv"].split()[0]
        # a node limit stops the last iteration before its end
        assert second["nodes"] >= 500 and not second["iterations"][-1]["completed"]
        # one checkpoint every 4096 events
        for r in records:
            assert len(r["checkpoints"]) == r["events"] // 4096
        assert first["state"]["tt_generation"] == 1 and second["state"]["tt_generation"] == 2

    def test_digest_of_the_events(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        records, lines = self._trace(["position startpos ; go depth 7"], "events", "1", "0", "1000000000")

        # the digest is 64-bit FNV-1a over the events, each followed by a newline
        events = [l.split(" ", 1) for l in lines if l[:1].isdigit()]
        assert [int(i) for i, _ in events] == list(range(len(events)))
        digest, checkpoints = 0xCBF29CE484222325, []
        for n, (_, event) in enumerate(events, 1):
            for byte in (event + "\n").encode():
                digest = ((digest ^ byte) * 0x100000001B3) & (2**64 - 1)
            if n % 4096 == 0:
                checkpoints.append(f"{digest:016X}")
        record = records[0]
        assert record["events"] == len(events)
        assert record["digest"] == f"{digest:016X}" and record["checkpoints"] == checkpoints
        assert events[0][1].startswith("start generation=1 ")
        assert sum(e.startswith("search ") for _, e in events) > 100
        assert events[-1][1].startswith("iteration_end depth=7 completed=1 ")

    def test_window_and_context(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        _, all_lines = self._trace(["position startpos ; go depth 6"], "events", "1", "0", "1000000000")
        _, lines = self._trace(["position startpos ; go depth 6"], "events", "1", "3000", "3010")

        events = [l for l in lines if l[:1].isdigit()]
        assert events == [l for l in all_lines if l[:1].isdigit()][3000:3010]
        # the nodes open at event 3000, from the root down
        context = [l[len("context "):] for l in lines if l.startswith("context ")]
        assert context[0].split(" ", 1)[1].startswith("search ply=0 ")
        assert all(int(c.split()[0]) < 3000 for c in context)

    def test_sessions_are_independent(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        a = "position startpos moves d2d4 ; go depth 6"
        b = "position startpos moves d2d4 d7d5 ; go depth 6"
        apart, _ = self._trace([a, b])
        together, _ = self._trace([a + " ; " + b])

        # each session starts from a cleared state ...
        alone, _ = self._trace([b])
        strip = lambda r: {k: v for k, v in r.items() if k != "session"}
        assert strip(apart[1]) == strip(alone[0])
        # ... and within a session the state is inherited
        assert together[1]["search"] == 2
        assert together[1]["state"]["tt_generation"] == 2
        assert together[1]["digest"] != apart[1]["digest"]

    def test_no_legal_move(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        records, _ = self._trace(["position fen R5k1/5ppp/8/8/8/8/8/4K3 b - - 0 1 ; go depth 3"])
        assert records[0]["bestmove"] == "(none)" and records[0]["info"] == ["depth 0 score mate 0"]
        assert records[0]["events"] == 0 and records[0]["nodes"] == 0
        # no search ran: the low ply history is what an earlier search left
        assert records[0]["state"]["low_ply_history"] is None

    def test_errors(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        records, _ = self._trace([
            "go depth 3",
            "position startpos ; go movetime 100",
            "position startpos moves e2e5 ; go depth 1",
            "position startpos ; go depth 1",
        ])
        assert [r["session"] for r in records] == [1, 2, 3, 4]
        assert records[0]["error"] == "go before any position"
        assert records[1]["error"] == "Only 'go depth N' and 'go nodes N' are allowed"
        assert "error" in records[2] and records[2]["search"] == 1
        assert records[3]["bestmove"] != "(none)"

    def test_golden_sample(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        sessions, expected = read_sessions("standard", 15)
        records, _ = self._trace(sessions)
        assert records == expected

    def test_golden_sample_chess960(self):
        if not self.available:
            print("no stockfish-trace next to the binary, skipped")
            return
        sessions, expected = read_sessions("chess960", 5)
        records, _ = self._trace(sessions, chess960=True)
        assert records == expected


def parse_args():
    parser = argparse.ArgumentParser(description="Run Stockfish with testing options")
    parser.add_argument("--valgrind", action="store_true", help="Run valgrind testing")
    parser.add_argument(
        "--valgrind-thread", action="store_true", help="Run valgrind-thread testing"
    )
    parser.add_argument(
        "--sanitizer-undefined",
        action="store_true",
        help="Run sanitizer-undefined testing",
    )
    parser.add_argument(
        "--sanitizer-thread", action="store_true", help="Run sanitizer-thread testing"
    )

    parser.add_argument(
        "--none", action="store_true", help="Run without any testing options"
    )
    parser.add_argument("stockfish_path", type=str, help="Path to Stockfish binary")

    return parser.parse_args()


if __name__ == "__main__":
    args = parse_args()

    EPD.create_bench_epd()
    Syzygy.download_syzygy()

    framework = MiniTestFramework()

    # Each test suite will be run inside a temporary directory
    framework.run(
        [
            TestCLI,
            TestInteractive,
            TestSyzygy,
            TestEnPassantSanitization,
            TestInvalidFEN,
            TestInvalidOptions,
            TestBenchFile,
            TestDump,
            TestDumpFile,
            TestDumpLine,
            TestDumpLineFile,
            TestSearchTraceUCI,
            TestSearchTrace,
        ]
    )

    EPD.delete_bench_epd()

    if framework.has_failed():
        sys.exit(1)

    sys.exit(0)

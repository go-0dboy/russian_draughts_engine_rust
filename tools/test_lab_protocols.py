#!/usr/bin/env python3
"""Real process tests for production/legacy match and SPSA adapters."""
import os
from pathlib import Path
import unittest
from match import Engine, EngineSpec, PairWorker

ROOT = Path(__file__).resolve().parents[1]
BINS = Path(os.environ.get("RD_TEST_BIN_DIR", ROOT / "target/release"))


class ProtocolTests(unittest.TestCase):
    def spec(self, production):
        return EngineSpec(
            BINS / ("classic_engine" if production else "examples/classic_server"),
            ROOT / "config/eval_default.txt",
            ROOT / "config/search_default.txt",
            1,
        )

    def test_opening_and_complete_pair_all_protocol_combinations(self):
        for a, b in ((False, False), (True, True), (True, False), (False, True)):
            with self.subTest(a=a, b=b):
                worker = PairWorker(self.spec(a), self.spec(b))
                try:
                    result = worker.play_pair(
                        [0x0000016F00000000], "nodes", 200, 64, 500, "abort"
                    )
                    self.assertIn(result[0], ("A", "B", "D", "X"))
                    self.assertIn(result[2], ("A", "B", "D", "X"))
                    self.assertGreater(result[1].moves_a + result[1].moves_b, 0)
                finally:
                    worker.close()
                self.assertIsNotNone(worker.a.p.poll())
                self.assertIsNotNone(worker.b.p.poll())

    def test_capture_and_promotion_cross_protocol(self):
        # Play an entire deterministic game, resolving every production move
        # against the legacy generator, including capture metadata.
        a = Engine(self.spec(True))
        b = Engine(self.spec(False))
        captures = 0
        try:
            for _ in range(400):
                self.assertEqual(a.status(), b.status())
                line, _ = a.go("nodes", 500, 64)
                if not line.startswith("bestmove "):
                    break
                move = line.split()[1]
                captures += move.split(":")[2] != "-"
                a.play(move)
                b.play(move)
            self.assertGreater(captures, 0)
        finally:
            a.close()
            b.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)

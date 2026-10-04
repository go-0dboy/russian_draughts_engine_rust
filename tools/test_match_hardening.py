#!/usr/bin/env python3
"""Regression tests for the process/protocol hardening in tools/match.py."""

from __future__ import annotations

import struct
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import match as lab
import tune_search as tuner


class EngineHarnessTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(
            prefix="rd-match-test-", dir=str(TOOLS.parent)
        )
        self.root = Path(self.temp.name)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def fake_engine(self, body: str) -> Path:
        path = self.root / "fake_engine.py"
        path.write_text(
            f"#!{sys.executable}\n" + textwrap.dedent(body),
            encoding="utf-8",
        )
        path.chmod(0o755)
        return path

    def spec(self, path: Path, **overrides: float) -> lab.EngineSpec:
        params = dict(
            startup_timeout_s=3.0,
            command_timeout_s=0.25,
            go_timeout_s=0.25,
            shutdown_timeout_s=0.20,
            kill_timeout_s=0.20,
        )
        params.update(overrides)
        return lab.EngineSpec(path, None, None, 1, **params)

    def test_partial_protocol_line_times_out(self) -> None:
        path = self.fake_engine(
            """
            import sys, time
            print("ready fake", flush=True)
            for line in sys.stdin:
                if line.strip() == "quit":
                    break
                sys.stdout.write("ok")
                sys.stdout.flush()
                time.sleep(60)
            """
        )
        engine = lab.Engine(self.spec(path))
        try:
            started = time.monotonic()
            with self.assertRaises(TimeoutError):
                engine.cmd("new")
            self.assertLess(time.monotonic() - started, 1.5)
        finally:
            engine.close()

    def test_close_does_not_wait_for_bye(self) -> None:
        path = self.fake_engine(
            """
            import sys, time
            print("ready fake", flush=True)
            for line in sys.stdin:
                if line.strip() == "quit":
                    time.sleep(60)
            """
        )
        engine = lab.Engine(self.spec(path))
        started = time.monotonic()
        engine.close()
        self.assertLess(time.monotonic() - started, 1.5)
        self.assertIsNotNone(engine.p.poll())

    def test_large_stderr_cannot_fill_a_pipe(self) -> None:
        path = self.fake_engine(
            """
            import sys
            sys.stderr.write("x" * 2_000_000)
            sys.stderr.flush()
            print("ready fake", flush=True)
            for line in sys.stdin:
                if line.strip() == "quit":
                    break
            """
        )
        engine = lab.Engine(self.spec(path, startup_timeout_s=5.0))
        self.assertEqual(engine.version, "fake")
        engine.close()

    def test_dynamic_scheduler_commits_pairs_in_id_order(self) -> None:
        path = self.fake_engine(
            """
            import sys, time
            side = "white"
            plies = 0
            print("ready fake", flush=True)
            for line in sys.stdin:
                parts = line.split()
                if not parts:
                    continue
                cmd = parts[0]
                if cmd == "new":
                    side = "white"
                    plies = 0
                    print("ok", flush=True)
                elif cmd == "play":
                    plies += 1
                    side = "black" if side == "white" else "white"
                    print("ok", flush=True)
                elif cmd == "side":
                    print("side " + side, flush=True)
                elif cmd == "status":
                    print("status draw Test" if plies >= 2 else "status ongoing", flush=True)
                elif cmd == "go":
                    time.sleep(0.003)
                    print("move 0000000100000000 score 0 depth 1 nodes 10 time_us 3000", flush=True)
                elif cmd == "quit":
                    print("bye", flush=True)
                    break
                else:
                    print("ok", flush=True)
            """
        )
        openings = self.root / "openings.rdopen"
        with openings.open("wb") as f:
            f.write(b"RDOPEN2\0")
            f.write(struct.pack("<IIQ", 1, 0, 8))
            for _ in range(8):
                f.write(struct.pack("<HH", 0, 0))

        completed = subprocess.run(
            [
                sys.executable,
                str(TOOLS / "match.py"),
                "--engine-a",
                str(path),
                "--engine-b",
                str(path),
                "--openings",
                str(openings),
                "--nodes",
                "10",
                "--pairs",
                "8",
                "--workers",
                "3",
                "--tt",
                "1",
                "--min-pairs",
                "8",
                "--elo0",
                "-2",
                "--elo1",
                "2",
                "--seed",
                "1",
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
            check=True,
        )
        pair_ids = [
            int(line.split()[1].rstrip(":"))
            for line in completed.stdout.splitlines()
            if line.startswith("pair ")
        ]
        self.assertEqual(pair_ids, list(range(1, 9)))
        self.assertIn("0 / 16 / 0 / 0", completed.stdout)

    def test_spsa_iteration_uses_dynamic_workers_without_losing_order(self) -> None:
        path = self.fake_engine(
            """
            import sys, time
            side = "white"
            plies = 0
            print("ready fake", flush=True)
            for line in sys.stdin:
                parts = line.split()
                if not parts:
                    continue
                cmd = parts[0]
                if cmd == "new":
                    side = "white"
                    plies = 0
                    print("ok", flush=True)
                elif cmd == "play":
                    plies += 1
                    side = "black" if side == "white" else "white"
                    print("ok", flush=True)
                elif cmd == "side":
                    print("side " + side, flush=True)
                elif cmd == "status":
                    print("status draw Test" if plies >= 2 else "status ongoing", flush=True)
                elif cmd == "go":
                    time.sleep(0.002)
                    print("move 0000000100000000 score 0 depth 1 nodes 10 time_us 2000", flush=True)
                elif cmd == "quit":
                    print("bye", flush=True)
                    break
                else:
                    print("ok", flush=True)
            """
        )
        eval_path = self.root / "eval.txt"
        plus_path = self.root / "plus.search"
        minus_path = self.root / "minus.search"
        for file in (eval_path, plus_path, minus_path):
            file.write_text("ignored\n", encoding="utf-8")
        stats, attempts = tuner.run_iteration(
            path,
            eval_path,
            plus_path,
            minus_path,
            [[] for _ in range(8)],
            list(range(8)),
            0,
            4,
            3,
            1,
            "nodes",
            10,
            64,
            20,
            2.0,
            0.5,
            0.5,
            0.2,
        )
        self.assertEqual(stats.pairs, 4)
        self.assertEqual(stats.score, 0.5)
        self.assertEqual(attempts, 4)

    def test_startup_timeout_is_bounded_and_process_is_reaped(self) -> None:
        path = self.fake_engine(
            """
            import time
            time.sleep(60)
            """
        )
        started = time.monotonic()
        with self.assertRaises(TimeoutError):
            lab.Engine(self.spec(path, startup_timeout_s=0.25))
        self.assertLess(time.monotonic() - started, 1.5)


if __name__ == "__main__":
    unittest.main(verbosity=2)

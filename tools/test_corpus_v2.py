#!/usr/bin/env python3
"""Offline contract tests. Synthetic stream != a test of Rust PDN/move rules."""
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import sqlite3
import contextlib
import time
import corpus_v2 as c
import rdtune3 as f
import reference_lab as ref
import test_corpus_engine as integration


def game(i, result=1, shared=True):
    moves = [[i % 32, (i + 1) % 32, i]] * 4
    g = "G\t%d\t1\t2147483648\t0\t0\t%d\t%s\t0,0,0,0\n" % (
        i,
        result,
        ";".join(":".join(map(str, m)) for m in moves),
    )
    for ply in range(4):
        # Valid bitboards for serialization; not asserted to be replay reachable.
        white = (
            1
            if shared and ply == 0
            else (1 << (i % 25)) | (1 << ((i // 25 + ply + 1) % 25))
        )
        g += "S\t%d\t%d\t2147483648\t0\t%d\t128\t0\t0\t%s\n" % (
            ply,
            white,
            ply % 2,
            ",".join(["0"] * 92),
        )
    return g


def stream(order=range(100)):
    return "RDSTREAM1\n" + "".join(game(i) for i in order) + "Z\t100\t0\n"


class CorpusTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def build(self, name="data", text=None):
        out = self.root / name
        c.build(io.StringIO(text or stream()), out, {"fixture": "synthetic"}, min_ply=0)
        return out

    def test_protocol_to_pdn_numbering(self):
        # Coordinates documented by the Rust PDN parser: a1=29, b8=1.
        self.assertEqual(integration.protocol_square_to_pdn(1), 29)
        self.assertEqual(integration.protocol_square_to_pdn(29), 1)
        self.assertEqual(integration.quiet_pdn("9:13:-"), "21-17")
        self.assertEqual(
            sorted(integration.protocol_square_to_pdn(i) for i in range(1, 33)),
            list(range(1, 33)),
        )
        for value in (0, 33):
            with self.assertRaises(ValueError):
                integration.protocol_square_to_pdn(value)

    def test_fixture_parser_failure_is_not_hidden_by_split(self):
        with self.assertRaisesRegex(AssertionError, "parser errors"):
            integration.validate_export("RDSTREAM1\nE\t0\t0\tIllegalMove\nZ\t1\t1\n", 1)
        with self.assertRaises(AssertionError):
            integration.validate_export("RDSTREAM1\nZ\t0\t0\n", 1)

    def test_optimized_filter_matches_role_priority(self):
        db = sqlite3.connect(":memory:")
        db.executescript(
            "CREATE TABLE games(gid TEXT PRIMARY KEY,role TEXT); "
            "CREATE TABLE samples(gid TEXT,pos BLOB,PRIMARY KEY(gid,pos)) WITHOUT ROWID; "
            "CREATE TABLE owner(pos BLOB PRIMARY KEY,priority INT) WITHOUT ROWID;"
        )
        expected = []
        for role, priority in c.PRIORITY.items():
            db.execute("INSERT INTO games VALUES(?,?)", (role, role))
            for owner in range(5):
                key = bytes([owner])
                db.execute("INSERT INTO samples VALUES(?,?)", (role, key))
                if owner <= priority:
                    expected.append((role, key))
        db.executemany(
            "INSERT INTO owner VALUES(?,?)", ((bytes([i]), i) for i in range(5))
        )
        c.drop_cross_role_samples(db)
        self.assertEqual(
            db.execute("SELECT gid,pos FROM samples ORDER BY gid,pos").fetchall(),
            sorted(expected),
        )
        db.close()

    def test_progress_keeps_binary_output_identical(self):
        a = self.build("without_progress")
        b = self.root / "with_progress"
        log = io.StringIO()
        with contextlib.redirect_stderr(log), c.Progress(0.01) as progress:
            c.build(
                io.StringIO(stream()),
                b,
                {"fixture": "synthetic"},
                min_ply=0,
                progress=progress,
            )
        for p in a.iterdir():
            self.assertEqual(p.read_bytes(), (b / p.name).read_bytes())
        self.assertIn("Сборка завершена", log.getvalue())

    def test_progress_heartbeat_without_records(self):
        log = io.StringIO()
        with contextlib.redirect_stderr(log), c.Progress(0.01) as progress:
            progress.stage("SQL")
            time.sleep(0.06)
        self.assertGreaterEqual(log.getvalue().count("SQL"), 3)

    def test_reproducible(self):
        a = self.build("a")
        b = self.build("b")
        self.assertEqual(
            (a / "manifest.json").read_bytes(), (b / "manifest.json").read_bytes()
        )
        for p in a.iterdir():
            self.assertEqual(p.read_bytes(), (b / p.name).read_bytes())

    def test_role_independent_of_source_order(self):
        a = self.build("a")
        b = self.build("b", stream(reversed(range(100))))
        for role in f.ROLES:
            self.assertEqual(
                (a / f"{role}.rdtune3").read_bytes(),
                (b / f"{role}.rdtune3").read_bytes(),
            )

    def test_audit_weights_and_leakage(self):
        out = self.build()
        counts = c.audit(out)
        self.assertTrue(all(n > 0 for n in counts.values()))
        self.assertGreater(
            json.loads((out / "manifest.json").read_text())["stats"][
                "cross_role_drops"
            ],
            0,
        )

    def test_final_explicit_lock(self):
        out = self.build()
        with self.assertRaises(ValueError):
            list(f.records(out / "final.rdtune3"))

    def test_corruption(self):
        out = self.build()
        p = out / "train.rdtune3"
        p.write_bytes(p.read_bytes() + b"x")
        with self.assertRaises(ValueError):
            c.audit(out)

    def test_truncated_stream_not_published(self):
        with self.assertRaises(ValueError):
            self.build(text=stream().rsplit("Z", 1)[0])
        self.assertFalse((self.root / "data").exists())

    def test_immutable_destination(self):
        self.build()
        with self.assertRaises(FileExistsError):
            self.build()

    def test_conflicting_duplicate_removed(self):
        text = "RDSTREAM1\n" + game(1, 0) + game(1, 2) + "Z\t2\t0\n"
        out = self.build(text=text)
        self.assertEqual(sum(c.audit(out).values()), 0)

    def test_duplicate_no_extra_weight(self):
        a = self.build("a", "RDSTREAM1\n" + game(5) + "Z\t1\t0\n")
        b = self.build("b", "RDSTREAM1\n" + game(5) + game(5) + "Z\t2\t0\n")
        for role in f.ROLES:
            self.assertEqual(
                (a / f"{role}.rdtune3").read_bytes(),
                (b / f"{role}.rdtune3").read_bytes(),
            )

    def test_stratified_cap(self):
        out = self.root / "small"
        c.build(io.StringIO(stream()), out, {}, cap=2, min_ply=0)
        self.assertLessEqual(sum(c.audit(out).values()), 200)

    def test_no_fake_stability(self):
        x = dict(usable=True, decision="9:13:-", score=5, depth=4)
        self.assertTrue(ref.stability([x, x], 20))
        self.assertFalse(ref.stability([x, dict(x, decision="10:14:-")], 20))
        self.assertFalse(ref.stability([x, dict(x, usable=False)], 20))
        self.assertFalse(ref.stability([x, dict(x, score=100)], 20))

    def test_teacher_and_benchmark_mock_protocol(self):
        corpus = self.build()
        engine = self.root / "engine"
        engine.write_text(
            "#!"
            + sys.executable
            + "\n"
            + """import sys
print('ready mock protocol rd-classic/1',flush=True)
for line in sys.stdin:
 p=line.split()
 if p[0]=='quit':break
 if p[0]=='go':print('bestmove 9:13:- score 5 depth 4 nodes '+p[2]+' time_us 1 stop node_limit',flush=True)
 else:print('ok',flush=True)
"""
        )
        engine.chmod(0o755)
        cfg = self.root / "cfg"
        cfg.write_text("fixture")
        common = [
            "--corpus",
            str(corpus),
            "--engine",
            str(engine),
            "--eval",
            str(cfg),
            "--search",
            str(cfg),
            "--budgets",
            "10",
            "20",
        ]
        tool = str(Path(ref.__file__))
        subprocess.run(
            [
                sys.executable,
                tool,
                "teacher",
                *common,
                "--out",
                str(self.root / "teacher"),
                "--limit",
                "3",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        subprocess.run(
            [
                sys.executable,
                tool,
                "benchmark",
                *common,
                "--teacher",
                str(self.root / "teacher"),
                "--out",
                str(self.root / "bench"),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        m = json.loads((self.root / "bench/manifest.json").read_text())
        self.assertEqual(m["summary"]["agreement_by_budget"]["20"]["agreement"], 1)
        result = subprocess.run(
            [
                sys.executable,
                tool,
                "teacher",
                *common,
                "--out",
                str(self.root / "teacher"),
                "--limit",
                "3",
            ],
            capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)

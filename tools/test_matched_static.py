#!/usr/bin/env python3
import io
import itertools
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import corpus_v2 as c
import rdtune3 as f
import matched_static as m
from test_corpus_v2 import stream


class Tests(unittest.TestCase):
    def test_pairing_and_weight_preservation(self):
        a = dict(
            game_id="a" * 64,
            ply=7,
            result=2,
            role="train",
            weight=0.25,
            position=[1, 2, 0, 0],
            features=[1] * 92,
            phase=256,
            flags=1,
        )
        q = dict(a, weight=0.5, position=[4, 2, 0, 1], features=[2] * 92)
        r = list(m.matched_rows([a], [q]))[0]
        self.assertEqual(r["weight"], q["weight"])
        self.assertEqual(r["position"], a["position"])
        self.assertEqual(r["features"], a["features"])
        self.assertEqual(r["result"], 2)
        with self.assertRaises(ValueError):
            list(m.matched_rows([a], [dict(q, ply=8)]))
        with self.assertRaises(ValueError):
            list(m.matched_rows([a], [dict(q, result=0)]))
        with self.assertRaises(ValueError):
            list(m.matched_rows([a], [q, q]))

    def test_complete_build_and_parent_guard(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = root / "static"
            q = root / "q"
            out = root / "matched"
            c.build(io.StringIO(stream()), source, {}, min_ply=0)
            shutil.copytree(source, q)
            qm = json.loads((q / "manifest.json").read_text())
            qm["mode"] = "QRESOLVED"
            qm["source"] = {"parent_manifest": f.sha(source / "manifest.json")}
            for role in ("train", "dev"):
                kept = []
                for _, group in itertools.groupby(
                    f.records(source / f"{role}.rdtune3"), lambda r: r["game_id"]
                ):
                    selected = list(group)[::2]
                    kept.extend(
                        dict(r, weight=1 / len(selected), features=[2] * 92)
                        for r in selected
                    )
                p = q / f"{role}.rdtune3"
                p.unlink()
                f.write(p, role, len(kept), kept)
                qm["counts"][role] = len(kept)
            qm["files"] = {name: f.sha(q / name) for name in qm["files"]}
            (q / "manifest.json").write_text(json.dumps(qm))
            result = m.build(source, q, out)
            self.assertEqual(result["counts"], qm["counts"])
            c.audit(out)
            for role in ("train", "dev"):
                for original, leaf in zip(
                    f.records(out / f"{role}.rdtune3"), f.records(q / f"{role}.rdtune3")
                ):
                    for k in ("game_id", "ply", "weight", "result", "role"):
                        self.assertEqual(original[k], leaf[k])
                    self.assertNotEqual(original["features"], leaf["features"])
            for role in ("reference", "arena", "final"):
                self.assertEqual(
                    (source / f"{role}.rdtune3").read_bytes(),
                    (out / f"{role}.rdtune3").read_bytes(),
                )
            with self.assertRaises(FileExistsError):
                m.build(source, q, out)
            qm["source"]["parent_manifest"] = "wrong"
            (q / "manifest.json").write_text(json.dumps(qm))
            with self.assertRaises(ValueError):
                m.build(source, q, root / "bad")
            self.assertFalse((root / "bad").exists())

    def test_fixed_scale_real_tuner(self):
        binary = Path("target/release/classic_tune_eval").resolve()
        if not binary.is_file():
            self.skipTest("requires release classic_tune_eval")
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = root / "static"
            c.build(io.StringIO(stream()), source, {}, min_ply=0)
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).with_name("hce_v2.py")),
                    "--corpus",
                    str(source),
                    "--base",
                    "config/eval_default.txt",
                    "--out",
                    str(root / "tune"),
                    "--binary",
                    str(binary),
                    "--epochs",
                    "1",
                    "--scale",
                    "626.751624",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            manifest = json.loads((root / "tune/manifest.json").read_text())
            self.assertEqual(manifest["identity"]["scale"], "626.751624")
            self.assertIn(
                "logistic scale: 626.751624", (root / "tune/training.log").read_text()
            )

    def test_scale_validation(self):
        tool = Path(__file__).with_name("hce_v2.py")
        for value in ("nan", "inf", "0", "-1", "bad"):
            r = subprocess.run(
                [
                    sys.executable,
                    str(tool),
                    "--corpus",
                    "missing",
                    "--base",
                    "missing",
                    "--out",
                    "missing",
                    "--scale",
                    value,
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(r.returncode, 2)
            self.assertIn("scale", r.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)

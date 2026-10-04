#!/usr/bin/env python3
"""Transport/leakage tests with a mock resolver; Rust tests cover minimax."""
import io
from pathlib import Path
import sys
import tempfile
import unittest
import corpus_v2 as c
import qresolved as q
import rdtune3 as f
from test_corpus_v2 import game


class QTests(unittest.TestCase):
    def test_derived_corpus_preserves_protected_roles_and_labels(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = root / "static"
            target = root / "q"
            text = (
                "RDSTREAM1\n"
                + "".join(
                    game(i, result=i % 3, shared=False).replace(
                        "\t0,0,0,0\n", "\t" + ",".join([f"{i+1:x}"] * 4) + "\n"
                    )
                    for i in range(100)
                )
                + "Z\t100\t0\n"
            )
            c.build(io.StringIO(text), source, {}, min_ply=0)
            # Exact synthetic board/features echo, no claim to implement draughts.
            engine = root / "mock"
            engine.write_text(
                "#!"
                + sys.executable
                + "\n"
                + """import sys
print('RDQ1',flush=True)
for line in sys.stdin:
 t=line.split();i=int(t[6].split(',')[0],16)-1
 for ply in map(int,t[5].split(',')):
  w=(1<<(i%25))|(1<<((i//25+ply+1)%25))
  print('Q',ply,0,1,w,2147483648,0,ply%2,128,','.join(['0']*92),'-',0,sep='\\t')
 print('Z',flush=True)
"""
            )
            engine.chmod(0o755)
            evaluation = root / "eval"
            evaluation.write_text("mock")
            m = q.derive(source, target, engine, evaluation)
            self.assertEqual(m["mode"], "QRESOLVED")
            self.assertGreater(m["qresolved_stats"]["resolved"], 0)
            c.audit(target)
            for role in f.ROLES:
                self.assertEqual(
                    (source / f"{role}.rdtune3").read_bytes(),
                    (target / f"{role}.rdtune3").read_bytes(),
                )
            with self.assertRaises(FileExistsError):
                q.derive(source, target, engine, evaluation)

    def test_q_leaf_cannot_enter_original_other_role(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = root / "static"
            target = root / "q"
            text = (
                "RDSTREAM1\n"
                + "".join(game(i, shared=False) for i in range(100))
                + "Z\t100\t0\n"
            )
            c.build(io.StringIO(text), source, {}, min_ply=0)
            protected = next(f.records(source / "dev.rdtune3"))["position"]
            engine = root / "mock"
            engine.write_text(
                "#!"
                + sys.executable
                + "\n"
                + "import sys\nposition="
                + repr(protected)
                + "\n"
                + "print('RDQ1',flush=True)\nfor line in sys.stdin:\n for ply in map(int,line.split()[5].split(',')):\n  print('Q',ply,0,1,*position,128,','.join(['0']*92),'-',0,sep='\\t')\n print('Z',flush=True)\n"
            )
            engine.chmod(0o755)
            evaluation = root / "eval"
            evaluation.write_text("mock")
            m = q.derive(source, target, engine, evaluation)
            self.assertGreater(m["qresolved_stats"]["skip_original_role_collision"], 0)
            self.assertEqual(m["counts"]["train"], 0)
            c.audit(target)
            for role in ("reference", "arena", "final"):
                self.assertEqual(
                    (source / f"{role}.rdtune3").read_bytes(),
                    (target / f"{role}.rdtune3").read_bytes(),
                )

    def test_sample_line_preserves_white_label_indirectly(self):
        # Result lives in G, never negated when Q switches the side to move.
        line = q.sample_line(
            dict(ply=3, position=[1, 2, 0, 1], phase=12, flags=2, features=[0] * 92)
        )
        self.assertEqual(line.split("\t")[5], "1")


if __name__ == "__main__":
    unittest.main(verbosity=2)

#!/usr/bin/env python3
"""Real-engine integration gate (requires release binaries; never uses FINAL to train)."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from match import Engine, EngineSpec
import corpus_v2 as c
import rdtune3 as f
import qresolved as q


def protocol_square_to_pdn(value):
    """Protocol uses bottom-up internal square+1; PDN numbers top-down."""
    square = int(value) - 1
    if not 0 <= square < 32:
        raise ValueError("protocol square outside 1..32")
    return (7 - square // 4) * 4 + square % 4 + 1


def quiet_pdn(move):
    start, end, captures = move.split(":")
    if captures != "-":
        raise AssertionError("fixture expected quiet opening")
    return f"{protocol_square_to_pdn(start)}-{protocol_square_to_pdn(end)}"


def checked(client, command):
    answer = client.cmd(command)
    if answer != "ok":
        raise AssertionError(f"{command}: {answer}")


def legal(client):
    answer = client.cmd("legal").split()
    if len(answer) < 2 or answer[0] != "legal" or int(answer[1]) != len(answer) - 2:
        raise AssertionError(f"invalid legal response: {answer}")
    return answer[2:]


def fixture_games(client):
    # A third move makes the exporter include the distinct two-ply boards.
    checked(client, "new")
    first = legal(client)
    games = []
    for a in first:
        checked(client, "new")
        checked(client, "play " + a.replace(":", " "))
        second = legal(client)
        for b in second:
            checked(client, "new")
            checked(client, "play " + a.replace(":", " "))
            checked(client, "play " + b.replace(":", " "))
            third = next((m for m in legal(client) if m.split(":")[2] == "-"), None)
            if third is None:
                continue
            games.append(
                '[Event "fixture"]\n[Result "1-1"]\n\n'
                f"1. {quiet_pdn(a)} {quiet_pdn(b)} 2. {quiet_pdn(third)} 1-1\n\n"
            )
    if len(games) < 20:
        raise AssertionError(f"too few fixture lines: {len(games)}")
    return games


def validate_export(text, expected_games):
    rows = [line.split("\t") for line in text.splitlines()]
    errors = [row for row in rows if row[0] == "E"]
    games = [row for row in rows if row[0] == "G"]
    samples = [row for row in rows if row[0] == "S"]
    if errors or len(games) != expected_games or len(samples) != 3 * expected_games:
        raise AssertionError(
            f"fixture export: games={len(games)}/{expected_games}, "
            f"samples={len(samples)}, parser errors={errors[:3]}"
        )
    if not rows or rows[-1] != ["Z", str(expected_games), "0"]:
        raise AssertionError(f"bad fixture export footer: {rows[-1:]}")


def main():
    root = Path(__file__).resolve().parents[1]
    binary = root / "target/release/classic_engine"
    client = Engine(EngineSpec(binary, None, None, 1))
    try:
        games = fixture_games(client)
    finally:
        client.close()
    with tempfile.TemporaryDirectory() as tmp:
        p = Path(tmp)
        pdn = p / "fixture.pdn"
        pdn.write_text("".join(games))
        export = subprocess.run(
            [str(root / "target/release/classic_corpus_stream"), str(pdn)],
            check=True,
            capture_output=True,
            text=True,
        )
        validate_export(export.stdout, len(games))
        # Small fixture loses shared boards to protected roles; search seeds for
        # nonempty train/dev, purely a test fixture, never done on a real corpus.
        for seed in range(100):
            dest = p / f"data{seed}"
            m = c.build(
                iter(export.stdout.splitlines(True)),
                dest,
                {"fixture": "real engine"},
                seed=seed,
                min_ply=2,
            )
            if m["counts"]["train"] and m["counts"]["dev"]:
                break
        else:
            raise AssertionError(
                f'no train/dev in test fixture; last counts={m["counts"]}, stats={m["stats"]}'
            )
        c.audit(dest)
        assert m["counts"]["reference"] > 0
        common = [
            sys.executable,
            str(root / "tools/reference_lab.py"),
            "teacher",
            "--corpus",
            str(dest),
            "--eval",
            str(root / "config/eval_default.txt"),
            "--search",
            str(root / "config/search_default.txt"),
            "--limit",
            "3",
            "--budgets",
            "200",
            "400",
        ]
        invalid = p / "wrong_protocol"
        failure = subprocess.run(
            common
            + [
                "--engine",
                str(root / "target/release/examples/classic_server"),
                "--out",
                str(invalid),
            ],
            capture_output=True,
            text=True,
        )
        assert failure.returncode != 0 and not invalid.exists()
        teacher = p / "teacher"
        subprocess.run(
            common + ["--engine", str(binary), "--out", str(teacher)], check=True
        )
        assert (
            json.loads((teacher / "manifest.json").read_text())["status"] == "complete"
        )

        for role in f.ROLES:
            for r in f.records(dest / f"{role}.rdtune3", allow_final=True):
                assert len(r["features"]) == 92
        subprocess.run(
            [
                sys.executable,
                str(root / "tools/hce_v2.py"),
                "--corpus",
                str(dest),
                "--base",
                str(root / "config/eval_default.txt"),
                "--binary",
                str(root / "target/release/classic_tune_eval"),
                "--out",
                str(p / "tune"),
                "--epochs",
                "1",
                "--batch",
                "8",
                "--polish-passes",
                "1",
                "--polish-samples",
                "8",
            ],
            check=True,
        )
        assert (
            json.loads((p / "tune/manifest.json").read_text())["status"] == "complete"
        )
        derived = p / "qcorpus"
        qm = q.derive(
            dest,
            derived,
            root / "target/release/classic_qresolve",
            root / "config/eval_default.txt",
        )
        assert qm["counts"]["train"] and qm["counts"]["dev"]
        for role in ("reference", "arena", "final"):
            assert (dest / f"{role}.rdtune3").read_bytes() == (
                derived / f"{role}.rdtune3"
            ).read_bytes()
        subprocess.run(
            [
                sys.executable,
                str(root / "tools/hce_v2.py"),
                "--corpus",
                str(derived),
                "--base",
                str(root / "config/eval_default.txt"),
                "--binary",
                str(root / "target/release/classic_tune_eval"),
                "--out",
                str(p / "qtune"),
                "--epochs",
                "1",
                "--batch",
                "8",
                "--polish-passes",
                "1",
                "--polish-samples",
                "8",
            ],
            check=True,
        )
        assert (p / "qtune/candidate.diagnostics.csv").is_file()
    print("real-engine Corpus V2 + QRESOLVED + weighted HCE + polish: OK")


if __name__ == "__main__":
    main()

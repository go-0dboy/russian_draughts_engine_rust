#!/usr/bin/env python3
"""Run the complete local acceptance gate for a source release.

Designed for Linux/macOS/Termux. The script fails on the first mandatory gate
and keeps the strength baseline separate from infrastructure changes.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from lab_common import Progress

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_PERFT_10 = 22_449_922
EXPECTED_FIXED = ("0000016f00000000", 2, 12, 100_000)


def run(args: list[str], *, capture: bool = False) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(args), flush=True)
    with Progress() as progress:
        progress.stage("Проверка: " + " ".join(args))
        return subprocess.run(
            args,
            cwd=ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE if capture else None,
            stderr=subprocess.STDOUT if capture else None,
        )


def fixed_node_result(engine: Path) -> tuple[str, int, int, int]:
    args = [
        str(engine),
        "--eval",
        "config/eval_default.txt",
        "--search",
        "config/search_default.txt",
        "--tt",
        "16",
    ]
    print("+", " ".join(args), "< fixed-node protocol", flush=True)
    proc = subprocess.run(
        args,
        cwd=ROOT,
        input="go nodes 100000 64\nquit\n",
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=True,
    )
    print(proc.stdout, end="")
    match = re.search(
        r"^move\s+([0-9a-fA-F]+)\s+score\s+(-?\d+)\s+depth\s+(\d+)\s+nodes\s+(\d+)\b",
        proc.stdout,
        re.MULTILINE,
    )
    if not match:
        raise RuntimeError(f"cannot parse fixed-node result from {engine}")
    return (
        match.group(1).lower(),
        int(match.group(2)),
        int(match.group(3)),
        int(match.group(4)),
    )


def main() -> int:
    run([sys.executable, "tools/check_source_hygiene.py"])
    run(
        [
            sys.executable,
            "-m",
            "py_compile",
            *map(str, sorted((ROOT / "tools").glob("*.py"))),
        ]
    )
    run([sys.executable, "tools/test_match_hardening.py"])
    run([sys.executable, "tools/test_corpus_v2.py"])
    run([sys.executable, "tools/test_qresolved.py"])

    run(["cargo", "fmt", "--all", "--", "--check"])
    run(["cargo", "check", "--release", "--all-targets"])
    run(["cargo", "clippy", "--release", "--all-targets", "--", "-D", "warnings"])
    run(["cargo", "test", "--release"])
    run(
        [
            "cargo",
            "build",
            "--release",
            "--example",
            "perft",
            "--example",
            "classic_server",
            "--bin",
            "classic_engine",
        ]
    )

    perft = run(["target/release/examples/perft", "10"], capture=True)
    assert perft.stdout is not None
    print(perft.stdout, end="")
    match = re.search(r"Узлов\s*:\s*(\d+)", perft.stdout)
    if not match or int(match.group(1)) != EXPECTED_PERFT_10:
        raise RuntimeError(
            f"perft(10) mismatch: expected {EXPECTED_PERFT_10}, got "
            f"{match.group(1) if match else 'unparseable output'}"
        )

    run([sys.executable, "tools/test_product_protocol.py"])
    run(
        [
            "cargo",
            "build",
            "--release",
            "--bin",
            "classic_corpus_stream",
            "--bin",
            "classic_tune_eval",
            "--bin",
            "classic_qresolve",
        ]
    )
    run([sys.executable, "tools/test_corpus_engine.py"])
    run([sys.executable, "tools/test_matched_static.py"])
    run([sys.executable, "tools/test_lab_protocols.py"])
    run([sys.executable, "tools/test_release.py"])

    candidate = ROOT / "target/release/examples/classic_server"
    candidate_result = fixed_node_result(candidate)
    if candidate_result != EXPECTED_FIXED:
        raise RuntimeError(
            f"fixed-node semantic mismatch: expected {EXPECTED_FIXED}, got {candidate_result}"
        )

    baseline = ROOT / "data/baselines/v1.7.0/classic_server"
    if baseline.is_file():
        baseline_result = fixed_node_result(baseline)
        if candidate_result != baseline_result:
            raise RuntimeError(
                f"candidate differs from v1.7.0 baseline: {candidate_result} != {baseline_result}"
            )
        print("fixed-node v1.7.0 comparison: EXACT")
    else:
        print("fixed-node v1.7.0 binary: not present; canonical expected tuple matched")

    print("\nALL MANDATORY RELEASE GATES: OK — все обязательные проверки пройдены")
    return 0


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

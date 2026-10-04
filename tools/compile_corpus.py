#!/usr/bin/env python3
"""Compile the PDN corpus and write reproducibility provenance.

This is the preferred product-laboratory entry point around `classic_corpus`.
It records SHA-256 identities of the source PDN and generated artifacts together
with the exact compile parameters, so a tuning/arena dataset can be reproduced
without relying on shell history.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path
from lab_common import Progress


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser()
    p.add_argument("input", type=Path)
    p.add_argument("--binary", type=Path, default=Path("target/release/classic_corpus"))
    p.add_argument("--tune", type=Path, default=Path("data/corpus/tuning.rdtune"))
    p.add_argument("--openings", type=Path, default=Path("data/corpus/openings.rdopen"))
    p.add_argument("--errors", type=Path, default=Path("data/corpus/rejected.tsv"))
    p.add_argument("--manifest", type=Path, default=Path("data/corpus/provenance.json"))
    p.add_argument("--tune-min-ply", type=int, default=10)
    p.add_argument("--tune-stride", type=int, default=3)
    p.add_argument("--tune-per-game", type=int, default=8)
    p.add_argument("--opening-count", type=int, default=4096)
    p.add_argument("--opening-min-ply", type=int, default=6)
    p.add_argument("--opening-max-ply", type=int, default=14)
    p.add_argument("--seed", default="0x20260925DA7A0001")
    p.add_argument("--include-fen-tuning", action="store_true")
    return p


def main() -> int:
    a = parser().parse_args()
    if not a.binary.is_file():
        raise SystemExit(f"missing corpus compiler: {a.binary}")
    if not a.input.is_file():
        raise SystemExit(f"missing input PDN: {a.input}")

    cmd = [
        str(a.binary),
        "compile",
        str(a.input),
        "--tune",
        str(a.tune),
        "--openings",
        str(a.openings),
        "--errors",
        str(a.errors),
        "--tune-min-ply",
        str(a.tune_min_ply),
        "--tune-stride",
        str(a.tune_stride),
        "--tune-per-game",
        str(a.tune_per_game),
        "--opening-count",
        str(a.opening_count),
        "--opening-min-ply",
        str(a.opening_min_ply),
        "--opening-max-ply",
        str(a.opening_max_ply),
        "--seed",
        str(a.seed),
    ]
    if a.include_fen_tuning:
        cmd.append("--include-fen-tuning")

    with Progress() as progress:
        progress.stage("Legacy corpus: компиляция PDN")
        subprocess.run(cmd, check=True)
    for path in (a.tune, a.openings, a.errors):
        if not path.is_file():
            raise SystemExit(f"compiler did not create expected artifact: {path}")

    a.manifest.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "schema": "rd-classic-corpus-provenance-v1",
        "command": cmd,
        "parameters": {
            "tune_min_ply": a.tune_min_ply,
            "tune_stride": a.tune_stride,
            "tune_per_game": a.tune_per_game,
            "opening_count": a.opening_count,
            "opening_min_ply": a.opening_min_ply,
            "opening_max_ply": a.opening_max_ply,
            "seed": str(a.seed),
            "include_fen_tuning": a.include_fen_tuning,
        },
        "sha256": {
            "compiler": sha256(a.binary),
            "source_pdn": sha256(a.input),
            "tuning": sha256(a.tune),
            "openings": sha256(a.openings),
            "rejected": sha256(a.errors),
        },
        "paths": {
            "source_pdn": str(a.input),
            "tuning": str(a.tune),
            "openings": str(a.openings),
            "rejected": str(a.errors),
        },
    }
    a.manifest.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"provenance: {a.manifest}")
    print(f"source sha256: {payload['sha256']['source_pdn']}")
    return 0


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

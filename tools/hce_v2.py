#!/usr/bin/env python3
"""Verified weighted HCE training entry point; never promotes a candidate."""
import argparse
import math
from pathlib import Path
import subprocess
import corpus_v2 as c
import rdtune3 as fmt
from reference_lab import atomic_json


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--corpus", type=Path, required=True)
    p.add_argument("--base", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument(
        "--binary", type=Path, default=Path("target/release/classic_tune_eval")
    )
    p.add_argument("--epochs", type=int, default=30)
    p.add_argument("--batch", type=int, default=4096)
    p.add_argument("--seed", type=int, default=20260930)
    p.add_argument("--lr", type=float, default=0.15)
    p.add_argument("--l2", type=float, default=0.00002)
    p.add_argument("--polish-passes", type=int, default=0)
    p.add_argument("--polish-samples", type=int, default=8192)
    p.add_argument(
        "--scale", default="auto", help="auto or fixed positive logistic scale"
    )
    a = p.parse_args()
    if a.scale != "auto":
        try:
            fixed_scale = float(a.scale)
        except ValueError:
            p.error("scale must be auto or a positive number")
        if not math.isfinite(fixed_scale) or fixed_scale <= 0:
            p.error("scale must be finite and positive")
    if not 0 <= a.l2 < 1 or a.polish_passes < 0 or a.polish_samples < 1:
        p.error("invalid polish/regularization settings")
    if a.epochs < 1 or a.batch < 1 or not 0 < a.lr < 100:
        p.error("invalid optimizer settings")
    with c.Progress() as progress:
        progress.stage("Проверка корпуса перед обучением")
        m = fmt.read_manifest(a.corpus)
    if (
        m["mode"] not in ("STATIC", "QRESOLVED")
        or not m["counts"]["train"]
        or not m["counts"]["dev"]
    ):
        raise ValueError("nonempty STATIC or QRESOLVED train/dev required")
    if a.out.exists():
        raise FileExistsError(
            "training run is immutable; choose a new output directory"
        )
    identity = dict(
        corpus_manifest=fmt.sha(a.corpus / "manifest.json"),
        base=fmt.sha(a.base),
        binary=fmt.sha(a.binary),
        epochs=a.epochs,
        batch=a.batch,
        seed=a.seed,
        lr=a.lr,
        scale=a.scale,
        l2=a.l2,
        polish_passes=a.polish_passes,
        polish_samples=a.polish_samples,
        tool=fmt.sha(__file__),
    )
    a.out.mkdir(parents=True)
    atomic_json(
        a.out / "manifest.json",
        dict(status="running", identity=identity, accepted=False),
    )
    cmd = [
        str(a.binary.resolve()),
        "--corpus",
        str(a.corpus.resolve()),
        "--base",
        str(a.base.resolve()),
        "--output",
        str((a.out / "candidate.txt").resolve()),
        "--epochs",
        str(a.epochs),
        "--batch",
        str(a.batch),
        "--seed",
        str(a.seed),
        "--lr",
        str(a.lr),
        "--scale",
        str(a.scale),
        "--l2",
        str(a.l2),
        "--polish-passes",
        str(a.polish_passes),
        "--polish-samples",
        str(a.polish_samples),
    ]
    with (a.out / "training.log").open("w") as log, c.Progress() as progress:
        progress.stage("HCE: загрузка, обучение и диагностика")
        result = subprocess.Popen(
            cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1
        )
        try:
            for line in result.stdout:
                log.write(line)
                log.flush()
                if line.startswith(
                    (
                        "epoch",
                        "polish",
                        "best epoch",
                        "baseline",
                        "engine-exact",
                        "split",
                        "logistic",
                        "early stop",
                        "Обучение",
                        "Эпоха",
                        "Integer",
                        "Загрузка",
                        "Диагностика",
                    )
                ):
                    print(line, end="", flush=True)
            result.wait()
        finally:
            if result.poll() is None:
                result.terminate()
            try:
                result.wait(timeout=5)
            except subprocess.TimeoutExpired:
                result.kill()
                result.wait()
            result.stdout.close()
    record = dict(
        status="complete" if result.returncode == 0 else "failed",
        identity=identity,
        accepted=False,
        returncode=result.returncode,
        log_sha256=fmt.sha(a.out / "training.log"),
    )
    if result.returncode == 0:
        record["candidate_sha256"] = fmt.sha(a.out / "candidate.txt")
        record["diagnostics_sha256"] = fmt.sha(a.out / "candidate.diagnostics.csv")
        record["continuous_sha256"] = fmt.sha(a.out / "candidate.continuous.txt")
    atomic_json(a.out / "manifest.json", record)
    if result.returncode:
        raise SystemExit(result.returncode)
    print(a.out / "candidate.txt")


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

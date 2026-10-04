#!/usr/bin/env python3
"""STATIC control with exactly the retained QRESOLVED observations and weights."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import shutil
import tempfile
import corpus_v2 as c
import rdtune3 as f


def key(record):
    return record["game_id"], record["ply"]


def matched_rows(static_rows, q_rows, progress=None, hist=None):
    source = iter(static_rows)
    current = None
    previous = None
    for n, q in enumerate(q_rows, 1):
        wanted = key(q)
        if previous is not None and wanted <= previous:
            raise ValueError("Q observations not strictly ordered")
        previous = wanted
        while current is None or key(current) < wanted:
            current = next(source, None)
            if current is None:
                raise ValueError("Q observation missing in STATIC")
        if key(current) != wanted:
            raise ValueError("Q observation missing in STATIC")
        if current["result"] != q["result"] or current["role"] != q["role"]:
            raise ValueError("Q label/role differs from original")
        r = dict(current, weight=q["weight"])
        if hist is not None:
            hist[c.sample_class(r)] += 1
        if progress:
            progress.count(n)
        yield r


def build(source, qcorpus, destination, progress=None):
    source = Path(source)
    qcorpus = Path(qcorpus)
    destination = Path(destination)
    if destination.exists():
        raise FileExistsError("choose a new output directory")
    c.report_stage(progress, "Проверка исходных корпусов и SHA256")
    sm = f.read_manifest(source)
    qm = f.read_manifest(qcorpus)
    if sm["mode"] != "STATIC" or qm["mode"] != "QRESOLVED":
        raise ValueError("requires STATIC and QRESOLVED")
    source_sha = f.sha(source / "manifest.json")
    q_sha = f.sha(qcorpus / "manifest.json")
    if qm["source"]["parent_manifest"] != source_sha:
        raise ValueError("Q corpus belongs to another STATIC parent")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".matched-static-", dir=destination.parent
    ) as td:
        stage = Path(td) / "corpus"
        stage.mkdir()
        classes = {}
        for role in f.ROLES:
            c.report_stage(progress, f"Сопоставимый STATIC: {role}", qm["counts"][role])
            if role in ("train", "dev"):
                hist = Counter()
                rows = matched_rows(
                    f.records(source / f"{role}.rdtune3"),
                    f.records(qcorpus / f"{role}.rdtune3"),
                    progress,
                    hist,
                )
                f.write(stage / f"{role}.rdtune3", role, qm["counts"][role], rows)
                classes[role] = dict(sorted(hist.items()))
            else:
                if sm["files"][f"{role}.rdtune3"] != qm["files"][f"{role}.rdtune3"]:
                    raise ValueError("protected role changed")
                shutil.copyfile(qcorpus / f"{role}.rdtune3", stage / f"{role}.rdtune3")
                classes[role] = sm["classes"][role]
                if progress:
                    progress.count(qm["counts"][role])
            shutil.copyfile(
                qcorpus / f"{role}.games.jsonl", stage / f"{role}.games.jsonl"
            )
        shutil.copyfile(qcorpus / "arena.rdopen", stage / "arena.rdopen")
        manifest = {
            k: qm[k]
            for k in (
                "format",
                "feature_schema",
                "seed",
                "max_per_game",
                "min_ply",
                "role_policy",
                "weighting",
                "final_status",
                "counts",
            )
        }
        manifest.update(
            mode="STATIC",
            sampling_policy="exact retained Q game_id/ply, same role/result/weight, original board/features",
            source=dict(
                static_manifest=source_sha,
                qresolved_manifest=q_sha,
                tool_sha256=f.sha(__file__),
                format_sha256=f.sha(f.__file__),
            ),
            classes=classes,
            stats=dict(
                matched_train=qm["counts"]["train"], matched_dev=qm["counts"]["dev"]
            ),
            limitations=[
                "matched subset is not the full STATIC corpus",
                "retrospective FINAL",
                "use a shared fixed logistic scale to isolate representation",
            ],
        )
        c.report_stage(progress, "Контрольные суммы сопоставимого STATIC")
        manifest["files"] = {
            p.name: f.sha(p) for p in sorted(stage.iterdir()) if p.is_file()
        }
        (stage / "manifest.json").write_text(
            json.dumps(manifest, sort_keys=True, indent=2) + "\n"
        )
        c.audit(stage, progress)
        os.rename(stage, destination)
    c.report_stage(progress, "Сопоставимый STATIC готов")
    return manifest


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--static", type=Path, required=True)
    p.add_argument("--qresolved", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    a = p.parse_args()
    with c.Progress() as progress:
        m = build(a.static, a.qresolved, a.out, progress)
    print(json.dumps(m["counts"], sort_keys=True))


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

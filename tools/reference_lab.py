#!/usr/bin/env python3
"""Frozen teacher/benchmark at fixed nodes with exact replayed game history.
Reports stability/agreement only: no unmeasured top-2, regret or exact best-move claims.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import os
import rdtune3 as fmt
from match import Engine, EngineSpec
from lab_common import Progress


def atomic_json(path, value):
    p = Path(path)
    tmp = p.with_suffix(p.suffix + ".tmp")
    tmp.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
    os.replace(tmp, p)


def choose(directory, limit, per_game):
    kept = {}
    for r in fmt.records(Path(directory) / "reference.rdtune3"):
        gid = r["game_id"]
        rank = hashlib.sha256(f"{gid}:{r['ply']}".encode()).hexdigest()
        item = dict(
            game_id=gid,
            ply=r["ply"],
            position=r["position"],
            phase=r["phase"],
            flags=r["flags"],
            sample_id=rank,
        )
        bucket = kept.setdefault(gid, [])
        bucket.append(item)
        bucket.sort(key=lambda x: x["sample_id"])
        del bucket[per_game:]
    return sorted((r for v in kept.values() for r in v), key=lambda x: x["sample_id"])[
        :limit
    ]


def restore(client, game, ply):
    w, b, k, side = game["root"]
    response = client.cmd(
        f"position {w:x} {b:x} {k:x} {'white' if side==0 else 'black'}"
    )
    if response != "ok":
        raise RuntimeError(response)
    for start, end, captures in game["moves"][:ply]:
        captured = ",".join(str(i + 1) for i in range(32) if captures & (1 << i)) or "-"
        response = client.cmd(f"play {start+1} {end+1} {captured}")
        if response != "ok":
            raise RuntimeError(response)


def search(client, game, sample, budget, depth):
    restore(
        client, game, sample["ply"]
    )  # cold TT at every budget and preserve counters via replay
    line, meta = client.go("nodes", budget, depth)
    words = line.split()
    if not words or words[0] != "bestmove":
        return dict(response=line, decision=None, usable=False, **meta)
    reason = words[words.index("stop") + 1] if "stop" in words else "missing"
    return dict(
        response=line,
        decision=words[1],
        usable=reason in ("node_limit", "completed", "proven_win_or_loss"),
        **meta,
    )


def stability(runs, tolerance):
    if len(runs) < 2 or not all(r["usable"] for r in runs):
        return False
    # Require all scheduled budgets, not just the last two. Still heuristic evidence.
    return (
        len({r["decision"] for r in runs}) == 1
        and max(r["score"] for r in runs) - min(r["score"] for r in runs) <= tolerance
        and all(r.get("depth", 0) > 0 for r in runs)
    )


def run(args):
    # Validate protocol before creating an immutable run directory.
    with Progress() as progress:
        progress.stage("Reference: проверка движка и корпуса")
        client = Engine(
            EngineSpec(
                Path(args.engine).resolve(),
                Path(args.eval).resolve(),
                Path(args.search).resolve(),
                args.tt,
                go_timeout_s=args.timeout,
            )
        )
        try:
            if not client.production_protocol:
                raise ValueError(
                    "Reference требует target/release/classic_engine (rd-classic/1)"
                )
            return _run(args, client, progress)
        finally:
            client.close()


def _run(args, client, progress):
    corpus = Path(args.corpus)
    fmt.read_manifest(corpus)
    teacher = None
    if args.cmd == "benchmark":
        tdir = Path(args.teacher)
        tm = json.loads((tdir / "manifest.json").read_text())
        if tm.get("status") != "complete" or tm.get("command") != "teacher":
            raise ValueError("teacher is not complete")
        if tm["results_sha256"] != fmt.sha(tdir / "results.jsonl"):
            raise ValueError("teacher checksum mismatch")
        if tm["identity"]["corpus_manifest"] != fmt.sha(corpus / "manifest.json"):
            raise ValueError("teacher corpus mismatch")
        teacher = {
            r["sample"]["sample_id"]: r
            for r in map(json.loads, (tdir / "results.jsonl").read_text().splitlines())
        }
        selected = [v["sample"] for v in teacher.values()]
    else:
        selected = choose(corpus, args.limit, args.per_game)
    if not selected:
        raise ValueError("empty reference selection")
    wanted = {s["game_id"] for s in selected}
    games = {}
    with (corpus / "reference.games.jsonl").open() as f:
        for line in f:
            g = json.loads(line)
            if g["game_id"] in wanted:
                games[g["game_id"]] = g
    if games.keys() != wanted:
        raise ValueError("missing game provenance")
    identity = dict(
        corpus_manifest=fmt.sha(corpus / "manifest.json"),
        engine=fmt.sha(args.engine),
        eval=fmt.sha(args.eval),
        search=fmt.sha(args.search),
        budgets=args.budgets,
        depth=args.depth,
        tt=args.tt,
        selection=hashlib.sha256(
            json.dumps(selected, sort_keys=True).encode()
        ).hexdigest(),
        teacher=fmt.sha(Path(args.teacher) / "manifest.json") if teacher else None,
        score_tolerance=args.score_tolerance,
        tool_sha256=fmt.sha(__file__),
        timeout=args.timeout,
        client_sha256=fmt.sha(Path(__file__).with_name("match.py")),
    )
    out = Path(args.out)
    # Immutable completed runs. Resume only an interrupted run with exact identity.
    if out.exists():
        old = json.loads((out / "manifest.json").read_text())
        if old["identity"] != identity or old["command"] != args.cmd:
            raise ValueError("resume identity mismatch")
        if old["status"] == "complete":
            raise ValueError("completed run is immutable")
    else:
        out.mkdir(parents=True)
        atomic_json(
            out / "manifest.json",
            dict(command=args.cmd, status="running", identity=identity),
        )
    results = out / "results.jsonl"
    existing = []
    if results.exists():
        # Only complete newline-terminated records survive a crash.
        raw = results.read_bytes()
        end = raw.rfind(b"\n") + 1
        if end != len(raw):
            with results.open("r+b") as f:
                f.truncate(end)
        existing = [json.loads(line) for line in raw[:end].splitlines()]
    if [r["sample"] for r in existing] != selected[: len(existing)]:
        raise ValueError("checkpoint order mismatch")
    progress.stage("Reference: обработка позиций", len(selected))
    progress.count(len(existing))
    try:
        if "protocol rd-classic/1" not in client.version:
            raise ValueError("production rd-classic/1 binary required")
        with results.open("a") as f:
            for sample in selected[len(existing) :]:
                runs = [
                    search(client, games[sample["game_id"]], sample, n, args.depth)
                    for n in args.budgets
                ]
                row = dict(
                    sample=sample,
                    runs=runs,
                    stable=stability(runs, args.score_tolerance),
                    acceptable_moves=None,
                    confidence="budget_stability_only",
                    tablebases=False,
                    book=False,
                )
                f.write(json.dumps(row, sort_keys=True) + "\n")
                f.flush()
                os.fsync(f.fileno())
                existing.append(row)
                progress.count(len(existing))
                print(
                    f'{len(existing)}/{len(selected)} stable={row["stable"]}',
                    flush=True,
                )
    finally:
        client.close()
    summary = dict(
        total=len(existing),
        stable=sum(r["stable"] for r in existing),
        move_stable=sum(
            all(x["usable"] for x in r["runs"])
            and len({x["decision"] for x in r["runs"]}) == 1
            for r in existing
        ),
    )
    if teacher:
        summary["agreement_by_budget"] = {}
        for i, n in enumerate(args.budgets):
            eligible = [
                r for r in existing if teacher[r["sample"]["sample_id"]]["stable"]
            ]
            usable = [r for r in eligible if r["runs"][i]["usable"]]
            hits = sum(
                r["runs"][i]["decision"]
                == teacher[r["sample"]["sample_id"]]["runs"][-1]["decision"]
                for r in usable
            )
            summary["agreement_by_budget"][str(n)] = dict(
                teacher_stable=len(eligible),
                usable=len(usable),
                hits=hits,
                agreement=hits / len(usable) if usable else None,
            )
    atomic_json(
        out / "manifest.json",
        dict(
            command=args.cmd,
            status="complete",
            identity=identity,
            results_sha256=fmt.sha(results),
            summary=summary,
            limitations=[
                "teacher is not an exact oracle",
                "top2/regret/PV stability unavailable",
            ],
        ),
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("teacher", "benchmark"):
        a = sub.add_parser(name)
        a.add_argument("--corpus", required=True)
        a.add_argument("--engine", required=True)
        a.add_argument("--eval", required=True)
        a.add_argument("--search", required=True)
        a.add_argument("--out", required=True)
        a.add_argument(
            "--budgets",
            type=int,
            nargs="+",
            default=(
                [250000, 1000000, 4000000]
                if name == "teacher"
                else [10000, 50000, 100000]
            ),
        )
        a.add_argument("--depth", type=int, default=64)
        a.add_argument("--tt", type=int, default=16)
        a.add_argument("--timeout", type=float, default=600)
        a.add_argument("--score-tolerance", type=int, default=20)
        if name == "teacher":
            a.add_argument("--limit", type=int, default=200)
            a.add_argument("--per-game", type=int, default=4)
        else:
            a.add_argument("--teacher", required=True)
    args = p.parse_args()
    if (
        args.budgets != sorted(set(args.budgets))
        or min(args.budgets) < 1
        or not 1 <= args.depth <= 126
        or args.tt < 1
        or args.timeout <= 0
        or args.score_tolerance < 0
    ):
        p.error("invalid budgets/depth/tt/timeout/tolerance")
    if args.cmd == "teacher" and (
        len(args.budgets) < 2 or args.limit < 1 or args.per_game < 1
    ):
        p.error("teacher needs >=2 budgets and positive sample limits")
    run(args)


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

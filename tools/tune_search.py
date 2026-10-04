#!/usr/bin/env python3
"""SPSA search-parameter tuner for russian_draughts_classic.

The engine owns the strict `search_v1` format.  This tool perturbs integer
parameters around a floating-point SPSA centre, plays color-reversed opening
pairs through the same production server used by GSPRT, then updates the centre.
It intentionally tunes search at fixed nodes by default; equal-time GSPRT is a
separate acceptance gate after tuning.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import tempfile
from dataclasses import dataclass
from pathlib import Path

import match as lab


SEARCH_KEYS = [
    "aspiration_window",
    "aspiration_min_depth",
    "max_extensions_per_line",
    "qsearch_promotion_depth",
    "forced_extension_min_depth",
    "promotion_extension_max_depth",
    "lmr_min_depth",
    "lmr_min_move",
    "lmr_second_depth",
    "lmr_second_move",
    "lmr_third_depth",
    "lmr_third_move",
    "lmr_base_reduction",
    "lmr_second_reduction",
    "lmr_third_reduction",
    "history_bonus_scale",
    "history_bonus_max",
    "history_malus_divisor",
    "enable_pvs",
    "enable_lmr",
]


@dataclass(frozen=True)
class TuneSpec:
    lo: int
    hi: int
    a: float
    c: float


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def parse_search(path: Path) -> dict[str, int | bool]:
    values: dict[str, int | bool] = {}
    fmt = None
    for line_no, raw in enumerate(path.read_text().splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if "=" not in line:
            raise ValueError(f"{path}:{line_no}: expected key=value")
        key, value = (part.strip() for part in line.split("=", 1))
        if key == "format":
            if fmt is not None:
                raise ValueError(f"{path}:{line_no}: duplicate format")
            fmt = value
            continue
        if key not in SEARCH_KEYS:
            raise ValueError(f"{path}:{line_no}: unknown search key {key}")
        if key in values:
            raise ValueError(f"{path}:{line_no}: duplicate search key {key}")
        if key in {"enable_pvs", "enable_lmr"}:
            if value not in {"true", "false"}:
                raise ValueError(f"{path}:{line_no}: expected true/false for {key}")
            values[key] = value == "true"
        else:
            values[key] = int(value)
    if fmt != "search_v1":
        raise ValueError(f"{path}: expected format=search_v1")
    missing = [key for key in SEARCH_KEYS if key not in values]
    if missing:
        raise ValueError(f"{path}: missing {', '.join(missing)}")
    return values


def render_search(values: dict[str, int | bool]) -> str:
    lines = ["format=search_v1"]
    for key in SEARCH_KEYS:
        value = values[key]
        if isinstance(value, bool):
            text = "true" if value else "false"
        else:
            text = str(int(value))
        lines.append(f"{key}={text}")
    return "\n".join(lines) + "\n"


def load_spsa(path: Path) -> tuple[dict[str, TuneSpec], float, float, float]:
    raw = json.loads(path.read_text())
    if raw.get("format") != "search_spsa_v1":
        raise ValueError(f"{path}: unsupported SPSA format")
    specs: dict[str, TuneSpec] = {}
    for name, item in raw.get("parameters", {}).items():
        if name not in SEARCH_KEYS or name in {"enable_pvs", "enable_lmr"}:
            raise ValueError(f"{path}: invalid tunable {name}")
        spec = TuneSpec(
            int(item["min"]), int(item["max"]), float(item["a"]), float(item["c"])
        )
        if spec.lo > spec.hi or spec.a <= 0.0 or spec.c <= 0.0:
            raise ValueError(f"{path}: invalid SPSA bounds/scales for {name}")
        specs[name] = spec
    if not specs:
        raise ValueError(f"{path}: no SPSA parameters")
    return (
        specs,
        float(raw.get("alpha", 0.602)),
        float(raw.get("gamma", 0.101)),
        float(raw.get("A", 20.0)),
    )


def project(
    values: dict[str, int | bool], specs: dict[str, TuneSpec]
) -> dict[str, int | bool]:
    out = dict(values)
    for name, spec in specs.items():
        out[name] = max(spec.lo, min(spec.hi, int(round(float(out[name])))))

    # SearchParams has monotone LMR threshold contracts.  Project into the
    # legal region deterministically so SPSA never wastes an iteration on a
    # candidate the engine would reject.
    out["lmr_second_depth"] = max(
        int(out["lmr_min_depth"]), int(out["lmr_second_depth"])
    )
    out["lmr_third_depth"] = max(
        int(out["lmr_second_depth"]), int(out["lmr_third_depth"])
    )
    out["lmr_second_move"] = max(int(out["lmr_min_move"]), int(out["lmr_second_move"]))
    out["lmr_third_move"] = max(int(out["lmr_second_move"]), int(out["lmr_third_move"]))
    return out


def candidate_from_theta(
    base: dict[str, int | bool],
    theta: dict[str, float],
    specs: dict[str, TuneSpec],
    delta: dict[str, int],
    iteration: int,
    gamma: float,
    sign: int,
    c_scale: float,
) -> tuple[dict[str, int | bool], dict[str, float]]:
    candidate = dict(base)
    ck: dict[str, float] = {}
    decay = (iteration + 1.0) ** gamma
    for name, spec in specs.items():
        step = max(1.0, spec.c * c_scale / decay)
        ck[name] = step
        raw = theta[name] + sign * step * delta[name]
        candidate[name] = int(round(max(spec.lo, min(spec.hi, raw))))
    return project(candidate, specs), ck


def run_iteration(
    engine: Path,
    eval_path: Path,
    plus_path: Path,
    minus_path: Path,
    openings: list[list[int]],
    opening_order: list[int],
    opening_offset: int,
    target_pairs: int,
    workers_count: int,
    tt: int,
    mode: str,
    limit: int,
    max_depth: int,
    max_plies: int,
    startup_timeout_s: float,
    command_timeout_s: float,
    go_timeout_s: float,
    shutdown_timeout_s: float,
) -> tuple[lab.MatchStats, int]:
    spec_kwargs = dict(
        startup_timeout_s=startup_timeout_s,
        command_timeout_s=command_timeout_s,
        go_timeout_s=go_timeout_s,
        shutdown_timeout_s=shutdown_timeout_s,
    )
    spec_plus = lab.EngineSpec(engine, eval_path, plus_path, tt, **spec_kwargs)
    spec_minus = lab.EngineSpec(engine, eval_path, minus_path, tt, **spec_kwargs)
    workers: list[lab.PairWorker] = []
    try:
        for _ in range(workers_count):
            workers.append(lab.PairWorker(spec_plus, spec_minus))
    except BaseException:
        for worker in workers:
            worker.close()
        raise
    stats = lab.MatchStats()
    attempted = 0
    next_commit = 0
    max_attempts = max(target_pairs * 2, target_pairs + workers_count)
    max_pending = max(workers_count * 4, workers_count)
    from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait

    pool = ThreadPoolExecutor(max_workers=workers_count)
    futures: dict[object, tuple[int, int]] = {}
    completed: dict[int, tuple[str, lab.GameTelemetry, str, lab.GameTelemetry]] = {}
    idle_workers = list(range(workers_count))
    workers_closed = False

    def submit_available() -> None:
        nonlocal attempted
        while (
            idle_workers
            and attempted < max_attempts
            and attempted < next_commit + max_pending
            and stats.pairs < target_pairs
        ):
            slot = idle_workers.pop()
            attempt_id = attempted
            attempted += 1
            idx = opening_order[(opening_offset + attempt_id) % len(opening_order)]
            opening = openings[idx]
            future = pool.submit(
                workers[slot].play_pair,
                opening,
                mode,
                limit,
                max_depth,
                max_plies,
                "abort",
            )
            futures[future] = (slot, attempt_id)

    try:
        submit_available()
        while stats.pairs < target_pairs and next_commit < max_attempts:
            if not futures:
                break
            done, _ = wait(tuple(futures), timeout=5, return_when=FIRST_COMPLETED)
            if not done:
                print(
                    f"SPSA: завершено {stats.pairs}/{target_pairs} пар; попыток {next_commit}/{max_attempts}",
                    flush=True,
                )
            for future in done:
                slot, attempt_id = futures.pop(future)
                completed[attempt_id] = future.result()
                idle_workers.append(slot)

            # Commit attempts in deterministic id order. Abort-heavy or fast
            # openings therefore cannot change SPSA statistics by finishing
            # earlier than slower openings.
            while next_commit in completed and stats.pairs < target_pairs:
                r0, t0, r1, t1 = completed.pop(next_commit)
                stats.add_game(r0, t0)
                stats.add_game(r1, t1)
                stats.add_pair(r0, r1)
                next_commit += 1

            submit_available()

        for future in futures:
            future.cancel()
        if stats.pairs < target_pairs:
            raise RuntimeError(
                f"only {stats.pairs}/{target_pairs} complete SPSA pairs after {next_commit} committed attempts; "
                f"aborted games={stats.aborted}"
            )
        # Reaching the requested complete-pair count can leave speculative
        # higher attempt ids running. Stop their child engines before waiting
        # for executor threads; those attempts are deliberately not committed.
        if futures:
            for worker in workers:
                worker.close()
            workers_closed = True
        pool.shutdown(wait=True, cancel_futures=True)
        return stats, next_commit
    except BaseException:
        for future in futures:
            future.cancel()
        for worker in workers:
            worker.close()
        workers_closed = True
        pool.shutdown(wait=True, cancel_futures=True)
        raise
    finally:
        if not workers_closed:
            for worker in workers:
                worker.close()


def save_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    tmp.replace(path)


def main() -> None:
    ap = argparse.ArgumentParser(
        description="SPSA tuner for versioned classic search parameters"
    )
    ap.add_argument("--engine", required=True, type=Path)
    ap.add_argument("--eval", required=True, type=Path)
    ap.add_argument("--base", required=True, type=Path)
    ap.add_argument("--spsa", type=Path, default=Path("config/search_spsa.json"))
    ap.add_argument("--openings", required=True, type=Path)
    ap.add_argument("--output", required=True, type=Path)
    ap.add_argument("--state", required=True, type=Path)
    ap.add_argument("--iterations", type=int, default=100)
    ap.add_argument("--pairs-per-iteration", type=int, default=16)
    group = ap.add_mutually_exclusive_group(required=True)
    group.add_argument("--nodes", type=int)
    group.add_argument("--time-ms", type=int)
    ap.add_argument("--workers", type=int, default=1)
    ap.add_argument("--startup-timeout-s", type=float, default=5.0)
    ap.add_argument("--command-timeout-s", type=float, default=5.0)
    ap.add_argument("--go-timeout-s", type=float, default=10.0)
    ap.add_argument("--shutdown-timeout-s", type=float, default=1.0)
    ap.add_argument("--tt", type=int, default=16)
    ap.add_argument("--max-depth", type=int, default=64)
    ap.add_argument("--max-plies", type=int, default=300)
    ap.add_argument("--seed", type=int, default=20260927)
    ap.add_argument("--a-scale", type=float, default=1.0)
    ap.add_argument("--c-scale", type=float, default=1.0)
    args = ap.parse_args()

    if (
        args.iterations < 1
        or args.pairs_per_iteration < 2
        or args.workers < 1
        or args.tt < 1
    ):
        ap.error("iterations/workers/tt must be positive and pairs-per-iteration >= 2")
    if args.nodes is not None and args.nodes < 1:
        ap.error("--nodes must be positive")
    if args.time_ms is not None and args.time_ms < 1:
        ap.error("--time-ms must be positive")
    if args.a_scale <= 0.0 or args.c_scale <= 0.0:
        ap.error("a-scale/c-scale must be positive")
    if (
        min(
            args.startup_timeout_s,
            args.command_timeout_s,
            args.go_timeout_s,
            args.shutdown_timeout_s,
        )
        <= 0.0
    ):
        ap.error("engine timeout values must be positive")

    base = parse_search(args.base)
    specs, alpha, gamma, A = load_spsa(args.spsa)
    for name, spec in specs.items():
        v = int(base[name])
        if not (spec.lo <= v <= spec.hi):
            ap.error(f"base {name}={v} outside SPSA [{spec.lo}, {spec.hi}]")

    openings = lab.load_openings(args.openings)
    if not openings:
        ap.error("opening suite is empty")
    opening_order = list(range(len(openings)))
    random.Random(args.seed).shuffle(opening_order)
    mode = "nodes" if args.nodes is not None else "time"
    limit = args.nodes if args.nodes is not None else args.time_ms
    assert limit is not None

    identity = {
        "format": 1,
        "engine": sha256(args.engine),
        "eval": sha256(args.eval),
        "base": sha256(args.base),
        "spsa": sha256(args.spsa),
        "openings": sha256(args.openings),
        "mode": mode,
        "limit": limit,
        "pairs_per_iteration": args.pairs_per_iteration,
        "workers": args.workers,
        "tt": args.tt,
        "max_depth": args.max_depth,
        "max_plies": args.max_plies,
        "seed": args.seed,
        "a_scale": args.a_scale,
        "c_scale": args.c_scale,
        "startup_timeout_s": args.startup_timeout_s,
        "command_timeout_s": args.command_timeout_s,
        "go_timeout_s": args.go_timeout_s,
        "shutdown_timeout_s": args.shutdown_timeout_s,
    }

    if args.state.exists():
        state = json.loads(args.state.read_text())
        if state.get("format") != 1 or state.get("experiment") != identity:
            ap.error("checkpoint belongs to a different SPSA experiment")
        start = int(state["iteration"])
        theta = {name: float(state["theta"][name]) for name in specs}
        print(f"resuming {args.state} at iteration {start + 1}")
    else:
        start = 0
        theta = {name: float(base[name]) for name in specs}

    print(
        f"search SPSA: params={len(specs)} iterations={args.iterations} pairs/iter={args.pairs_per_iteration}"
    )
    print(
        f"mode={mode} limit={limit} workers={args.workers} tt={args.tt}MB seed={args.seed}"
    )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.state.parent.mkdir(parents=True, exist_ok=True)

    for k in range(start, args.iterations):
        rng = random.Random(args.seed ^ ((k + 1) * 0x9E3779B1))
        delta = {name: (1 if rng.getrandbits(1) else -1) for name in specs}
        plus, _ = candidate_from_theta(
            base, theta, specs, delta, k, gamma, +1, args.c_scale
        )
        minus, _ = candidate_from_theta(
            base, theta, specs, delta, k, gamma, -1, args.c_scale
        )
        if plus == minus:
            raise RuntimeError(
                "SPSA perturbation collapsed after integer projection; increase c-scale"
            )

        with tempfile.TemporaryDirectory(prefix="rd-spsa-") as td:
            td = Path(td)
            plus_path = td / "plus.search"
            minus_path = td / "minus.search"
            plus_path.write_text(render_search(plus))
            minus_path.write_text(render_search(minus))
            stats, attempts = run_iteration(
                args.engine,
                args.eval,
                plus_path,
                minus_path,
                openings,
                opening_order,
                k * args.pairs_per_iteration,
                args.pairs_per_iteration,
                args.workers,
                args.tt,
                mode,
                limit,
                args.max_depth,
                args.max_plies,
                args.startup_timeout_s,
                args.command_timeout_s,
                args.go_timeout_s,
                args.shutdown_timeout_s,
            )

        # Candidate A is theta+c*delta and candidate B is theta-c*delta.  The
        # paired score therefore directly supplies the two-sided SPSA signal.
        signal = 2.0 * (stats.score - 0.5)
        ak_decay = (A + k + 1.0) ** alpha
        for name, spec in specs.items():
            ak = spec.a * args.a_scale / ak_decay
            # Use the *effective* integer perturbation after clamping and LMR
            # constraint projection. Near a bound (or after threshold ordering)
            # it can be smaller than the requested c_k; using requested c_k
            # would bias the gradient. If projection collapsed this coordinate,
            # that coordinate contributes no information in this iteration.
            span = float(plus[name]) - float(minus[name])
            if abs(span) < 0.5:
                continue
            effective_c = abs(span) * 0.5
            orientation = 1.0 if span > 0.0 else -1.0
            grad = signal * orientation / effective_c
            theta[name] = max(spec.lo, min(spec.hi, theta[name] + ak * grad))

        current = dict(base)
        for name in specs:
            current[name] = int(round(theta[name]))
        current = project(current, specs)
        args.output.write_text(render_search(current))
        state_payload = {
            "format": 1,
            "experiment": identity,
            "iteration": k + 1,
            "theta": theta,
            "current": current,
            "last": {
                "score_plus": stats.score,
                "pentanomial": stats.pentanomial,
                "complete_pairs": stats.pairs,
                "attempted_pairs": attempts,
                "aborted_games": stats.aborted,
                "signal": signal,
                "plus": plus,
                "minus": minus,
            },
        }
        save_json(args.state, state_payload)
        print(
            f"iter {k + 1:4d}: score(+/-)={stats.score:.5f} signal={signal:+.5f} "
            f"P={stats.pentanomial} complete={stats.pairs}/{attempts} aborted={stats.aborted}"
        )

    print(f"output: {args.output}")
    print(f"state : {args.state}")


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

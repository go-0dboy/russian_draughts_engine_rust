#!/usr/bin/env python3
"""Paired engine laboratory for russian_draughts_classic.

Uses the RDOPEN2 suite generated from real PDN games, reverses colors for every
opening, reports pentanomial results and can stop early with a score-constrained
pentanomial GSPRT.  Only Python's standard library is required.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import random
import selectors
import struct
import subprocess
import tempfile
import time
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass, asdict
from pathlib import Path


@dataclass(frozen=True)
class EngineSpec:
    path: Path
    eval_path: Path | None
    search_path: Path | None
    tt_mb: int
    startup_timeout_s: float = 5.0
    command_timeout_s: float = 5.0
    go_timeout_s: float = 10.0
    shutdown_timeout_s: float = 1.0
    kill_timeout_s: float = 1.0


class Engine:
    def __init__(self, spec: EngineSpec):
        command = [str(spec.path), "--tt", str(spec.tt_mb)]
        if spec.eval_path is not None:
            command += ["--eval", str(spec.eval_path)]
        if spec.search_path is not None:
            command += ["--search", str(spec.search_path)]
        self.spec = spec
        self._stdout_buffer = bytearray()
        self._selector = selectors.DefaultSelector()
        self._stderr_file = tempfile.TemporaryFile(mode="w+b")
        self.p = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr_file,
            bufsize=0,
        )
        assert self.p.stdout is not None
        self._selector.register(self.p.stdout, selectors.EVENT_READ)
        try:
            ready = self._read(spec.startup_timeout_s)
            if not ready.startswith("ready "):
                raise RuntimeError(f"{spec.path}: unexpected greeting: {ready!r}")
            self.version = ready.split(maxsplit=1)[1]
            self.production_protocol = "protocol rd-classic/1" in self.version
        except BaseException:
            self.close()
            raise

    def _stderr_tail(self, limit: int = 16_384) -> str:
        if self.p.poll() is None:
            return ""
        try:
            self._stderr_file.flush()
            self._stderr_file.seek(0, os.SEEK_END)
            size = self._stderr_file.tell()
            self._stderr_file.seek(max(0, size - limit))
            return self._stderr_file.read().decode("utf-8", errors="replace").strip()
        except Exception:
            return ""

    def _read(self, timeout_s: float) -> str:
        deadline = time.monotonic() + timeout_s
        assert self.p.stdout is not None
        fd = self.p.stdout.fileno()
        while True:
            newline = self._stdout_buffer.find(b"\n")
            if newline >= 0:
                raw = bytes(self._stdout_buffer[:newline])
                del self._stdout_buffer[: newline + 1]
                return raw.rstrip(b"\r").decode("utf-8", errors="replace")

            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{self.spec.path}: timed out waiting for protocol response after {timeout_s:.3f}s"
                )
            events = self._selector.select(remaining)
            if not events:
                raise TimeoutError(
                    f"{self.spec.path}: timed out waiting for protocol response after {timeout_s:.3f}s"
                )
            try:
                chunk = os.read(fd, 4096)
            except OSError as exc:
                raise RuntimeError(
                    f"{self.spec.path}: cannot read server response: {exc}"
                ) from exc
            if not chunk:
                rc = self.p.poll()
                err = self._stderr_tail() if rc is not None else ""
                suffix = f"; stderr: {err}" if err else ""
                raise RuntimeError(
                    f"{self.spec.path}: server stopped (rc={rc}){suffix}"
                )
            self._stdout_buffer.extend(chunk)

    def _send(self, command: str) -> None:
        if self.p.poll() is not None:
            err = self._stderr_tail()
            suffix = f"; stderr: {err}" if err else ""
            raise RuntimeError(f"{self.spec.path}: server already stopped{suffix}")
        assert self.p.stdin is not None
        try:
            self.p.stdin.write((command + "\n").encode("utf-8"))
            self.p.stdin.flush()
        except (BrokenPipeError, OSError) as exc:
            raise RuntimeError(
                f"{self.spec.path}: cannot send {command!r}: {exc}"
            ) from exc

    def cmd(self, command: str, timeout_s: float | None = None) -> str:
        self._send(command)
        return self._read(
            self.spec.command_timeout_s if timeout_s is None else timeout_s
        )

    def new(self) -> None:
        answer = self.cmd("new")
        if answer != "ok":
            raise RuntimeError(f"new: {answer}")

    @staticmethod
    def move_spec(raw: int) -> str:
        if not 0 <= raw < 1 << 64:
            raise ValueError("packed move must fit u64")
        captures = ",".join(str(i + 1) for i in range(32) if raw & (1 << i)) or "-"
        return f"{((raw >> 37) & 31) + 1}:{((raw >> 32) & 31) + 1}:{captures}"

    def play(self, move: int | str) -> None:
        if self.production_protocol:
            spec = self.move_spec(move) if isinstance(move, int) else move
            command = "play " + spec.replace(":", " ")
        else:
            if isinstance(move, str):
                # Resolve through the canonical generator: metadata bits must
                # never be guessed when crossing production/legacy protocols.
                legal = self.cmd("legal").split()
                if len(legal) < 2 or legal[0] != "legal":
                    raise RuntimeError(f"bad legal response: {legal}")
                matches = [
                    int(raw, 16)
                    for raw in legal[2:]
                    if self.move_spec(int(raw, 16)) == move
                ]
                if not matches:
                    raise RuntimeError(f"move not found in legacy legal list: {move}")
                move = matches[0]
            command = f"play {move:016x}"
        answer = self.cmd(command)
        if answer != "ok":
            raise RuntimeError(f"{command}: {answer}")

    def side(self) -> str:
        answer = self.cmd("side").split()
        if len(answer) != 2 or answer[0] != "side":
            raise RuntimeError(f"bad side response: {answer}")
        return answer[1]

    def status(self) -> str:
        return self.cmd("status")

    def go(self, mode: str, limit: int, max_depth: int) -> tuple[str, dict[str, int]]:
        timeout = self.spec.go_timeout_s
        if mode == "time":
            timeout = max(timeout, limit / 1000.0 * 2.0 + 2.0)
        line = self.cmd(f"go {mode} {limit} {max_depth}", timeout_s=timeout)
        parts = line.split()
        meta: dict[str, int] = {}
        keys = {"score", "depth", "nodes", "time_us"}
        for i, key in enumerate(parts[:-1]):
            if key in keys:
                try:
                    meta[key] = int(parts[i + 1])
                except ValueError:
                    pass
        return line, meta

    def close(self) -> None:
        p = getattr(self, "p", None)
        if p is None:
            return
        if p.poll() is None:
            try:
                self._send("quit")
            except Exception:
                pass
            try:
                p.wait(timeout=self.spec.shutdown_timeout_s)
            except subprocess.TimeoutExpired:
                try:
                    p.terminate()
                except OSError:
                    pass
                try:
                    p.wait(timeout=self.spec.shutdown_timeout_s)
                except subprocess.TimeoutExpired:
                    try:
                        p.kill()
                    except OSError:
                        pass
                    try:
                        p.wait(timeout=self.spec.kill_timeout_s)
                    except subprocess.TimeoutExpired:
                        pass
        for stream_name in ("stdin", "stdout"):
            stream = getattr(p, stream_name, None)
            if stream is not None:
                try:
                    stream.close()
                except Exception:
                    pass
        selector = getattr(self, "_selector", None)
        if selector is not None:
            try:
                selector.close()
            except Exception:
                pass
        stderr_file = getattr(self, "_stderr_file", None)
        if stderr_file is not None:
            try:
                stderr_file.close()
            except Exception:
                pass


@dataclass
class GameTelemetry:
    nodes_a: int = 0
    nodes_b: int = 0
    moves_a: int = 0
    moves_b: int = 0


@dataclass
class MatchStats:
    a_wins: int = 0
    draws: int = 0
    b_wins: int = 0
    aborted: int = 0
    adjudicated_draws: int = 0
    pentanomial: list[int] | None = None
    pairs: int = 0
    nodes_a: int = 0
    nodes_b: int = 0
    moves_a: int = 0
    moves_b: int = 0

    def __post_init__(self) -> None:
        if self.pentanomial is None:
            self.pentanomial = [0, 0, 0, 0, 0]

    @property
    def completed_games(self) -> int:
        return self.a_wins + self.draws + self.b_wins

    @property
    def score(self) -> float:
        # The statistical unit is a complete color-reversed pair. Exclude any
        # half of an aborted pair from the reported candidate score as well as
        # from the pentanomial SPRT.
        if not self.pairs or self.pentanomial is None:
            return 0.0
        pair_points_x2 = sum(
            index * count for index, count in enumerate(self.pentanomial)
        )
        return pair_points_x2 / (4.0 * self.pairs)

    def add_game(self, result: str, t: GameTelemetry) -> None:
        if result == "A":
            self.a_wins += 1
        elif result == "B":
            self.b_wins += 1
        elif result == "D":
            self.draws += 1
        elif result == "J":
            self.draws += 1
            self.adjudicated_draws += 1
        else:
            self.aborted += 1
        self.nodes_a += t.nodes_a
        self.nodes_b += t.nodes_b
        self.moves_a += t.moves_a
        self.moves_b += t.moves_b

    def add_pair(self, first: str, second: str) -> None:
        if "X" in (first, second):
            return
        points = score_for_a(first) + score_for_a(second)
        index = int(round(points * 2.0))
        assert 0 <= index <= 4
        assert self.pentanomial is not None
        self.pentanomial[index] += 1
        self.pairs += 1


def score_for_a(result: str) -> float:
    return {"A": 1.0, "D": 0.5, "J": 0.5, "B": 0.0}[result]


def load_openings(path: Path) -> list[list[int]]:
    with path.open("rb") as f:
        if f.read(8) != b"RDOPEN2\0":
            raise ValueError(f"{path}: not RDOPEN2")
        version, reserved = struct.unpack("<II", f.read(8))
        if version != 1:
            raise ValueError(f"{path}: unsupported version {version}")
        if reserved != 0:
            raise ValueError(f"{path}: unsupported header flags {reserved}")
        (count,) = struct.unpack("<Q", f.read(8))
        if count > 10_000_000:
            raise ValueError("unreasonable opening count")
        out: list[list[int]] = []
        for _ in range(count):
            length, flags = struct.unpack("<HH", f.read(4))
            if flags != 0:
                raise ValueError(f"{path}: unsupported opening flags {flags}")
            data = f.read(length * 8)
            if len(data) != length * 8:
                raise ValueError("truncated opening file")
            out.append(list(struct.unpack(f"<{length}Q", data)) if length else [])
        if f.read(1):
            raise ValueError("trailing bytes in opening file")
        return out


def reset_and_replay(a: Engine, b: Engine, opening: list[int]) -> None:
    a.new()
    b.new()
    for mv in opening:
        a.play(mv)
        b.play(mv)


def terminal_result(status: str, a_color: str, adjudication: str) -> str | None:
    if status.startswith("status white_win"):
        return "A" if a_color == "white" else "B"
    if status.startswith("status black_win"):
        return "A" if a_color == "black" else "B"
    if status.startswith("status draw"):
        return "D"
    if status.startswith("status adjudication"):
        return "J" if adjudication == "draw" else "X"
    return None


def play_game(
    a: Engine,
    b: Engine,
    opening: list[int],
    a_color: str,
    mode: str,
    limit: int,
    max_depth: int,
    max_plies: int,
    adjudication: str,
) -> tuple[str, GameTelemetry]:
    reset_and_replay(a, b, opening)
    telemetry = GameTelemetry()
    for _ply in range(max_plies):
        sa = a.status()
        sb = b.status()
        if sa != sb:
            raise RuntimeError(f"rule-state divergence: A={sa!r} B={sb!r}")
        terminal = terminal_result(sa, a_color, adjudication)
        if terminal is not None:
            return terminal, telemetry

        side_a = a.side()
        side_b = b.side()
        if side_a != side_b:
            raise RuntimeError(f"side divergence: A={side_a} B={side_b}")
        actor_is_a = side_a == a_color
        actor = a if actor_is_a else b
        reply, meta = actor.go(mode, limit, max_depth)
        nodes = meta.get("nodes", 0)
        if actor_is_a:
            telemetry.nodes_a += nodes
            telemetry.moves_a += 1
        else:
            telemetry.nodes_b += nodes
            telemetry.moves_b += 1

        parts = reply.split()
        if not parts:
            return "X", telemetry
        if parts[0] in {"claim", "claim_move"}:
            return "D", telemetry
        if parts[0] == "adjudication":
            return ("J" if adjudication == "draw" else "X"), telemetry
        if parts[0] == "status":
            return "X", telemetry
        if parts[0] not in {"move", "bestmove"} or len(parts) < 2:
            raise RuntimeError(f"unexpected search response: {reply}")
        mv = parts[1] if parts[0] == "bestmove" else int(parts[1], 16)

        a.play(mv)
        b.play(mv)
    return "X", telemetry


class PairWorker:
    def __init__(self, spec_a: EngineSpec, spec_b: EngineSpec):
        self.a = Engine(spec_a)
        try:
            self.b = Engine(spec_b)
        except BaseException:
            self.a.close()
            raise

    @property
    def versions(self) -> tuple[str, str]:
        return self.a.version, self.b.version

    def play_pair(
        self,
        opening: list[int],
        mode: str,
        limit: int,
        max_depth: int,
        max_plies: int,
        adjudication: str,
    ) -> tuple[str, GameTelemetry, str, GameTelemetry]:
        first, t0 = play_game(
            self.a,
            self.b,
            opening,
            "white",
            mode,
            limit,
            max_depth,
            max_plies,
            adjudication,
        )
        second, t1 = play_game(
            self.a,
            self.b,
            opening,
            "black",
            mode,
            limit,
            max_depth,
            max_plies,
            adjudication,
        )
        return first, t0, second, t1

    def close(self) -> None:
        self.a.close()
        self.b.close()


def elo_to_score(elo: float) -> float:
    return 1.0 / (1.0 + 10.0 ** (-elo / 400.0))


def constrained_multinomial_ll(counts: list[int], mean: float) -> float:
    """Maximum multinomial log likelihood under E[pair_score_per_game]=mean.

    Pair categories have candidate average game scores [0,.25,.5,.75,1].
    The rational Lagrange solution is solved by one-dimensional bisection.
    Tiny pseudo-counts only stabilize categories unseen early in a run; the
    returned likelihood itself uses the real counts.
    """
    xs = [0.0, 0.25, 0.5, 0.75, 1.0]
    n = sum(counts)
    if n == 0:
        return 0.0
    eps = 1e-9
    denom_n = n + 5.0 * eps
    q = [(c + eps) / denom_n for c in counts]
    d = [x - mean for x in xs]

    lo = -math.inf
    hi = math.inf
    for di in d:
        if di > 0:
            lo = max(lo, -1.0 / di)
        elif di < 0:
            hi = min(hi, -1.0 / di)
    margin = 1e-12
    lo = (lo + margin) if math.isfinite(lo) else -1e12
    hi = (hi - margin) if math.isfinite(hi) else 1e12

    def f(b: float) -> float:
        return sum(qi * di / (1.0 + b * di) for qi, di in zip(q, d))

    # f is strictly decreasing on the feasible interval.
    for _ in range(100):
        mid = (lo + hi) * 0.5
        if f(mid) > 0.0:
            lo = mid
        else:
            hi = mid
    b = (lo + hi) * 0.5
    probs = [qi / (1.0 + b * di) for qi, di in zip(q, d)]
    total = sum(probs)
    probs = [max(p / total, 1e-300) for p in probs]
    return sum(c * math.log(p) for c, p in zip(counts, probs) if c)


def pentanomial_llr(counts: list[int], elo0: float, elo1: float) -> float:
    return constrained_multinomial_ll(
        counts, elo_to_score(elo1)
    ) - constrained_multinomial_ll(counts, elo_to_score(elo0))


def sprt_state(
    counts: list[int], elo0: float, elo1: float, alpha: float, beta: float
) -> tuple[str, float, float, float]:
    llr = pentanomial_llr(counts, elo0, elo1)
    lower = math.log(beta / (1.0 - alpha))
    upper = math.log((1.0 - beta) / alpha)
    if llr <= lower:
        state = "rejected"
    elif llr >= upper:
        state = "accepted"
    else:
        state = "continue"
    return state, llr, lower, upper


def file_sha256(path: Path | None) -> str | None:
    if path is None:
        return None
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def experiment_identity(
    args: argparse.Namespace, mode: str, limit: int
) -> dict[str, object]:
    return {
        "format": 1,
        "engine_a_sha256": file_sha256(args.engine_a),
        "engine_b_sha256": file_sha256(args.engine_b),
        "engine_a_eval_sha256": file_sha256(args.engine_a_eval),
        "engine_b_eval_sha256": file_sha256(args.engine_b_eval),
        "engine_a_search_sha256": file_sha256(args.engine_a_search),
        "engine_b_search_sha256": file_sha256(args.engine_b_search),
        "openings_sha256": file_sha256(args.openings),
        "mode": mode,
        "limit": limit,
        "max_depth": args.max_depth,
        "max_plies": args.max_plies,
        "tt": args.tt,
        "seed": args.seed,
        "elo0": args.elo0,
        "elo1": args.elo1,
        "alpha": args.alpha,
        "beta": args.beta,
        "min_pairs": args.min_pairs,
        "workers": args.workers,
        "adjudication": args.adjudication,
        "startup_timeout_s": args.startup_timeout_s,
        "command_timeout_s": args.command_timeout_s,
        "go_timeout_s": args.go_timeout_s,
        "shutdown_timeout_s": args.shutdown_timeout_s,
    }


def save_state(
    path: Path | None,
    stats: MatchStats,
    next_pair: int,
    identity: dict[str, object],
) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "format": 1,
        "experiment": identity,
        "stats": asdict(stats),
        "next_pair": next_pair,
    }
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    tmp.replace(path)


def load_state(path: Path, identity: dict[str, object]) -> tuple[MatchStats, int]:
    data = json.loads(path.read_text())
    if data.get("format") != 1:
        raise ValueError("unsupported checkpoint format")
    if data.get("experiment") != identity:
        raise ValueError("checkpoint belongs to a different experiment")
    stats = MatchStats(**data["stats"])
    return stats, int(data["next_pair"])


def main() -> None:
    ap = argparse.ArgumentParser(
        description="paired Russian draughts engine match with pentanomial GSPRT"
    )
    ap.add_argument("--engine-a", required=True, type=Path)
    ap.add_argument("--engine-b", required=True, type=Path)
    ap.add_argument("--engine-a-eval", type=Path)
    ap.add_argument("--engine-b-eval", type=Path)
    ap.add_argument("--engine-a-search", type=Path)
    ap.add_argument("--engine-b-search", type=Path)
    ap.add_argument("--openings", required=True, type=Path)
    ap.add_argument(
        "--pairs", type=int, default=1000, help="hard maximum; SPRT may stop earlier"
    )
    group = ap.add_mutually_exclusive_group(required=True)
    group.add_argument("--nodes", type=int)
    group.add_argument("--time-ms", type=int)
    ap.add_argument("--max-depth", type=int, default=64)
    ap.add_argument("--max-plies", type=int, default=300)
    ap.add_argument("--tt", type=int, default=16)
    ap.add_argument("--seed", type=int, default=20260925)
    ap.add_argument("--elo0", type=float, default=0.0)
    ap.add_argument("--elo1", type=float, default=5.0)
    ap.add_argument("--alpha", type=float, default=0.05)
    ap.add_argument("--beta", type=float, default=0.05)
    ap.add_argument("--min-pairs", type=int, default=20)
    ap.add_argument(
        "--workers",
        type=int,
        default=1,
        help="parallel opening pairs; two engines per worker",
    )
    ap.add_argument(
        "--startup-timeout-s", type=float, default=5.0, help="engine greeting timeout"
    )
    ap.add_argument(
        "--command-timeout-s",
        type=float,
        default=5.0,
        help="timeout for non-search protocol commands",
    )
    ap.add_argument(
        "--go-timeout-s",
        type=float,
        default=10.0,
        help="minimum timeout for one search response",
    )
    ap.add_argument(
        "--shutdown-timeout-s",
        type=float,
        default=1.0,
        help="grace period before terminate/kill",
    )
    ap.add_argument(
        "--adjudication",
        choices=("abort", "draw"),
        default="abort",
        help="policy for states requiring external endgame adjudication; default abort excludes the whole pair from SPRT, while draw is an explicit development approximation",
    )
    ap.add_argument(
        "--state",
        type=Path,
        help="checkpoint JSON; resumes only if the experiment fingerprint matches",
    )
    args = ap.parse_args()

    if (
        args.max_plies < 1
        or args.max_depth > 126
        or args.pairs < 1
        or args.max_depth < 1
        or args.tt < 1
        or args.workers < 1
        or args.min_pairs < 1
    ):
        ap.error("pairs/depth/tt/workers/min-pairs must be positive")
    if args.min_pairs > args.pairs:
        ap.error("--min-pairs cannot exceed --pairs")
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
    if args.nodes is not None and args.nodes < 1:
        ap.error("--nodes must be positive")
    if args.time_ms is not None and args.time_ms < 1:
        ap.error("--time-ms must be positive")
    if not (0.0 < args.alpha < 0.5 and 0.0 < args.beta < 0.5):
        ap.error("alpha/beta must be in (0, 0.5)")
    if not all(
        math.isfinite(x)
        for x in (
            args.elo0,
            args.elo1,
            args.startup_timeout_s,
            args.command_timeout_s,
            args.go_timeout_s,
            args.shutdown_timeout_s,
        )
    ):
        ap.error("Elo and timeouts must be finite")
    if args.elo1 <= args.elo0:
        ap.error("elo1 must be greater than elo0")

    openings = load_openings(args.openings)
    if not openings:
        ap.error("opening suite is empty")
    order = list(range(len(openings)))
    random.Random(args.seed).shuffle(order)

    mode = "nodes" if args.nodes is not None else "time"
    limit = args.nodes if args.nodes is not None else args.time_ms
    assert limit is not None
    identity = experiment_identity(args, mode, limit)

    if args.state and args.state.exists():
        try:
            stats, start_pair = load_state(args.state, identity)
        except ValueError as exc:
            ap.error(str(exc))
        print(f"resuming {args.state} from pair {start_pair}")
    else:
        stats = MatchStats()
        start_pair = 0

    if start_pair >= args.pairs:
        ap.error("checkpoint is already at or beyond --pairs")

    spec_kwargs = dict(
        startup_timeout_s=args.startup_timeout_s,
        command_timeout_s=args.command_timeout_s,
        go_timeout_s=args.go_timeout_s,
        shutdown_timeout_s=args.shutdown_timeout_s,
    )
    spec_a = EngineSpec(
        args.engine_a, args.engine_a_eval, args.engine_a_search, args.tt, **spec_kwargs
    )
    spec_b = EngineSpec(
        args.engine_b, args.engine_b_eval, args.engine_b_search, args.tt, **spec_kwargs
    )
    worker_count = min(args.workers, args.pairs - start_pair)
    workers: list[PairWorker] = []
    try:
        for _ in range(worker_count):
            workers.append(PairWorker(spec_a, spec_b))
    except BaseException:
        for worker in workers:
            worker.close()
        raise
    versions = workers[0].versions
    if any(worker.versions != versions for worker in workers[1:]):
        for worker in workers:
            worker.close()
        raise RuntimeError("engine version differs between workers")

    print(f"A: {versions[0]} {args.engine_a}")
    print(f"B: {versions[1]} {args.engine_b}")
    print(
        f"openings={len(openings)} pairs<= {args.pairs} {mode}={limit} "
        f"tt={args.tt}MB workers={worker_count} seed={args.seed}"
    )
    print(
        f"SPRT elo0={args.elo0:+.2f} elo1={args.elo1:+.2f} alpha={args.alpha} beta={args.beta}"
    )

    final_state = "continue"
    next_commit = start_pair
    next_schedule = start_pair
    max_pending = max(worker_count * 4, worker_count)
    pool = ThreadPoolExecutor(max_workers=worker_count)
    futures: dict[object, tuple[int, int]] = {}
    completed: dict[int, tuple[str, GameTelemetry, str, GameTelemetry]] = {}
    idle_workers = list(range(worker_count))
    workers_closed = False

    def submit_available() -> None:
        nonlocal next_schedule
        while (
            idle_workers
            and next_schedule < args.pairs
            and next_schedule < next_commit + max_pending
            and final_state == "continue"
        ):
            slot = idle_workers.pop()
            pair = next_schedule
            next_schedule += 1
            opening = openings[order[pair % len(order)]]
            future = pool.submit(
                workers[slot].play_pair,
                opening,
                mode,
                limit,
                args.max_depth,
                args.max_plies,
                args.adjudication,
            )
            futures[future] = (slot, pair)

    try:
        submit_available()
        while next_commit < args.pairs and final_state == "continue":
            if not futures:
                raise RuntimeError("dynamic scheduler has no runnable pair")

            done, _ = wait(tuple(futures), timeout=5, return_when=FIRST_COMPLETED)
            if not done:
                print(
                    f"Ожидание: завершено {next_commit}/{args.pairs} пар; "
                    f"в работе {len(futures)}, ожидают порядка {len(completed)}",
                    flush=True,
                )
            for future in done:
                slot, pair = futures.pop(future)
                completed[pair] = future.result()
                idle_workers.append(slot)

            # Statistical state, logs and checkpoints are committed strictly by
            # pair id even though execution is dynamically scheduled. Fast pairs
            # therefore cannot bias an early SPRT decision.
            while next_commit in completed and final_state == "continue":
                r0, t0, r1, t1 = completed.pop(next_commit)
                stats.add_game(r0, t0)
                stats.add_game(r1, t1)
                stats.add_pair(r0, r1)
                state, llr, lower, upper = sprt_state(
                    stats.pentanomial or [0] * 5,
                    args.elo0,
                    args.elo1,
                    args.alpha,
                    args.beta,
                )
                print(
                    f"pair {next_commit + 1:5d}: {r0}/{r1}  "
                    f"A/D/B/X={stats.a_wins}/{stats.draws}/{stats.b_wins}/{stats.aborted} "
                    f"adjD={stats.adjudicated_draws} "
                    f"P={stats.pentanomial} score={stats.score:.4f} "
                    f"LLR={llr:+.3f} [{lower:+.3f},{upper:+.3f}] {state}",
                    flush=True,
                )
                next_commit += 1
                save_state(args.state, stats, next_commit, identity)
                if stats.pairs >= args.min_pairs and state != "continue":
                    final_state = state

            submit_available()

        # Do not include already-running higher pair ids after an SPRT boundary.
        # Python cannot cancel a Future that is already executing, so close the
        # child engines before waiting for executor threads. This keeps early
        # SPRT termination bounded even when a speculative pair is very long.
        for future in futures:
            future.cancel()
        if futures:
            for worker in workers:
                worker.close()
            workers_closed = True
    except BaseException:
        for future in futures:
            future.cancel()
        # Kill/terminate child engines before waiting for worker threads. This
        # makes Ctrl-C and protocol failures bounded even if a child stopped
        # answering while a worker was inside a command.
        for worker in workers:
            worker.close()
        workers_closed = True
        pool.shutdown(wait=True, cancel_futures=True)
        raise
    else:
        pool.shutdown(wait=True, cancel_futures=True)
    finally:
        if not workers_closed:
            for worker in workers:
                worker.close()

    state, llr, lower, upper = sprt_state(
        stats.pentanomial or [0] * 5,
        args.elo0,
        args.elo1,
        args.alpha,
        args.beta,
    )
    if final_state == "continue":
        final_state = state if stats.pairs >= args.min_pairs else "continue"
    print("\nfinal")
    print(
        f"A wins / draws / B wins / aborted: {stats.a_wins} / {stats.draws} / {stats.b_wins} / {stats.aborted}"
    )
    print(f"adjudicated draws: {stats.adjudicated_draws} (policy={args.adjudication})")
    print(f"pentanomial: {stats.pentanomial}")
    print(f"A score    : {stats.score:.6f}")
    print(f"LLR        : {llr:+.6f} bounds [{lower:+.6f}, {upper:+.6f}]")
    print(f"SPRT       : {final_state}")
    if stats.moves_a:
        print(f"A avg nodes/move: {stats.nodes_a / stats.moves_a:.1f}")
    if stats.moves_b:
        print(f"B avg nodes/move: {stats.nodes_b / stats.moves_b:.1f}")


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

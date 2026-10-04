#!/usr/bin/env python3
"""Derive a capture-qresolved corpus from a frozen STATIC Corpus V2."""
import argparse
from collections import Counter
import itertools
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import corpus_v2 as c
import rdtune3 as f


def derive(
    source,
    destination,
    binary,
    evaluation,
    node_limit=10000,
    max_games=0,
    progress=None,
):
    source = Path(source)
    destination = Path(destination)
    if destination.exists():
        raise FileExistsError("choose a new corpus directory")
    c.report_stage(progress, "QRESOLVED: проверка STATIC и замороженной оценки")
    manifest = f.read_manifest(source)
    if manifest["mode"] != "STATIC":
        raise ValueError("requires original STATIC corpus")
    identity = dict(
        parent_manifest=f.sha(source / "manifest.json"),
        eval_sha256=f.sha(evaluation),
        resolver_sha256=f.sha(binary),
        tool_sha256=f.sha(__file__),
        builder_sha256=f.sha(c.__file__),
        format_sha256=f.sha(f.__file__),
        node_limit=node_limit,
        max_games=max_games,
        qsearch="capture-only-full-minimax-v1",
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".qresolved-", dir=destination.parent
    ) as tmp:
        stage = Path(tmp)
        output = stage / "corpus"
        stats = Counter()
        db = sqlite3.connect(stage / "protected.sqlite")
        db.execute("PRAGMA journal_mode=OFF")
        db.execute("PRAGMA synchronous=OFF")
        db.execute("PRAGMA cache_size=-32768")
        db.execute("PRAGMA temp_store=FILE")
        db.execute(
            "CREATE TABLE original(pos BLOB PRIMARY KEY,role TEXT) WITHOUT ROWID"
        )
        try:
            # Also protect original DEV boards which might disappear on resolution.
            for role in f.ROLES:
                c.report_stage(
                    progress,
                    f"Индекс исходных позиций: {role}",
                    manifest["counts"][role],
                )
                for n, r in enumerate(
                    f.records(source / f"{role}.rdtune3", allow_final=True), 1
                ):
                    key = c.position_key(r["position"])
                    old = db.execute(
                        "SELECT role FROM original WHERE pos=?", (key,)
                    ).fetchone()
                    if old and old[0] != role:
                        raise ValueError("source leakage")
                    db.execute(
                        "INSERT OR IGNORE INTO original VALUES(?,?)", (key, role)
                    )
                    if progress:
                        progress.count(n)
                db.commit()
            proc = subprocess.Popen(
                [
                    str(Path(binary).resolve()),
                    str(Path(evaluation).resolve()),
                    str(node_limit),
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                text=True,
                bufsize=1,
            )
            try:
                if proc.stdout.readline().strip() != "RDQ1":
                    raise ValueError("wrong qresolver greeting")
                with (stage / "q_provenance.jsonl").open("w") as provenance:

                    def stream():
                        c.report_stage(progress, "QRESOLVED: партий обработано")
                        yield "RDSTREAM1\n"
                        game_index = 0
                        training_games = 0
                        for role in f.ROLES:
                            with (source / f"{role}.games.jsonl").open() as histories:
                                history = None
                                for gid, group in itertools.groupby(
                                    f.records(
                                        source / f"{role}.rdtune3", allow_final=True
                                    ),
                                    lambda r: r["game_id"],
                                ):
                                    rows = list(group)
                                    while history is None or history["game_id"] < gid:
                                        line = histories.readline()
                                        if not line:
                                            raise ValueError("missing history")
                                        history = json.loads(line)
                                    if history["game_id"] != gid:
                                        raise ValueError("history mismatch")
                                    if (
                                        c.game_id(history["root"], history["moves"])
                                        != gid
                                        or c.role_for(gid, manifest["seed"]) != role
                                    ):
                                        raise ValueError(
                                            "semantic identity/role mismatch"
                                        )
                                    if role in ("train", "dev"):
                                        if (
                                            max_games
                                            and training_games >= max_games
                                            and role == "train"
                                        ):
                                            continue
                                        if role == "train":
                                            training_games += 1
                                    game_index += 1
                                    yield "G\t" + str(game_index) + "\t" + "\t".join(
                                        map(str, history["root"])
                                    ) + "\t" + str(history["result"]) + "\t" + ";".join(
                                        ":".join(map(str, m)) for m in history["moves"]
                                    ) + "\t" + ",".join(
                                        f"{m:x}" for m in history["raw_moves"]
                                    ) + "\n"
                                    if role in ("train", "dev"):
                                        request = (
                                            "G "
                                            + " ".join(map(str, history["root"]))
                                            + " "
                                            + ",".join(str(r["ply"]) for r in rows)
                                            + " "
                                            + ",".join(
                                                f"{m:x}" for m in history["raw_moves"]
                                            )
                                            + "\n"
                                        )
                                        proc.stdin.write(request)
                                        proc.stdin.flush()
                                        for original in rows:
                                            parts = (
                                                proc.stdout.readline()
                                                .rstrip("\n")
                                                .split("\t")
                                            )
                                            if (
                                                len(parts) < 4
                                                or parts[0] not in ("Q", "X")
                                                or int(parts[1]) != original["ply"]
                                            ):
                                                raise ValueError(
                                                    f"qresolver response error: {parts[:4]}"
                                                )
                                            stats["attempted"] += 1
                                            stats["nodes"] += int(parts[3])
                                            meta = dict(
                                                game_id=gid,
                                                ply=original["ply"],
                                                source_position=original["position"],
                                                source_flags=original["flags"],
                                                nodes=int(parts[3]),
                                                role=role,
                                            )
                                            if parts[0] == "X":
                                                stats["skip_" + parts[2]] += 1
                                                meta["status"] = parts[2]
                                                provenance.write(
                                                    json.dumps(
                                                        meta, separators=(",", ":")
                                                    )
                                                    + "\n"
                                                )
                                                continue
                                            if len(parts) != 12:
                                                raise ValueError("invalid Q record")
                                            r = dict(
                                                original,
                                                position=list(map(int, parts[4:8])),
                                                phase=int(parts[8]),
                                                features=list(
                                                    map(int, parts[9].split(","))
                                                ),
                                                flags=int(parts[11]),
                                            )
                                            meta.update(
                                                qscore_stm=int(parts[2]),
                                                leaf_position=r["position"],
                                                path=(
                                                    []
                                                    if parts[10] == "-"
                                                    else [
                                                        int(x, 16)
                                                        for x in parts[10].split(",")
                                                    ]
                                                ),
                                            )
                                            owner = db.execute(
                                                "SELECT role FROM original WHERE pos=?",
                                                (c.position_key(r["position"]),),
                                            ).fetchone()
                                            if owner and owner[0] != role:
                                                stats[
                                                    "skip_original_role_collision"
                                                ] += 1
                                                meta["status"] = (
                                                    "original_role_collision"
                                                )
                                            else:
                                                meta["status"] = (
                                                    "resolved_before_final_dedup"
                                                )
                                                stats["resolved"] += 1
                                                yield sample_line(r)
                                            provenance.write(
                                                json.dumps(meta, separators=(",", ":"))
                                                + "\n"
                                            )
                                        if proc.stdout.readline().strip() != "Z":
                                            raise ValueError("missing qresolver footer")
                                    else:
                                        for r in rows:
                                            yield sample_line(r)
                        yield f"Z\t{game_index}\t0\n"

                    result = c.build(
                        stream(),
                        output,
                        identity,
                        seed=manifest["seed"],
                        cap=manifest["max_per_game"],
                        min_ply=manifest["min_ply"],
                        progress=progress,
                    )
                proc.stdin.close()
                if proc.wait(timeout=10) != 0:
                    raise RuntimeError("qresolver failed")
            finally:
                if proc.poll() is None:
                    proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
                proc.stdout.close()
                if not proc.stdin.closed:
                    proc.stdin.close()
            os.rename(stage / "q_provenance.jsonl", output / "q_provenance.jsonl")
            result.update(
                mode="QRESOLVED",
                qresolved_stats=dict(stats),
                qresolved_policy="TRAIN/DEV capture-only minimax leaves; protected roles STATIC; original white game labels; terminal/claim/unresolved dropped",
                provenance_policy="q_provenance includes attempts BEFORE leaf dedup/leakage filtering; ply is original replay ply",
                limitations=[
                    "retrospective FINAL",
                    "capture-only qsearch: no quiet promotion extension",
                    "not production search qsearch; frozen offline minimax",
                    "no resume; interrupted output unpublished",
                ],
            )
            result["files"]["q_provenance.jsonl"] = f.sha(output / "q_provenance.jsonl")
            (output / "manifest.json").write_text(
                json.dumps(result, sort_keys=True, indent=2) + "\n"
            )
            c.report_stage(progress, "Аудит QRESOLVED")
            c.audit(output, progress)
            os.rename(output, destination)
            c.report_stage(progress, "QRESOLVED завершён")
            return result
        finally:
            db.close()


def sample_line(r):
    return (
        "S\t"
        + str(r["ply"])
        + "\t"
        + "\t".join(map(str, r["position"]))
        + "\t"
        + str(r["phase"])
        + "\t"
        + str(r["flags"] & 1)
        + "\t"
        + str((r["flags"] >> 1) & 1)
        + "\t"
        + ",".join(map(str, r["features"]))
        + "\n"
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--corpus", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--eval", type=Path, required=True)
    p.add_argument(
        "--binary", type=Path, default=Path("target/release/classic_qresolve")
    )
    p.add_argument("--node-limit", type=int, default=10000)
    p.add_argument("--max-train-games", type=int, default=0)
    a = p.parse_args()
    if a.node_limit < 1 or a.max_train_games < 0:
        p.error("invalid limits")
    with c.Progress() as progress:
        m = derive(
            a.corpus, a.out, a.binary, a.eval, a.node_limit, a.max_train_games, progress
        )
    print(
        json.dumps(
            dict(counts=m["counts"], qresolved_stats=m["qresolved_stats"]),
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

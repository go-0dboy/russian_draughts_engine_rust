#!/usr/bin/env python3
"""Disk-backed deterministic Corpus V2 builder; requires trusted Rust exporter.
No engine rules are reimplemented in Python. STATIC only in format version 1.
"""
from __future__ import annotations
import argparse
from collections import Counter, defaultdict
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import struct
import subprocess
import tempfile
from contextlib import nullcontext
import rdtune3 as fmt

from lab_common import Progress


def report_stage(progress, label, total=None):
    if progress:
        progress.stage(label, total)


def count(progress, n):
    if progress:
        progress.count(n)


def drop_cross_role_samples(db):
    # One scan of samples, two indexed lookups per row. Avoid the Cartesian
    # probing of independent gid IN (...) and pos IN (...) lists.
    db.execute(
        """DELETE FROM samples WHERE EXISTS (
        SELECT 1 FROM owner o JOIN games g ON g.gid=samples.gid
        WHERE o.pos=samples.pos AND o.priority > CASE g.role
        WHEN 'train' THEN 0 WHEN 'dev' THEN 1 WHEN 'arena' THEN 2
        WHEN 'reference' THEN 3 WHEN 'final' THEN 4 END)"""
    )


PRIORITY = {"train": 0, "dev": 1, "arena": 2, "reference": 3, "final": 4}


def canonical_game(root, moves):
    return json.dumps([root, moves], separators=(",", ":")).encode()


def game_id(root, moves):
    return hashlib.sha256(canonical_game(root, moves)).hexdigest()


def role_for(gid, seed):
    bucket = (
        int.from_bytes(
            hashlib.sha256(f"corpus-v2/1:{seed}:{gid}".encode()).digest()[:8], "big"
        )
        % 100
    )
    return (
        "train"
        if bucket < 75
        else (
            "dev"
            if bucket < 85
            else "reference" if bucket < 90 else "arena" if bucket < 95 else "final"
        )
    )


def position_key(p):
    return struct.pack("<IIIB", *p)


def sample_class(r):
    w, b, k, _ = r["position"]
    n = (w | b).bit_count()
    material = "men" if not k else "king-heavy" if k.bit_count() * 2 >= n else "mixed"
    stage = (
        "opening"
        if r["ply"] < 16
        else "early" if r["ply"] < 32 else "middle" if r["ply"] < 64 else "late"
    )
    return f"{stage}/{material}/{'capture' if r['flags']&1 else 'quiet'}/{'promotion' if r['flags']&2 else 'other'}"


def select_samples(gid, rows, cap, min_ply):
    # Round-robin strata, hash ranking within strata, no random dependence on file order.
    buckets = defaultdict(list)
    seen = set()
    for r in rows:
        key = position_key(r["position"])
        if r["ply"] < min_ply or key in seen:
            continue
        seen.add(key)
        rank = hashlib.sha256(bytes.fromhex(gid) + key).digest()
        buckets[sample_class(r)].append((rank, r))
    for bucket in buckets.values():
        bucket.sort(key=lambda item: item[0], reverse=True)
    chosen = []
    while len(chosen) < cap:
        changed = False
        for key in sorted(buckets):
            if buckets[key] and len(chosen) < cap:
                chosen.append(buckets[key].pop()[1])
                changed = True
        if not changed:
            break
    return sorted(chosen, key=lambda r: r["ply"])


def ingest(lines, db, seed, cap, min_ply, progress=None):
    report_stage(progress, "Чтение PDN: партий обработано")
    if next(lines, "").strip() != "RDSTREAM1":
        raise ValueError("invalid exporter greeting")
    db.executescript(
        """CREATE TABLE games(gid TEXT PRIMARY KEY,role TEXT,root TEXT,moves TEXT,raw TEXT,result INT,valid INT);
    CREATE TABLE samples(gid TEXT,pos BLOB,ply INT,payload TEXT,PRIMARY KEY(gid,pos)) WITHOUT ROWID;"""
    )
    current = None
    rows = []
    stats = Counter()
    rejected = []
    ended = False

    def flush():
        nonlocal rows
        if current is None:
            return
        gid, role, root, moves, raw, result = current
        old = db.execute(
            "SELECT result,valid FROM games WHERE gid=?", (gid,)
        ).fetchone()
        if old:
            stats["duplicate_games"] += 1
            if old[0] != result:
                db.execute("UPDATE games SET valid=0 WHERE gid=?", (gid,))
                stats["conflicting_duplicates"] += 1
            rows = []
            return
        db.execute(
            "INSERT INTO games VALUES(?,?,?,?,?,?,1)",
            (gid, role, json.dumps(root), json.dumps(moves), json.dumps(raw), result),
        )
        selected = select_samples(gid, rows, cap, min_ply)

        def batch():
            for r in selected:
                r.update(game_id=gid, result=result)
                yield gid, position_key(r["position"]), r["ply"], json.dumps(
                    r, separators=(",", ":")
                )

        db.executemany("INSERT INTO samples VALUES(?,?,?,?)", batch())
        stats["games"] += 1
        stats["selected_before_leakage"] += len(selected)
        rows = []
        if stats["games"] % 500 == 0:
            db.commit()

    for line in lines:
        t = line.rstrip("\r\n").split("\t")
        if ended:
            raise ValueError("data after exporter footer")
        if t[0] == "G":
            flush()
            if len(t) != 9:
                raise ValueError("bad game row")
            count(progress, int(t[1]))
            root = list(map(int, t[2:6]))
            result = int(t[6])
            if result not in (0, 1, 2):
                raise ValueError("unknown result")
            moves = [list(map(int, m.split(":"))) for m in t[7].split(";") if m]
            raw = [int(x, 16) for x in t[8].split(",") if x]
            if len(raw) != len(moves):
                raise ValueError("packed replay length mismatch")
            gid = game_id(root, moves)
            current = (gid, role_for(gid, seed), root, moves, raw, result)
        elif t[0] == "S":
            if current is None or len(t) != 10:
                raise ValueError("sample outside game/bad sample")
            ply = int(t[1])
            p = list(map(int, t[2:6]))
            phase = int(t[6])
            flags = int(t[7]) + 2 * int(t[8])
            if ply >= len(current[3]):
                raise ValueError("sample exceeds replay")
            r = dict(
                ply=ply,
                position=p,
                phase=phase,
                flags=flags,
                features=list(map(int, t[9].split(","))),
            )
            # Apply the binary reader's structural invariants also at ingestion.
            fmt.unpack(
                fmt.pack(dict(r, game_id=current[0], result=current[5], weight=1.0))
            )
            rows.append(r)
        elif t[0] == "E":
            rejected.append(t[1:])
            stats["rejected_games"] += 1
            count(progress, int(t[1]))
        elif t[0] == "Z":
            flush()
            current = None
            ended = True
            stats["source_games"] = int(t[1])
            stats["parser_rejected"] = int(t[2])
        else:
            raise ValueError(f"unknown exporter row {t[0]}")
    if not ended:
        raise ValueError("exporter ended without footer")
    db.commit()
    return stats, rejected


def build(
    lines, destination, identity, seed=20260930, cap=96, min_ply=6, progress=None
):
    destination = Path(destination)
    if destination.exists():
        raise FileExistsError(f"will not overwrite {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".corpus-v2-", dir=destination.parent
    ) as tmp:
        stage = Path(tmp)
        db = sqlite3.connect(stage / "work.sqlite")
        db.set_progress_handler(lambda: 0, 10000)
        try:
            db.execute("PRAGMA journal_mode=OFF")
            db.execute("PRAGMA synchronous=OFF")
            db.execute("PRAGMA cache_size=-32768")
            db.execute("PRAGMA temp_store=FILE")
            stats, rejected = ingest(iter(lines), db, seed, cap, min_ply, progress)
            report_stage(progress, "Индекс ролей и удаление конфликтных партий")
            db.execute("CREATE INDEX games_role_gid ON games(role,gid)")
            db.executescript(
                """DELETE FROM samples WHERE gid IN (SELECT gid FROM games WHERE valid=0);
            CREATE TABLE owner(pos BLOB PRIMARY KEY,priority INT) WITHOUT ROWID;"""
            )
            for role, priority in PRIORITY.items():
                report_stage(progress, f"Владельцы позиций: {role} (SQL)")
                db.execute(
                    """INSERT INTO owner SELECT s.pos,? FROM samples s JOIN games g USING(gid) WHERE g.role=?
                GROUP BY s.pos ON CONFLICT(pos) DO UPDATE SET priority=MAX(priority,excluded.priority)""",
                    (priority, role),
                )
            before = db.execute("SELECT COUNT(*) FROM samples").fetchone()[0]
            report_stage(progress, "Удаление пересечений (SQL, один проход)")
            drop_cross_role_samples(db)
            after = db.execute("SELECT COUNT(*) FROM samples").fetchone()[0]
            stats["cross_role_drops"] = before - after
            report_stage(progress, "Подсчёт весов партий (SQL)")
            db.execute(
                "CREATE TABLE counts AS SELECT gid,COUNT(*) AS n FROM samples GROUP BY gid"
            )
            db.execute("CREATE UNIQUE INDEX counts_gid ON counts(gid)")
            db.commit()
            counts = {}
            classes = {}
            for role in fmt.ROLES:
                count = db.execute(
                    "SELECT COUNT(*) FROM samples JOIN games USING(gid) WHERE role=?",
                    (role,),
                ).fetchone()[0]
                counts[role] = count
                hist = Counter()
                report_stage(progress, f"Запись {role}", count)

                def output_rows():
                    query = """SELECT s.payload,c.n FROM samples s JOIN games g USING(gid) JOIN counts c USING(gid)
                    WHERE g.role=? ORDER BY s.gid,s.ply"""
                    for done, (payload, n) in enumerate(db.execute(query, (role,)), 1):
                        r = json.loads(payload)
                        r["weight"] = 1.0 / n
                        hist[sample_class(r)] += 1
                        if progress:
                            progress.count(done)
                        yield r

                fmt.write(stage / f"{role}.rdtune3", role, count, output_rows())
                classes[role] = dict(sorted(hist.items()))
                report_stage(progress, f"Истории партий: {role}")
                with (stage / f"{role}.games.jsonl").open("w") as f:
                    for gid, root, moves, raw, result in db.execute(
                        """SELECT gid,root,moves,raw,result FROM games JOIN counts USING(gid)
                    WHERE role=? ORDER BY gid""",
                        (role,),
                    ):
                        f.write(
                            json.dumps(
                                dict(
                                    game_id=gid,
                                    root=json.loads(root),
                                    moves=json.loads(moves),
                                    raw_moves=json.loads(raw),
                                    result=result,
                                ),
                                sort_keys=True,
                                separators=(",", ":"),
                            )
                            + "\n"
                        )
            # RDOPEN2 compatibility export: one retained, unique final position
            # per opening; full path must start at the canonical initial board.
            report_stage(progress, "Подготовка дебютов арены")
            openings = []
            opening_seen = set()
            query = """SELECT s.pos,s.ply,g.root,g.raw FROM samples s JOIN games g USING(gid)
                     WHERE g.role='arena' AND s.ply BETWEEN 6 AND 14 ORDER BY s.gid,s.ply"""
            for key, ply, root, raw in db.execute(query):
                if json.loads(root) != [4095, 4293918720, 0, 0] or key in opening_seen:
                    continue
                opening_seen.add(key)
                openings.append(json.loads(raw)[:ply])
                if len(openings) >= 4096:
                    break
            with (stage / "arena.rdopen").open("wb") as f:
                f.write(struct.pack("<8sIIQ", b"RDOPEN2\0", 1, 0, len(openings)))
                for line in openings:
                    f.write(struct.pack("<HH", len(line), 0))
                    f.write(struct.pack("<" + "Q" * len(line), *line))
            stats["arena_openings"] = len(openings)
            (stage / "rejected.json").write_text(
                json.dumps(rejected, ensure_ascii=False) + "\n"
            )
            manifest = dict(
                format="corpus-v2/1",
                mode="STATIC",
                feature_schema="hce-92-raw-white-v1",
                source=identity,
                seed=seed,
                max_per_game=cap,
                min_ply=min_ply,
                role_policy="75/10/5/5/5 sha256 semantic game; final>reference>arena>dev>train",
                weighting="sum=1 per retained game per role",
                final_status="RETROSPECTIVE_NOT_BLIND",
                stats=dict(stats),
                counts=counts,
                classes=classes,
                limitations=[
                    "STATIC export; use qresolved.py for derived capture-only leaves",
                    "not a blind FINAL from previously used source",
                    "features omit historical draw context; replay retained for reference search",
                ],
            )
        finally:
            db.close()
        (stage / "work.sqlite").unlink()
        report_stage(progress, "Контрольные суммы выходных файлов")
        manifest["files"] = {
            p.name: fmt.sha(p) for p in sorted(stage.iterdir()) if p.is_file()
        }
        (stage / "manifest.json").write_text(
            json.dumps(manifest, sort_keys=True, indent=2) + "\n"
        )
        # Directory only becomes visible as a completed corpus after every file is written.
        os.rename(stage, destination)
    report_stage(progress, "Сборка завершена")
    return manifest


def audit(directory, progress=None):
    report_stage(progress, "Аудит: контрольные суммы")
    m = fmt.read_manifest(directory)
    with tempfile.TemporaryDirectory() as tmp:
        db = sqlite3.connect(Path(tmp) / "audit.sqlite")
        db.execute(
            "CREATE TABLE positions(pos BLOB PRIMARY KEY,role TEXT) WITHOUT ROWID"
        )
        counts = {}
        for role in fmt.ROLES:
            report_stage(progress, f"Аудит {role}", m["counts"][role])
            previous_gid = None
            previous_key = None
            total = 0.0
            n = 0
            for r in fmt.records(Path(directory) / f"{role}.rdtune3", allow_final=True):
                if r["role"] != role:
                    raise ValueError("role/header mismatch")
                current_key = (r["game_id"], r["ply"])
                if previous_key is not None and current_key <= previous_key:
                    raise ValueError("records not strictly ordered by game/ply")
                previous_key = current_key
                if previous_gid != r["game_id"]:
                    if previous_gid is not None and abs(total - 1) > 1e-9:
                        raise ValueError("game weights do not sum to one")
                    previous_gid = r["game_id"]
                    total = 0.0
                total += r["weight"]
                n += 1
                if progress:
                    progress.count(n)
                key = position_key(r["position"])
                old = db.execute(
                    "SELECT role FROM positions WHERE pos=?", (key,)
                ).fetchone()
                if old and old[0] != role:
                    raise ValueError("cross-role exact-position leakage")
                db.execute("INSERT OR IGNORE INTO positions VALUES(?,?)", (key, role))
            if previous_gid is not None and abs(total - 1) > 1e-9:
                raise ValueError("game weights do not sum to one")
            counts[role] = n
            db.commit()
        db.close()
    if counts != m["counts"]:
        raise ValueError("manifest count mismatch")
    return counts


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("build")
    c.add_argument("--pdn", type=Path, required=True)
    c.add_argument(
        "--exporter", type=Path, default=Path("target/release/classic_corpus_stream")
    )
    c.add_argument("--out", type=Path, required=True)
    c.add_argument("--seed", type=int, default=20260930)
    c.add_argument("--max-per-game", type=int, default=96)
    c.add_argument("--min-ply", type=int, default=6)
    a = sub.add_parser("audit")
    a.add_argument("directory", type=Path)
    for cmd in (c, a):
        cmd.add_argument("--progress-seconds", type=float, default=5)
        cmd.add_argument("--quiet", action="store_true")
    args = p.parse_args()
    if not args.progress_seconds > 0:
        p.error("progress interval must be positive")
    with (
        nullcontext(None) if args.quiet else Progress(args.progress_seconds)
    ) as progress:
        run(args, p, progress)


def run(args, p, progress):
    if args.cmd == "audit":
        print(json.dumps(audit(args.directory, progress), sort_keys=True))
        return
    if args.max_per_game < 1 or args.min_ply < 0:
        p.error("invalid sampling limits")
    report_stage(progress, "Контрольные суммы PDN и инструментов")
    identity = dict(
        pdn_sha256=fmt.sha(args.pdn),
        exporter_sha256=fmt.sha(args.exporter),
        builder_sha256=fmt.sha(__file__),
        format_sha256=fmt.sha(fmt.__file__),
    )
    with tempfile.TemporaryFile() as errors:
        proc = subprocess.Popen(
            [str(args.exporter.resolve()), str(args.pdn.resolve())],
            stdout=subprocess.PIPE,
            stderr=errors,
            text=True,
        )

        def checked_lines():
            yield from proc.stdout
            if proc.wait() != 0:
                raise RuntimeError("Rust exporter failed; corpus not published")

        try:
            m = build(
                checked_lines(),
                args.out,
                identity,
                args.seed,
                args.max_per_game,
                args.min_ply,
                progress,
            )
            print(json.dumps(m["counts"], sort_keys=True))
        finally:
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

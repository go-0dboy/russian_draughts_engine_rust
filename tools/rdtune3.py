"""RDTUNE3 v1: fixed records, per-file SHA-256, no pickle/native layouts.
Features are exact WHITE-perspective raw HCE terms, not weighted scores.
Weights sum to one per game within its role AFTER dedup/filtering.
"""

from __future__ import annotations
import hashlib
import json
import struct
from pathlib import Path

ROLES = ("train", "dev", "reference", "arena", "final")
MAGIC = b"RDTUNE3\0"
# magic/version/record bytes/count/feature count/role/reserved
HEADER = struct.Struct("<8sIIQHBB")
# gid SHA256, board, stm, result(black=0/draw=1/white=2), ply, phase,
# classification flags(capture=1,promotion=2), pieces, kings, reserved, weight
PREFIX = struct.Struct("<32sIIIBBHHBBBBd")
FEATURES = struct.Struct("<92h")
SIZE = PREFIX.size + FEATURES.size


def sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for data in iter(lambda: f.read(1024 * 1024), b""):
            h.update(data)
    return h.hexdigest()


def pack(r):
    w, b, k, side = r["position"]
    values = r["features"]
    if len(values) != 92:
        raise ValueError("expected 92 raw features")
    return PREFIX.pack(
        bytes.fromhex(r["game_id"]),
        w,
        b,
        k,
        side,
        r["result"],
        r["ply"],
        r["phase"],
        r["flags"],
        (w | b).bit_count(),
        k.bit_count(),
        0,
        r["weight"],
    ) + FEATURES.pack(*values)


def unpack(data):
    a = PREFIX.unpack_from(data)
    gid, w, b, k, side, result, ply, phase, flags, pieces, kings, reserved, weight = a
    if (
        reserved
        or side not in (0, 1)
        or result not in (0, 1, 2)
        or phase > 256
        or flags > 3
        or w & b
        or k & ~(w | b)
        or w.bit_count() > 12
        or b.bit_count() > 12
        or pieces != (w | b).bit_count()
        or kings != k.bit_count()
        or not 0 < weight <= 1
    ):
        raise ValueError("invalid RDTUNE3 record")
    return dict(
        game_id=gid.hex(),
        position=[w, b, k, side],
        result=result,
        ply=ply,
        phase=phase,
        flags=flags,
        weight=weight,
        features=list(FEATURES.unpack_from(data, PREFIX.size)),
    )


def records(path, expected_sha=None, allow_final=False):
    path = Path(path)
    if expected_sha is not None and sha(path) != expected_sha:
        raise ValueError("checksum mismatch")
    with path.open("rb") as f:
        head = f.read(HEADER.size)
        if len(head) != HEADER.size:
            raise ValueError("truncated RDTUNE3 header")
        magic, version, size, count, nfeatures, role, reserved = HEADER.unpack(head)
        if (magic, version, size, nfeatures, reserved) != (
            MAGIC,
            1,
            SIZE,
            92,
            0,
        ) or role >= 5:
            raise ValueError("unsupported RDTUNE3 header")
        if role == 4 and not allow_final:
            raise ValueError("FINAL requires explicit allow_final")
        if path.stat().st_size != HEADER.size + count * SIZE:
            raise ValueError("wrong file length")
        for _ in range(count):
            r = unpack(f.read(SIZE))
            r["role"] = ROLES[role]
            yield r


def write(path, role, count, rows):
    with open(path, "xb") as f:
        f.write(HEADER.pack(MAGIC, 1, SIZE, count, 92, ROLES.index(role), 0))
        actual = 0
        for r in rows:
            f.write(pack(r))
            actual += 1
        if actual != count:
            raise ValueError("record count mismatch")


def read_manifest(directory):
    p = Path(directory)
    m = json.loads((p / "manifest.json").read_text())
    if m.get("format") != "corpus-v2/1":
        raise ValueError("unsupported manifest")
    required = {
        f"{role}.{suffix}" for role in ROLES for suffix in ("rdtune3", "games.jsonl")
    } | {"arena.rdopen"}
    if not required <= m.get("files", {}).keys():
        raise ValueError("manifest missing required files")
    if set(m.get("counts", {})) != set(ROLES) or any(
        type(n) is not int or n < 0 for n in m["counts"].values()
    ):
        raise ValueError("invalid manifest counts")
    for name, digest in m["files"].items():
        if Path(name).name != name or sha(p / name) != digest:
            raise ValueError(f"checksum mismatch: {name}")
    return m

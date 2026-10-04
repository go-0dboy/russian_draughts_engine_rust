#!/usr/bin/env python3
"""Cheap release-tree hygiene checks independent of Rust tooling."""

from __future__ import annotations

import hashlib
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FORBIDDEN_PARTS = {"__pycache__", ".pytest_cache", ".ruff_cache"}
FORBIDDEN_SUFFIXES = {".pyc", ".pyo"}


def manifest_paths() -> list[str]:
    out: list[str] = []
    seen: set[str] = set()
    for raw in (ROOT / ".release-manifest").read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        path = Path(line)
        if path.is_absolute() or ".." in path.parts or not path.parts:
            raise SystemExit(f"unsafe manifest path: {line}")
        normalized = path.as_posix()
        if any(
            normalized == old
            or normalized.startswith(old + "/")
            or old.startswith(normalized + "/")
            for old in seen
        ):
            raise SystemExit(f"overlapping manifest path: {normalized}")
        if normalized in seen:
            raise SystemExit(f"duplicate manifest path: {normalized}")
        seen.add(normalized)
        out.append(normalized)
    return out


def iter_manifest_files() -> list[Path]:
    files: list[Path] = []
    for rel in manifest_paths():
        path = ROOT / rel
        if not path.exists():
            raise SystemExit(f"manifest path missing: {rel}")
        if path.is_dir():
            files.extend(p for p in path.rglob("*") if p.is_file())
        else:
            files.append(path)
    return sorted(set(files))


def write_checksums() -> None:
    release_owned = {p.relative_to(ROOT).as_posix() for p in iter_manifest_files()}
    release_owned.add(".release-manifest")
    release_owned.discard("SOURCE_SHA256SUMS.txt")
    rows = []
    for rel in sorted(release_owned):
        path = ROOT / rel
        if not path.is_file():
            raise SystemExit(f"checksum source is not a file: {rel}")
        rows.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  ./{rel}")
    target = ROOT / "SOURCE_SHA256SUMS.txt"
    tmp = target.with_suffix(target.suffix + ".tmp")
    tmp.write_text("\n".join(rows) + "\n", encoding="utf-8")
    os.replace(tmp, target)
    print(f"source checksums: wrote {len(rows)} entries")


def main() -> int:
    if len(sys.argv) > 2 or (len(sys.argv) == 2 and sys.argv[1] != "--write-checksums"):
        print("usage: check_source_hygiene.py [--write-checksums]", file=sys.stderr)
        return 2
    if len(sys.argv) == 2:
        write_checksums()

    errors: list[str] = []

    manifest_files = iter_manifest_files()
    release_owned = {p.relative_to(ROOT).as_posix() for p in manifest_files}
    release_owned.add(".release-manifest")
    for path in manifest_files:
        rel = path.relative_to(ROOT)
        if path.is_symlink():
            errors.append(f"release source cannot be a symlink: {rel}")
        if (
            any(part in FORBIDDEN_PARTS for part in rel.parts)
            or path.suffix in FORBIDDEN_SUFFIXES
        ):
            errors.append(f"generated cache artifact is release-owned: {rel}")
    if "SOURCE_SHA256SUMS.txt" not in release_owned:
        errors.append("SOURCE_SHA256SUMS.txt must be release-owned")

    checksum_path = ROOT / "SOURCE_SHA256SUMS.txt"
    if not checksum_path.is_file():
        errors.append("SOURCE_SHA256SUMS.txt is missing")
    if checksum_path.exists():
        listed: dict[str, str] = {}
        for line in checksum_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                digest, rel = line.split(None, 1)
            except ValueError:
                errors.append(f"bad checksum line: {line!r}")
                continue
            rel = rel.removeprefix("./")
            if rel in listed or not re.fullmatch(r"[0-9a-f]{64}", digest):
                errors.append(f"invalid/duplicate checksum entry: {rel}")
            if Path(rel).is_absolute() or ".." in Path(rel).parts:
                errors.append(f"unsafe checksum path: {rel}")
                continue
            listed[rel] = digest
        expected = release_owned - {"SOURCE_SHA256SUMS.txt"}
        if set(listed) != expected:
            missing = sorted(expected - set(listed))
            extra = sorted(set(listed) - expected)
            if missing:
                errors.append(f"checksum entries missing: {missing}")
            if extra:
                errors.append(f"checksum entries not release-owned: {extra}")
        for rel, expected_digest in listed.items():
            path = ROOT / rel
            if not path.is_file():
                errors.append(f"checksum target missing: {rel}")
                continue
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            if digest != expected_digest:
                errors.append(f"checksum mismatch: {rel}")

    for doc in [
        "README.md",
        "ARCHITECTURE.md",
        "TESTING.md",
        "ROADMAP.md",
        "API.md",
        "PROTOCOL.md",
    ]:
        if not (ROOT / doc).is_file():
            errors.append(f"required documentation missing: {doc}")

    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(f"source hygiene: OK ({len(manifest_files)} release files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

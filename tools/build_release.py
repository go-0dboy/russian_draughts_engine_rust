#!/usr/bin/env python3
"""Build a manifest-only, byte-reproducible source release ZIP.

The staging tree contains only paths owned by `.release-manifest`; generated
cache files and unknown developer files can never leak into the archive. ZIP
metadata is normalized so rebuilding unchanged sources produces identical bytes.
A standalone verified updater is emitted beside the archive for bootstrap
upgrades from pre-v1.9 installations.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ZIP_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


def package_version() -> str:
    in_package = False
    for raw in (ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if line == "[package]":
            in_package = True
            continue
        if in_package and line.startswith("["):
            break
        if in_package and line.startswith("version"):
            return line.split("=", 1)[1].strip().strip('"')
    raise RuntimeError("cannot read package version")


def manifest_paths() -> list[str]:
    paths: list[str] = []
    for raw in (ROOT / ".release-manifest").read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if line:
            paths.append(line)
    return paths


def copy_owned(stage: Path) -> None:
    # `.release-manifest` is release metadata handled specially by update.sh;
    # it must be shipped but must not list itself as an owned install path.
    shutil.copy2(ROOT / ".release-manifest", stage / ".release-manifest")
    for rel in manifest_paths():
        if rel == "SOURCE_SHA256SUMS.txt":
            continue
        src = ROOT / rel
        dst = stage / rel
        if not src.exists():
            raise RuntimeError(f"manifest path missing: {rel}")
        dst.parent.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            shutil.copytree(
                src,
                dst,
                ignore=shutil.ignore_patterns(
                    "__pycache__", "*.pyc", "*.pyo", ".pytest_cache", ".ruff_cache"
                ),
            )
        else:
            shutil.copy2(src, dst)


def write_checksums(stage: Path) -> None:
    rows: list[str] = []
    for path in sorted(p for p in stage.rglob("*") if p.is_file()):
        rel = path.relative_to(stage).as_posix()
        if rel == "SOURCE_SHA256SUMS.txt":
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        rows.append(f"{digest}  ./{rel}")
    (stage / "SOURCE_SHA256SUMS.txt").write_text(
        "\n".join(rows) + "\n", encoding="utf-8"
    )


def write_reproducible_zip(stage: Path, top: str, archive: Path) -> None:
    with zipfile.ZipFile(
        archive,
        "w",
        compression=zipfile.ZIP_DEFLATED,
        compresslevel=9,
        strict_timestamps=True,
    ) as zf:
        for path in sorted(p for p in stage.rglob("*") if p.is_file()):
            rel = (Path(top) / path.relative_to(stage)).as_posix()
            info = zipfile.ZipInfo(rel, ZIP_TIMESTAMP)
            info.create_system = 3
            executable = bool(path.stat().st_mode & 0o111)
            mode = 0o100755 if executable else 0o100644
            info.external_attr = mode << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            info.flag_bits |= 0x800  # UTF-8 names
            zf.writestr(
                info,
                path.read_bytes(),
                compress_type=zipfile.ZIP_DEFLATED,
                compresslevel=9,
            )


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "out_dir",
        nargs="?",
        default=str(ROOT / "dist"),
        help="output directory (default: ./dist)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    version = package_version()
    top = f"russian_draughts_classic_v{version}"
    archive = out_dir / f"{top}.zip"
    bootstrap = out_dir / f"update_classic_v{version}.sh"

    with tempfile.TemporaryDirectory(prefix="rd-release-") as td:
        stage = Path(td) / top
        stage.mkdir()
        copy_owned(stage)
        write_checksums(stage)
        write_reproducible_zip(stage, top, archive)

    shutil.copy2(ROOT / "update.sh", bootstrap)
    bootstrap.chmod(0o755)

    sums = out_dir / "SHA256SUMS.txt"
    sums.write_text(
        f"{sha256(archive)}  {archive.name}\n{sha256(bootstrap)}  {bootstrap.name}\n",
        encoding="utf-8",
    )

    print(archive)
    print(f"sha256 {sha256(archive)}")
    print(bootstrap)
    print(f"sha256 {sha256(bootstrap)}")
    print(sums)
    return 0


if __name__ == "__main__":
    from lab_common import run_cli

    run_cli(main)

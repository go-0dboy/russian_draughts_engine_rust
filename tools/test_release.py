#!/usr/bin/env python3
"""Release reproducibility, updater preservation and hostile-archive regression tests."""
from pathlib import Path
import os
import shlex
import shutil
import stat
import subprocess
import tempfile
import unittest
import zipfile
import build_release as release

ROOT = Path(__file__).resolve().parents[1]


class ReleaseTests(unittest.TestCase):
    def test_archive_update_rollback_and_invalid_payloads(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            stage = root / "stage"
            stage.mkdir()
            release.copy_owned(stage)
            release.write_checksums(stage)
            a = root / "a.zip"
            b = root / "b.zip"
            release.write_reproducible_zip(stage, "release", a)
            release.write_reproducible_zip(stage, "release", b)
            self.assertEqual(a.read_bytes(), b.read_bytes())
            with zipfile.ZipFile(a) as z:
                self.assertFalse(
                    any("__pycache__" in n or "/target/" in n for n in z.namelist())
                )
            installed = root / "installed"
            shutil.copytree(stage, installed)
            sentinels = ("data/keep", "target/keep", ".git/keep", "notes.txt")
            for rel in sentinels:
                p = installed / rel
                p.parent.mkdir(exist_ok=True, parents=True)
                p.write_text("keep")
            # Simulate an old source while retaining a valid project name.
            cargo = installed / "Cargo.toml"
            cargo.write_text(cargo.read_text().replace("1.11.1", "1.11.0"))
            before = cargo.read_bytes()

            def run(archive, *flags, ok=True, env=None):
                proc = subprocess.run(
                    ["bash", str(installed / "update.sh"), *flags, str(archive)],
                    cwd=installed,
                    text=True,
                    capture_output=True,
                    timeout=30,
                    env=env,
                )
                if ok:
                    self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
                else:
                    self.assertNotEqual(proc.returncode, 0)

            run(a, "--dry-run")
            self.assertEqual(cargo.read_bytes(), before)
            for kind in ("tampered", "extra", "traversal", "symlink"):
                bad = root / f"{kind}.zip"
                with zipfile.ZipFile(a) as src, zipfile.ZipFile(bad, "w") as dst:
                    for info in src.infolist():
                        data = src.read(info)
                        if kind == "tampered" and info.filename.endswith("/README.md"):
                            data += b"changed"
                        dst.writestr(info, data)
                    if kind == "extra":
                        dst.writestr("release/extra.txt", "bad")
                    if kind == "traversal":
                        dst.writestr("../outside", "bad")
                    if kind == "symlink":
                        info = zipfile.ZipInfo("release/link")
                        info.create_system = 3
                        info.external_attr = (stat.S_IFLNK | 0o777) << 16
                        dst.writestr(info, "../../outside")
                run(bad, ok=False)
                self.assertEqual(cargo.read_bytes(), before)
                self.assertFalse((root / "outside").exists())
            shim = root / "shim"
            shim.mkdir()
            cp = shim / "cp"
            cp.write_text(
                '#!/bin/sh\ncase "$*" in *rust-toolchain.toml*) exit 13;; esac\nexec '
                + shlex.quote(shutil.which("cp"))
                + ' "$@"\n'
            )
            cp.chmod(0o755)
            run(
                a,
                ok=False,
                env=dict(os.environ, PATH=str(shim) + os.pathsep + os.environ["PATH"]),
            )
            self.assertEqual(cargo.read_bytes(), before)
            self.assertTrue((installed / "src/lib.rs").is_file())
            self.assertFalse(list((installed / ".updates").glob("backup-*")))
            run(a)
            self.assertIn("1.11.1", cargo.read_text())
            for rel in sentinels:
                self.assertEqual((installed / rel).read_text(), "keep")
            proc = subprocess.run(
                ["bash", str(installed / "update.sh"), "--rollback"],
                cwd=installed,
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertEqual(cargo.read_bytes(), before)
            for rel in sentinels:
                self.assertEqual((installed / rel).read_text(), "keep")


if __name__ == "__main__":
    unittest.main(verbosity=2)

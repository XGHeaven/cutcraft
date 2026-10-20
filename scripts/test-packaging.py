#!/usr/bin/env python3
"""Platform-independent release contract tests (no fake compiler success)."""
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import release


class PackagingTests(unittest.TestCase):
    def test_archives_and_complete_matrix(self):
        with tempfile.TemporaryDirectory() as directory:
            dest = Path(directory)
            for target in release.TARGETS:
                name = release.archive_name(target)
                prefix = name.removesuffix(".zip").removesuffix(".tar.gz")
                executable = release.binary_name(target)
                release.archive(dest / name, prefix, [(executable, b"fixture only", True)])
                if name.endswith(".zip"):
                    with zipfile.ZipFile(dest / name) as archive:
                        self.assertEqual(archive.namelist(), [prefix + "/cutcraft.exe"])
                else:
                    with tarfile.open(dest / name) as archive:
                        self.assertEqual(archive.getnames(), [prefix + "/cutcraft"])
            release.assemble(dest)
            sums = (dest / "SHA256SUMS").read_text(encoding="utf-8")
            for target in release.TARGETS:
                self.assertIn(release.archive_name(target), sums)
            (dest / release.archive_name("armv7-unknown-linux-gnueabihf")).unlink()
            with self.assertRaisesRegex(ValueError, "incomplete"):
                release.assemble(dest)
            release.assemble(dest, partial=True)
            (dest / "cutcraft-v0.1.0-unsupported.zip").touch()
            with self.assertRaisesRegex(ValueError, "unexpected"):
                release.assemble(dest, partial=True)

    def test_cross_build_cannot_skip_execution(self):
        with patch.object(release, "run", return_value="host: x86_64-unknown-linux-gnu"):
            with self.assertRaisesRegex(ValueError, "--runner"):
                release.package("aarch64-unknown-linux-gnu", Path("unused"))

    def test_failed_smoke_does_not_package(self):
        with patch.object(release, "run", return_value="cutcraft 999.0.0"):
            with self.assertRaisesRegex(ValueError, "version mismatch"):
                release.smoke(Path("fake-binary"), ["qemu-aarch64"])


if __name__ == "__main__":
    unittest.main()

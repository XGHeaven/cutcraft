#!/usr/bin/env python3
"""Verify a cross archive and execute its shipped binary with an explicit runner."""
import argparse
import hashlib
from pathlib import Path
import shlex
import tarfile
import tempfile

import release


def test(dist, target, runner):
    name = release.archive_name(target)
    lines = (dist / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
    sums = [line.split("  ")[0] for line in lines if line.endswith("  " + name)]
    assert len(sums) == 1
    assert hashlib.sha256((dist / name).read_bytes()).hexdigest() == sums[0]
    with tempfile.TemporaryDirectory(prefix="cutcraft cross archive ") as directory:
        binary = Path(directory) / "cutcraft"
        with tarfile.open(dist / name) as archive:
            entries = archive.getmembers()
            prefix = f"cutcraft-v{release.version()}-{target}"
            assert {entry.name for entry in entries} == {
                f"{prefix}/{file}" for file in ["cutcraft", "LICENSE", "README.md", "CHANGELOG.md"]}
            assert len(entries) == 4 and all(entry.isfile() for entry in entries)
            binary.write_bytes(archive.extractfile(prefix + "/cutcraft").read())
        binary.chmod(0o755)
        release.smoke(binary, runner)
    print(f"Cross archive verified under {runner[0]}: {target}; media rendering not exercised.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--target", choices=release.TARGETS, required=True)
    parser.add_argument("--runner", required=True)
    args = parser.parse_args()
    test(args.dist, args.target, shlex.split(args.runner))

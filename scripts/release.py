#!/usr/bin/env python3
"""Local/CI release tooling. Python 3.11+, stdlib only; never publishes or tags."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import shlex
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "armv7-unknown-linux-gnueabihf",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


def binary_name(target):
    return "cutcraft.exe" if "windows" in target else "cutcraft"


def archive_name(target):
    suffix = ".zip" if "windows" in target else ".tar.gz"
    return f"cutcraft-v{version()}-{target}{suffix}"


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, encoding="utf-8", cwd=ROOT, **kwargs).strip()


def version():
    return tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]


def notes():
    text = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    match = re.search(r"^## " + re.escape(version()) + r"\n(.*?)(?=^## |\Z)", text, re.M | re.S)
    if not match or not match[1].strip():
        raise ValueError("CHANGELOG.md needs an entry for " + version())
    return match[1].strip() + "\n"


def check(tag=None):
    v = version()
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", v):
        raise ValueError("unsupported release version: " + v)
    if tag is not None and tag != "v" + v:
        raise ValueError(f"tag {tag!r} must equal v{v}")
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    if manifest["package"].get("publish") is not False:
        raise ValueError("crates.io publication must remain disabled")
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    own = [p for p in lock["package"] if p["name"] == "cutcraft" and "source" not in p]
    if len(own) != 1 or own[0]["version"] != v:
        raise ValueError("Cargo.lock version does not match Cargo.toml")
    notes()
    return v


def archive(path, prefix, files):
    # Stable archive metadata; binary reproducibility still depends on the toolchain.
    if path.suffix == ".zip":
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as out:
            for name, content, executable in sorted(files):
                entry = zipfile.ZipInfo(prefix + "/" + name, (1980, 1, 1, 0, 0, 0))
                entry.compress_type = zipfile.ZIP_DEFLATED
                entry.external_attr = (0o100755 if executable else 0o100644) << 16
                out.writestr(entry, content)
        return
    with path.open("wb") as out, gzip.GzipFile(filename="", fileobj=out, mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w") as tar:
            for name, content, executable in sorted(files):
                entry = tarfile.TarInfo(prefix + "/" + name)
                entry.size = len(content)
                entry.mode = 0o755 if executable else 0o644
                tar.addfile(entry, io.BytesIO(content))


def smoke(binary, runner=()):
    """Run the actual target executable, natively or via an explicit emulator."""
    command = [*runner, str(binary)]
    if run(*command, "--version") != "cutcraft " + version():
        raise ValueError("binary version mismatch")
    json.loads(run(*command, "capabilities"))
    with tempfile.TemporaryDirectory(prefix="cutcraft smoke ") as directory:
        project = str(Path(directory) / "project.cut")
        json.loads(run(*command, "init", project))
        result = json.loads(run(*command, "-p", project, "validate"))
        if result.get("ok") is not True:
            raise ValueError("packaged binary project validation failed")


def package(target, dest, runner=()):
    v = check()
    host = next(line[6:] for line in run("rustc", "-vV").splitlines() if line.startswith("host: "))
    if target != host and not runner:
        raise ValueError("cross packaging requires --runner to smoke-test the target binary")
    subprocess.run(["cargo", "build", "--release", "--locked", "--target", target], cwd=ROOT, check=True)
    metadata = json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"))
    binary = Path(metadata["target_directory"]) / target / "release" / binary_name(target)
    smoke(binary, runner)
    name = f"cutcraft-v{v}-{target}"
    archive(dest / archive_name(target), name, [
        (binary_name(target), binary.read_bytes(), True),
        ("LICENSE", (ROOT / "LICENSE").read_bytes(), False),
        ("README.md", (ROOT / "docs/install.md").read_bytes(), False),
        ("CHANGELOG.md", (ROOT / "CHANGELOG.md").read_bytes(), False),
    ])


def assemble(dest, partial=False):
    v = check()
    expected = {archive_name(t) for t in TARGETS}
    actual = {p.name for p in dest.glob("cutcraft-v*")
              if p.name.endswith((".tar.gz", ".zip"))}
    if not actual or (not partial and actual != expected) or not actual <= expected:
        raise ValueError(f"unexpected/incomplete platform archives: {sorted(actual)}; expected {sorted(expected)}")
    files = [("LICENSE", (ROOT / "LICENSE").read_bytes(), False),
             ("README.md", (ROOT / "docs/install.md").read_bytes(), False),
             ("scripts/install-skill.sh", (ROOT / "scripts/install-skill.sh").read_bytes(), True)]
    for path in sorted((ROOT / "skills/cutcraft").rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts and not path.name.startswith("."):
            files.append((path.relative_to(ROOT).as_posix(), path.read_bytes(), False))
    skill = f"cutcraft-skill-v{v}.tar.gz"
    archive(dest / skill, f"cutcraft-skill-v{v}", files)
    # The installer is distributed from the repository, independently of releases.
    # Remove the generated asset left by older local assembly runs.
    (dest / "install.sh").unlink(missing_ok=True)
    (dest / "RELEASE_NOTES.md").write_text(notes(), encoding="utf-8", newline="\n")
    names = sorted(actual | {skill, "RELEASE_NOTES.md"})
    (dest / "SHA256SUMS").write_text("".join(
        hashlib.sha256((dest / name).read_bytes()).hexdigest() + "  " + name + "\n" for name in names),
        encoding="utf-8", newline="\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("check")
    p.add_argument("--tag")
    p = commands.add_parser("package")
    p.add_argument("--target", required=True, choices=TARGETS)
    p.add_argument("--dist", type=Path, default=ROOT / "dist")
    p.add_argument("--runner", help="emulator command, e.g. 'qemu-aarch64 -L /usr/aarch64-linux-gnu'")
    p = commands.add_parser("assemble")
    p.add_argument("--dist", type=Path, default=ROOT / "dist")
    p.add_argument("--partial", action="store_true", help="local rehearsal only; allow missing platforms")
    args = parser.parse_args()
    if args.command == "check":
        print(check(args.tag))
    else:
        args.dist.mkdir(parents=True, exist_ok=True)
        if args.command == "package":
            runner = shlex.split(args.runner) if args.runner else []
            package(args.target, args.dist, runner)
        else:
            assemble(args.dist, args.partial)
        print(args.dist.resolve())


if __name__ == "__main__":
    try:
        main()
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error

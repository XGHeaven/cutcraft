#!/usr/bin/env python3
"""Exercise real release packages and the installer in temporary directories."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def invoke(*args, ok=True, contains=None, **kwargs):
    result = subprocess.run([str(x) for x in args], capture_output=True, text=True, **kwargs)
    if (result.returncode == 0) != ok:
        raise AssertionError(f"unexpected exit {result.returncode}: {args}\n{result.stdout}\n{result.stderr}")
    if contains and contains not in result.stdout + result.stderr:
        raise AssertionError(f"missing {contains!r}: {result.stdout}\n{result.stderr}")
    return result.stdout


def test(dist):
    v = release.check()
    host = next(line[6:] for line in release.run("rustc", "-vV").splitlines() if line.startswith("host: "))
    asset = dist / f"cutcraft-v{v}-{host}.tar.gz"
    installer = ROOT / "scripts/install.sh"
    sums = dist / "SHA256SUMS"
    for line in sums.read_text().splitlines():
        expected, name = line.split("  ", 1)
        assert hashlib.sha256((dist / name).read_bytes()).hexdigest() == expected, name
    assert not (dist / "install.sh").exists()
    assert "install.sh" not in sums.read_text()
    with tarfile.open(asset) as tar:
        entries = tar.getmembers()
        assert len(entries) == 4 and all(e.isfile() for e in entries)
        assert {Path(e.name).name for e in entries} == {"cutcraft", "LICENSE", "README.md", "CHANGELOG.md"}
    invoke("python3", ROOT / "scripts/release.py", "check", "--tag", "v" + v)
    invoke("python3", ROOT / "scripts/release.py", "check", "--tag", "v999.0.0", ok=False, contains="must equal")
    with tempfile.TemporaryDirectory(prefix="cutcraft release test ") as directory:
        tmp = Path(directory)
        bin_dir = tmp / "bin with spaces"
        base = ["sh", installer, "--version", v, "--archive", asset,
                "--checksums", sums, "--bin-dir", bin_dir]
        invoke(*base)
        binary = bin_dir / "cutcraft"
        original = binary.read_bytes()
        assert invoke(binary, "--version").strip() == "cutcraft " + v
        json.loads(invoke(binary, "capabilities"))
        project = tmp / "fresh project.cut"
        invoke(binary, "init", project)
        invoke(binary, "-p", project, "validate")
        assert json.loads(invoke(binary, "-p", project, "inspect"))
        # Exercise the shipped executable and its external media dependencies, not cargo run.
        source = tmp / "red source.mp4"
        invoke("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=c=red:s=32x24:r=4:d=1",
               "-c:v", "libx264", "-pix_fmt", "yuv420p", "-color_primaries", "bt709",
               "-color_trc", "bt709", "-colorspace", "bt709", source)
        invoke(binary, "-p", project, "import", source, "--managed", "--append", "--color-space", "rec709")
        invoke(binary, "-p", project, "validate", "--level", "media")
        preview = json.loads(invoke(binary, "-p", project, "preview"))
        video = Path(preview["video"])
        assert video.is_file()
        streams = json.loads(invoke("ffprobe", "-v", "error", "-show_streams", "-of", "json", video))["streams"]
        visual = next(s for s in streams if s["codec_type"] == "video")
        assert visual["codec_name"] == "h264" and max(visual["width"], visual["height"]) <= 640
        assert abs(float(visual["duration"]) - 1.0) < 0.05
        installed_stat = binary.stat()
        invoke(*base, contains="already installed")
        assert binary.stat().st_mtime_ns == installed_stat.st_mtime_ns
        assert binary.stat().st_ino == installed_stat.st_ino
        invoke(*base, "--force")
        assert binary.read_bytes() == original
        # Recognized installations update without --force, including explicit version switches.
        for previous in ["0.0.1", "999.0.0", "0.1.0-preview.1"]:
            binary.write_text(f'#!/bin/sh\nprintf "cutcraft {previous}\\n"\n')
            invoke(*base, contains=f"Updated cutcraft {previous} -> {v}")
            assert binary.read_bytes() == original
        # Other files and symlinks still need an explicit replacement request.
        for content in [b"unrecognized", b'#!/bin/sh\nprintf "cutcraft 0.0.1\\nextra\\n"\n']:
            binary.write_bytes(content)
            invoke(*base, ok=False, contains="not a recognized Cutcraft")
            assert binary.read_bytes() == content
            invoke(*base, "--force")
            assert binary.read_bytes() == original
        binary.unlink()
        linked = tmp / "linked cutcraft"
        linked.write_bytes(original)
        linked.chmod(0o755)
        binary.symlink_to(linked)
        invoke(*base, ok=False, contains="not a recognized Cutcraft")
        assert binary.is_symlink()
        invoke(*base, "--force")
        assert not binary.is_symlink() and binary.read_bytes() == original
        assert linked.read_bytes() == original
        binary.unlink()
        binary.mkdir()
        for args in [[], ["--force"]]:
            invoke(*base, *args, ok=False, contains="Destination is a directory")
        binary.rmdir()
        invoke(*base)
        damaged = tmp / asset.name
        damaged.write_bytes(asset.read_bytes() + b"corruption")
        invoke("sh", installer, "--version", v, "--archive", damaged,
               "--checksums", sums, "--bin-dir", bin_dir, "--force", ok=False, contains="SHA-256 mismatch")
        assert binary.read_bytes() == original
        wrong_sums = tmp / "wrong-sums"
        wrong_sums.write_text("")
        invoke("sh", installer, "--version", v, "--archive", asset,
               "--checksums", wrong_sums, "--bin-dir", bin_dir, "--force", ok=False, contains="Missing or duplicate")
        invoke("sh", installer, "--version", "../invalid", ok=False, contains="valid --version")
        invoke("sh", installer, "--version", v, "--archive", asset, ok=False, contains="requires --checksums")
        # A checksum-valid archive with the wrong binary version must preserve the install.
        release.archive(damaged, f"cutcraft-v{v}-{host}", [("cutcraft", b"#!/bin/sh\necho cutcraft 999.0.0\n", True)])
        wrong_sums.write_text(hashlib.sha256(damaged.read_bytes()).hexdigest() + "  " + damaged.name + "\n")
        invoke("sh", installer, "--version", v, "--archive", damaged,
               "--checksums", wrong_sums, "--bin-dir", bin_dir, "--force", ok=False, contains="version does not match")
        assert binary.read_bytes() == original
        # Exercise URL construction/download handling without a public release or network.
        fake_bin = tmp / "fake-bin"
        fake_bin.mkdir()
        fake_curl = fake_bin / "curl"
        fake_curl.write_text('''#!/bin/sh
set -eu
url=''
output=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    https://github.com/*) url=$1; shift ;;
    -o) output=$2; shift 2 ;;
    *) shift ;;
  esac
done
printf '%s\\n' "$url" >> "$CUTCRAFT_TEST_LOG"
case "$url" in
  */releases/latest)
    [ "${CUTCRAFT_TEST_FAIL:-}" != latest ] || exit 22
    printf '%s' "$CUTCRAFT_TEST_LATEST"
    exit 0 ;;
  */SHA256SUMS)
    [ "${CUTCRAFT_TEST_FAIL:-}" != checksums ] || exit 22 ;;
  */releases/download/v*)
    [ "${CUTCRAFT_TEST_FAIL:-}" != archive ] || exit 22 ;;
  *) exit 22 ;;
esac
cp "$CUTCRAFT_TEST_DIST/${url##*/}" "$output"
''')
        fake_curl.chmod(0o755)
        log = tmp / "download.log"
        release_url = f"https://github.com/example/cutcraft/releases/download/v{v}"
        latest_url = "https://github.com/example/cutcraft/releases/latest"
        downloads = [release_url + "/" + asset.name, release_url + "/SHA256SUMS"]
        env = dict(os.environ, PATH=str(fake_bin) + os.pathsep + os.environ["PATH"],
                   CUTCRAFT_TEST_DIST=str(dist), CUTCRAFT_TEST_LOG=str(log),
                   CUTCRAFT_TEST_LATEST=f"https://github.com/example/cutcraft/releases/tag/v{v}")
        # An interrupted pipe must not run a partially downloaded installer,
        # even if the received prefix contains the final binary replacement.
        script = installer.read_text()
        interrupted_dir = tmp / "interrupted pipe"
        for end in [script.index('case "$version"'), script.index("printf 'Installed")]:
            log.write_text("")
            invoke("sh", "-s", "--", "--bin-dir", interrupted_dir,
                   input=script[:end], env=env, ok=False)
            assert log.read_text() == ""
            assert not interrupted_dir.exists()
        invoke("sh", "-s", "--", "--help", input=script, contains="sh -s --")
        invoke("sh", installer, "--version", "v" + v, "--repo", "example/cutcraft", "--bin-dir", tmp / "downloaded", env=env)
        assert log.read_text().splitlines() == downloads
        # The official repository works without --repo, for both latest and pinned installs.
        official = "https://github.com/XGHeaven/cutcraft/releases"
        official_env = dict(env, CUTCRAFT_TEST_LATEST=f"{official}/tag/v{v}")
        for args in [[], ["--version", v]]:
            log.write_text("")
            invoke("sh", installer, "--bin-dir", bin_dir, "--force", *args, env=official_env)
            expected_urls = ([] if args else [official + "/latest"]) + [
                f"{official}/download/v{v}/{asset.name}", f"{official}/download/v{v}/SHA256SUMS"]
            assert log.read_text().splitlines() == expected_urls
            assert binary.read_bytes() == original
        for args in [[], ["--version", "latest"]]:
            log.write_text("")
            binary.write_text('#!/bin/sh\nprintf "cutcraft 0.0.1\\n"\n')
            # Also support downloading the script and passing it to sh on stdin.
            invoke("sh", "-s", "--", "--repo", "example/cutcraft", "--bin-dir", bin_dir,
                   *args, input=installer.read_text(), env=env, contains=f"Updated cutcraft 0.0.1 -> {v}")
            assert log.read_text().splitlines() == [latest_url, *downloads]
            assert binary.read_bytes() == original
            log.write_text("")
            invoke("sh", "-s", "--", "--repo", "example/cutcraft", "--bin-dir", bin_dir,
                   *args, input=script, env=env, contains="already installed")
            assert log.read_text().splitlines() == [latest_url]
        log.write_text("")
        invoke("sh", "-s", "--", "--version", v, "--bin-dir", bin_dir,
               input=script, env=official_env, contains="already installed")
        assert log.read_text() == ""
        assert binary.read_bytes() == original
        invoke("sh", "-s", "--", "--version", v, "--bin-dir", bin_dir,
               "--force", input=script, env=official_env)
        assert binary.read_bytes() == original
        invoke("sh", "-s", "--", "--version", v, "--archive", damaged,
               "--checksums", sums, "--bin-dir", bin_dir, "--force",
               input=script, ok=False, contains="SHA-256 mismatch")
        assert binary.read_bytes() == original
        binary.write_text('#!/bin/sh\nprintf "cutcraft 0.0.1\\n"\n')
        old_install = binary.read_bytes()
        for mode, message in [("latest", "Cannot resolve latest"), ("archive", "Could not download"),
                              ("checksums", "Could not download SHA256SUMS")]:
            invoke("sh", installer, "--repo", "example/cutcraft", "--bin-dir", bin_dir,
                   env=dict(env, CUTCRAFT_TEST_FAIL=mode), ok=False, contains=message)
            assert binary.read_bytes() == old_install
        invoke("sh", installer, "--version", v, "--archive", damaged,
               "--checksums", sums, "--bin-dir", bin_dir, ok=False, contains="SHA-256 mismatch")
        assert binary.read_bytes() == old_install
        invoke(*base)
        assert binary.read_bytes() == original
        for url in [latest_url, "https://example.org/releases/tag/v" + v,
                    "https://github.com/example/cutcraft/releases/tag/v../invalid"]:
            log.write_text("")
            invoke("sh", installer, "--repo", "example/cutcraft", "--bin-dir", bin_dir, "--force",
                   env=dict(env, CUTCRAFT_TEST_LATEST=url), ok=False, contains="release")
            assert log.read_text().splitlines() == [latest_url]
            assert binary.read_bytes() == original
        invoke("sh", installer, "--archive", asset, "--checksums", sums,
               ok=False, contains="explicit --version")
        invoke("sh", installer, "--archive", asset, "--checksums", sums, "--version", "latest",
               ok=False, contains="explicit --version")
        # Platform rejection must happen before contacting GitHub.
        fake_uname = fake_bin / "uname"
        for machine, target in [("aarch64", "aarch64-unknown-linux-gnu"),
                                ("arm64", "aarch64-unknown-linux-gnu"),
                                ("armv7l", "armv7-unknown-linux-gnueabihf"),
                                ("armv8l", "armv7-unknown-linux-gnueabihf")]:
            fake_uname.write_text(f'#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo {machine} ;; esac\n')
            fake_uname.chmod(0o755)
            log.write_text("")
            invoke("sh", installer, "--version", v, "--repo", "example/cutcraft",
                   "--bin-dir", bin_dir, "--force", env=dict(env, CUTCRAFT_TEST_FAIL="archive"),
                   ok=False, contains="Could not download")
            assert log.read_text().splitlines() == [
                f"https://github.com/example/cutcraft/releases/download/v{v}/cutcraft-v{v}-{target}.tar.gz"]
            assert binary.read_bytes() == original
        fake_uname.write_text("#!/bin/sh\nprintf 'unsupported\\n'\n")
        fake_uname.chmod(0o755)
        log.write_text("")
        invoke("sh", installer, "--repo", "example/cutcraft", env=env,
               ok=False, contains="Supported platforms")
        assert log.read_text() == ""
        empty = tmp / "empty-dist"
        invoke("python3", ROOT / "scripts/release.py", "assemble", "--dist", empty,
               ok=False, contains="incomplete platform")
        partial = tmp / "partial-dist"
        partial.mkdir()
        shutil.copy(asset, partial / asset.name)
        invoke("python3", ROOT / "scripts/release.py", "assemble", "--dist", partial,
               ok=False, contains="incomplete platform")
        # Reassembling an older local bundle removes the retired installer asset.
        (partial / "install.sh").write_text("old generated installer\n")
        invoke("python3", ROOT / "scripts/release.py", "assemble", "--dist", partial, "--partial")
        assert not (partial / "install.sh").exists()
        assert "install.sh" not in (partial / "SHA256SUMS").read_text()
        skill_name = f"cutcraft-skill-v{v}"
        with tarfile.open(dist / (skill_name + ".tar.gz")) as tar:
            for entry in tar.getmembers():
                assert entry.isfile() and entry.name.startswith(skill_name + "/")
                assert not Path(entry.name).is_absolute() and ".." not in Path(entry.name).parts
                dest = tmp / entry.name
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_bytes(tar.extractfile(entry).read())
        skill_installer = tmp / skill_name / "scripts/install-skill.sh"
        skill_dir = tmp / "agent skills"
        invoke("sh", skill_installer, skill_dir)
        assert (skill_dir / "cutcraft/SKILL.md").read_bytes() == (ROOT / "skills/cutcraft/SKILL.md").read_bytes()
        invoke("sh", skill_installer, skill_dir, ok=False, contains="Already exists")
        # Uninstall is local, idempotent and limited to the selected executable.
        uninstall_args = ["--uninstall", "--bin-dir", bin_dir]
        neighbor = bin_dir / "keep.txt"
        neighbor.write_text("keep")
        preserved = [project / "cutcraft.json", source, video, neighbor,
                     skill_dir / "cutcraft/SKILL.md"]
        before_uninstall = {path: path.read_bytes() for path in preserved}
        for option in ["--version", "--repo", "--archive", "--checksums"]:
            invoke("sh", installer, *uninstall_args, option, "unused",
                   ok=False, contains="cannot be combined")
            assert binary.read_bytes() == original
        # Even a prefix containing the removal code must not uninstall anything.
        invoke("sh", "-s", "--", *uninstall_args,
               input=script[:script.index('case "$version"')], ok=False)
        assert binary.read_bytes() == original
        log.write_text("")
        invoke("sh", "-s", "--", *uninstall_args, input=script, env=env,
               contains="Uninstalled cutcraft")
        assert not binary.exists() and bin_dir.is_dir()
        assert log.read_text() == ""  # No downloads; fake uname is also unsupported here.
        invoke("sh", installer, *uninstall_args, env=env, contains="not installed")
        for path, content in before_uninstall.items():
            assert path.read_bytes() == content
        missing_dir = tmp / "not created"
        invoke("sh", installer, "--uninstall", "--bin-dir", missing_dir, contains="not installed")
        assert not missing_dir.exists()
        binary.write_text("not cutcraft")
        invoke("sh", installer, *uninstall_args, ok=False, contains="not a recognized Cutcraft")
        assert binary.read_text() == "not cutcraft"
        invoke("sh", installer, *uninstall_args, "--force", contains="Uninstalled cutcraft")
        for target in [linked, tmp / "missing target"]:
            binary.symlink_to(target)
            invoke("sh", installer, *uninstall_args, ok=False, contains="not a recognized Cutcraft")
            assert binary.is_symlink()
            invoke("sh", installer, *uninstall_args, "--force")
            assert not binary.is_symlink()
            assert linked.read_bytes() == original
        binary.mkdir()
        marker = binary / "keep.txt"
        marker.write_text("keep")
        for args in [[], ["--force"]]:
            invoke("sh", installer, *uninstall_args, *args, ok=False, contains="Destination is a directory")
            assert marker.read_text() == "keep"
        assert not list(bin_dir.glob(".cutcraft-install.*"))
    print("Release checks passed: archives, version/tag, installed CLI/media preview, automatic updates/version switches, same-version no-op, corruption/overwrite protection, file/piped installs, interrupted script downloads, pinned/latest downloads, network failures, platform guard, skill installation, local/idempotent uninstall and retained user files.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=ROOT / "dist")
    test(parser.parse_args().dist.resolve())

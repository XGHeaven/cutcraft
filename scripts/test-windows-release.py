#!/usr/bin/env python3
"""Windows native package, PowerShell installer and media smoke checks."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import zipfile

import release


def invoke(*args, ok=True, contains=None):
    result = subprocess.run([str(x) for x in args], capture_output=True, text=True, encoding="utf-8")
    assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
    if contains:
        assert contains in result.stdout + result.stderr, (result.stdout, result.stderr)
    return result.stdout


def test(dist):
    v = release.check()
    archive = dist / release.archive_name("x86_64-pc-windows-msvc")
    sums = dist / "SHA256SUMS"
    for line in sums.read_text(encoding="utf-8").splitlines():
        checksum, name = line.split("  ")
        assert hashlib.sha256((dist / name).read_bytes()).hexdigest() == checksum
    with zipfile.ZipFile(archive) as package:
        assert len(package.infolist()) == 4
        assert {Path(entry.filename).name for entry in package.infolist()} == {
            "cutcraft.exe", "LICENSE", "README.md", "CHANGELOG.md"}
    with tempfile.TemporaryDirectory(prefix="cutcraft Windows test ") as directory:
        root = Path(directory)
        bin_dir = root / "bin with spaces"
        binary = bin_dir / "cutcraft.exe"
        # Exercise both supported shells, including the Windows default 5.1.
        for shell in ["powershell.exe", "pwsh"]:
            base = [shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
                    release.ROOT / "scripts/install.ps1", "-Version", v,
                    "-Archive", archive, "-Checksums", sums, "-BinDir", bin_dir]
            if binary.exists():
                binary.unlink()
            invoke(*base, contains="Installed cutcraft")
            original = binary.read_bytes()
            invoke(*base, ok=False, contains="Already exists")
            invoke(*base, "-Force")
            assert binary.read_bytes() == original
            bad = root / archive.name
            bad.write_bytes(archive.read_bytes() + b"corrupt")
            invoke(*base[:base.index("-Archive")], "-Archive", bad, "-Checksums", sums,
                   "-BinDir", bin_dir, "-Force", ok=False, contains="SHA-256 mismatch")
            wrong_sums = root / "bad-checksums"
            wrong_sums.write_text(sums.read_text(encoding="utf-8") * 2, encoding="utf-8")
            invoke(*base[:base.index("-Checksums")], "-Checksums", wrong_sums,
                   "-BinDir", bin_dir, "-Force", ok=False, contains="Missing or duplicate checksum")
            wrong_name = "cutcraft-v999.0.0-x86_64-pc-windows-msvc"
            wrong_archive = root / (wrong_name + ".zip")
            release.archive(wrong_archive, wrong_name, [("cutcraft.exe", original, True)])
            wrong_sums.write_text(hashlib.sha256(wrong_archive.read_bytes()).hexdigest()
                                 + "  " + wrong_archive.name + "\n", encoding="utf-8")
            invoke(*base[:base.index("-Version")], "-Version", "999.0.0",
                   "-Archive", wrong_archive, "-Checksums", wrong_sums, "-BinDir", bin_dir,
                   "-Force", ok=False, contains="Binary version does not match")
            assert binary.read_bytes() == original
            assert not list(bin_dir.glob(".cutcraft-install-*"))
        release.smoke(binary)
        # Exercise actual file replacement and rendering on Windows, including
        # spaces/non-ASCII in the project path.
        project = root / "demo 空格.cut"
        invoke(binary, "init", project)
        source = root / "source.mp4"
        invoke("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=c=red:s=32x24:r=4:d=1",
               "-c:v", "libx264", "-pix_fmt", "yuv420p", "-color_primaries", "bt709",
               "-color_trc", "bt709", "-colorspace", "bt709", source)
        invoke(binary, "-p", project, "import", source, "--managed", "--append", "--color-space", "rec709")
        invoke(binary, "-p", project, "validate", "--level", "media")
        preview = json.loads(invoke(binary, "-p", project, "preview"))
        assert Path(preview["video"]).is_file()
        streams = json.loads(invoke("ffprobe", "-v", "error", "-show_streams", "-of", "json", preview["video"]))["streams"]
        visual = next(s for s in streams if s["codec_type"] == "video")
        assert visual["codec_name"] == "h264" and max(visual["width"], visual["height"]) <= 640
        assert abs(float(visual["duration"]) - 1) < .05
        # Test online URL pinning without depending on a published Windows asset.
        invoke("pwsh", "-NoProfile", "-File", release.ROOT / "scripts/test-windows-download.ps1",
               "-Archive", archive, "-Checksums", sums, "-Version", v, "-BinDir", root / "online")
    print("Windows package/install/overwrite/checksum/download/media checks passed.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, required=True)
    test(parser.parse_args().dist.resolve())

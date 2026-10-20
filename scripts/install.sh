#!/bin/sh
# No Rust/Python required. Verify a release archive before installing its binary.
set -eu
usage() {
    cat <<'EOF'
Usage:
  install.sh [--repo OWNER/REPO] [--version VERSION|latest] [--bin-dir DIR] [--force]
  install.sh --archive FILE --checksums FILE --version VERSION [--bin-dir DIR] [--force]

Online installs default to the latest stable Release from XGHeaven/cutcraft.
VERSION may have a leading v. Offline installs require an explicit VERSION.
Default destination: $HOME/.local/bin.
Existing installations are preserved unless --force is supplied.
FFmpeg / ffprobe and the Agent skill are installed separately.
EOF
}
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
repo='' version='' archive='' checksums='' force=false
bin_dir=${HOME}/.local/bin
while [ "$#" -gt 0 ]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --force) force=true; shift ;;
        --repo|--version|--archive|--checksums|--bin-dir)
            [ "$#" -ge 2 ] || fail "Missing value for $1"
            [ -n "$2" ] || fail "Missing value for $1"
            case "$1" in
                --repo) repo=$2 ;;
                --version) version=${2#v} ;;
                --archive) archive=$2 ;;
                --checksums) checksums=$2 ;;
                --bin-dir) bin_dir=$2 ;;
            esac
            shift 2 ;;
        *) fail "Unknown argument: $1" ;;
    esac
done
valid_version() {
    printf '%s\n' "$1" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$'
}
case "$version" in
    ''|latest) ;;
    *) valid_version "$version" || fail 'Provide a valid --version' ;;
esac
if [ -n "$archive" ]; then
    [ -z "$repo" ] || fail '--archive and --repo are mutually exclusive'
    [ -n "$checksums" ] || fail '--archive requires --checksums'
    valid_version "$version" || fail 'Offline installation requires an explicit --version'
else
    [ -z "$checksums" ] || fail '--checksums requires --archive'
    repo=${repo:-XGHeaven/cutcraft}
    printf '%s\n' "$repo" | LC_ALL=C grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' || fail 'Provide --repo OWNER/REPO'
    command -v curl >/dev/null 2>&1 || fail 'curl is required for downloads'
fi
case "$(uname -s)/$(uname -m)" in
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    *) fail 'Supported platforms: Linux x86_64, macOS arm64/x86_64' ;;
esac
# Resolve latest once, then pin both downloads to the same immutable tag name.
# No JSON parser or GitHub API token is needed for public releases.
fetch() {
    curl --fail --location --silent --show-error --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 300 --retry 2 "$@"
}
if [ -z "$archive" ] && { [ -z "$version" ] || [ "$version" = latest ]; }; then
    printf 'Resolving latest release from %s...\n' "$repo" >&2
    resolved=$(fetch "https://github.com/$repo/releases/latest" -o /dev/null --write-out '%{url_effective}') ||
        fail "Cannot resolve latest release for $repo; check the repository, published releases and network"
    prefix=https://github.com/${repo}/releases/tag/v
    case "$resolved" in
        "$prefix"*) version=${resolved#"$prefix"} ;;
        *) fail "Unexpected latest release URL: $resolved; use --version to select a vVERSION tag" ;;
    esac
    valid_version "$version" || fail 'Latest release must use a valid vVERSION tag; use --version to select another release'
fi
name=cutcraft-v${version}-${target}
asset=${name}.tar.gz
tmp=$(mktemp -d "${TMPDIR:-/tmp}/cutcraft-install.XXXXXX")
staged=
cleanup() {
    [ -z "$staged" ] || rm -f -- "$staged"
    rm -rf -- "$tmp"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
if [ -n "$archive" ]; then
    [ "$(basename -- "$archive")" = "$asset" ] || fail "Expected archive $asset"
    cp -- "$archive" "$tmp/$asset"
    cp -- "$checksums" "$tmp/SHA256SUMS"
else
    base=https://github.com/${repo}/releases/download/v${version}
    printf 'Downloading cutcraft %s (%s)...\n' "$version" "$target" >&2
    fetch "$base/$asset" -o "$tmp/$asset" || fail "Could not download $asset; check the release assets and network"
    fetch "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || fail 'Could not download SHA256SUMS; installation aborted'
fi
expected=$(awk -v name="$asset" '$2 == name {print $1}' "$tmp/SHA256SUMS")
[ "${#expected}" -eq 64 ] || fail 'Missing or duplicate checksum'
case "$expected" in *[!0-9a-f]*) fail 'Malformed SHA-256 checksum' ;; esac
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
else
    fail 'sha256sum or shasum is required'
fi
[ "$actual" = "$expected" ] || fail 'SHA-256 mismatch; installation aborted'
# Extract only the expected binary to stdout, never archive paths into the filesystem.
tar -xzOf "$tmp/$asset" "$name/cutcraft" > "$tmp/cutcraft"
chmod 755 "$tmp/cutcraft"
[ "$("$tmp/cutcraft" --version)" = "cutcraft $version" ] || fail 'Binary cannot run or version does not match'
mkdir -p -- "$bin_dir"
if [ -e "$bin_dir/cutcraft" ] || [ -L "$bin_dir/cutcraft" ]; then
    [ "$force" = true ] || fail 'cutcraft already exists; use --force to replace it'
    [ ! -d "$bin_dir/cutcraft" ] || fail 'Destination is a directory'
fi
staged=$(mktemp "$bin_dir/.cutcraft-install.XXXXXX")
cp -- "$tmp/cutcraft" "$staged"
chmod 755 "$staged"
mv -f -- "$staged" "$bin_dir/cutcraft"
staged=
printf 'Installed cutcraft %s to %s/cutcraft\n' "$version" "$bin_dir"
printf 'Add this directory to PATH if needed: %s\n' "$bin_dir"
for dependency in ffmpeg ffprobe; do
    if ! command -v "$dependency" >/dev/null 2>&1; then
        printf 'Media operations additionally require %s.\n' "$dependency"
    fi
done

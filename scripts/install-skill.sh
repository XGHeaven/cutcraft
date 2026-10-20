#!/bin/sh
# Install the distributable skill; the CLI and FFmpeg are separate dependencies.
set -eu
if [ "$#" -gt 1 ]; then
    printf '%s\n' 'Usage: scripts/install-skill.sh [skills-directory]' >&2
    exit 2
fi
if [ "${1:-}" = '--help' ] || [ "${1:-}" = '-h' ]; then
    # Show the expression literally so the default is understandable on any machine.
    # shellcheck disable=SC2016
    printf '%s\n' 'Usage: scripts/install-skill.sh [skills-directory]' \
        'Default: ${CODEX_HOME:-$HOME/.codex}/skills' \
        'Installs a new cutcraft skill; refuses to overwrite an existing installation.'
    exit 0
fi
repo_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
source_dir="$repo_dir/skills/cutcraft"
if [ ! -f "$source_dir/SKILL.md" ]; then
    printf 'Missing skill source: %s\n' "$source_dir/SKILL.md" >&2
    exit 1
fi
skills_dir=${1:-${CODEX_HOME:-$HOME/.codex}/skills}
# Absolute target makes invocation independent of the caller's working directory.
mkdir -p -- "$skills_dir"
skills_dir=$(CDPATH='' cd -- "$skills_dir" && pwd)
target_dir="$skills_dir/cutcraft"
if [ -e "$target_dir" ] || [ -L "$target_dir" ]; then
    printf 'Already exists; left unchanged: %s\n' "$target_dir" >&2
    exit 1
fi
# mkdir also prevents two installers from taking ownership of the same target.
mkdir -- "$target_dir"
if ! cp -R "$source_dir/." "$target_dir/"; then
    printf 'Copy failed; inspect the incomplete installation: %s\n' "$target_dir" >&2
    exit 1
fi
printf 'Installed skill: %s\n' "$target_dir"
# $cutcraft is a literal skill invocation, not a shell variable.
# shellcheck disable=SC2016
printf '%s\n' 'Use $cutcraft in a new agent session.' \
    'Requires the Cutcraft CLI and ffmpeg/ffprobe for media work.'

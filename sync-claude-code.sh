#!/usr/bin/env bash
# Materialize active skills from manifest.json into ~/.claude/skills/ as symlinks.
#
# Usage:
#   bin/sync-claude-code.sh                          # all skills matching this host
#   bin/sync-claude-code.sh --tags homelab,personal  # filter by tag (any-match)
#   bin/sync-claude-code.sh --dry-run                # print plan, don't touch fs
#
# The script:
#   1. Reads manifest.json
#   2. Filters by enabled, agents (must include 'claude-code' or 'all'), and platform
#      (uname -s -> 'darwin' or 'linux' must be in the skill's platforms, or 'all')
#   3. Optionally further filters by --tags (skill must have at least one matching tag)
#   4. Symlinks each matching skill dir into ~/.claude/skills/<name>
#   5. Removes any existing symlink in ~/.claude/skills/ that points into this repo
#      but is no longer in the active set (so disabling a skill in the manifest
#      removes it from the agent on next sync)
#
# Existing non-symlink directories in ~/.claude/skills/ are left untouched — the
# script will skip and warn rather than overwrite a real directory.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$REPO_ROOT/manifest.json"
TARGET_DIR="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"

DRY_RUN=0
TAGS_FILTER=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=1; shift ;;
        --tags)    TAGS_FILTER="$2"; shift 2 ;;
        --tags=*)  TAGS_FILTER="${1#--tags=}"; shift ;;
        -h|--help)
            sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

if [[ ! -f "$MANIFEST" ]]; then
    echo "manifest not found: $MANIFEST" >&2
    exit 1
fi

OS_RAW="$(uname -s)"
case "$OS_RAW" in
    Darwin) HOST_PLATFORM="darwin" ;;
    Linux)  HOST_PLATFORM="linux"  ;;
    *)      HOST_PLATFORM="$(echo "$OS_RAW" | tr '[:upper:]' '[:lower:]')" ;;
esac

mkdir -p "$TARGET_DIR"

ACTIVE_NAMES=()

while IFS=$'\t' read -r name path; do
    ACTIVE_NAMES+=("$name")
    src="$REPO_ROOT/$path"
    dst="$TARGET_DIR/$name"

    if [[ ! -d "$src" ]]; then
        echo "  SKIP $name (source missing: $src)" >&2
        continue
    fi

    if [[ -L "$dst" ]]; then
        existing="$(readlink "$dst")"
        if [[ "$existing" == "$src" ]]; then
            echo "  ok   $name (already linked)"
            continue
        fi
        if (( DRY_RUN )); then
            echo "  PLAN relink $name -> $src (was: $existing)"
        else
            rm "$dst"
            ln -s "$src" "$dst"
            echo "  link $name -> $src (was: $existing)"
        fi
    elif [[ -e "$dst" ]]; then
        echo "  WARN $name exists as non-symlink at $dst — leaving alone" >&2
        continue
    else
        if (( DRY_RUN )); then
            echo "  PLAN link $name -> $src"
        else
            ln -s "$src" "$dst"
            echo "  link $name -> $src"
        fi
    fi
done < <(
    REPO_ROOT="$REPO_ROOT" \
    HOST_PLATFORM="$HOST_PLATFORM" \
    TAGS_FILTER="$TAGS_FILTER" \
    python3 - "$MANIFEST" <<'PY'
import json, os, sys

manifest_path = sys.argv[1]
host = os.environ["HOST_PLATFORM"]
tags_filter = [t.strip() for t in os.environ.get("TAGS_FILTER", "").split(",") if t.strip()]

with open(manifest_path) as f:
    m = json.load(f)

for s in m.get("skills", []):
    if not s.get("enabled", True):
        continue
    agents = s.get("agents", ["all"])
    if "all" not in agents and "claude-code" not in agents:
        continue
    platforms = s.get("platforms", ["all"])
    if "all" not in platforms and host not in platforms:
        continue
    if tags_filter:
        skill_tags = set(s.get("tags", []))
        if not skill_tags.intersection(tags_filter):
            continue
    print(f"{s['name']}\t{s['path']}")
PY
)

shopt -s nullglob
for entry in "$TARGET_DIR"/*; do
    [[ -L "$entry" ]] || continue
    target="$(readlink "$entry")"
    case "$target" in
        "$REPO_ROOT"/*) ;;
        *) continue ;;
    esac
    name="$(basename "$entry")"
    keep=0
    for active in "${ACTIVE_NAMES[@]}"; do
        if [[ "$active" == "$name" ]]; then keep=1; break; fi
    done
    if (( keep == 0 )); then
        if (( DRY_RUN )); then
            echo "  PLAN remove stale symlink $name"
        else
            rm "$entry"
            echo "  rm   stale symlink $name"
        fi
    fi
done

echo
echo "host=$HOST_PLATFORM  active=${#ACTIVE_NAMES[@]}  target=$TARGET_DIR"
(( DRY_RUN )) && echo "(dry run — no changes made)"

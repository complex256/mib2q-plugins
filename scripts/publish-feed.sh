#!/usr/bin/env bash
# Publish built .mpk files to the official mibpkg collection.
#
#   scripts/publish-feed.sh <tag> <mpk>...
#
# The collection is complex256/mibpkg-packages. Each release carries the previous entries
# forward, so this adds/updates only the packages given here. The feed's sources.json is
# attached to every release; without it new cars would not see the official collection.
#
# mibpkg must be on PATH, or set MIBPKG=/path/to/mibpkg. Publishing uses gh, so run
# `gh auth login` or set GH_TOKEN with contents:write on the collection repository.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MIBPKG=${MIBPKG:-mibpkg}
TAG=${1:?usage: publish-feed.sh <tag> <mpk>...}
shift
[ "$#" -gt 0 ] || { echo "usage: publish-feed.sh <tag> <mpk>..." >&2; exit 2; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Keep the existing list; fall back to the canonical copy while bootstrapping or after a
# release that predates this script.
url=https://github.com/complex256/mibpkg-packages/releases/latest/download/sources.json
curl -fsSL "$url" -o "$TMP/sources.json" ||
    curl -fsSL https://raw.githubusercontent.com/complex256/mibpkg/main/packaging/sources.json -o "$TMP/sources.json"

"$MIBPKG" release "$@" \
    --repo complex256/mibpkg-packages \
    --tag "$TAG" \
    --id complex256-mibpkg \
    --name mibpkg \
    --asset "$TMP/sources.json" \
    --out "$ROOT/out/release-$TAG" \
    --publish

"$MIBPKG" verify github:complex256/mibpkg-packages --latest

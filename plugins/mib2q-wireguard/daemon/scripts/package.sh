#!/bin/sh
# Build the mibpkg package for mib2q-wireguard.
#   scripts/package.sh [out-dir]
# Requires scripts/build.sh output and a host mibpkg (MIBPKG=...).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/build/mibwg
VERSION=$(cat "$ROOT/VERSION")
MIBPKG=${MIBPKG:-mibpkg}
OUT=${1:-$ROOT/build/mpk}
[ -f "$BIN" ] || { echo "missing $BIN - run scripts/build.sh first" >&2; exit 1; }
[ -x "$MIBPKG" ] || { echo "missing host mibpkg at $MIBPKG - pass MIBPKG=/path/to/mibpkg" >&2; exit 1; }
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/bin"
sed "s/@VERSION@/$VERSION/" "$ROOT/packaging/manifest.toml.in" > "$STAGE/manifest.toml"
cp "$BIN" "$STAGE/bin/mibwg"
chmod 755 "$STAGE/bin/mibwg"
cp "$ROOT/deploy/mibwg.sh" "$STAGE/bin/mibwg.sh"
chmod 755 "$STAGE/bin/mibwg.sh"
cp "$ROOT/packaging/mibwg.conf.example" "$STAGE/mibwg.conf.example"
mkdir -p "$OUT"
"$MIBPKG" build "$STAGE" "$OUT"
ls -l "$OUT"/wireguard-"$VERSION".mpk

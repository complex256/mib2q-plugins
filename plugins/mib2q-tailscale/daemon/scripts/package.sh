#!/bin/sh
# Build the mibpkg package for mib2q-tailscale.
#   scripts/package.sh [out-dir]
# Requires scripts/build.sh output and a host mibpkg (MIBPKG=...).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/build
VERSION=$(cat "$ROOT/VERSION")
MIBPKG=${MIBPKG:-mibpkg}
OUT=${1:-$ROOT/build/mpk}
[ -x "$BIN/tailscaled" ] || { echo "missing $BIN/tailscaled - run scripts/build.sh first" >&2; exit 1; }
[ -x "$MIBPKG" ] || { echo "missing host mibpkg at $MIBPKG - pass MIBPKG=/path/to/mibpkg" >&2; exit 1; }
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/bin"
sed "s/@VERSION@/$VERSION/" "$ROOT/packaging/manifest.toml.in" > "$STAGE/manifest.toml"
for b in tailscaled tailscale sntp tsstatus libtsclock.so; do
    cp "$BIN/$b" "$STAGE/bin/$b"
    chmod 755 "$STAGE/bin/$b"
done
cp "$ROOT/deploy/tailscale.sh" "$STAGE/bin/tailscale.sh"
chmod 755 "$STAGE/bin/tailscale.sh"
cp "$ROOT/packaging/authkey.example" "$STAGE/authkey.example"
cp "$ROOT/packaging/accept-routes.example" "$STAGE/accept-routes.example"
mkdir -p "$OUT"
"$MIBPKG" build "$STAGE" "$OUT"
ls -l "$OUT"/tailscale-"$VERSION".mpk

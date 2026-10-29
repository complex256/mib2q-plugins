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
PLUGIN_STAGE=${PLUGIN_STAGE:-$ROOT/../plugin/build/mibpkg-stage}
[ -x "$BIN/tailscaled" ] || { echo "missing $BIN/tailscaled - run scripts/build.sh first" >&2; exit 1; }
[ -f "$PLUGIN_STAGE/tailscale-status.jar" ] || { echo "missing $PLUGIN_STAGE/tailscale-status.jar - build the plugin package stage first" >&2; exit 1; }
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
cp "$PLUGIN_STAGE/tailscale-status.jar" "$STAGE/tailscale-status.jar"
cp "$PLUGIN_STAGE/tailscale-status.jar.stamp" "$STAGE/tailscale-status.jar.stamp"
mkdir -p "$STAGE/icons"
cp "$PLUGIN_STAGE"/icons/*.png "$STAGE/icons/"
mkdir -p "$OUT"
"$MIBPKG" build "$STAGE" "$OUT"
ls -l "$OUT"/tailscale-"$VERSION".mpk

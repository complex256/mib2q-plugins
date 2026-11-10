#!/usr/bin/env bash
# Build rsyncd-<version>.mpk for mibpkg.
#   scripts/package.sh [out-dir]    (default: target/mpk)
# Needs scripts/build.sh output and a host mibpkg (MIBPKG=..., default
# ../mibpkg/target/release/mibpkg).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/target/armv7-unknown-nto-qnx650/release
MIBPKG=${MIBPKG:-$ROOT/../mibpkg/target/release/mibpkg}
OUT=${1:-$ROOT/target/mpk}
PLUGIN_STAGE=${PLUGIN_STAGE:-$ROOT/../plugin/build/mibpkg-stage}
[ -f "$BIN/rsyncd" ] || { echo "missing $BIN/rsyncd - run scripts/build.sh first" >&2; exit 1; }
[ -f "$BIN/rsync" ] || { echo "missing $BIN/rsync - run scripts/build.sh first" >&2; exit 1; }
[ -f "$PLUGIN_STAGE/rsync-status.jar" ] || { echo "missing $PLUGIN_STAGE/rsync-status.jar - build the plugin package stage first" >&2; exit 1; }
[ -x "$MIBPKG" ] || { echo "missing $MIBPKG - cargo build --release in ../mibpkg" >&2; exit 1; }
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/bin" "$STAGE/icons"
sed "s/@VERSION@/$VERSION/" "$ROOT/packaging/manifest.toml.in" > "$STAGE/manifest.toml"
cp "$BIN/rsyncd" "$STAGE/bin/rsyncd"
cp "$BIN/rsync" "$STAGE/bin/rsync"
chmod 755 "$STAGE/bin/rsyncd" "$STAGE/bin/rsync"
cp "$ROOT/deploy/rsyncd.toml.example" "$STAGE/rsyncd.toml.example"
cp "$PLUGIN_STAGE/rsync-status.jar" "$STAGE/rsync-status.jar"
cp "$PLUGIN_STAGE/rsync-status.jar.stamp" "$STAGE/rsync-status.jar.stamp"
cp "$PLUGIN_STAGE/requires.toml" "$STAGE/requires.toml"
cp "$PLUGIN_STAGE"/icons/*.png "$STAGE/icons/"
mkdir -p "$OUT"
"$MIBPKG" build "$STAGE" "$OUT"
ls -l "$OUT"/rsyncd-"$VERSION".mpk

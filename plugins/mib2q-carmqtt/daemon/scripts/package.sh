#!/usr/bin/env bash
# Build carmqtt-<version>.mpk for mibpkg.
#   scripts/package.sh [out-dir]    (default: target/mpk)
# Needs scripts/build.sh output and a host mibpkg (MIBPKG=..., default ../mibpkg/target/release/mibpkg).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=$ROOT/target/armv7-unknown-nto-qnx650/release/carmqtt
MIBPKG=${MIBPKG:-$ROOT/../mibpkg/target/release/mibpkg}
OUT=${1:-$ROOT/target/mpk}
[ -f "$BIN" ] || { echo "missing $BIN - run scripts/build.sh first" >&2; exit 1; }
[ -x "$MIBPKG" ] || { echo "missing $MIBPKG - cargo build --release in ../mibpkg" >&2; exit 1; }
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/bin"
sed "s/@VERSION@/$VERSION/" "$ROOT/packaging/manifest.toml.in" > "$STAGE/manifest.toml"
cp "$BIN" "$STAGE/bin/carmqtt"
cp "$ROOT/deploy/carmqtt.toml.example" "$STAGE/carmqtt.toml.example"
"$MIBPKG" build "$STAGE" "$OUT"

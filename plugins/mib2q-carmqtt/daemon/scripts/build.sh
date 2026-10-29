#!/usr/bin/env bash
# Cross-build carmqtt for the MHI2q (QNX 6.5, armv7) in the qnx65-armv7-toolchain image.
#   scripts/build.sh            -> target/armv7-unknown-nto-qnx650/release/carmqtt
# Needs the image built with Rust (../qnx65-armv7-toolchain: host-scripts/qnx-run.sh build 8.5-rust).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMG=${QNX_IMAGE:-qnx65-armv7-toolchain:8.5-rust}
docker run --rm -v "$ROOT":/src/carmqtt -w /src "$IMG" build-std /src/carmqtt
ls -l "$ROOT/target/armv7-unknown-nto-qnx650/release/carmqtt"

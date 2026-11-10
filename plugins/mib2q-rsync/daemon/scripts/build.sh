#!/usr/bin/env bash
# Cross-build rsyncd and rsync for the MHI2q (QNX 6.5, armv7) in the
# qnx65-armv7-toolchain image.
#   scripts/build.sh
# Output:
#   target/armv7-unknown-nto-qnx650/release/rsyncd
#   target/armv7-unknown-nto-qnx650/release/rsync   (cached across rebuilds)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMG=${QNX_IMAGE:-qnx65-armv7-toolchain:8.5-rust}
RELEASE=$ROOT/target/armv7-unknown-nto-qnx650/release
mkdir -p "$RELEASE"

# 1. The Rust supervisor + upstream rsync.
#    In CI and normal Docker setups, `-v "$ROOT":/src/rsyncd` bind-mounts the
#    source directory directly. If the active Docker VM (e.g. a scoped Colima
#    instance) does not share $ROOT with the VM, fall back to streaming the
#    project in and the built binaries out over tar.
if docker run --rm -v "$ROOT":/src/rsyncd "$IMG" test -f /src/rsyncd/Cargo.toml; then
    docker run --rm -v "$ROOT":/src/rsyncd -w /src "$IMG" build-std /src/rsyncd
    ls -l "$RELEASE/rsyncd"
    if [ -x "$RELEASE/rsync" ]; then
        echo "using cached $RELEASE/rsync"
    else
        docker run --rm \
            -v "$ROOT":/src/rsyncd \
            -w /src/rsyncd \
            "$IMG" sh scripts/build-rsync.sh
    fi
else
    NEED_RSYNC=1
    [ -x "$RELEASE/rsync" ] && NEED_RSYNC=0
    tar -C "$ROOT" --exclude='./target' --exclude='./build' -cf - . |
        docker run --rm -i "$IMG" sh -euc '
            mkdir -p /work/rsyncd
            tar -C /work/rsyncd -xf -
            cd /work
            build-std /work/rsyncd >&2
            if [ "'"$NEED_RSYNC"'" = "1" ]; then
                cd /work/rsyncd
                sh scripts/build-rsync.sh /work/rsyncd/target/armv7-unknown-nto-qnx650/release >&2
                tar -C /work/rsyncd/target/armv7-unknown-nto-qnx650/release -cf - rsyncd rsync
            else
                tar -C /work/rsyncd/target/armv7-unknown-nto-qnx650/release -cf - rsyncd
            fi
        ' | tar -C "$RELEASE" -xf -
fi
ls -l "$RELEASE/rsyncd" "$RELEASE/rsync"

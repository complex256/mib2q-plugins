#!/bin/sh
# Cross-build upstream rsync for arm-unknown-nto-qnx6.5.0eabi.
#
# Run inside the qnx65-armv7-toolchain:8.5-rust image (scripts/build.sh does
# this), with the daemon root as the working directory. Downloads and source
# trees are kept under target/ so a retry reuses them.
#
#   sh scripts/build-rsync.sh [release-dir]
set -eu
ROOT=$(pwd)
VERSION=${RSYNC_VERSION:-3.2.7}
RELEASE=${1:-$ROOT/target/armv7-unknown-nto-qnx650/release}
TARBALL=$ROOT/target/rsync-$VERSION.tar.gz
SRC=$ROOT/target/rsync-src-$VERSION
URL="https://download.samba.org/pub/rsync/src/rsync-$VERSION.tar.gz"

mkdir -p "$RELEASE" "$ROOT/target"

if [ ! -f "$SRC/configure" ]; then
    if [ ! -f "$TARBALL" ]; then
        echo "downloading $URL"
        if command -v curl >/dev/null 2>&1; then
            curl -fsSL "$URL" -o "$TARBALL"
        elif command -v wget >/dev/null 2>&1; then
            wget -q "$URL" -O "$TARBALL"
        else
            echo "error: neither curl nor wget is available to fetch rsync $VERSION" >&2
            exit 1
        fi
    fi
    rm -rf "$SRC"
    mkdir -p "$SRC"
    tar -xzf "$TARBALL" -C "$SRC" --strip-components=1
fi

export CC=arm-unknown-nto-qnx6.5.0eabi-gcc
export AR=arm-unknown-nto-qnx6.5.0eabi-ar
export RANLIB=arm-unknown-nto-qnx6.5.0eabi-ranlib
# QNX's <sys/types.h> defines makedev(node, major, minor); upstream rsync has a
# switch for exactly that signature.
export CFLAGS="-O2 -DMAKEDEV_TAKES_3_ARGS ${CFLAGS:-}"

cd "$SRC"
./configure \
    --host=arm-unknown-nto-qnx6.5.0eabi \
    --disable-openssl \
    --disable-xxhash \
    --disable-zstd \
    --disable-lz4 \
    --disable-iconv \
    --disable-acl-support \
    --disable-xattr-support \
    --disable-md2man

JOBS=${JOBS:-$(nproc 2>/dev/null || echo 2)}
make -j"$JOBS"

cp rsync "$RELEASE/rsync"
chmod 755 "$RELEASE/rsync"
ls -l "$RELEASE/rsync"

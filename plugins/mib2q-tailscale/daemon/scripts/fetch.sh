#!/bin/sh
# Fetch the pinned Tailscale source, plus the two upstream trees we vendor a
# QNX port into ourselves: tailscale's own wireguard-go fork (a different
# module from golang.zx2c4.com/wireguard, which Task 2 already ported - the
# two have diverged) and go-isatty. None are vendored into this repo: they're
# ~200 MB / small / small respectively, and we only add a handful of files.
#
# The wireguard-go and go-isatty pins are read out of the fetched tailscale
# tree's own go.mod, so they always match exactly what tailscaled builds
# against, even if TS_VERSION moves later.
#
# Pinned to a RELEASE, deliberately. v1.102.4's go.mod wants go 1.26.6 and
# pins tailscale/wireguard-go 2e01ba5b00f0, whose tun.Device still takes the
# Read([][]byte, []int, int) shape this port implements. The newer slab/
# ReadPacket API exists only on upstream main (wireguard-go 383e864) - no
# released tag carries it - and following main to get it would mean running
# unreleased code on the car. That port is parked on the
# wireguard-readpacket-api branch for whenever a release does ship it.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TS_VERSION=${TS_VERSION:-v1.102.4}
SRC=$ROOT/build/tailscale-src
WGSRC=$ROOT/build/wireguard-go-src
ISATTYSRC=$ROOT/build/go-isatty-src

if [ -d "$SRC/.git" ] && [ "$(git -C "$SRC" describe --tags 2>/dev/null)" = "$TS_VERSION" ]; then
    echo "tailscale $TS_VERSION already fetched"
else
    rm -rf "$SRC"
    mkdir -p "$(dirname "$SRC")"
    git clone -q --depth 1 --branch "$TS_VERSION" https://github.com/tailscale/tailscale.git "$SRC"
    echo "fetched tailscale $TS_VERSION -> $SRC"
fi
grep -m1 '^go ' "$SRC/go.mod"

# Pin the two forks to exactly what this tailscale tree's go.mod requires.
WG_PSEUDO=$(grep -m1 'github.com/tailscale/wireguard-go ' "$SRC/go.mod" | awk '{print $2}')
WG_COMMIT=$(printf '%s' "$WG_PSEUDO" | sed -E 's/.*-([0-9a-f]{12})$/\1/')
ISATTY_VERSION=$(grep -m1 'github.com/mattn/go-isatty ' "$SRC/go.mod" | awk '{print $2}')

if [ -d "$WGSRC/.git" ] && [ "$(git -C "$WGSRC" rev-parse --short=12 HEAD 2>/dev/null)" = "$WG_COMMIT" ]; then
    echo "tailscale/wireguard-go $WG_COMMIT already fetched"
else
    rm -rf "$WGSRC"
    mkdir -p "$(dirname "$WGSRC")"
    # Not a tag - a plain commit pinned via go.mod's pseudo-version - so this
    # needs the full history, not --depth 1.
    git clone -q https://github.com/tailscale/wireguard-go.git "$WGSRC"
    git -C "$WGSRC" checkout -q "$WG_COMMIT"
    echo "fetched tailscale/wireguard-go $WG_COMMIT -> $WGSRC"
fi

if [ -d "$ISATTYSRC/.git" ] && [ "$(git -C "$ISATTYSRC" describe --tags 2>/dev/null)" = "$ISATTY_VERSION" ]; then
    echo "go-isatty $ISATTY_VERSION already fetched"
else
    rm -rf "$ISATTYSRC"
    mkdir -p "$(dirname "$ISATTYSRC")"
    git clone -q --depth 1 --branch "$ISATTY_VERSION" https://github.com/mattn/go-isatty.git "$ISATTYSRC"
    echo "fetched go-isatty $ISATTY_VERSION -> $ISATTYSRC"
fi

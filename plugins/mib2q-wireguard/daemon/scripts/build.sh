#!/bin/sh
# Build mibwg for the MHI2q (QNX 6.5.0 armle-v7).
#
#   scripts/build.sh            # -> build/mibwg, inside the toolchain image
#   scripts/build.sh test       # -> build/{config,daemon,sntp}.test for tests/qemu-net.sh
#
# Uses qnx65-armv7-toolchain:8.5-go (GOOS=qnx Go port). To skip Docker, point
# QNX_GOROOT at a native build of that port's go/ tree (make.bash on the host
# is ~1 min and needs no QNX gcc - mibwg links internally, CGO_ENABLED=0).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMG=${QNX_IMAGE:-qnx65-armv7-toolchain:8.5-go}
VERSION=${VERSION:-$(git -C "$ROOT" describe --tags --always --dirty 2>/dev/null || echo dev)}
WHAT=${1:-mibwg}

# -B none -buildid=: QNX's loader chokes on the build-id NOTE phdr Go puts
# before .text. -s -w: /tmp on the unit is RAM, and big binaries exec badly
# (32-bit exec ceiling ~15 MB).
LDFLAGS="-s -w -B none -buildid= -X main.version=$VERSION"
cmd="mkdir -p build &&"
case $WHAT in
    mibwg) cmd="$cmd go build -trimpath -ldflags='$LDFLAGS' -o build/mibwg ./cmd/mibwg" ;;
    test)  for p in config daemon sntp; do
               cmd="$cmd go test -c -trimpath -ldflags='$LDFLAGS' -o build/$p.test ./internal/$p &&"
           done
           cmd="$cmd true" ;;
    *) echo "usage: $0 [mibwg|test]" >&2; exit 2 ;;
esac
export GOOS=qnx GOARCH=arm GOARM=7 CGO_ENABLED=0 GOTOOLCHAIN=local

if [ -n "${QNX_GOROOT:-}" ]; then
    cd "$ROOT" && GOROOT=$QNX_GOROOT PATH=$QNX_GOROOT/bin:$PATH sh -c "$cmd"
else
    if ! docker image inspect "$IMG" >/dev/null 2>&1; then
        echo "pulling $IMG"
        docker pull "$IMG" || {
            echo "$IMG unavailable - build/publish it from qnx65-armv7-toolchain (host-scripts/qnx-run.sh build 8.5-go127) or docker login ghcr.io" >&2
            exit 1
        }
    fi
    # Module cache in a named volume: fetched once, reused across builds.
    docker run --rm --platform=linux/amd64 -v "$ROOT":/src -w /src \
        -v mibwg-gomod:/root/go/pkg/mod \
        -e GOOS -e GOARCH -e GOARM -e CGO_ENABLED -e GOTOOLCHAIN -e GOFLAGS=-buildvcs=false \
        "$IMG" sh -c "$cmd"
fi
ls -l "$ROOT"/build/

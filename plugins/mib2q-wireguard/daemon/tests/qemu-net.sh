#!/bin/sh
# Run an armle-v7 QNX binary on real QNX 6.5 under QEMU with the io-pkt TCP/IP
# stack up (loopback only) - the toolchain's tests/qemu-run.sh boots without
# networking, and everything mibwg does is sockets.
#
#   tests/qemu-net.sh <binary> [args...]
#
# Stack: io-pkt-v6-hc (what the MHI2q runs) + lo0 127.0.0.1, and `random` for
# /dev/urandom (Go's crypto/rand reads it). Boot pieces (startup, serial
# driver, u-boot) come from the toolchain checkout:
#   QNX_TOOLCHAIN   default: ../qnx65-armv7-toolchain next to this repo
#   QNX_IMAGE       default: qnx65-armv7-toolchain:8.5-go (only mkifs is used)
#   QEMU_RUN_SECS   how long the program gets once booted (default 120)
#   QEMU_FILES      extra host files, placed in /proc/boot by basename
#   QEMU_PRE        extra boot-script lines run before the program (e.g. gif
#                   setup); the IFS script runs commands in sequence fine
#   QEMU_TOOLS      extra QNX userland binaries from the sysroot (cat cp ...);
#                   ksh is always included, linked as /bin/sh
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${1:?usage: qemu-net.sh <binary> [args...]}
shift
ARGS="$*"
TC=${QNX_TOOLCHAIN:-$(cd "$HERE/../.." && pwd)/qnx65-armv7-toolchain}
IMG=${QNX_IMAGE:-qnx65-armv7-toolchain:8.5-go}
Q=$TC/tests/qemu
command -v qemu-system-arm >/dev/null || { echo "qemu-system-arm not installed" >&2; exit 1; }
[ -f "$Q/u-boot.bin" ] || { echo "no QEMU boot pieces at $Q (set QNX_TOOLCHAIN)" >&2; exit 1; }
docker image inspect "$IMG" >/dev/null 2>&1 || { echo "$IMG not built" >&2; exit 1; }

WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
NAME=$(basename "$BIN")
cp "$BIN" "$WORK/$NAME"
cp "$Q/startup-virt" "$Q/libstartup.a" "$Q/devc-serdebug" "$WORK/"
EXTRA=""
for f in ${QEMU_FILES:-}; do cp "$f" "$WORK/"; EXTRA="$EXTRA
$(basename "$f")"; done
for t in ksh ${QEMU_TOOLS:-}; do EXTRA="$EXTRA
$t"; done

# CACHE_MSYNC=1: io-pkt on this hand-built virt BSP otherwise dlopens a
# nonexistent cache-*.so and dies (see the toolchain's rust/tests notes).
cat > "$WORK/test.build" <<EOF
[linker="(40;1;)qcc -bootstrap -nostdlib -Wl,--no-keep-memory -Vgcc_ntoarmv7%(m!=3,%(m!=6,%(e==0, -EL%)%(e==1, -EB%)%)%)%(h!=0, -Wl,-Ttext -Wl,0x%t%)%(d!=0, -Wl,-Tdata -Wl,0x%d%) -o%o %i %[M -L%^i -Wl,-uinit_%n -lmod_%n%] /w/libstartup.a -L/opt/qnx650/target/qnx6/armle-v7/lib -L/opt/qnx650/target/qnx6/armle-v7/usr/lib -llzo -lucl -ldrvr"]
[linker="(40;2;)cp %i %o"]
[image=0x40200000]
[virtual=armle-v7,raw] .bootstrap = {
    startup-virt -v -S
    PATH=:/proc/boot LD_LIBRARY_PATH=:/proc/boot procnto-smp -vvv
}
[+script] .script = {
    procmgr_symlink ../../proc/boot/libc.so.3 /usr/lib/ldqnx.so.2
    devc-serdebug -e -F -S
    waitfor /dev/ser1 4
    reopen /dev/ser1
    random -t &
    waitfor /dev/urandom 4
    # pipe(2) is a resource manager on QNX, not a kernel feature; without it
    # os/exec (and ksh job control) fail with ENOSYS.
    pipe &
    waitfor /dev/pipe 4
    CACHE_MSYNC=1 io-pkt-v6-hc &
    waitfor /dev/socket 8
    ifconfig lo0 127.0.0.1 up
${QEMU_PRE:-}
    display_msg "@@QNXRUN-START@@"
    $NAME $ARGS
    display_msg "@@QNXRUN-END@@"
}
[type=link] /usr/lib/ldqnx.so.2=/proc/boot/libc.so.3
[type=link] /tmp=/dev/shmem
[type=link] /bin/sh=/proc/boot/ksh
# data=copy: mkifs defaults to data=uip (data segment used in place), which
# makes every boot-image executable startable only once - a second exec of
# ifconfig, or anything a shell script runs twice, fails with ENOENT.
[perms=+r,+x data=copy]
libc.so.3
libm.so.2
libz.so.2
libsocket.so.3
libsocket.so.2
procnto-smp
devc-serdebug
io-pkt-v6-hc
ifconfig
random
pipe
$NAME$EXTRA
EOF

docker run --rm --platform=linux/amd64 -v "$WORK":/w -w /w "$IMG" sh -c '
T=/opt/qnx650/target/qnx6/armle-v7
cp $T/boot/sys/procnto-smp /w/
export MKIFS_PATH=/w:$T/boot/sys:$T/sbin:$T/usr/sbin:$T/bin:$T/usr/bin:$T/lib:$T/usr/lib
mkifs test.build ifs.bin' >"$WORK/mkifs.log" 2>&1 || {
    echo "mkifs failed:" >&2; tail -8 "$WORK/mkifs.log" >&2; exit 1; }

mkdir -p "$WORK/fat"; mv "$WORK/ifs.bin" "$WORK/fat/"
LOG="$WORK/qemu.log"
# u-boot eats the first keystrokes for its autoboot prompt, hence the leading
# newline and the pauses.
( printf '\n'; sleep 4
  printf 'virtio scan\n'; sleep 2
  printf 'fatload virtio 0:1 0x40200000 ifs.bin\n'; sleep 3
  printf 'go 0x40200000\n'; sleep "${QEMU_RUN_SECS:-120}" ) | \
  timeout "$(( ${QEMU_RUN_SECS:-120} + 60 ))" qemu-system-arm -M virt -m 512 -cpu cortex-a15 \
    -bios "$Q/u-boot.bin" -drive file=fat:rw:"$WORK/fat",format=raw,media=disk \
    -display none -serial stdio > "$LOG" 2>&1 || true
[ -n "${QEMU_KEEP_LOG:-}" ] && cp "$LOG" "$QEMU_KEEP_LOG"

if ! grep -q '@@QNXRUN-START@@' "$LOG"; then
    echo "=== boot did not reach the program ===" >&2
    tail -20 "$LOG" >&2; exit 1
fi
awk '
  /@@QNXRUN-START@@/ { sub(/.*@@QNXRUN-START@@/, ""); inside = 1 }
  /@@QNXRUN-END@@/   { sub(/@@QNXRUN-END@@.*/, ""); if (length($0)) print; exit }
  inside && length($0) { print }
' "$LOG"
if ! grep -q '@@QNXRUN-END@@' "$LOG"; then
    echo "warning: program did not return in ${QEMU_RUN_SECS:-120}s - hung, or crashed hard" >&2
    exit 1
fi
# procnto -vvv reports exits and fatal signals.
if grep -qE "\($NAME\) terminated SIG" "$LOG"; then
    grep -oE "Process [0-9]+ \($NAME\) terminated SIG[A-Z]+[^ ]*" "$LOG" | tail -1 >&2
    exit 1
fi
code=$(grep -oE "Process [0-9]+ \($NAME\) exited status=[0-9]+" "$LOG" | tail -1 | grep -oE '[0-9]+$' || true)
[ -n "$code" ] && [ "$code" != 0 ] && { echo "(exit status $code)" >&2; exit "$code"; }
exit 0

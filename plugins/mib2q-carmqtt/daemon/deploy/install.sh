#!/usr/bin/env bash
# Install carmqtt on the head unit and add its boot hook.
#   deploy/install.sh [ssh-host]      (default: audi)
# Needs: scripts/build.sh output, and deploy/carmqtt.toml (from the example).
set -euo pipefail
HOST=${1:-audi}
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
BIN=$ROOT/target/armv7-unknown-nto-qnx650/release/carmqtt
CONF=${CARMQTT_CONF:-$HERE/carmqtt.toml}
DEST=/mnt/app/root/carmqtt
BOOT=/mnt/system/etc/boot

[ -f "$BIN" ] || { echo "missing $BIN - run scripts/build.sh first" >&2; exit 1; }
[ -f "$CONF" ] || { echo "missing $CONF - copy deploy/carmqtt.toml.example and fill in [mqtt]" >&2; exit 1; }

TMP=$(mktemp -d)
cleanup() {
    rm -rf "$TMP"
    # /mnt/app is mounted read-only on the unit; always put it back, even if
    # something above failed while it was writable.
    ssh -q "$HOST" "mount -ur /mnt/app" 2>/dev/null || echo "warning: failed to remount /mnt/app read-only on $HOST" >&2
}
trap cleanup EXIT

ssh -q "$HOST" "mount -uw /mnt/app" || { echo "failed to mount /mnt/app read-write on $HOST" >&2; exit 1; }

ssh -q "$HOST" "mkdir -p $DEST"
scp -q "$BIN" "$HERE/carmqtt.sh" "$HOST:$DEST/"
scp -q "$CONF" "$HOST:$DEST/carmqtt.toml"
ssh -q "$HOST" "chmod 755 $DEST/carmqtt $DEST/carmqtt.sh"
echo "installed files in $HOST:$DEST"

scp -q "$HOST:$BOOT/startup.sh" "$TMP/startup.sh"
if grep -q "$DEST/carmqtt.sh" "$TMP/startup.sh"; then
    echo "boot hook already present"
else
    python3 "$HERE/add_hook.py" add "$TMP/startup.sh" "$TMP/startup.sh.new"
    scp -q "$TMP/startup.sh.new" "$HOST:/tmp/startup.sh.new"
    ssh -q "$HOST" "ksh -n /tmp/startup.sh.new" || { echo "ksh -n rejected the edited startup.sh; unit left untouched" >&2; exit 1; }
    # Atomic boot script update: stage, validate, then move; restore from backup on failure.
    # Uses boot_edit.sh to atomically replace startup.sh with staged file. It
    # remounts /mnt/system read-write/read-only around its own edit.
    scp -q "$HERE/boot_edit.sh" "$HOST:/tmp/boot_edit.sh"
    ssh -q "$HOST" "sh /tmp/boot_edit.sh $BOOT startup.sh.pre-carmqtt" || { echo "installing startup.sh failed" >&2; exit 1; }
    echo "boot hook installed (backup: $BOOT/startup.sh.pre-carmqtt)"
fi
echo "start now with: ssh $HOST $DEST/carmqtt.sh start"

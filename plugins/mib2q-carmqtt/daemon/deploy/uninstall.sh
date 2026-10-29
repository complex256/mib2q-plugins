#!/usr/bin/env bash
# Stop carmqtt, remove its boot hook, and delete its files from the unit.
#   deploy/uninstall.sh [ssh-host]    (default: audi)
set -euo pipefail
HOST=${1:-audi}
HERE=$(cd "$(dirname "$0")" && pwd)
DEST=/mnt/app/root/carmqtt
BOOT=/mnt/system/etc/boot

TMP=$(mktemp -d)
cleanup() {
    rm -rf "$TMP"
    # /mnt/app is mounted read-only on the unit; always put it back, even if
    # the final rm -rf below (or anything after the mount -uw) failed.
    ssh -q "$HOST" "mount -ur /mnt/app" 2>/dev/null || echo "warning: failed to remount /mnt/app read-only on $HOST" >&2
}
trap cleanup EXIT

ssh -q "$HOST" "[ -x $DEST/carmqtt.sh ] && $DEST/carmqtt.sh stop || true"
scp -q "$HOST:$BOOT/startup.sh" "$TMP/startup.sh"
if grep -q "$DEST/carmqtt.sh" "$TMP/startup.sh"; then
    python3 "$HERE/add_hook.py" remove "$TMP/startup.sh" "$TMP/startup.sh.new"
    scp -q "$TMP/startup.sh.new" "$HOST:/tmp/startup.sh.new"
    ssh -q "$HOST" "ksh -n /tmp/startup.sh.new" || { echo "ksh -n rejected the edited startup.sh; unit left untouched" >&2; exit 1; }
    # Atomic boot script update: stage, validate, then move; restore from backup on failure.
    # Uses boot_edit.sh to atomically replace startup.sh with staged file. It
    # remounts /mnt/system read-write/read-only around its own edit.
    scp -q "$HERE/boot_edit.sh" "$HOST:/tmp/boot_edit.sh"
    ssh -q "$HOST" "sh /tmp/boot_edit.sh $BOOT startup.sh.pre-carmqtt-uninstall" || { echo "removing boot hook failed" >&2; exit 1; }
    echo "boot hook removed"
fi

ssh -q "$HOST" "mount -uw /mnt/app" || { echo "failed to mount /mnt/app read-write on $HOST" >&2; exit 1; }
ssh -q "$HOST" "rm -rf $DEST; rm -f /tmp/carmqtt /tmp/carmqtt.*"
echo "carmqtt removed from $HOST"

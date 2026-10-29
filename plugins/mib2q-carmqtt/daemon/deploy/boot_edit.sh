#!/bin/sh
# Atomic boot script update with rollback. Used by both install and uninstall.
# Called as: boot_edit.sh BOOTDIR BACKUPNAME
#
# Expects /tmp/startup.sh.new to exist and contain the new boot script.
# Creates BACKUPNAME backup of current startup.sh (only if absent).
# Stages to BOOTDIR/startup.sh.carmqtt-new, validates, then atomically moves.
# On any failure before move, removes staged file and leaves startup.sh untouched.
# On any failure after move, restores from backup and exits non-zero.
#
# /mnt/system is mounted read-only on the unit, so this remounts it
# read-write for the duration of the edit and always puts it back read-only
# on the way out (trap ... EXIT), success or failure. Set BOOT_EDIT_NO_MOUNT=1
# to skip the mount/sync calls entirely, for running this off the unit (e.g.
# deploy/test_boot_edit.sh on a dev machine that has no /mnt/system).

set -e
BOOTDIR=$1
BACKUPNAME=$2
BACKUP=$BOOTDIR/$BACKUPNAME
STAGED=$BOOTDIR/startup.sh.carmqtt-new
STARTUP=$BOOTDIR/startup.sh

remount_ro() {
    [ -n "${BOOT_EDIT_NO_MOUNT:-}" ] && return 0
    sync
    mount -ur /mnt/system
}
trap remount_ro EXIT
# A dropped ssh session (HUP) or a signal must still run the EXIT trap, so
# /mnt/system never stays read-write. startup.sh itself only ever changes
# through the validated mv below.
trap 'exit 1' HUP INT TERM

if [ -z "${BOOT_EDIT_NO_MOUNT:-}" ]; then
    mount -uw /mnt/system || { echo "startup.sh update failed: could not mount /mnt/system read-write" >&2; exit 1; }
fi

# Ensure /tmp/startup.sh.new exists and is readable
[ -f /tmp/startup.sh.new ] || { echo "startup.sh update failed: /tmp/startup.sh.new not found" >&2; exit 1; }

# Create backup if not present
if [ ! -f "$BACKUP" ]; then
    cp -p "$STARTUP" "$BACKUP" || { echo "startup.sh update failed: could not create backup" >&2; exit 1; }
fi

# Stage the new file. QNX ksh has no stat(1), so the only way to keep
# startup.sh's mode is to copy the original first (cp -p carries mode) and
# then overwrite its content, rather than creating STAGED fresh.
cp -p "$STARTUP" "$STAGED" || { rm -f "$STAGED"; echo "startup.sh update failed: could not stage new file" >&2; exit 1; }
if ! cat /tmp/startup.sh.new > "$STAGED"; then
    rm -f "$STAGED"
    echo "startup.sh update failed: could not write staged content" >&2
    exit 1
fi

# Validate staged file; on failure, remove staged file and leave startup.sh untouched
if ! ksh -n "$STAGED" 2>/dev/null; then
    rm -f "$STAGED"
    echo "startup.sh update failed: staged file failed syntax validation" >&2
    exit 1
fi

# Atomically move staged file to live location
mv "$STAGED" "$STARTUP" || { echo "startup.sh update failed: could not move staged file to live location" >&2; exit 1; }

# Validate final file; on failure, restore from backup
if ! ksh -n "$STARTUP" 2>/dev/null; then
    echo "startup.sh update failed: final file failed syntax validation; restoring from backup" >&2
    if ! cp "$BACKUP" "$STARTUP"; then
        echo "CRITICAL: startup.sh update failed AND restoring from $BACKUP also failed; the unit's boot script may be broken, fix manually before rebooting" >&2
    fi
    exit 1
fi

exit 0

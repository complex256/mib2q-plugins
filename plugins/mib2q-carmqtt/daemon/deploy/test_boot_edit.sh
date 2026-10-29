#!/bin/sh
# Test deploy/boot_edit.sh logic locally with a temp directory standing in for $BOOT.
# Tests: (a) happy path with valid file, (b) failure on syntax error, (c) no staged
# file left, (d) idempotent backup, (e) staged file keeps the original's mode,
# (f) staging failure leaves no staged file behind.
#
# BOOT_EDIT_NO_MOUNT=1 tells boot_edit.sh to skip the real `mount`/`sync` calls
# it otherwise makes against /mnt/system - this machine has no such mount.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Check which shell to use for validation
sh_cmd=sh
command -v ksh >/dev/null && sh_cmd=ksh

echo "Testing boot_edit.sh with $sh_cmd..."

# Test 1: Happy path - valid boot script replacement
echo "Test 1: Happy path - valid boot script..."
mkdir -p "$work/boot1"
cat > "$work/boot1/startup.sh" <<'EOF'
start_late_drivers() {
    echo "boot"
}
EOF
cat > /tmp/startup.sh.new <<'EOF'
start_late_drivers() {
    echo "boot with hook"
}
EOF
BOOT_EDIT_NO_MOUNT=1 $sh_cmd "$here/boot_edit.sh" "$work/boot1" "startup.sh.pre-carmqtt" 2>&1
# Verify: startup.sh was replaced, backup exists, no staged file
if ! grep -q "with hook" "$work/boot1/startup.sh"; then
    echo "FAIL: startup.sh was not replaced" >&2
    exit 1
fi
if [ ! -f "$work/boot1/startup.sh.pre-carmqtt" ]; then
    echo "FAIL: backup was not created" >&2
    exit 1
fi
if [ -f "$work/boot1/startup.sh.carmqtt-new" ]; then
    echo "FAIL: staged file was not cleaned up" >&2
    exit 1
fi
echo "PASS: startup.sh replaced, backup present, staged file removed"

# Test 2: Staged file fails syntax validation
echo "Test 2: Staged file fails ksh -n (invalid syntax)..."
mkdir -p "$work/boot2"
cat > "$work/boot2/startup.sh" <<'EOF'
start_late_drivers() {
    echo "original"
}
EOF
cat > /tmp/startup.sh.new <<'EOF'
start_late_drivers() {
    bad syntax here
EOF
# This should fail and leave startup.sh untouched
set +e
BOOT_EDIT_NO_MOUNT=1 $sh_cmd "$here/boot_edit.sh" "$work/boot2" "startup.sh.pre-carmqtt" 2>&1 | grep -q "failed syntax validation"
exit_code=$?
set -e
if [ $exit_code -ne 0 ]; then
    echo "FAIL: expected syntax validation error" >&2
    exit 1
fi
# Verify: startup.sh was NOT changed, staged file was removed
if ! grep -q "original" "$work/boot2/startup.sh"; then
    echo "FAIL: startup.sh was modified despite validation failure" >&2
    exit 1
fi
if [ -f "$work/boot2/startup.sh.carmqtt-new" ]; then
    echo "FAIL: staged file was not cleaned up after validation failure" >&2
    exit 1
fi
echo "PASS: startup.sh unchanged, staged file removed after validation failure"

# Test 3: Backup already exists (idempotency)
echo "Test 3: Backup already exists (idempotent)..."
mkdir -p "$work/boot3"
cat > "$work/boot3/startup.sh" <<'EOF'
start_late_drivers() {
    echo "boot"
}
EOF
cat > "$work/boot3/startup.sh.pre-carmqtt" <<'EOF'
start_late_drivers() {
    echo "original before backup"
}
EOF
cat > /tmp/startup.sh.new <<'EOF'
start_late_drivers() {
    echo "new version"
}
EOF
BOOT_EDIT_NO_MOUNT=1 $sh_cmd "$here/boot_edit.sh" "$work/boot3" "startup.sh.pre-carmqtt" 2>&1
# Verify: startup.sh was replaced, backup was NOT overwritten (still has "original before backup")
if ! grep -q "new version" "$work/boot3/startup.sh"; then
    echo "FAIL: startup.sh was not replaced" >&2
    exit 1
fi
if ! grep -q "original before backup" "$work/boot3/startup.sh.pre-carmqtt"; then
    echo "FAIL: backup was overwritten (should be idempotent)" >&2
    exit 1
fi
echo "PASS: startup.sh replaced, existing backup preserved"

# Test 4: staged file keeps the original startup.sh's mode (no stat(1) on the
# unit, so this can only be verified by comparing `ls -l` permission bits).
echo "Test 4: staged file keeps the original's mode..."
mkdir -p "$work/boot4"
cat > "$work/boot4/startup.sh" <<'EOF'
start_late_drivers() {
    echo "boot"
}
EOF
chmod 700 "$work/boot4/startup.sh"
cat > /tmp/startup.sh.new <<'EOF'
start_late_drivers() {
    echo "boot with hook"
}
EOF
BOOT_EDIT_NO_MOUNT=1 $sh_cmd "$here/boot_edit.sh" "$work/boot4" "startup.sh.pre-carmqtt" 2>&1
mode=$(ls -l "$work/boot4/startup.sh" | cut -c1-10)
if [ "$mode" != "-rwx------" ]; then
    echo "FAIL: startup.sh mode is $mode, expected -rwx------ (original's mode)" >&2
    exit 1
fi
echo "PASS: startup.sh mode preserved across the update"

# Test 5: staging cp failure (can't write into $BOOTDIR) leaves no staged
# file behind and startup.sh untouched. Meaningless as root, which ignores
# permission bits, so skip there.
if [ "$(id -u 2>/dev/null || echo 0)" = "0" ]; then
    echo "SKIP: Test 5 needs an unwritable directory to fail cp; not meaningful as root"
else
    echo "Test 5: staging cp failure leaves no staged file..."
    mkdir -p "$work/boot5"
    cat > "$work/boot5/startup.sh" <<'EOF'
start_late_drivers() {
    echo "boot"
}
EOF
    # Pre-create the backup so the backup step (which also needs to write
    # into the directory) is skipped by idempotency, isolating the failure
    # to the staging cp itself.
    cp "$work/boot5/startup.sh" "$work/boot5/startup.sh.pre-carmqtt"
    cat > /tmp/startup.sh.new <<'EOF'
start_late_drivers() {
    echo "new"
}
EOF
    chmod 555 "$work/boot5"
    set +e
    BOOT_EDIT_NO_MOUNT=1 $sh_cmd "$here/boot_edit.sh" "$work/boot5" "startup.sh.pre-carmqtt" 2>&1 | grep -q "could not stage"
    exit_code=$?
    set -e
    chmod 755 "$work/boot5"
    if [ $exit_code -ne 0 ]; then
        echo "FAIL: expected a 'could not stage new file' error" >&2
        exit 1
    fi
    if [ -f "$work/boot5/startup.sh.carmqtt-new" ]; then
        echo "FAIL: staged file was left behind after a staging failure" >&2
        exit 1
    fi
    if ! grep -q '"boot"' "$work/boot5/startup.sh"; then
        echo "FAIL: startup.sh was modified despite staging failure" >&2
        exit 1
    fi
    echo "PASS: no staged file left after staging failure"
fi

echo "boot_edit.sh tests OK"

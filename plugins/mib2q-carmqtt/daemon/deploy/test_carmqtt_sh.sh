#!/bin/sh
# Smoke-test deploy/carmqtt.sh on the dev machine with a stand-in binary.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'CM_DIR=$work/dir CM_RUN=$work/run sh "$here/carmqtt.sh" stop >/dev/null 2>&1 || true; CM_DIR=$work/dir2 CM_RUN=$work/run2 sh "$here/carmqtt.sh" stop >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
mkdir -p "$work/dir"
cp "$here/carmqtt.sh" "$work/dir/"
cat > "$work/dir/carmqtt" <<'EOF'
#!/bin/sh
echo "fake carmqtt $*" >> "${CM_RUN}/fake.log"
exec sleep 30
EOF
chmod 755 "$work/dir/carmqtt"
: > "$work/dir/carmqtt.toml"
export CM_DIR="$work/dir" CM_RUN="$work/run"
for shell in sh ksh; do
    command -v "$shell" >/dev/null || continue
    $shell -n "$here/carmqtt.sh"
done
sh "$here/carmqtt.sh" start
sleep 1
sh "$here/carmqtt.sh" status
grep -q -- "--config $work/dir/carmqtt.toml" "$work/run/fake.log"
sh "$here/carmqtt.sh" start | grep -q "already running"

# The binary must be staged as carmqtt.new and mv'd into place, never left
# half-copied at the live path.
if [ -f "$work/run/carmqtt.new" ]; then
    echo "FAIL: carmqtt.new was left behind after start" >&2
    exit 1
fi
[ -x "$work/run/carmqtt" ] || { echo "FAIL: $work/run/carmqtt is missing or not executable" >&2; exit 1; }
echo "PASS: binary installed atomically (no leftover carmqtt.new)"

# First start must not have produced a wrapper.log.prev (nothing preceded it).
if [ -f "$work/run/carmqtt.wrapper.log.prev" ]; then
    echo "FAIL: wrapper.log.prev exists after the very first start" >&2
    exit 1
fi

sh "$here/carmqtt.sh" stop
if sh "$here/carmqtt.sh" status; then echo "still running after stop" >&2; exit 1; fi

# Second start: wrapper.log from the first run must survive as .prev, and
# this run's wrapper.log must start fresh (capped, not left to grow forever).
echo "leftover from previous run" >> "$work/run/carmqtt.wrapper.log"
sh "$here/carmqtt.sh" start
sleep 1
if [ ! -f "$work/run/carmqtt.wrapper.log.prev" ]; then
    echo "FAIL: wrapper.log.prev was not created on the second start" >&2
    exit 1
fi
if ! grep -q "leftover from previous run" "$work/run/carmqtt.wrapper.log.prev"; then
    echo "FAIL: wrapper.log.prev does not contain the previous run's log" >&2
    exit 1
fi
if grep -q "leftover from previous run" "$work/run/carmqtt.wrapper.log"; then
    echo "FAIL: wrapper.log was not truncated on start" >&2
    exit 1
fi
echo "PASS: wrapper.log capped with one previous copy kept, like tailscale.sh"
sh "$here/carmqtt.sh" stop

echo "carmqtt.sh smoke test OK"

# --- restart race: stop must wait for a slow-to-die daemon before it
# reports success and clears the pid files, not just fire-and-forget kill. ---
mkdir -p "$work/dir2"
cp "$here/carmqtt.sh" "$work/dir2/"
cat > "$work/dir2/carmqtt" <<'EOF'
#!/bin/sh
# Ignores the first TERM for a bit, to exercise cm_stop's wait loop. The
# background+wait idiom (matching cm_loop's own "cmd & ... wait $!") is what
# actually lets the trap interrupt promptly, unlike a plain foreground sleep.
trap 'sleep 2; exit 0' TERM
sleep 30 &
wait $!
EOF
chmod 755 "$work/dir2/carmqtt"
: > "$work/dir2/carmqtt.toml"
mkdir -p "$work/run2"
CM_DIR="$work/dir2" CM_RUN="$work/run2" sh "$here/carmqtt.sh" start
sleep 1
daemon_pid=$(cat "$work/run2/carmqtt.pid")
CM_DIR="$work/dir2" CM_RUN="$work/run2" sh "$here/carmqtt.sh" stop
# By the time stop returns, the slow-dying daemon must actually be gone -
# not just "signaled and hoped for the best".
if kill -0 "$daemon_pid" 2>/dev/null; then
    echo "FAIL: stop returned before the slow-to-die daemon actually exited" >&2
    exit 1
fi
echo "PASS: stop waits for the daemon to actually exit"

echo "carmqtt.sh restart-race test OK"

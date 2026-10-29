#!/bin/ksh
# Behaviour test for tailscale.sh's debug mode: the watchdog, the per-boot
# history on the card, and - just as important - that none of it happens
# with the flag off.
#
# Runs tailscale.sh on the host against fake binaries: a "tailscaled" that is
# just a sleep loop which, like Go, dumps and exits on SIGQUIT; a "tailscale"
# whose `status` hangs while a wedge file exists, the way the real CLI hangs
# on a wedged daemon's socket; and a pidin that records it was asked. What it
# cannot cover is the unit's own ksh, pidin output and SD card, which are
# verified on the unit.
#
#   deploy/tailscale_test.sh
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/tailscale.sh
WORK=$(mktemp -d)
cleanup() {
    for p in "$WORK"/run/ts_watchdog.pid "$WORK"/run/tailscaled.pid "$WORK"/run/tsstatus.pid; do
        [ -f "$p" ] && kill -9 "$(cat "$p")" 2>/dev/null
    done
    rm -rf "$WORK"
}
trap cleanup EXIT; [ -n "${KEEP:-}" ] && trap - EXIT

fail() { echo "FAIL: $1" >&2; [ -n "${KEEP:-}" ] && cat "$WORK"/run/ts_events.log >&2; exit 1; }
pass() { echo "ok: $1"; }

APP=$WORK/app
BIN=$WORK/bin
mkdir -p "$APP" "$BIN"

# Python, not sh: the daemon is started as a background job, which inherits
# SIGQUIT ignored, and a shell can't trap a signal it inherited ignored. Go
# installs its SIGQUIT handler regardless, and so does this.
cat >"$APP/tailscaled" <<'EOF'
#!/usr/bin/env python3 -I
import signal, sys, time
def quit(*_):
    print("SIGQUIT: quit\ngoroutine 1 [semacquire]:", flush=True)
    sys.exit(2)
signal.signal(signal.SIGQUIT, quit)
print("fake tailscaled up", flush=True)
while True:
    time.sleep(1)
EOF
cat >"$APP/tailscale" <<'EOF'
#!/bin/sh
case "$*" in
    *status*) [ -f "$FAKE_WEDGE" ] && exec sleep 1000; echo "100.64.0.1 fake" ;;
    *" up "*) echo "$*" >"$TS_RUN_FOR_FAKE/up.args" ;;
esac
exit 0
EOF
cat >"$APP/sntp" <<'EOF'
#!/bin/sh
echo 0
EOF
# tsstatus: records its argv, then idles like the real one until killed.
cat >"$APP/tsstatus" <<'EOF'
#!/bin/sh
echo "$*" >"$TS_RUN_FOR_FAKE/tsstatus.args"
exec sleep 1000
EOF
: >"$APP/libtsclock.so"
cat >"$BIN/pidin" <<'EOF'
#!/bin/sh
echo "fake pidin $*"
EOF
chmod +x "$APP"/* "$BIN"/*

# A fresh "boot": empty RAM dir. STATE (flash) and CARD persist across these.
boot() {
    [ -f "$WORK/run/ts_watchdog.pid" ] && kill -9 "$(cat "$WORK/run/ts_watchdog.pid")" 2>/dev/null
    [ -f "$WORK/run/tailscaled.pid" ] && kill -9 "$(cat "$WORK/run/tailscaled.pid")" 2>/dev/null
    [ -f "$WORK/run/tsstatus.pid" ] && kill -9 "$(cat "$WORK/run/tsstatus.pid")" 2>/dev/null
    rm -rf "$WORK/run"
    mkdir -p "$WORK/run"
}

ts() {
    PATH="$BIN:$PATH" TS_DIR="$APP" TS_RUN="$WORK/run" TS_STATE="$WORK/state" \
        FAKE_WEDGE="$WORK/wedge" TS_RUN_FOR_FAKE="$WORK/run" TS_START_WAIT=1 \
        TS_WD_INTERVAL=1 TS_WD_PROBE_TIMEOUT=2 TS_WD_GRACE=1 TS_WD_FLUSH_EVERY=3 \
        TS_DEBUG_KEEP=3 \
        ${TS_SHELL:-ksh} "$SCRIPT" "$@"
}

# wait_for <seconds> <test command...>
wait_for() {
    wf_n=$1; shift
    while [ "$wf_n" -gt 0 ]; do
        "$@" && return 0
        sleep 1
        wf_n=$((wf_n - 1))
    done
    return 1
}

CARD=$WORK/card
SESS=$CARD/00001

# --- flag off: behaves as before, writes nothing new ------------------------
boot
ts start >/dev/null || fail "start with debug off"
[ -f "$WORK/run/ts_watchdog.pid" ] && fail "watchdog started with debug off"
[ -f "$WORK/run/ts_events.log" ] && fail "events written with debug off"
[ -f "$WORK/run/ts_boot" ] && fail "boot marker written with debug off"
[ -e "$WORK/state/debug.bootseq" ] && fail "boot counter written to flash with debug off"
grep -q ts-debug "$WORK/run/tailscaled.log" && fail "debug lines in the log with debug off"
ts stop >/dev/null
pass "debug off: no watchdog, no events, nothing on flash or card"

# --- accept-routes: on by default, per-unit opt-out in $STATE -----------------
routes_case() { # <file content or "-" for none> <env or ""> <expected flag>
    rm -f "$WORK/state/accept-routes"
    [ "$1" != - ] && { mkdir -p "$WORK/state"; echo "$1" >"$WORK/state/accept-routes"; }
    boot
    TS_ACCEPT_ROUTES=$2 ts start >/dev/null || fail "start for accept-routes '$1' '$2'"
    wait_for 5 test -f "$WORK/run/up.args" || fail "tailscale up never ran ('$1' '$2')"
    grep -q -- "--accept-dns=false $3\( \|\$\)" "$WORK/run/up.args" \
        || fail "accept-routes '$1' env '$2': want $3, got: $(cat "$WORK/run/up.args")"
    ts stop >/dev/null
}
routes_case - "" "--accept-routes"
routes_case 1 "" "--accept-routes"
routes_case 0 "" "--accept-routes=false"
routes_case accept-routes=0 "" "--accept-routes=false"
routes_case off "" "--accept-routes=false"
routes_case 0 1 "--accept-routes"
routes_case - 0 "--accept-routes=false"
rm -f "$WORK/state/accept-routes"
pass "accept-routes: default on; \$STATE/accept-routes=0 or TS_ACCEPT_ROUTES=0 turns it off"

# --- tsstatus sidecar lives and dies with tailscaled ------------------------
boot
ts start >/dev/null || fail "start for tsstatus"
wait_for 5 test -f "$WORK/run/tsstatus.args" || fail "tsstatus never started"
stpid=$(cat "$WORK/run/tsstatus.pid")
kill -0 "$stpid" 2>/dev/null || fail "tsstatus.pid names no live process"
grep -q -- "--socket=$WORK/run/tailscaled.sock" "$WORK/run/tsstatus.args" || fail "tsstatus not pointed at the tailscaled socket"
grep -q -- "--clock-offset-file=$WORK/run/ts_clock_offset" "$WORK/run/tsstatus.args" || fail "tsstatus not given the clock offset file"
ts start >/dev/null
[ "$(cat "$WORK/run/tsstatus.pid")" = "$stpid" ] || fail "a second start spawned another tsstatus"
ts stop >/dev/null
kill -0 "$stpid" 2>/dev/null && fail "stop left tsstatus running"
[ -f "$WORK/run/tsstatus.pid" ] && fail "stop left tsstatus.pid"
pass "tsstatus starts with tailscaled, once, and stops with it"

# --- a busy / uncopyable tsstatus never blocks the daemon -------------------
# On the unit, cp over a running binary fails ("text file busy"). Reproduced
# two ways: tsstatus still running with a changed binary on flash (the
# watchdog / late-offset restart path), and a cp that refuses tsstatus outright.
boot
ts start >/dev/null || fail "start for busy test"
stpid=$(cat "$WORK/run/tsstatus.pid")
kill -9 "$(cat "$WORK/run/tailscaled.pid")"; rm -f "$WORK/run/tailscaled.pid"
printf '#!/bin/sh\necho "$*" >"$TS_RUN_FOR_FAKE/tsstatus.args"\n# v2 (different size)\nexec sleep 1000\n' >"$APP/tsstatus"
chmod +x "$APP/tsstatus"
ts start >/dev/null || fail "restart with tsstatus still running failed"
[ "$(cat "$WORK/run/tsstatus.pid")" = "$stpid" ] || fail "running tsstatus was replaced by a restart"
grep -q 'v2' "$WORK/run/tsstatus" || fail "new tsstatus not installed alongside the running one"
ts stop >/dev/null
mkdir -p "$WORK/failcp"
cat >"$WORK/failcp/cp" <<'EOF'
#!/bin/sh
for a in "$@"; do case "$a" in */tsstatus*) echo "cp: text file busy" >&2; exit 1 ;; esac; done
exec /bin/cp "$@"
EOF
chmod +x "$WORK/failcp/cp"
boot
PATH="$WORK/failcp:$PATH" ts start >/dev/null 2>&1 || fail "a failing tsstatus copy blocked tailscaled"
ts_running_check() { [ -f "$WORK/run/tailscaled.pid" ] && kill -0 "$(cat "$WORK/run/tailscaled.pid")" 2>/dev/null; }
ts_running_check || fail "tailscaled not running after a failed tsstatus copy"
ts stop >/dev/null
pass "tsstatus copy failure / busy binary is non-fatal for tailscaled"

# --- a missing tsstatus binary changes nothing ------------------------------
mv "$APP/tsstatus" "$APP/tsstatus.off"
boot
ts start >/dev/null || fail "start without a tsstatus binary"
[ -f "$WORK/run/tsstatus.pid" ] && fail "tsstatus.pid written without a binary"
ts stop >/dev/null
mv "$APP/tsstatus.off" "$APP/tsstatus"
pass "no tsstatus binary: tailscaled still starts"

# --- turning it on ----------------------------------------------------------
ts debug on "$CARD" >/dev/null || fail "debug on"
grep -q '^TS_DEBUG=1$' "$WORK/state/debug.conf" || fail "debug on wrote no flag"
pass "debug on writes the flag to STATE"

# --- boot 1: start, history on the card, watchdog running -------------------
boot
ts start >/dev/null || fail "start with debug on"
[ -f "$SESS/events.log" ] || fail "no events.log in $SESS"
grep -q 'first start since power-on' "$SESS/events.log" || fail "boot not recorded"
[ -f "$SESS/bootinfo.txt" ] || fail "no boot info on the card"
grep -q 'tailscale up exit 0' "$SESS/events.log" || fail "up outcome not recorded"
ts debug status | grep -q 'watchdog: running' || fail "watchdog not running"
pass "boot 1 recorded on the card, watchdog up"

# --- heartbeat flushes the live log -----------------------------------------
wait_for 15 grep -q heartbeat "$SESS/events.log" || fail "no heartbeat flushed to the card"
pass "heartbeat reaches the card"

# --- a wedge: captured, dumped, restarted -----------------------------------
oldpid=$(cat "$WORK/run/tailscaled.pid")
: >"$WORK/wedge"
wait_for 30 grep -q 'restart 1/3' "$WORK/run/ts_events.log" || fail "wedge never led to a restart"
rm -f "$WORK/wedge"
grep -q 'WEDGED' "$SESS/events.log" || fail "wedge not on the card"
[ -f "$SESS/capture.1.txt" ] || fail "no capture file for instance 1"
grep -q "fake pidin -p $oldpid" "$SESS/capture.1.txt" || fail "capture has no thread states"
grep -q 'goroutine 1' "$SESS/tailscaled.1.log" || fail "SIGQUIT dump not saved with instance 1's log"
wait_for 10 test -f "$SESS/tailscaled.2.log" || fail "restarted instance's log not on the card"
newpid=$(cat "$WORK/run/tailscaled.pid")
[ "$newpid" != "$oldpid" ] && kill -0 "$newpid" 2>/dev/null || fail "no new tailscaled after the wedge"
pass "wedge: thread states + goroutine dump saved, tailscaled restarted"

# --- a death nobody asked for -----------------------------------------------
kill -9 "$newpid"
wait_for 20 grep -q 'restart 2/3' "$WORK/run/ts_events.log" || fail "death never led to a restart"
grep -q 'gone and nobody stopped it' "$WORK/run/ts_events.log" || fail "death not recorded"
pass "unexpected death: recorded and restarted"

# --- a deliberate stop is not a death ---------------------------------------
wait_for 10 test -f "$SESS/tailscaled.3.log"
ts stop >/dev/null
sleep 3
grep -q 'restart 3/3' "$WORK/run/ts_events.log" && fail "watchdog restarted a deliberate stop"
[ -f "$WORK/run/ts_watchdog.pid" ] && fail "stop left the watchdog running"
pass "deliberate stop: no restart, watchdog gone"

# --- a restart within the same boot keeps the boot number -------------------
ts start >/dev/null
ts stop >/dev/null
[ -d "$CARD/00002" ] && fail "a restart without a reboot counted as a new boot"
pass "restart in the same boot stays boot 1"

# --- reboots number up, and only the newest DBG_KEEP survive ---------------
for b in 2 3 4 5; do
    boot
    ts start >/dev/null
    ts stop >/dev/null
done
[ -d "$CARD/00005" ] || fail "boot 5 not recorded"
[ -d "$CARD/00001" ] && fail "boot 1 not pruned with TS_DEBUG_KEEP=3"
[ -d "$CARD/00002" ] && fail "boot 2 not pruned with TS_DEBUG_KEEP=3"
[ -d "$CARD/00003" ] || fail "boot 3 pruned too early"
pass "boots numbered across reboots; pruned to TS_DEBUG_KEEP"

# --- off again --------------------------------------------------------------
ts debug off >/dev/null
[ -f "$WORK/state/debug.conf" ] && fail "debug off left the conf"
boot
ts start >/dev/null
[ -f "$WORK/run/ts_watchdog.pid" ] && fail "watchdog started after debug off"
ts stop >/dev/null
pass "debug off: back to the old behaviour"

# --- turned on mid-boot, then `restart` (what happened on the unit) ----------
# The stop half runs before debug mode has numbered the boot; it must not
# file that under a bogus boot 0.
rm -rf "$WORK/state" "$CARD"
boot
ts start >/dev/null
ts debug on "$CARD" >/dev/null
ts restart >/dev/null
[ -d "$CARD/00000" ] && fail "a stop before the first debug start made a boot-0 directory"
[ -f "$CARD/00001/tailscaled.0.log" ] || fail "the pre-debug instance's log was not kept under boot 1"
ts stop >/dev/null
pass "debug on mid-boot + restart: no boot 0, earlier log kept"

echo "all tailscale.sh debug tests passed"

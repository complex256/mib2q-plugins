#!/bin/sh
# mibwg control script for the MHI2q head unit.
#
#   mibwg.sh start | stop | restart | status | log | edit
#
# edit: open mibwg.conf in vi (remounting its filesystem read-write for the
# duration), validate it, and restart mibwg only if it is valid.
#
# Lives next to mibwg and mibwg.conf in a persistent directory (default: this
# script's own). Persistent storage on the unit is not mmap-capable for exec,
# so start copies the binary to /tmp (/dev/shmem, RAM) and runs it from there.
# /tmp is cleared between sessions, which is why every start re-copies.
#
# The unit's userland is sparse: no dirname, tail, nohup or kill binary (ksh's
# builtin kill is used). Stick to ksh builtins plus cat/cp/chmod/rm/sed/sleep.
case $0 in */*) here=${0%/*} ;; *) here=. ;; esac
DIR=${MIBWG_DIR:-$(cd "$here" && pwd)}
CONF=${MIBWG_CONF:-$DIR/mibwg.conf}
BIN=/tmp/mibwg
PID=/tmp/mibwg.pid
STATUS=/tmp/mibwg.status
# Log in RAM by default - no flash wear. Set MIBWG_LOG to a persistent path
# when debugging something that happens across reboots.
LOG=${MIBWG_LOG:-/tmp/mibwg.log}
# The persistent filesystems are mounted read-only; edit remounts this one.
MNT=${MIBWG_MOUNT:-/mnt/app}

# Function names are prefixed: QNX's ksh predefines aliases such as
# stop='kill -STOP', and aliases expand even in a function definition.
# lastlines N FILE - tail(1) stand-in.
lastlines() {
    [ -f "$2" ] && sed -e :a -e "\$q;N;$(($1 + 1)),\$D;ba" "$2"
}

mibwg_running() {
    [ -f "$PID" ] && kill -0 "$(cat "$PID")" 2>/dev/null
}

mibwg_start() {
    if mibwg_running; then
        echo "mibwg already running (pid $(cat "$PID"))"
        return 0
    fi
    [ -f "$CONF" ] || { echo "no config at $CONF" >&2; return 1; }
    mibwg_install || return 1
    "$BIN" check "$CONF" || return 1
    # No nohup on the unit; `mibwg up` ignores SIGHUP itself, so it outlives
    # the SSH session that started it.
    "$BIN" up -status "$STATUS" -log "$LOG" "$CONF" </dev/null >/dev/null 2>&1 &
    echo $! > "$PID"
    sleep 2
    if mibwg_running; then
        echo "mibwg started (pid $(cat "$PID")), log $LOG"
    else
        echo "mibwg exited during startup:" >&2
        lastlines 5 "$LOG" >&2
        return 1
    fi
}

# mibwg_install copies the binary to /tmp, the only place the unit can exec
# it from. Running from /tmp itself is fine - nothing to copy. -ef, not a
# string compare: /tmp is a symlink to /dev/shmem, so pwd reports the latter.
mibwg_install() {
    if [ ! "$DIR/mibwg" -ef "$BIN" ]; then
        cp "$DIR/mibwg" "$BIN" || return 1
    fi
    chmod 755 "$BIN"
}

mibwg_edit() {
    mibwg_install || return 1
    mount -uw "$MNT" || { echo "could not remount $MNT read-write" >&2; return 1; }
    ${EDITOR:-vi} "$CONF"
    sync
    mount -ur "$MNT"
    if "$BIN" check "$CONF"; then
        mibwg_stop
        mibwg_start
    else
        echo "config is invalid - not restarting; run '$0 edit' again to fix it" >&2
        return 1
    fi
}

mibwg_stop() {
    if ! mibwg_running; then
        echo "mibwg not running"
        rm -f "$PID"
        return 0
    fi
    kill "$(cat "$PID")"
    i=0
    while mibwg_running && [ $i -lt 10 ]; do sleep 1; i=$((i + 1)); done
    mibwg_running && kill -9 "$(cat "$PID")"
    rm -f "$PID"
    echo "mibwg stopped"
}

# mibpkg service mode: copy the binary to /tmp, validate the config and exec
# the daemon in the foreground. The first argument after `serve` is the config
# path (mibpkg passes @config/mibwg.conf).
mibwg_serve() {
    if [ -n "${2:-}" ]; then
        CONF=$2
    fi
    [ -f "$CONF" ] || { echo "no config at $CONF" >&2; return 1; }
    mibwg_install || return 1
    "$BIN" check "$CONF" || return 1
    exec "$BIN" up -status "$STATUS" -log "$LOG" "$CONF"
}

case $1 in
    start) mibwg_start ;;
    stop) mibwg_stop ;;
    restart) mibwg_stop; mibwg_start ;;
    status)
        if mibwg_running; then
            echo "mibwg running (pid $(cat "$PID"))"
            if [ -f "$STATUS" ]; then cat "$STATUS"; fi
        else
            echo "mibwg not running"
            exit 1
        fi ;;
    log) lastlines 50 "$LOG" ;;
    edit) mibwg_edit ;;
    serve) mibwg_serve "$@" ;;
    *) echo "usage: $0 start|stop|restart|status|log|edit|serve" >&2; exit 2 ;;
esac

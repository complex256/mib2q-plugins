#!/bin/sh
# carmqtt control script for the MHI2q head unit.
#
#   carmqtt.sh start | stop | restart | status | log
#
# The unit has no dirname, tail, nohup or date; ksh predefines aliases such
# as stop='kill -STOP', so every function here is prefixed with cm_.
# Persistent storage can't exec binaries, so the binary is copied to /tmp
# (RAM) and run from there. A closed ssh session sends SIGHUP, so the
# restart loop ignores it.
case $0 in */*) here=${0%/*} ;; *) here=. ;; esac
DIR=${CM_DIR:-$(cd "$here" && pwd)}
# The unit's /tmp is /dev/shmem, which cannot hold directories (mkdir fails
# with ENOSYS), so every runtime file is a flat, carmqtt-prefixed name in it.
RUN=${CM_RUN:-/tmp}
LOOPPID=$RUN/carmqtt.loop.pid
PID=$RUN/carmqtt.pid
WRAPLOG=$RUN/carmqtt.wrapper.log

cm_running() {
    [ -f "$LOOPPID" ] && kill -0 "$(cat "$LOOPPID")" 2>/dev/null
}

# Wait up to 5s for pid $1 to stop existing.
cm_wait_gone() {
    i=0
    while kill -0 "$1" 2>/dev/null && [ $i -lt 5 ]; do
        sleep 1
        i=$((i + 1))
    done
}

cm_loop() {
    while :; do
        "$RUN/carmqtt" --config "$DIR/carmqtt.toml" &
        echo $! > "$PID"
        wait $!
        echo "carmqtt exited with status $?; restarting in 10s"
        sleep 10
        # /tmp is RAM and this is the only record of every carmqtt run's
        # output, so it grows forever unless bounded. Simple cap: start each
        # relaunch from empty (cm_start already keeps one previous copy from
        # before the loop began).
        [ -f "$WRAPLOG" ] && : > "$WRAPLOG"
    done
}

cm_start() {
    if cm_running; then
        echo "carmqtt already running (loop pid $(cat "$LOOPPID"))"
        return 0
    fi
    # mkdir -p on an existing /tmp fails on the unit, so only create RUN if absent.
    [ -d "$RUN" ] || mkdir -p "$RUN" || return 1
    cp "$DIR/carmqtt" "$RUN/carmqtt.new" || return 1
    chmod 755 "$RUN/carmqtt.new"
    mv "$RUN/carmqtt.new" "$RUN/carmqtt" || return 1
    # Keep one previous wrapper.log across restarts (evidence of the last
    # run survives one restart), then start this run's from empty.
    [ -f "$WRAPLOG" ] && cp "$WRAPLOG" "$WRAPLOG.prev"
    : > "$WRAPLOG"
    ( trap '' HUP; cm_loop ) </dev/null >>"$WRAPLOG" 2>&1 &
    echo $! > "$LOOPPID"
    echo "carmqtt started (loop pid $!), log $RUN/carmqtt.log"
}

cm_stop() {
    if ! cm_running; then
        echo "carmqtt not running"
        rm -f "$LOOPPID" "$PID"
        return 0
    fi
    looppid=$(cat "$LOOPPID")
    daemonpid=
    [ -f "$PID" ] && daemonpid=$(cat "$PID")
    kill "$looppid" 2>/dev/null
    [ -n "$daemonpid" ] && kill "$daemonpid" 2>/dev/null
    # Give both a moment to actually exit before the pid files (and the
    # "not running" answer they imply) disappear out from under a restart.
    cm_wait_gone "$looppid"
    [ -n "$daemonpid" ] && cm_wait_gone "$daemonpid"
    rm -f "$LOOPPID" "$PID"
    echo "carmqtt stopped"
}

cm_status() {
    if cm_running; then
        echo "carmqtt running (loop pid $(cat "$LOOPPID"), daemon pid $(cat "$PID" 2>/dev/null))"
    else
        echo "carmqtt not running"
        return 1
    fi
}

case $1 in
    start) cm_start ;;
    stop) cm_stop ;;
    restart) cm_stop; cm_start ;;
    status) cm_status ;;
    log) cat "$RUN/carmqtt.log" ;;
    *) echo "usage: $0 start|stop|restart|status|log" >&2; exit 2 ;;
esac

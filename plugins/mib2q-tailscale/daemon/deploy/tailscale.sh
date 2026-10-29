#!/bin/sh
# tailscaled control script for the MHI2q head unit.
#
#   tailscale.sh start | stop | restart | status | log
#   tailscale.sh debug on [DIR] | off | status | snapshot
#
# Constraints this script works within (all verified on a unit): no dirname,
# tail, nohup, date or kill binary; ksh predefines aliases such as
# stop='kill -STOP', so every function here is prefixed; persistent storage
# is not mmap-capable for exec, so binaries run from /tmp (RAM, cleared
# between sessions), while state lives in /mnt/persist (flash).
case $0 in */*) here=${0%/*} ;; *) here=. ;; esac
DIR=${TS_DIR:-$(cd "$here" && pwd)}
STATE=${TS_STATE:-/mnt/persist/tailscale}
# RAM directory for the binaries and everything that only lives for one boot.
# Always /tmp on the unit; overridable so the host test can run this script.
RUN=${TS_RUN:-/tmp}
SOCK=$RUN/tailscaled.sock
PID=$RUN/tailscaled.pid
# tsstatus: loopback status endpoint (127.0.0.1:19850) for the plugin SDK.
STPID=$RUN/tsstatus.pid
LOG=${TS_LOG:-$RUN/tailscaled.log}
# Offset in whole seconds between this unit's clock and network time, learned
# by sntp and applied inside tailscaled by libtsclock.so. In RAM: it is only
# valid for the current boot, and the clock resets to 1970 on the next one.
OFFSET=${TS_OFFSET:-$RUN/ts_clock_offset}
# NTP sources, tried in order. The anycast literals come first because at boot
# the local resolver is not up yet: every boot logged
#   sntp: pool.ntp.org: dial udp: lookup pool.ntp.org on [::1]:53: server misbehaving
# lost the foreground attempt, and paid for it with a ~60 s background retry and
# a tailscaled restart. The uplink itself is up by then - all four addresses
# below answered on the first try from the unit, agreeing to the second.
#
# These are documented anycast addresses (Cloudflare 162.159.200.x, Google
# 216.239.35.x) meant to be used directly. pool.ntp.org deliberately has no
# stable address - it is DNS round-robin over volunteer servers and the project
# asks you not to pin member IPs - so it stays here as a hostname fallback.
NTP_SERVERS=${TS_NTP:-"162.159.200.123 216.239.35.0 pool.ntp.org time.google.com"}
# How long start waits before checking tailscaled survived. A knob for the
# host test only.
START_WAIT=${TS_START_WAIT:-5}

# Debug mode: a watchdog plus a log that survives the car being switched off.
# OFF unless $STATE/debug.conf sets TS_DEBUG=1 (`tailscale.sh debug on`
# writes it). With it off, nothing below runs and start/stop behave exactly
# as they did before this existed. The conf lives in /mnt/persist, so it
# survives a redeploy of /mnt/app and is never set by an install.
#
# The conf is sourced, so any TS_DEBUG_* / TS_WD_* knob below can go in it:
#   TS_DEBUG=1
#   TS_DEBUG_DIR=/fs/sdb0/tailscale-debug   # default: first writable SD card
#   TS_DEBUG_MOUNT=/fs/sdb0                 # remounted rw if DIR isn't writable
# Subnet routes: ON by default. The car reaches its home MQTT broker through
# the tailnet's subnet router, so `tailscale up` passes --accept-routes. A unit
# whose LAN peers fall inside an advertised route (the bench: its laptop and Pi
# sit inside a 192.168.0.0/16 home route, so once logged in the unit answers
# them through the tunnel and drops off its LAN) opts out per unit with
#   echo 0 >/mnt/persist/tailscale/accept-routes
# ("0", "accept-routes=0", "false", "no" or "off"; anything else, or no file,
# keeps the default). TS_ACCEPT_ROUTES overrides the file. It lives in
# /mnt/persist, so a redeploy of /mnt/app never changes it, and it applies at
# the next start (or live: tailscale --socket=/tmp/tailscaled.sock set
# --accept-routes=false).
ts_accept_routes() {
    # mibpkg service mode passes the read-only @config dir in CONF; manual
    # starts keep the old /mnt/persist/tailscale/accept-routes location.
    routes_conf=${CONF:-$STATE}/accept-routes
    tar_v=${TS_ACCEPT_ROUTES:-}
    if [ -z "$tar_v" ] && [ -f "$routes_conf" ]; then
        read tar_v <"$routes_conf"
    fi
    tar_v=${tar_v#accept-routes=}
    case $tar_v in
        0|false|no|off) echo "--accept-routes=false" ;;
        *) echo "--accept-routes" ;;
    esac
}

DEBUG_CONF=$STATE/debug.conf
TS_DEBUG=0
[ -f "$DEBUG_CONF" ] && . "$DEBUG_CONF"
# Boots of history to keep on the card, one directory each.
DBG_KEEP=${TS_DEBUG_KEEP:-30}
# Watchdog: probe every WD_INTERVAL s; WD_FAILS probes in a row that get no
# answer within WD_PROBE_TIMEOUT s is a wedge. WD_GRACE is the quiet period
# after each start. A wedge or a death is followed by a restart, at most
# WD_MAX_RESTARTS times per watchdog (TS_WD_RESTART=0 to capture only).
WD_INTERVAL=${TS_WD_INTERVAL:-30}
WD_PROBE_TIMEOUT=${TS_WD_PROBE_TIMEOUT:-20}
WD_FAILS=${TS_WD_FAILS:-2}
WD_GRACE=${TS_WD_GRACE:-60}
WD_RESTART=${TS_WD_RESTART:-1}
WD_MAX_RESTARTS=${TS_WD_MAX_RESTARTS:-3}
# Heartbeat + copy to the card every this many probes (10 x 30 s = 5 min).
WD_FLUSH_EVERY=${TS_WD_FLUSH_EVERY:-10}
# Everything is written to RAM first and copied to the card by ts_dbg_flush,
# because at boot the card may not be mounted yet and a write to it must
# never be able to hold up or fail a start.
EVENTS=$RUN/ts_events.log
BOOTMARK=$RUN/ts_boot
INSTANCE=$RUN/ts_instance
WDPID=$RUN/ts_watchdog.pid
# Present while a deliberate stop/start is in flight, so the watchdog does
# not mistake it for a death and race it with a restart of its own.
INTENT=$RUN/ts_intent
BOOTSEQ=$STATE/debug.bootseq

ts_dbg() {
    [ "$TS_DEBUG" = 1 ]
}

# The unit's clock is never stepped (see ts_clock_once), so it reads 1970 plus
# time since power-on: ls -l of a freshly touched file is uptime to the
# minute, with no date(1) needed. That is exactly what tells a cold boot from
# a car restart where the unit only slept: uptime resets on one and not the
# other.
ts_clk() {
    : >"$RUN/ts_clk" 2>/dev/null
    set -- $(ls -l "$RUN/ts_clk" 2>/dev/null)
    echo "$6-$7-$8"
}

ts_bootid() {
    tbi=?
    [ -f "$BOOTMARK" ] && read tbi <"$BOOTMARK"
    echo "$tbi"
}

ts_instance() {
    tin=0
    [ -f "$INSTANCE" ] && read tin <"$INSTANCE"
    echo "$tin"
}

# ts_event MESSAGE: one line in the debug timeline, and the same line in
# tailscaled's log so it lands between tailscaled's own timestamped lines.
ts_event() {
    ts_dbg || return 0
    tev="[up $(ts_clk) boot $(ts_bootid).$(ts_instance)] $*"
    echo "$tev" >>"$EVENTS" 2>/dev/null
    [ -f "$LOG" ] && echo "ts-debug: $tev" >>"$LOG" 2>/dev/null
    return 0
}

ts_pad5() {
    case $1 in
        ?) echo "0000$1" ;;
        ??) echo "000$1" ;;
        ???) echo "00$1" ;;
        ????) echo "0$1" ;;
        *) echo "$1" ;;
    esac
}

# First start since power-on? /tmp is RAM, so a missing marker means the unit
# really rebooted. Number the boot from a counter in flash (one tiny write
# per boot), and record what the unit looked like at that moment.
ts_dbg_boot() {
    ts_dbg || return 0
    [ -f "$BOOTMARK" ] && return 0
    tdb_n=0
    [ -f "$BOOTSEQ" ] && read tdb_n <"$BOOTSEQ"
    case $tdb_n in '' | *[!0-9]*) tdb_n=0 ;; esac
    tdb_n=$((tdb_n + 1))
    echo "$tdb_n" >"$BOOTSEQ" 2>/dev/null
    echo "$tdb_n" >"$BOOTMARK"
    echo 0 >"$INSTANCE"
    ts_event "boot: first start since power-on (no marker in $RUN)"
    {
        echo "== pidin info"; pidin info
        echo "== mount"; mount
        echo "== ifconfig -a"; ifconfig -a
        echo "== pidin (every process and thread)"; pidin
    } >"$RUN/ts_bootinfo.txt" 2>&1
}

# Where the durable copy goes, or fail. Remounts the card read-write if it
# has to; the answer is cached for the rest of the boot.
ts_dbg_dir() {
    if [ -f "$RUN/ts_dbgdir" ]; then
        cat "$RUN/ts_dbgdir"
        return 0
    fi
    if [ -n "$TS_DEBUG_DIR" ]; then
        tdd_try="$TS_DEBUG_DIR:$TS_DEBUG_MOUNT"
    else
        # Where this unit mounts its two SD slots (both FAT32; navigation
        # lives on eMMC at /mnt/navdb, so neither card is the map card).
        tdd_try="/fs/sda0/tailscale-debug:/fs/sda0 /fs/sdb0/tailscale-debug:/fs/sdb0"
    fi
    for tdd_c in $tdd_try; do
        tdd_d=${tdd_c%%:*}
        tdd_m=${tdd_c#*:}
        # The probe write is in a subshell on purpose: `:` is a special
        # builtin, and on the unit's ksh a failed redirection on one exits the
        # whole shell - which is what a read-only card does at every boot.
        if ! { mkdir -p "$tdd_d" && ( : >"$tdd_d/.w" ); } 2>/dev/null; then
            [ -n "$tdd_m" ] && [ -d "$tdd_m" ] || continue
            mount -uw "$tdd_m" 2>/dev/null || continue
            { mkdir -p "$tdd_d" && ( : >"$tdd_d/.w" ); } 2>/dev/null || continue
            ts_event "debug: remounted $tdd_m read-write"
        fi
        rm -f "$tdd_d/.w"
        echo "$tdd_d" >"$RUN/ts_dbgdir"
        echo "$tdd_d"
        return 0
    done
    return 1
}

# Keep the newest DBG_KEEP boot directories (names are zero-padded, so the
# glob's order is their age).
ts_dbg_prune() {
    set -- "$1"/[0-9][0-9][0-9][0-9][0-9]
    [ -d "$1" ] || return 0
    tdp_n=$(($# - DBG_KEEP))
    while [ $tdp_n -gt 0 ]; do
        rm -rf "$1"
        shift
        tdp_n=$((tdp_n - 1))
    done
}

# Copy this boot's RAM-side evidence to the card. Best effort: a missing card
# or a failed write only means the copy waits for the next flush.
ts_dbg_flush() {
    ts_dbg || return 0
    tdf_root=$(ts_dbg_dir) || return 0
    # No boot number yet (a stop before debug mode's first start this boot):
    # nothing to file it under, and the next start flushes the same log.
    tdf_b=$(ts_bootid)
    case $tdf_b in '' | *[!0-9]*) return 0 ;; esac
    tdf_s=$tdf_root/$(ts_pad5 "$tdf_b")
    if [ ! -d "$tdf_s" ]; then
        mkdir -p "$tdf_s" 2>/dev/null || return 0
        ts_dbg_prune "$tdf_root"
    fi
    [ -f "$EVENTS" ] && cp "$EVENTS" "$tdf_s/events.log" 2>/dev/null
    [ -f "$RUN/ts_bootinfo.txt" ] && cp "$RUN/ts_bootinfo.txt" "$tdf_s/bootinfo.txt" 2>/dev/null
    for tdf_f in "$RUN"/ts_capture.*.txt; do
        [ -f "$tdf_f" ] && cp "$tdf_f" "$tdf_s/${tdf_f##*/ts_}" 2>/dev/null
    done
    [ -f "$LOG" ] && cp "$LOG" "$tdf_s/tailscaled.$(ts_instance).log" 2>/dev/null
    sync 2>/dev/null
    return 0
}

ts_wd_running() {
    [ -f "$WDPID" ] && kill -0 "$(cat "$WDPID")" 2>/dev/null
}

ts_wd_start() {
    ts_dbg || return 0
    ts_wd_running && return 0
    (
        trap '' HUP
        ts_watchdog
    ) </dev/null >>"$EVENTS" 2>&1 &
    echo $! >"$WDPID"
}

ts_wd_stop() {
    ts_wd_running && kill "$(cat "$WDPID")" 2>/dev/null
    rm -f "$WDPID"
}

# ts_wd_probe: does tailscaled's LocalAPI answer at all? In the known wedge
# (a GC stop-the-world that never completes) the socket still accepts, the
# kernel queues the request, and nothing ever replies - so the test is
# whether the CLI *finishes* in time, whatever it says. The CLI is a separate
# process with a working scheduler, so it can be timed out from here.
ts_wd_probe() {
    "$RUN/tailscale" --socket="$SOCK" status --peers=false >"$RUN/ts_probe.out" 2>&1 &
    twp_p=$!
    twp_i=0
    while kill -0 "$twp_p" 2>/dev/null; do
        if [ $twp_i -ge "$WD_PROBE_TIMEOUT" ]; then
            kill -9 "$twp_p" 2>/dev/null
            wait "$twp_p" 2>/dev/null
            return 1
        fi
        sleep 1
        twp_i=$((twp_i + 1))
    done
    wait "$twp_p" 2>/dev/null
    return 0
}

# Thread states and CPU time, twice, a few seconds apart. The wedge's
# fingerprint: every thread in SEM, none in REPLY, CPU barely moving.
ts_wd_pidin() {
    twi_p=$1
    echo "== pidin -p $twi_p"; pidin -p "$twi_p"
    echo "== pidin -p $twi_p times"; pidin -p "$twi_p" times
    sleep 3
    echo "== pidin -p $twi_p times (3 s later)"; pidin -p "$twi_p" times
    echo "== ifconfig tun9"; ifconfig tun9
}

# ts_wd_capture REASON: save everything worth having about the current
# instance, then (for a wedge) make it dump every goroutine and die.
ts_wd_capture() {
    twc_why=$1
    twc_pid=
    [ -f "$PID" ] && read twc_pid <"$PID"
    twc_f=$RUN/ts_capture.$(ts_instance).txt
    {
        echo "== $twc_why"
        echo "== last probe output"; cat "$RUN/ts_probe.out"
        [ -n "$twc_pid" ] && ts_wd_pidin "$twc_pid"
    } >"$twc_f" 2>&1
    if [ "$twc_why" = wedged ] && [ -n "$twc_pid" ]; then
        # SIGQUIT makes the Go runtime print every goroutine (GOTRACEBACK=system
        # adds the runtime's own) to stderr, i.e. $LOG, and exit. It worked on
        # a live wedge before: the signal handler doesn't need the scheduler.
        ts_event "watchdog: sending SIGQUIT to $twc_pid for a goroutine dump"
        kill -QUIT "$twc_pid" 2>/dev/null
        twc_i=0
        while kill -0 "$twc_pid" 2>/dev/null && [ $twc_i -lt 15 ]; do
            sleep 1
            twc_i=$((twc_i + 1))
        done
        if kill -0 "$twc_pid" 2>/dev/null; then
            ts_event "watchdog: $twc_pid ignored SIGQUIT for 15 s; SIGKILL"
            kill -9 "$twc_pid" 2>/dev/null
        fi
        rm -f "$PID"
    fi
    ts_dbg_flush
}

ts_wd_recover() {
    if [ "$WD_RESTART" != 1 ]; then
        ts_event "watchdog: not restarting (TS_WD_RESTART=$WD_RESTART)"
        return 1
    fi
    if [ "$twd_restarts" -ge "$WD_MAX_RESTARTS" ]; then
        ts_event "watchdog: already restarted $twd_restarts times this boot; leaving it down"
        return 1
    fi
    twd_restarts=$((twd_restarts + 1))
    ts_event "watchdog: restart $twd_restarts/$WD_MAX_RESTARTS"
    ts_start >/dev/null 2>&1
    sleep "$WD_GRACE"
    twd_last=$SECONDS
}

ts_watchdog() {
    twd_fails=0
    twd_restarts=0
    twd_tick=0
    twd_down=
    ts_event "watchdog: up; probing every ${WD_INTERVAL}s after a ${WD_GRACE}s grace"
    sleep "$WD_GRACE"
    twd_last=$SECONDS
    while :; do
        # A sleep that took far longer than asked means the whole unit was
        # suspended - the car went off and came back without a reboot.
        twd_gap=$((SECONDS - twd_last))
        if [ $twd_gap -gt $((WD_INTERVAL * 3 + WD_PROBE_TIMEOUT)) ]; then
            ts_event "watchdog: ${twd_gap}s went by in one ${WD_INTERVAL}s cycle; the unit was suspended and resumed without a reboot"
            ts_dbg_flush
        fi
        if [ -f "$INTENT" ]; then
            :   # a deliberate stop/start is in flight; not ours to judge
        elif ! ts_running; then
            if [ -z "$twd_down" ]; then
                ts_event "watchdog: tailscaled is gone and nobody stopped it"
                ts_wd_capture died
                ts_wd_recover || twd_down=1
            fi
        elif ts_wd_probe; then
            twd_down=
            [ $twd_fails -gt 0 ] && ts_event "watchdog: LocalAPI answered again after $twd_fails missed probe(s)"
            twd_fails=0
        else
            twd_fails=$((twd_fails + 1))
            ts_event "watchdog: probe $twd_fails/$WD_FAILS got no answer in ${WD_PROBE_TIMEOUT}s"
            if [ $twd_fails -ge "$WD_FAILS" ]; then
                ts_event "watchdog: tailscaled is WEDGED; capturing"
                ts_wd_capture wedged
                twd_fails=0
                ts_wd_recover || twd_down=1
            fi
        fi
        twd_tick=$((twd_tick + 1))
        if [ $((twd_tick % WD_FLUSH_EVERY)) -eq 0 ]; then
            ts_event "watchdog: heartbeat; log $(ts_filesize "$LOG") bytes, $twd_restarts restart(s)"
            ts_dbg_flush
        fi
        twd_last=$SECONDS
        sleep "$WD_INTERVAL"
    done
}

ts_lastlines() {
    [ -f "$2" ] && sed -e :a -e "\$q;N;$(($1 + 1)),\$D;ba" "$2"
}

ts_running() {
    [ -f "$PID" ] && kill -0 "$(cat "$PID")" 2>/dev/null
}

# Size of $1 in bytes, or empty if it can't be determined. The unit has no
# wc(1), so `wc -c` yields nothing here; ls -l's size field is the fallback.
# Callers must treat empty as "unknown", never as zero.
ts_filesize() {
    tsz_f=$1
    tsz_n=$(wc -c <"$tsz_f" 2>/dev/null)
    # wc pads its output ("       5"), so split on whitespace to get the
    # bare number. This also has to happen before the ls fallback reuses
    # the positional parameters, which is why the name is saved above.
    set -- $tsz_n
    tsz_n=$1
    if [ -z "$tsz_n" ]; then
        set -- $(ls -l "$tsz_f" 2>/dev/null)
        tsz_n=$5
    fi
    # $5 is only a size if ls -l has the column count we expect. Anything
    # else (a missing group column, an ACL marker splitting a field) would
    # hand back something like "Sep" for both files being compared, they
    # would compare equal, and ts_install would skip a copy it needed to
    # make - leaving the unit running the old binary after a deploy, with
    # nothing said. Non-numeric means unknown.
    case $tsz_n in
        '' | *[!0-9]*) tsz_n= ;;
    esac
    echo "$tsz_n"
}

ts_install() {
    for b in tailscaled tailscale sntp tsstatus libtsclock.so; do
        # tsstatus is optional: a deploy without it must still bring the node up.
        [ "$b" = tsstatus ] && [ ! -f "$DIR/$b" ] && continue
        src="$DIR/$b"
        dst="$RUN/$b"
        # The flash copy and the /tmp (RAM) copy are never the same file
        # (different filesystems), so an -ef check here is never true and
        # every start re-copies ~22 MB into RAM for nothing. Compare sizes
        # instead: not a content hash, but cheap and honest about what it
        # is actually checking, and good enough to skip the copy when the
        # deployed binary hasn't changed. An unknown size (empty) copies,
        # which is the safe direction.
        srcsize=$(ts_filesize "$src")
        dstsize=$(ts_filesize "$dst")
        if [ -z "$srcsize" ] || [ -z "$dstsize" ] || [ "$srcsize" != "$dstsize" ]; then
            if [ "$b" = tsstatus ]; then
                # Never let the optional sidecar block the daemon. It may be
                # running right now (watchdog restart, late-offset restart),
                # and cp over a running binary fails "text file busy": copy
                # beside it and rename over it, which a running image allows.
                # A failure leaves the old copy in place and is only logged.
                if cp "$src" "$dst.new" 2>/dev/null && chmod 755 "$dst.new" && mv "$dst.new" "$dst"; then
                    :
                else
                    rm -f "$dst.new"
                    ts_event "install: tsstatus not updated; keeping the old copy"
                fi
                continue
            fi
            cp "$src" "$dst" || return 1
        fi
        chmod 755 "$dst" 2>/dev/null || [ "$b" = tsstatus ]
    done
}

# The RTC boots at 1970 and TLS to the control plane fails outright against
# that clock, so something has to set it before tailscaled is any use. At
# boot this script runs 30s after inetd, which is easily before the uplink is
# usable - and sntp was a single shot, so a boot that missed that window left
# the node offline until a human ran this script by hand. Nothing ever
# re-set the clock.
#
# Two stages. One foreground attempt (sntp itself tries 3 times against each
# of 2 servers, ~68s worst case) so the common case starts with a good clock,
# then a background retrier only if that missed. tailscaled retries the
# control plane indefinitely, so it picks up a late correction on its own; no
# restart needed. Both stages log to $LOG, because at boot this script's
# stdout goes to /dev/null and the log is the only evidence anyone gets.
ts_clock_bg() {
    i=1
    while [ $i -le 30 ]; do
        sleep 60
        if ts_clock_once; then
            echo "sntp: offset learned on background attempt $i; restarting tailscaled to apply it"
            # The shim reads TS_CLOCK_OFFSET once, at load, so a late offset
            # only reaches a fresh process. Restarting is the honest way to
            # apply it - tailscaled cannot pick this up on its own the way it
            # used to when the system clock was being stepped underneath it.
            # Only the daemon restarts: a debug watchdog stays up across it,
            # told by $INTENT that this stop is deliberate.
            ts_event "sntp: late offset on background attempt $i; restarting tailscaled"
            : >"$INTENT"
            ts_stop_daemon
            ts_start
            return 0
        fi
        i=$((i + 1))
    done
    echo "sntp: giving up after 30 background attempts; this node will not authenticate"
    return 1
}

# ts_clock_once asks sntp how far this unit's clock is behind network time and
# records the answer for the shim. It deliberately does NOT set the clock.
#
# Stepping the system clock is what the old code did, and it is why the unit's
# navigation restarted about a minute into every boot: the step expires every
# pending CLOCK_REALTIME wait at once, which fires the HMI's
# NavInitializationChecker, which concludes navigation never came up and
# restarts the whole navigation domain. Measured across four boots, the timer
# fired within a second of the step every time - once only 48 ms after it.
ts_clock_once() {
    # Unquoted on purpose: NTP_SERVERS is a list of words, one per server.
    offset=$("$RUN/sntp" -print $NTP_SERVERS 2>>"$LOG") || return 1
    case $offset in
        ''|*[!0-9-]*) echo "sntp: ignoring malformed offset '$offset'"; return 1 ;;
    esac
    echo "$offset" > "$OFFSET" || return 1
    echo "sntp: clock is $offset s behind network time; correcting inside tailscaled only"
    return 0
}

ts_clock() {
    if ts_clock_once; then
        return 0
    fi
    echo "sntp: first attempt failed (uplink not up yet?); retrying in the background"
    ts_clock_bg &
    return 1
}

ts_start() {
    if ts_running; then
        echo "tailscaled already running (pid $(cat "$PID"))"
        return 0
    fi
    ts_dbg_boot
    ts_event "start: begin"
    : >"$INTENT"
    ts_start_daemon
    tss_rc=$?
    rm -f "$INTENT"
    ts_event "start: done (status $tss_rc)"
    ts_dbg_flush
    return $tss_rc
}

ts_start_daemon() {
    if ! ts_install; then
        ts_event "start: copying the binaries to $RUN failed"
        return 1
    fi
    mkdir -p "$STATE"
    # /tmp is RAM (/dev/shmem) and this is the only record of tailscaled's
    # output (ts_omit_logtail removed the network log path), so it grows
    # forever unless something bounds it. There is no truncate(1)/tail(1)
    # on this unit, so start each run from an empty file; ts_status reports
    # its size so growth between starts is at least visible.
    #
    # Keep ONE previous log. Truncating outright destroyed the evidence of
    # the exact failure worth diagnosing: when a boot-time start hangs, the
    # first thing anyone does is restart, and that restart used to wipe the
    # log of the boot that failed. The previous run's output survives one
    # restart now, which is all it takes to look at it.
    #
    # In debug mode the outgoing log is first copied to the card under its
    # own instance number, so no restart ever loses one.
    if [ -f "$LOG" ]; then
        ts_dbg_flush
        cp "$LOG" "$LOG.prev"
    fi
    ts_dbg && echo $(($(ts_instance) + 1)) >"$INSTANCE"
    : >"$LOG"
    ts_clock >>"$LOG" 2>&1
    # No nohup on this unit: a closed ssh session sends tailscaled SIGHUP
    # and kills it. Ignore SIGHUP in the subshell before exec'ing into
    # tailscaled; exec replaces the subshell's process image in place (same
    # pid, no fork), and an ignored signal's disposition survives exec, so
    # $! below is tailscaled's own pid, not some wrapper's.
    # The unit's clock stays at 1970 on purpose (see ts_clock_once). tailscaled
    # gets the truth privately: libtsclock.so offsets CLOCK_REALTIME for this
    # process only, so TLS to the control plane, key expiry and wireguard-go's
    # handshake timestamps all see real time while nothing else on the unit
    # notices. CLOCK_MONOTONIC is passed through untouched.
    tsoffset=0
    [ -f "$OFFSET" ] && read tsoffset < "$OFFSET"
    ts_event "start: clock offset ${tsoffset}s; launching tailscaled"
    (
        trap '' HUP
        # Debug mode: put the runtime's own goroutines in a crash or SIGQUIT
        # dump too, since the wedge worth catching is in the scheduler.
        # (This background job inherits SIGQUIT ignored, and no shell will
        # undo that; the Go runtime installs its SIGQUIT handler regardless -
        # it only honours an inherited ignore for SIGHUP and SIGINT.)
        if ts_dbg; then
            GOTRACEBACK=system
            export GOTRACEBACK
        fi
        TS_CLOCK_OFFSET="$tsoffset" LD_PRELOAD="$RUN/libtsclock.so" \
        exec "$RUN/tailscaled" --tun=tun9 --state="$STATE/tailscaled.state" --socket="$SOCK" \
            --statedir="$STATE"
    ) </dev/null >>"$LOG" 2>&1 &
    echo $! > "$PID"
    ts_event "start: tailscaled is pid $(cat "$PID")"
    sleep "$START_WAIT"
    if ! ts_running; then
        ts_event "start: tailscaled exited within ${START_WAIT}s"
        echo "tailscaled exited during startup:" >&2
        ts_lastlines 10 "$LOG" >&2
        return 1
    fi
    # Running: from here on the watchdog may judge it, including while `up`
    # below is still waiting on it.
    rm -f "$INTENT"
    ts_wd_start
    ts_st_start
    # --timeout bounds `up`: a missing key or a dead uplink must not leave
    # an orphan `tailscale up` polling forever after this script returns.
    #
    # --auth-key=file: rather than the key itself: argv is world-readable
    # through `pidin ar`, and a key pasted there leaks to anyone who can
    # list processes (it happened once during development). The file is
    # mode 600 in flash and never appears in the process table.
    #
    # `up` must not be able to fail the boot, hence the tolerated failure -
    # but a tolerated failure that nobody records is how an expired auth key
    # turns into an unexplained offline node. Its output goes to $LOG (at
    # boot this script's stdout is /dev/null), and the outcome is stated in
    # both places.
    # Every non-default pref has to be named here. `tailscale up` refuses to
    # run rather than silently reset one:
    #   Error: changing settings via 'tailscale up' requires mentioning all
    #   non-default flags.
    # --accept-routes (ts_accept_routes: on unless this unit opted out) is what
    # reaches the 192.168/16 subnet route the tailnet's subnet router advertises; leaving it off this line failed every start with
    # that error while the node stayed up on its persisted prefs, so the boot log
    # carried a warning that looked like an auth problem and was not one. Keep
    # this list in step with `tailscale debug prefs`.
    upfail=
    authkey=${CONF:-$STATE}/authkey
    if [ -s "$authkey" ]; then
        "$RUN/tailscale" --socket="$SOCK" up --timeout=60s --auth-key="file:$authkey" --accept-dns=false $(ts_accept_routes) >>"$LOG" 2>&1 || upfail=$?
    else
        "$RUN/tailscale" --socket="$SOCK" up --timeout=60s --accept-dns=false $(ts_accept_routes) >>"$LOG" 2>&1 || upfail=$?
    fi
    ts_event "start: tailscale up exit ${upfail:-0}"
    if [ -n "$upfail" ]; then
        echo "warning: tailscale up failed (exit $upfail); the node is running but may not be authenticated" >>"$LOG"
        echo "warning: tailscale up failed (exit $upfail); see $LOG"
    fi
    echo "tailscaled started (pid $(cat "$PID")), log $LOG"
}

# tsstatus lives and dies with tailscaled. It reports "down" on its own while
# the daemon is away, so the watchdog's in-boot restarts need not touch it;
# only start/stop do. Optional: a missing binary changes nothing else.
ts_st_running() {
    [ -f "$STPID" ] && kill -0 "$(cat "$STPID")" 2>/dev/null
}

ts_st_start() {
    ts_st_running && return 0
    [ -x "$RUN/tsstatus" ] || return 0
    (
        trap '' HUP
        exec "$RUN/tsstatus" --socket="$SOCK" --clock-offset-file="$OFFSET"
    ) </dev/null >>"$LOG" 2>&1 &
    echo $! >"$STPID"
}

ts_st_stop() {
    if ts_st_running; then
        kill "$(cat "$STPID")" 2>/dev/null
        # Wait briefly so a following start can rebind 127.0.0.1:19850.
        tsst_i=0
        while ts_st_running && [ $tsst_i -lt 3 ]; do sleep 1; tsst_i=$((tsst_i + 1)); done
        ts_st_running && kill -9 "$(cat "$STPID")" 2>/dev/null
    fi
    rm -f "$STPID"
}

# A user's stop: the watchdog goes first, or it would restart what we stop.
ts_stop() {
    ts_wd_stop
    ts_st_stop
    ts_stop_daemon
    ts_dbg_flush
}

ts_stop_daemon() {
    if ! ts_running; then
        echo "tailscaled not running"
        rm -f "$PID"
        return 0
    fi
    ts_event "stop: stopping pid $(cat "$PID")"
    kill "$(cat "$PID")"
    i=0
    while ts_running && [ $i -lt 10 ]; do sleep 1; i=$((i + 1)); done
    if ts_running; then
        ts_event "stop: pid $(cat "$PID") ignored SIGTERM for 10 s; SIGKILL"
        kill -9 "$(cat "$PID")"
    fi
    rm -f "$PID"
    echo "tailscaled stopped"
}

ts_status() {
    if [ -f "$LOG" ]; then
        logsize=$(ts_filesize "$LOG")
        [ -n "$logsize" ] || logsize="?"
        echo "log: $LOG ($logsize bytes; /tmp is RAM, watch this grow)"
    fi
    ts_dbg && echo "debug: on (boot $(ts_bootid)); see: $0 debug status"
    if ts_running; then
        echo "tailscaled running (pid $(cat "$PID"))"
        "$RUN/tailscale" --socket="$SOCK" status
    else
        echo "tailscaled not running"
        return 1
    fi
}

ts_debug() {
    case $1 in
        on)
            mkdir -p "$STATE"
            echo "TS_DEBUG=1" >"$DEBUG_CONF" || return 1
            [ -n "$2" ] && echo "TS_DEBUG_DIR=$2" >>"$DEBUG_CONF"
            [ -n "$3" ] && echo "TS_DEBUG_MOUNT=$3" >>"$DEBUG_CONF"
            echo "debug on ($DEBUG_CONF); takes effect at the next start - '$0 restart' to apply now"
            ;;
        off)
            rm -f "$DEBUG_CONF"
            echo "debug off; the watchdog and the card copy stop at the next start - '$0 restart' to apply now"
            ;;
        status)
            if ts_dbg; then
                echo "debug: on ($DEBUG_CONF)"
            else
                echo "debug: off (no TS_DEBUG=1 in $DEBUG_CONF)"
            fi
            if ts_wd_running; then
                echo "watchdog: running (pid $(cat "$WDPID"))"
            else
                echo "watchdog: not running"
            fi
            echo "boot: $(ts_bootid), tailscaled instance $(ts_instance)"
            if ! ts_dbg; then
                :   # off: don't go creating directories or remounting cards
            elif tsd_d=$(ts_dbg_dir); then
                echo "card: $tsd_d"
            else
                echo "card: no writable SD card found; evidence stays in $RUN only"
            fi
            echo "== last events"
            ts_lastlines 20 "$EVENTS"
            ;;
        snapshot)
            ts_dbg || { echo "debug is off" >&2; return 1; }
            ts_event "snapshot: requested by hand"
            ts_dbg_boot
            tsd_p=
            [ -f "$PID" ] && read tsd_p <"$PID"
            {
                echo "== snapshot by hand"
                [ -n "$tsd_p" ] && ts_wd_pidin "$tsd_p"
            } >"$RUN/ts_capture.$(ts_instance).snapshot.txt" 2>&1
            ts_dbg_flush
            echo "saved to $(ts_dbg_dir || echo "$RUN (no card)")"
            ;;
        *)
            echo "usage: $0 debug on [DIR [MOUNTPOINT]] | off | status | snapshot" >&2
            return 2
            ;;
    esac
}

# mibpkg service mode: start the daemon and stay in the foreground so the
# supervisor owns a real process. The first argument after `serve` is the
# read-only config directory (mibpkg passes @config), which holds authkey and
# accept-routes. The state directory stays writable at /mnt/persist/tailscale.
ts_serve() {
    if [ -n "${2:-}" ]; then
        CONF=$2
    fi
    ts_start || return 1
    trap 'ts_stop; exit 0' TERM INT HUP
    while ts_running; do
        sleep 5
    done
    ts_stop
}

case $1 in
    start) ts_start ;;
    stop) ts_stop ;;
    restart) ts_stop; ts_start ;;
    status) ts_status ;;
    log) ts_lastlines 50 "$LOG" ;;
    debug) shift; ts_debug "$@" ;;
    serve) ts_serve "$@" ;;
    *) echo "usage: $0 start|stop|restart|status|log|serve|debug" >&2; exit 2 ;;
esac

#!/bin/sh
# Behaviour test for the clock shim.
#
# The shim is deployed on QNX, but LD_PRELOAD and clock_gettime behave the same
# on the Linux build host, so the logic is tested natively here - fast, and it
# runs in the same container as the rest of the build. What this cannot cover
# is the QNX dynamic linker itself; that is verified on the unit.
#
#   qnxplat/clockshim/tsclock_test.sh
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# The build image puts the QNX cross tools first on PATH, so a bare `cc` here
# picks up the cross assembler and dies with "unrecognized option '--64'".
# These two compiles are deliberately native, so pin the native toolchain.
CC=${CC:-cc}
native() { PATH=/usr/bin:/bin "$CC" "$@"; }

native -shared -fPIC -O2 -o "$WORK/libtsclock.so" "$HERE/tsclock.c" -ldl
cat > "$WORK/probe.c" <<'EOF'
#include <stdio.h>
#include <time.h>
int main(void) {
    struct timespec r, m;
    clock_gettime(CLOCK_REALTIME, &r);
    clock_gettime(CLOCK_MONOTONIC, &m);
    printf("%lld %lld\n", (long long)r.tv_sec, (long long)m.tv_sec);
    return 0;
}
EOF
native -O2 -o "$WORK/probe" "$WORK/probe.c"

fail() { echo "FAIL: $1" >&2; exit 1; }
near() { # near <got> <want> <slack>
    d=$(( $1 - $2 )); [ "$d" -lt 0 ] && d=$(( -d ))
    [ "$d" -le "$3" ]
}

base=$("$WORK/probe")
base_real=${base% *}
base_mono=${base#* }

# 1. CLOCK_REALTIME is shifted by the offset.
got=$(LD_PRELOAD="$WORK/libtsclock.so" TS_CLOCK_OFFSET=1000 "$WORK/probe")
got_real=${got% *}
got_mono=${got#* }
near "$got_real" "$(( base_real + 1000 ))" 3 ||
    fail "CLOCK_REALTIME = $got_real, want ~$(( base_real + 1000 )) (base $base_real + offset 1000)"

# 2. CLOCK_MONOTONIC is untouched - Go's timers and deadlines depend on it.
near "$got_mono" "$base_mono" 3 ||
    fail "CLOCK_MONOTONIC = $got_mono, want ~$base_mono (the shim must not shift it)"

# 3. With no offset set, the shim is transparent.
got=$(LD_PRELOAD="$WORK/libtsclock.so" "$WORK/probe")
got_real=${got% *}
near "$got_real" "$base_real" 3 ||
    fail "CLOCK_REALTIME = $got_real with no TS_CLOCK_OFFSET, want ~$base_real (unmodified)"

# 4. A garbage offset is ignored rather than applied as nonsense.
got=$(LD_PRELOAD="$WORK/libtsclock.so" TS_CLOCK_OFFSET=banana "$WORK/probe")
got_real=${got% *}
near "$got_real" "$base_real" 3 ||
    fail "CLOCK_REALTIME = $got_real with a non-numeric offset, want ~$base_real (ignored)"

# ---------------------------------------------------------------------------
# sem_timedwait: absolute CLOCK_REALTIME deadlines must be shifted back.
#
# This is the regression test for the freeze diagnosed 2026-09-26. Before the
# sem_timedwait wrapper existed, a deadline built from the shifted clock was
# judged by the kernel against the real one, so with a 1000 s offset this probe
# waited 1000 s instead of 0.2 s. `timeout` is what turns that into a FAIL
# rather than a hung test run. Linux sem_timedwait takes the same absolute
# CLOCK_REALTIME deadline QNX does, so the logic under test is the same.
cat > "$WORK/semprobe.c" <<'EOF'
#include <errno.h>
#include <semaphore.h>
#include <stdio.h>
#include <time.h>

/* Prints: rc is_etimedout elapsed_ms deadline_unmodified */
int main(int argc, char **argv) {
    (void)argv;
    sem_t s;
    struct timespec d, saved, a, b;
    long ms;
    int rc;

    if (sem_init(&s, 0, 0) != 0) { printf("sem_init failed\n"); return 2; }
    if (argc > 1) {
        /* A deadline NOT built from the shifted clock, and smaller than the
         * offset. Exercises the pre-epoch clamp: unclamped (or done in an
         * unsigned time_t) this wraps to ~136 years and never returns. */
        d.tv_sec = 100;
        d.tv_nsec = 0;
    } else {
        if (clock_gettime(CLOCK_REALTIME, &d) != 0) { printf("clock_gettime failed\n"); return 2; }
        d.tv_nsec += 200000000L;
        if (d.tv_nsec >= 1000000000L) { d.tv_sec++; d.tv_nsec -= 1000000000L; }
    }
    saved = d;

    clock_gettime(CLOCK_MONOTONIC, &a);
    rc = sem_timedwait(&s, &d);
    clock_gettime(CLOCK_MONOTONIC, &b);

    ms = (b.tv_sec - a.tv_sec) * 1000 + (b.tv_nsec - a.tv_nsec) / 1000000;
    printf("%d %d %ld %d\n", rc, (rc == 0 ? 0 : errno == ETIMEDOUT), ms,
           (saved.tv_sec == d.tv_sec && saved.tv_nsec == d.tv_nsec));
    return 0;
}
EOF
native -O2 -o "$WORK/semprobe" "$WORK/semprobe.c" -lpthread

# semrun [VAR=val...] [-- probe args...]
# Echoes "rc etimedout ms unmodified", or TIMEOUT if it did not return in 5 s.
# The timeout is the point: the bug this guards against is an infinite wait, so
# without a bound the test would hang instead of failing.
semrun() {
    semenv=""
    while [ $# -gt 0 ] && [ "$1" != "--" ]; do
        semenv="$semenv $1"
        shift
    done
    [ "${1:-}" = "--" ] && shift
    # semenv holds VAR=val words with no spaces in them, so splitting is safe.
    if out=$(timeout 5 env $semenv "$WORK/semprobe" "$@" 2>&1); then
        echo "$out"
    else
        echo "TIMEOUT"
    fi
}

# 5. A timed park with an offset in effect still times out, and on time.
got=$(semrun LD_PRELOAD="$WORK/libtsclock.so" TS_CLOCK_OFFSET=1000)
[ "$got" = TIMEOUT ] &&
    fail "sem_timedwait never returned with TS_CLOCK_OFFSET=1000 - the absolute deadline was not shifted back (this is the 2026-09-26 freeze)"
set -- $got
[ "$1" = "-1" ] || fail "sem_timedwait returned $1 with an offset, want -1 (timed out)"
[ "$2" = "1" ] || fail "sem_timedwait did not set ETIMEDOUT with an offset"
[ "$3" -lt 2000 ] || fail "sem_timedwait waited $3 ms for a 200 ms deadline with an offset, want < 2000"

# 6. The caller's timespec is not modified. The Go runtime passes &mp.ts and
#    reads it back, so mutating it in place would corrupt the runtime.
[ "$4" = "1" ] || fail "sem_timedwait modified the caller's timespec in place"

# 7. With no offset the wrapper is transparent.
got=$(semrun LD_PRELOAD="$WORK/libtsclock.so")
[ "$got" = TIMEOUT ] && fail "sem_timedwait never returned with no offset set"
set -- $got
[ "$1" = "-1" ] || fail "sem_timedwait returned $1 with no offset, want -1 (timed out)"
[ "$3" -lt 2000 ] || fail "sem_timedwait waited $3 ms with no offset, want < 2000"

# 8. A negative offset (clock ahead of network time) is handled symmetrically.
got=$(semrun LD_PRELOAD="$WORK/libtsclock.so" TS_CLOCK_OFFSET=-1000)
[ "$got" = TIMEOUT ] && fail "sem_timedwait never returned with a negative offset"
set -- $got
[ "$1" = "-1" ] || fail "sem_timedwait returned $1 with a negative offset, want -1 (timed out)"

# 9. A deadline that was NOT built from the shifted clock must still return.
#    Undoing the shift sends it before the epoch; that has to clamp to an
#    immediate timeout rather than wrap. time_t is unsigned on QNX 6.5, so the
#    unclamped form lands ~136 years out and hangs exactly like the bug this
#    file fixes. Only the cross compiler sees that type; this test is what
#    covers the logic.
got=$(semrun LD_PRELOAD="$WORK/libtsclock.so" TS_CLOCK_OFFSET=1000 -- raw)
[ "$got" = TIMEOUT ] &&
    fail "sem_timedwait never returned for a deadline smaller than the offset - the pre-epoch clamp is missing or wrapped"
set -- $got
[ "$1" = "-1" ] || fail "sem_timedwait returned $1 for a pre-epoch deadline, want -1"
# ETIMEDOUT specifically, not just any error: an unclamped subtraction yields a
# negative tv_sec, which Linux rejects with EINVAL just as promptly. Demanding
# ETIMEDOUT is what makes this test fail for the unclamped form on a signed
# time_t host, standing in for the unsigned wrap it causes on QNX.
[ "$2" = "1" ] || fail "sem_timedwait did not set ETIMEDOUT for a pre-epoch deadline (EINVAL means the deadline was passed through unclamped)"
[ "$3" -lt 2000 ] || fail "sem_timedwait waited $3 ms for a pre-epoch deadline, want an immediate timeout"

# 10. The deadline arithmetic itself, called directly. This is the only test
#     that covers the unsigned-time_t wrap: on this signed host the broken form
#     is indistinguishable through sem_timedwait, but on QNX 6.5 it is an
#     infinite wait. Testing the arithmetic is host-independent.
cat > "$WORK/mathprobe.c" <<'EOF'
#include <stdio.h>

long long tsclock_unshift_sec(long long abstime_sec, long offset);

int main(void) {
    int bad = 0;
    struct { long long in; long offset; long long want; const char *what; } t[] = {
        { 1790463375LL, 1790463175L,  200LL, "normal: shifted deadline comes back to real time" },
        {        100LL,       1000L,    0LL, "deadline smaller than offset clamps, does not wrap" },
        {          0LL,       1000L,    0LL, "zero deadline clamps" },
        {       1000LL,      -1000L, 2000LL, "negative offset (clock ahead) adds" },
        { 1790463175LL, 1790463175L,    0LL, "exactly the offset clamps to expired" },
    };
    unsigned i;

    for (i = 0; i < sizeof(t) / sizeof(t[0]); i++) {
        long long got = tsclock_unshift_sec(t[i].in, t[i].offset);
        if (got != t[i].want) {
            printf("FAIL: %s: tsclock_unshift_sec(%lld, %ld) = %lld, want %lld\n",
                   t[i].what, t[i].in, t[i].offset, got, t[i].want);
            bad = 1;
        }
    }
    if (!bad) printf("ok\n");
    return bad;
}
EOF
native -O2 -o "$WORK/mathprobe" "$WORK/mathprobe.c" "$HERE/tsclock.c" -ldl
out=$("$WORK/mathprobe") || fail "deadline arithmetic: $out"
[ "$out" = ok ] || fail "deadline arithmetic: $out"

echo "ok: clock shim offsets CLOCK_REALTIME only, and shifts sem_timedwait deadlines back"

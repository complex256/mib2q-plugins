/*
 * tsclock - give one process a corrected wall clock without touching the
 * system clock.
 *
 * The MHI2q boots with its clock at 1970. Stepping it once the network is up
 * (which is what sntp used to do) expires every pending CLOCK_REALTIME wait in
 * every process at once. In the HMI that fires OperationManager's
 * NavInitializationChecker, which concludes navigation never initialised and
 * restarts the whole navigation domain ~60 s into each boot - measured, with
 * the timer firing within 48 ms of the step.
 *
 * So the clock is left alone and tailscaled is told the truth privately:
 * LD_PRELOAD this, set TS_CLOCK_OFFSET to the difference in whole seconds
 * (sntp -print reports it), and CLOCK_REALTIME reads correctly in this process
 * only. Everything tailscaled needs real time for - TLS to the control plane,
 * node key expiry, wireguard-go's handshake timestamps - lives in this process.
 *
 * CLOCK_MONOTONIC is passed through untouched.
 *
 * WHY sem_timedwait IS ALSO INTERPOSED
 *
 * An earlier version of this file shifted only clock_gettime, on the reasoning
 * that "Go's timers and deadlines are built on CLOCK_MONOTONIC". That is true
 * of runtime.nanotime, but NOT of the mechanism that makes a timed park wake
 * up. QNX sem_timedwait takes an ABSOLUTE CLOCK_REALTIME deadline - it is the
 * only clock it accepts - so runtime.semasleep (os_qnx.go) builds its deadline
 * by calling clock_gettime(CLOCK_REALTIME), i.e. through this shim.
 *
 * The deadline therefore left the process shifted forward by the offset, while
 * procnto evaluated it against the real, unshifted system clock. With an offset
 * of ~1.79e9 s every deadline landed ~56.7 years in the future and no timed
 * park ever timed out. The one that matters is stopTheWorldWithSema, which
 * re-preempts straggler Ps via notetsleep(&sched.stopnote, 100*1000): that
 * 100 us retry never fired, so any GC stop-the-world that had a straggler P
 * never completed and tailscaled froze permanently. Diagnosed 2026-09-26 from
 * a SIGQUIT dump of a live wedge; GOGC=off ran clean where the same binary
 * froze at 0.065 s of CPU.
 *
 * So: shift the clock on the way out, and shift absolute deadlines back on the
 * way in. The pair is what keeps the process consistent with the kernel.
 *
 * Scope: sem_timedwait is the only absolute-deadline call the Go runtime makes,
 * which is why it is the only one wrapped here. Getting this wrong in the other
 * direction is cheap - semasleep treats ETIMEDOUT/EAGAIN/EINTR as a spurious
 * wake and retries - whereas the bug it replaces was a permanent freeze.
 */

#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <semaphore.h>
#include <stdlib.h>
#include <time.h>

static long tsclock_offset;

static int (*real_clock_gettime)(clockid_t, struct timespec *);
static int (*real_sem_timedwait)(sem_t *, const struct timespec *);

/* Read the offset once, at load. A value that is not a whole number is
 * ignored rather than half-applied: a wrong offset is worse than none, since
 * the daemon would then fail handshakes for a reason nothing reports. */
__attribute__((constructor)) static void tsclock_init(void)
{
    const char *s = getenv("TS_CLOCK_OFFSET");
    char *end;
    long v;

    if (s == NULL || *s == '\0')
        return;
    v = strtol(s, &end, 10);
    if (*end == '\0')
        tsclock_offset = v;
}

int clock_gettime(clockid_t id, struct timespec *ts)
{
    int (*next)(clockid_t, struct timespec *) = real_clock_gettime;
    int rc;

    if (next == NULL) {
        next = (int (*)(clockid_t, struct timespec *))dlsym(RTLD_NEXT, "clock_gettime");
        if (next == NULL) {
            /* Nothing sane to return: report the failure rather than invent a
             * time. The caller sees the same errno it would from libc. */
            errno = ENOSYS;
            return -1;
        }
        real_clock_gettime = next;
    }

    rc = next(id, ts);
    if (rc == 0 && id == CLOCK_REALTIME && tsclock_offset != 0)
        ts->tv_sec += tsclock_offset;
    return rc;
}

/* Undo the offset for an absolute deadline. Factored out, and given external
 * linkage, because this arithmetic is the whole correctness argument of the
 * sem_timedwait wrapper and it cannot be exercised through sem_timedwait on a
 * signed-time_t host: there, both the correct and the broken form produce an
 * immediate timeout. On QNX 6.5 time_t is UNSIGNED, so doing this in time_t
 * wraps for any deadline smaller than the offset and yields a time ~136 years
 * out - an infinite wait, which is the very bug the wrapper exists to prevent.
 * tsclock_test.sh calls this directly so the wrap case is covered on any host.
 *
 * Returns whole seconds, clamped at the epoch (0 means "already expired").  */
long long tsclock_unshift_sec(long long abstime_sec, long offset)
{
    long long sec = abstime_sec - (long long)offset;

    return sec <= 0 ? 0 : sec;
}

int sem_timedwait(sem_t *sem, const struct timespec *abstime)
{
    int (*next)(sem_t *, const struct timespec *) = real_sem_timedwait;
    struct timespec adjusted;
    long long sec;

    if (next == NULL) {
        next = (int (*)(sem_t *, const struct timespec *))dlsym(RTLD_NEXT, "sem_timedwait");
        if (next == NULL) {
            errno = ENOSYS;
            return -1;
        }
        real_sem_timedwait = next;
    }

    if (abstime == NULL || tsclock_offset == 0)
        return next(sem, abstime);

    /* The caller built this deadline from our shifted CLOCK_REALTIME; the
     * kernel will judge it against the real one. Undo the shift.
     *
     * Do the arithmetic in long long, not in time_t. time_t is UNSIGNED on
     * QNX 6.5, so `adjusted.tv_sec -= tsclock_offset` would wrap for any
     * deadline smaller than the offset and produce a time ~136 years out -
     * reintroducing exactly the infinite wait this wrapper exists to prevent,
     * in a form that only shows up for a caller that did not build its
     * deadline from our shifted clock. The cross compiler flags the signed
     * comparison this replaces as always-false; that warning was the tell.
     *
     * Copy rather than adjust in place: abstime is const, and the Go runtime
     * hands us &mp.ts, which it reads back and reuses. */
    adjusted = *abstime;
    sec = tsclock_unshift_sec((long long)abstime->tv_sec, tsclock_offset);
    if (sec == 0) {
        /* Before the epoch, so not expressible. Ask for an immediate timeout:
         * a spurious early wake is recoverable (semasleep treats it as a
         * wake and retries), an infinite wait is not. */
        adjusted.tv_nsec = 0;
    }
    adjusted.tv_sec = (time_t)sec;
    return next(sem, &adjusted);
}

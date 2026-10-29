//go:build qnx

package sntp

import (
	"syscall"
	"time"
)

const clockRealtime = 0 // CLOCK_REALTIME, <time.h>

func setClock(t time.Time) error {
	ts := syscall.NsecToTimespec(t.UnixNano())
	return syscall.ClockSettime(clockRealtime, &ts)
}

//go:build qnx

package main

import (
	"syscall"
	"time"
)

// setClock steps CLOCK_REALTIME. Nothing on the boot path calls this any more:
// stepping the clock while the HMI is running fires its pending timers, which
// restarts the unit's navigation domain. See the note in main.go.
func setClock(t time.Time) error {
	ts := syscall.NsecToTimespec(t.UnixNano())
	return syscall.ClockSettime(0, &ts) // 0 = CLOCK_REALTIME
}

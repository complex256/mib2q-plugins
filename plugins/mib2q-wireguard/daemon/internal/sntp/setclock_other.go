//go:build !qnx

package sntp

import (
	"errors"
	"time"
)

// Setting the clock is only wanted on the head unit; elsewhere the OS keeps
// time already.
func setClock(time.Time) error { return errors.New("not supported on this OS") }

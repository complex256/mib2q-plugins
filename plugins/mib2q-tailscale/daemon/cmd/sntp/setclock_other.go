//go:build !qnx

package main

import (
	"errors"
	"time"
)

// setClock is QNX-only; this stub exists so the package (and its tests) build
// on a development host.
func setClock(time.Time) error {
	return errors.New("sntp: setting the clock is implemented only on QNX")
}

package main

import (
	"bytes"
	"errors"
	"strconv"
	"strings"
	"testing"
	"time"
)

// -print is the mode tailscale.sh uses: it must report how far the clock is
// off and never touch it, because stepping the system clock mid-boot fires the
// HMI's pending timers and restarts the unit's navigation domain.
func TestPrintReportsOffsetSecondsOnStdout(t *testing.T) {
	now := time.Date(1970, 1, 1, 0, 0, 30, 0, time.UTC)
	netTime := time.Date(2026, 9, 23, 23, 1, 15, 0, time.UTC)
	var out, errOut bytes.Buffer

	code := run([]string{"-print", "ntp.example"}, deps{
		now:    func() time.Time { return now },
		query:  func(string) (time.Time, error) { return netTime, nil },
		out:    &out,
		errOut: &errOut,
	})

	if code != 0 {
		t.Fatalf("exit code = %d, want 0 (stderr: %s)", code, errOut.String())
	}
	want := "1790204445" // netTime - now, in whole seconds
	if got := strings.TrimSpace(out.String()); got != want {
		t.Errorf("stdout = %q, want %q (stdout carries only the offset, so the shell can read it)", got, want)
	}
}

// A boot with no uplink must not print a bogus offset: tailscale.sh reads
// stdout with a command substitution, so anything printed here would be
// applied as a real offset.
func TestPrintFailsWithoutTouchingStdoutWhenNoServerAnswers(t *testing.T) {
	var out, errOut bytes.Buffer

	code := run([]string{"-print", "a.example", "b.example"}, deps{
		now:    func() time.Time { return time.Unix(30, 0).UTC() },
		query:  func(string) (time.Time, error) { return time.Time{}, errors.New("no route to host") },
		out:    &out,
		errOut: &errOut,
	})

	if code == 0 {
		t.Errorf("exit code = 0, want non-zero when every server failed")
	}
	if out.String() != "" {
		t.Errorf("stdout = %q, want empty so the shell reads no offset", out.String())
	}
}

// A reply claiming a pre-2024 year is bad data (misbehaving or spoofed
// server), not a real time; the next server must get a turn.
func TestPrintSkipsImplausibleReplyAndUsesNextServer(t *testing.T) {
	now := time.Unix(30, 0).UTC()
	good := time.Date(2026, 9, 23, 23, 1, 15, 0, time.UTC)
	var out, errOut bytes.Buffer

	code := run([]string{"-print", "bad.example", "good.example"}, deps{
		now: func() time.Time { return now },
		query: func(server string) (time.Time, error) {
			if server == "bad.example" {
				return time.Date(1999, 1, 1, 0, 0, 0, 0, time.UTC), nil
			}
			return good, nil
		},
		out:    &out,
		errOut: &errOut,
	})

	if code != 0 {
		t.Fatalf("exit code = %d, want 0 (stderr: %s)", code, errOut.String())
	}
	want := strconv.FormatInt(int64(good.Sub(now)/time.Second), 10)
	if got := strings.TrimSpace(out.String()); got != want {
		t.Errorf("stdout = %q, want %q (the good server's offset)", got, want)
	}
}

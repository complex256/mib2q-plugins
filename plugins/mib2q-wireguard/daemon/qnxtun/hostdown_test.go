//go:build qnx

package qnxtun

import (
	"errors"
	"runtime"
	"sync/atomic"
	"syscall"
	"testing"
	"time"
)

func newTestDevice(name string) *Device {
	return &Device{name: name, closeCh: make(chan struct{})}
}

// Read must retry on EHOSTDOWN (the interface isn't READY yet - no address
// assigned) rather than returning it as a fatal error, and must succeed
// once the underlying read stops failing.
func TestReadRetriesOnEHOSTDOWN(t *testing.T) {
	restore := hostDownRetry
	hostDownRetry = time.Millisecond
	defer func() { hostDownRetry = restore }()

	d := newTestDevice("tun9")
	var calls int32
	want := []byte("packet")
	n, err := d.read(func() (int, error) {
		c := atomic.AddInt32(&calls, 1)
		if c < 4 {
			return 0, syscall.EHOSTDOWN
		}
		return len(want), nil
	})
	if err != nil {
		t.Fatalf("read() error = %v, want nil", err)
	}
	if n != len(want) {
		t.Fatalf("read() n = %d, want %d", n, len(want))
	}
	if calls != 4 {
		t.Fatalf("underlying read called %d times, want 4", calls)
	}
	if !d.loggedHostDown {
		t.Errorf("loggedHostDown = false, want true after at least one EHOSTDOWN")
	}
}

// A non-EHOSTDOWN error must pass straight through, unretried - EINTR/EAGAIN
// handling belongs to the caller (as it already does in both consumers),
// and every other error is genuinely fatal.
func TestReadPassesThroughOtherErrors(t *testing.T) {
	d := newTestDevice("tun9")
	var calls int32
	wantErr := syscall.EINTR
	_, err := d.read(func() (int, error) {
		atomic.AddInt32(&calls, 1)
		return 0, wantErr
	})
	if !errors.Is(err, wantErr) {
		t.Fatalf("read() error = %v, want %v", err, wantErr)
	}
	if calls != 1 {
		t.Fatalf("underlying read called %d times, want 1 (no retry on non-EHOSTDOWN)", calls)
	}
}

// Close during the retry wait must break the loop promptly (not spin until
// some external event) and return a real, non-nil error - not silently
// swallow the wait.
func TestReadUnblocksOnCloseDuringRetry(t *testing.T) {
	restore := hostDownRetry
	hostDownRetry = time.Hour // would hang the test if Close didn't interrupt it
	defer func() { hostDownRetry = restore }()

	d := newTestDevice("tun9")
	done := make(chan error, 1)
	go func() {
		_, err := d.read(func() (int, error) { return 0, syscall.EHOSTDOWN })
		done <- err
	}()

	// Give the reader time to enter the retry wait.
	time.Sleep(20 * time.Millisecond)
	close(d.closeCh)

	select {
	case err := <-done:
		if err == nil {
			t.Fatal("read() returned nil error after Close, want a real error")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("read() did not return within 2s of Close during retry wait")
	}
}

// Close before Read is ever called must make Read return immediately
// instead of attempting the underlying read at all.
func TestReadReturnsImmediatelyIfAlreadyClosed(t *testing.T) {
	d := newTestDevice("tun9")
	close(d.closeCh)
	var calls int32
	_, err := d.read(func() (int, error) {
		atomic.AddInt32(&calls, 1)
		return 0, nil
	})
	if err == nil {
		t.Fatal("read() returned nil error on an already-closed device, want a real error")
	}
	if calls != 0 {
		t.Errorf("underlying read called %d times, want 0", calls)
	}
}

// No goroutine leak: a Read that unblocks via Close must not leave any
// extra goroutine running behind it.
func TestReadCloseLeavesNoGoroutineLeak(t *testing.T) {
	restore := hostDownRetry
	hostDownRetry = time.Hour
	defer func() { hostDownRetry = restore }()

	before := runtime.NumGoroutine()

	d := newTestDevice("tun9")
	done := make(chan struct{})
	go func() {
		d.read(func() (int, error) { return 0, syscall.EHOSTDOWN })
		close(done)
	}()
	time.Sleep(20 * time.Millisecond)
	close(d.closeCh)

	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("read() did not return after Close")
	}

	// Let the runtime settle, then confirm goroutine count returned to
	// baseline (no leaked timer/reader goroutine left behind).
	deadline := time.Now().Add(time.Second)
	for {
		if runtime.NumGoroutine() <= before {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("goroutine count after close = %d, want <= %d (before)", runtime.NumGoroutine(), before)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

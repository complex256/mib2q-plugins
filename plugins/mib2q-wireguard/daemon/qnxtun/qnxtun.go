//go:build qnx

package qnxtun

import (
	"fmt"
	"log"
	"sync"
	"syscall"
	"time"
)

// hostDownRetry is how long Read waits between retries while io-pkt's tun
// is not yet READY (see the Read doc comment). Var, not const, so a test
// can shrink it.
var hostDownRetry = 20 * time.Millisecond

// Device is an open /dev/tunN. Safe for one reader and one writer
// concurrently, which is how wireguard-go uses a tun.
type Device struct {
	fd      int
	name    string
	closeM  sync.Mutex
	closed  bool
	closeCh chan struct{}

	loggedHostDown bool // Read only; single-reader by contract.
}

// Open opens the device for an interface that already exists.
func Open(name string) (*Device, error) {
	if err := ValidName(name); err != nil {
		return nil, err
	}
	fd, err := syscall.Open(DevPath(name), syscall.O_RDWR, 0)
	if err != nil {
		return nil, fmt.Errorf("open %s: %w", DevPath(name), err)
	}
	return &Device{fd: fd, name: name, closeCh: make(chan struct{})}, nil
}

// Read returns one bare IPv4 packet (no address-family header).
//
// It blocks. poll(2) never reports /dev/tunN readable on QNX: io-pkt queues
// the packet and counts it in the interface's Opkts, but the poll wakeup
// never comes, so a poll-driven reader reads nothing at all and traffic dies
// silently in one direction. Close unblocks this by closing the fd.
//
// Before an address is assigned, NetBSD-derived tun (which io-pkt's is) is
// not yet READY - that takes TUN_OPEN | TUN_INITED | TUN_IASET, i.e. an
// address, not just "up" - and read() fails with EHOSTDOWN (errno 264)
// instead of blocking. A caller that opens the device and starts reading
// before an address lands (Tailscale's router assigns it later, via
// router.Set) would otherwise see that as a fatal read error. Read treats
// EHOSTDOWN as "not ready yet": it logs once and retries on a short timer,
// indefinitely, until a read succeeds or Close is called.
func (d *Device) Read(p []byte) (int, error) {
	return d.read(func() (int, error) { return syscall.Read(d.fd, p) })
}

// read holds the EHOSTDOWN retry loop; it takes the raw read as a func so
// tests can drive it without a real /dev/tunN.
func (d *Device) read(sysRead func() (int, error)) (int, error) {
	for {
		select {
		case <-d.closeCh:
			return 0, fmt.Errorf("qnxtun: %s: closed", d.name)
		default:
		}
		n, err := sysRead()
		if err != syscall.EHOSTDOWN {
			return n, err
		}
		if !d.loggedHostDown {
			d.loggedHostDown = true
			log.Printf("qnxtun: %s: read: interface not ready yet (EHOSTDOWN); waiting for an address", d.name)
		}
		t := time.NewTimer(hostDownRetry)
		select {
		case <-d.closeCh:
			t.Stop()
			return 0, fmt.Errorf("qnxtun: %s: closed while waiting for an address", d.name)
		case <-t.C:
		}
	}
}

// Write injects one bare IPv4 packet.
func (d *Device) Write(p []byte) (int, error) { return syscall.Write(d.fd, p) }

func (d *Device) Fd() int      { return d.fd }
func (d *Device) Name() string { return d.name }

func (d *Device) Close() error {
	d.closeM.Lock()
	defer d.closeM.Unlock()
	if d.closed {
		return nil
	}
	d.closed = true
	close(d.closeCh)
	return syscall.Close(d.fd)
}

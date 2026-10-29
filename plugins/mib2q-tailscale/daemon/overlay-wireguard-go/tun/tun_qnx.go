//go:build qnx

package tun

import (
	"fmt"
	"os"
	"os/exec"
	"sync"

	"github.com/complex256/mib2q-wireguard/qnxtun"
)

// CreateTUN creates interface `name` with io-pkt's tun cloner and opens its
// device. IPv4 only: without the TUNSIFHEAD ioctl (the QNX Go port has no
// ioctl binding) tun treats every written packet as AF_INET.
//
// This is tailscale's own fork of wireguard-go (a separate module from
// golang.zx2c4.com/wireguard, which Task 2 ported) - the two have diverged,
// so this file is a second copy of Task 2's CreateTUN rather than a
// repointed import. Both call the same qnxtun package, so there is exactly
// one place that owns the actual device.
func CreateTUN(name string, mtu int) (Device, error) {
	if err := qnxtun.ValidName(name); err != nil {
		return nil, err
	}
	ifconfig := lookupTool("ifconfig")
	exec.Command(ifconfig, name, "destroy").Run() // leftover from a crashed run
	if out, err := exec.Command(ifconfig, name, "create").CombinedOutput(); err != nil {
		return nil, fmt.Errorf("ifconfig %s create: %v: %s (does this io-pkt have a tun cloner? `ifconfig -C`)", name, err, out)
	}
	if out, err := exec.Command(ifconfig, name, "mtu", fmt.Sprint(mtu)).CombinedOutput(); err != nil {
		exec.Command(ifconfig, name, "destroy").Run()
		return nil, fmt.Errorf("ifconfig %s mtu: %v: %s", name, err, out)
	}
	if out, err := exec.Command(ifconfig, name, "up").CombinedOutput(); err != nil {
		exec.Command(ifconfig, name, "destroy").Run()
		return nil, fmt.Errorf("ifconfig %s up: %v: %s", name, err, out)
	}
	dev, err := qnxtun.Open(name)
	if err != nil {
		exec.Command(ifconfig, name, "destroy").Run()
		return nil, err
	}
	t := &qnxDevice{dev: dev, name: name, mtu: mtu, events: make(chan Event, 4)}
	t.events <- EventUp
	return t, nil
}

func lookupTool(name string) string {
	for _, d := range []string{"/sbin", "/usr/sbin", "/bin", "/usr/bin", "/armle/sbin", "/armle/bin", "/armle/usr/sbin", "/armle/usr/bin", "/proc/boot"} {
		p := d + "/" + name
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	return name
}

type qnxDevice struct {
	dev       *qnxtun.Device
	name      string
	mtu       int
	events    chan Event
	closeOnce sync.Once
}

// Read blocks on qnxtun.Device.Read (a plain syscall.Read on the fd).
//
// This must never become poll-driven: poll(2) never reports /dev/tunN
// readable on QNX (io-pkt queues the packet and counts it in the
// interface's Opkts, but the wakeup never comes), so a poll-based reader
// silently reads nothing and traffic dies in this direction while looking
// fine everywhere else.
func (t *qnxDevice) Read(bufs [][]byte, sizes []int, offset int) (int, error) {
	n, err := t.dev.Read(bufs[0][offset:])
	if err != nil {
		return 0, err
	}
	sizes[0] = n
	return 1, nil
}

func (t *qnxDevice) Write(bufs [][]byte, offset int) (int, error) {
	for i, b := range bufs {
		p := b[offset:]
		if len(p) == 0 || p[0]>>4 != 4 {
			continue // IPv6 can't be expressed without TUNSIFHEAD
		}
		if _, err := t.dev.Write(p); err != nil {
			return i, err
		}
	}
	return len(bufs), nil
}

func (t *qnxDevice) MTU() (int, error)     { return t.mtu, nil }
func (t *qnxDevice) Name() (string, error) { return t.name, nil }
func (t *qnxDevice) Events() <-chan Event  { return t.events }
func (t *qnxDevice) BatchSize() int        { return 1 }
func (t *qnxDevice) File() *os.File        { return nil }

func (t *qnxDevice) Close() error {
	var err error
	t.closeOnce.Do(func() {
		err = t.dev.Close()
		exec.Command(lookupTool("ifconfig"), t.name, "destroy").Run()
		close(t.events)
	})
	return err
}

//go:build qnx

package backend

import (
	"fmt"
	"syscall"

	"github.com/complex256/mib2q-wireguard/qnxtun"
	"golang.zx2c4.com/wireguard/tun"
)

// TunIface is the tun unit mibwg owns. /dev/tun0-3 exist on the MHI2q from
// boot; tun9 is created and destroyed per run so nothing the firmware might
// use is touched.
const TunIface = "tun9"

// Tun is a tun.Device over io-pkt's tun(4), where the firmware's io-pkt has
// the cloner (`ifconfig -C` lists tun; the SDK's io-pkt does not). Verified on
// an MHI2q: /dev/tunN reads and writes bare IP packets, no header.
//
// IPv4 only: without the TUNSIFHEAD ioctl (the QNX Go port has no ioctl
// binding yet) tun assumes AF_INET for every written packet, so IPv6
// addresses and routes are skipped rather than silently black-holed.
type Tun struct {
	*kernelIface
	dev *qnxtun.Device
}

func NewTun(o KernelOptions) (Backend, error) {
	t := &Tun{kernelIface: newKernelIface(TunIface, o)}
	if err := t.setup(); err != nil {
		t.Close()
		return nil, err
	}
	t.events <- tun.EventUp
	go t.reader()
	return t, nil
}

func (t *Tun) setup() error {
	run("ifconfig", TunIface, "destroy") // leftover from a crashed run
	if err := run("ifconfig", TunIface, "create"); err != nil {
		return fmt.Errorf("%w (does this io-pkt have tun? `ifconfig -C`; else use Backend = gif)", err)
	}
	if err := t.configure(false); err != nil {
		return err
	}
	var err error
	if t.dev, err = qnxtun.Open(TunIface); err != nil {
		return err
	}
	return nil
}

// reader loops on qnxtun.Device.Read, which blocks (see its doc comment for
// why this can't be poll-driven). Close unblocks it by closing the device.
func (t *Tun) reader() {
	buf := make([]byte, 65535)
	for {
		n, err := t.dev.Read(buf)
		if err != nil {
			select {
			case <-t.closed:
				return
			default:
			}
			if err == syscall.EINTR || err == syscall.EAGAIN {
				continue
			}
			t.opts.Logf("%s: read: %v", TunIface, err)
			return
		}
		if n == 0 || buf[0]>>4 != 4 {
			continue
		}
		if !t.deliver(buf[:n]) {
			return
		}
	}
}

func (t *Tun) Write(bufs [][]byte, offset int) (int, error) {
	for i, b := range bufs {
		p := b[offset:]
		if len(p) == 0 || p[0]>>4 != 4 {
			continue // IPv6 can't be expressed without TUNSIFHEAD; drop
		}
		if _, err := t.dev.Write(p); err != nil {
			return i, err
		}
	}
	return len(bufs), nil
}

func (t *Tun) TUN() tun.Device { return t }

func (t *Tun) Close() error {
	t.shutdown(func() {
		if t.dev != nil {
			t.dev.Close()
		}
	})
	return nil
}

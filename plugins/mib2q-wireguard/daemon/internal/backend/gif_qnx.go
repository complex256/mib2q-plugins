//go:build qnx

package backend

import (
	"fmt"
	"net/netip"
	"syscall"

	"golang.zx2c4.com/wireguard/tun"
)

// GifIface is the gif unit mibwg owns. An unusual number keeps clear of any
// gif the firmware might configure; a stale one from a crashed run is ours to
// destroy.
const GifIface = "gif9"

const ipHdrincl = 2 // <netinet/in.h>

// Gif is a tun.Device over gif(4) + raw sockets, for io-pkt builds without a
// tun cloner. See ipip.go for the framing.
type Gif struct {
	*kernelIface
	fd4, fd6 int
}

func NewGif(o KernelOptions) (Backend, error) {
	g := &Gif{kernelIface: newKernelIface(GifIface, o), fd4: -1, fd6: -1}
	if err := g.setup(); err != nil {
		g.Close()
		return nil, err
	}
	g.events <- tun.EventUp
	go g.reader(g.fd4)
	if g.fd6 >= 0 {
		go g.reader(g.fd6)
	}
	return g, nil
}

func (g *Gif) setup() error {
	// Leftovers from a run that didn't get to clean up.
	run("ifconfig", GifIface, "destroy")
	for _, a := range []netip.Addr{TunLocal, TunPeer} {
		run("ifconfig", "lo0", a.String(), "delete")
		if err := run("ifconfig", "lo0", a.String(), "netmask", "255.255.255.255", "alias"); err != nil {
			return err
		}
	}
	if err := run("ifconfig", GifIface, "create"); err != nil {
		return err
	}
	if err := run("ifconfig", GifIface, "tunnel", TunLocal.String(), TunPeer.String()); err != nil {
		return err
	}
	if err := g.configure(true); err != nil {
		return err
	}
	var err error
	if g.fd4, err = rawSocket(protoIPIP, true); err != nil {
		return fmt.Errorf("raw IPIP socket: %w", err)
	}
	if g.gw6.IsValid() {
		if g.fd6, err = rawSocket(protoIPv6, false); err != nil {
			return fmt.Errorf("raw IPv6-in-IPv4 socket: %w", err)
		}
	}
	return nil
}

func rawSocket(proto int, hdrincl bool) (int, error) {
	fd, err := syscall.Socket(syscall.AF_INET, syscall.SOCK_RAW, proto)
	if err != nil {
		return -1, err
	}
	if hdrincl {
		if err := syscall.SetsockoptInt(fd, syscall.IPPROTO_IP, ipHdrincl, 1); err != nil {
			syscall.Close(fd)
			return -1, err
		}
	}
	return fd, nil
}

func (g *Gif) reader(fd int) {
	buf := make([]byte, 65535)
	pollRead(fd, g.closed, func() bool {
		n, _, err := syscall.Recvfrom(fd, buf, 0)
		if err != nil {
			return true
		}
		if inner := decap(buf[:n]); inner != nil {
			return g.deliver(inner)
		}
		return true
	})
}

// pollRead calls read each time fd is readable, until closed is closed or
// read returns false. The poll timeout means Close is noticed without
// relying on close() to wake a blocked read.
func pollRead(fd int, closed <-chan struct{}, read func() bool) {
	pfd := []syscall.PollFd{{Fd: int32(fd), Events: syscall.POLLIN}}
	for {
		select {
		case <-closed:
			return
		default:
		}
		if n, err := syscall.Poll(pfd, 500); err != nil || n == 0 {
			continue
		}
		if !read() {
			return
		}
	}
}

func (g *Gif) Write(bufs [][]byte, offset int) (int, error) {
	to := &syscall.SockaddrInet4{Addr: TunLocal.As4()}
	for i, b := range bufs {
		if err := syscall.Sendto(g.fd4, encap(b[offset:]), 0, to); err != nil {
			return i, err
		}
	}
	return len(bufs), nil
}

func (g *Gif) TUN() tun.Device { return g }

func (g *Gif) Close() error {
	g.shutdown(func() {
		if g.fd4 >= 0 {
			syscall.Close(g.fd4)
		}
		if g.fd6 >= 0 {
			syscall.Close(g.fd6)
		}
		for _, a := range []netip.Addr{TunLocal, TunPeer} {
			run("ifconfig", "lo0", a.String(), "delete")
		}
	})
	return nil
}

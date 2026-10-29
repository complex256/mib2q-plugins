// Package proxy lets programs on the head unit (or on devices joined to its
// Wi-Fi hotspot) send traffic through the tunnel without a tun interface:
// SOCKS5 (RFC 1928, CONNECT, no auth) and HTTP (CONNECT + absolute-URI).
package proxy

import (
	"bufio"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"strconv"
	"time"

	"github.com/complex256/mib2q-wireguard/internal/forward"
)

// Dialer opens connections on the tunnel side (backend.Network satisfies it).
type Dialer interface {
	DialContext(ctx context.Context, network, address string) (net.Conn, error)
}

const dialTimeout = 15 * time.Second

// ServeSOCKS5 accepts until ln is closed.
func ServeSOCKS5(ctx context.Context, ln net.Listener, d Dialer, logf func(string, ...any)) {
	for {
		c, err := ln.Accept()
		if err != nil {
			if ctx.Err() == nil {
				logf("socks5: accept: %v", err)
			}
			return
		}
		go func() {
			defer c.Close()
			if err := socks5(ctx, c, d); err != nil {
				logf("socks5: %s: %v", c.RemoteAddr(), err)
			}
		}()
	}
}

const (
	socksVer       = 5
	cmdConnect     = 1
	atypIPv4       = 1
	atypDomain     = 3
	atypIPv6       = 4
	repSucceeded   = 0
	repFailure     = 1
	repUnreachable = 4
	repCmdUnsupp   = 7
	repAtypUnsupp  = 8
)

func socks5(ctx context.Context, c net.Conn, d Dialer) error {
	c.SetDeadline(time.Now().Add(30 * time.Second))
	br := bufio.NewReader(c)

	// Greeting: VER NMETHODS METHODS...
	var hdr [2]byte
	if _, err := io.ReadFull(br, hdr[:]); err != nil {
		return err
	}
	if hdr[0] != socksVer {
		return fmt.Errorf("not SOCKS5 (version %d)", hdr[0])
	}
	methods := make([]byte, hdr[1])
	if _, err := io.ReadFull(br, methods); err != nil {
		return err
	}
	noAuth := false
	for _, m := range methods {
		noAuth = noAuth || m == 0
	}
	if !noAuth {
		c.Write([]byte{socksVer, 0xff})
		return errors.New("client offers no acceptable auth method")
	}
	if _, err := c.Write([]byte{socksVer, 0}); err != nil {
		return err
	}

	// Request: VER CMD RSV ATYP DST.ADDR DST.PORT
	var req [4]byte
	if _, err := io.ReadFull(br, req[:]); err != nil {
		return err
	}
	var host string
	switch req[3] {
	case atypIPv4, atypIPv6:
		ip := make(net.IP, 4)
		if req[3] == atypIPv6 {
			ip = make(net.IP, 16)
		}
		if _, err := io.ReadFull(br, ip); err != nil {
			return err
		}
		host = ip.String()
	case atypDomain:
		n, err := br.ReadByte()
		if err != nil {
			return err
		}
		name := make([]byte, n)
		if _, err := io.ReadFull(br, name); err != nil {
			return err
		}
		host = string(name)
	default:
		reply(c, repAtypUnsupp)
		return fmt.Errorf("address type %d unsupported", req[3])
	}
	var pb [2]byte
	if _, err := io.ReadFull(br, pb[:]); err != nil {
		return err
	}
	if req[1] != cmdConnect {
		reply(c, repCmdUnsupp)
		return fmt.Errorf("command %d unsupported (CONNECT only)", req[1])
	}
	target := net.JoinHostPort(host, strconv.Itoa(int(binary.BigEndian.Uint16(pb[:]))))

	dctx, cancel := context.WithTimeout(ctx, dialTimeout)
	up, err := d.DialContext(dctx, "tcp", target)
	cancel()
	if err != nil {
		reply(c, repUnreachable)
		return fmt.Errorf("dial %s: %w", target, err)
	}
	defer up.Close()
	if err := reply(c, repSucceeded); err != nil {
		return err
	}
	c.SetDeadline(time.Time{})
	forward.Pipe(&bufferedConn{c, br}, up)
	return nil
}

// reply sends a response with a zero IPv4 bind address; clients ignore it for
// CONNECT.
func reply(c net.Conn, code byte) error {
	_, err := c.Write([]byte{socksVer, code, 0, atypIPv4, 0, 0, 0, 0, 0, 0})
	return err
}

// bufferedConn drains bytes the handshake reader already buffered before
// reading from the connection itself.
type bufferedConn struct {
	net.Conn
	r *bufio.Reader
}

func (b *bufferedConn) Read(p []byte) (int, error) { return b.r.Read(p) }

func (b *bufferedConn) CloseWrite() error {
	if cw, ok := b.Conn.(interface{ CloseWrite() error }); ok {
		return cw.CloseWrite()
	}
	return b.Conn.Close()
}

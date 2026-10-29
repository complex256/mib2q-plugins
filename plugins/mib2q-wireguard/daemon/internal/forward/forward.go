// Package forward exposes head-unit services on the tunnel: it listens on the
// unit's tunnel addresses and relays each connection (TCP) or flow (UDP) to a
// target on the host network stack.
package forward

import (
	"context"
	"io"
	"net"
	"net/netip"
	"sync"
	"time"

	"github.com/complex256/mib2q-wireguard/internal/backend"
	"github.com/complex256/mib2q-wireguard/internal/config"
)

// UDPIdleTimeout ends a UDP flow after this long without traffic either way.
var UDPIdleTimeout = 2 * time.Minute

var hostDialer net.Dialer

// Start opens every listener up front, so a port clash fails startup rather
// than surfacing as a silent dead forward.
func Start(ctx context.Context, tnet backend.Network, fws []config.Forward, logf func(string, ...any)) error {
	for _, fw := range fws {
		for _, a := range tnet.Addrs() {
			ap := netip.AddrPortFrom(a, fw.ListenPort)
			switch fw.Proto {
			case "tcp":
				ln, err := tnet.ListenTCP(ap)
				if err != nil {
					return err
				}
				go closeOnDone(ctx, ln)
				go serveTCP(ctx, ln, fw.Target, logf)
			case "udp":
				pc, err := tnet.ListenUDP(ap)
				if err != nil {
					return err
				}
				go closeOnDone(ctx, pc)
				go serveUDP(ctx, pc, fw.Target, logf)
			}
			logf("forward: %s %s -> %s", fw.Proto, ap, fw.Target)
		}
	}
	return nil
}

func closeOnDone(ctx context.Context, c io.Closer) {
	<-ctx.Done()
	c.Close()
}

func serveTCP(ctx context.Context, ln net.Listener, target string, logf func(string, ...any)) {
	for {
		c, err := ln.Accept()
		if err != nil {
			if ctx.Err() == nil {
				logf("forward: accept %s: %v", ln.Addr(), err)
			}
			return
		}
		go func() {
			defer c.Close()
			dctx, cancel := context.WithTimeout(ctx, 10*time.Second)
			up, err := hostDialer.DialContext(dctx, "tcp", target)
			cancel()
			if err != nil {
				logf("forward: %s -> %s: %v", c.RemoteAddr(), target, err)
				return
			}
			defer up.Close()
			Pipe(c, up)
		}()
	}
}

// Pipe copies both ways until either side finishes, then closes both.
func Pipe(a, b net.Conn) {
	var wg sync.WaitGroup
	wg.Add(2)
	cp := func(dst, src net.Conn) {
		defer wg.Done()
		io.Copy(dst, src)
		// Half-close when the conn supports it so request/response protocols
		// see EOF; otherwise tear the whole thing down.
		if cw, ok := dst.(interface{ CloseWrite() error }); ok {
			cw.CloseWrite()
		} else {
			dst.Close()
		}
	}
	go cp(a, b)
	go cp(b, a)
	wg.Wait()
}

// serveUDP gives each tunnel-side client address its own host socket, so
// replies from the target route back to the right client.
func serveUDP(ctx context.Context, pc net.PacketConn, target string, logf func(string, ...any)) {
	var mu sync.Mutex
	flows := map[string]net.Conn{}
	buf := make([]byte, 65535)
	for {
		n, from, err := pc.ReadFrom(buf)
		if err != nil {
			if ctx.Err() == nil {
				logf("forward: udp read %s: %v", pc.LocalAddr(), err)
			}
			return
		}
		key := from.String()
		mu.Lock()
		up := flows[key]
		if up == nil {
			up, err = hostDialer.DialContext(ctx, "udp", target)
			if err != nil {
				mu.Unlock()
				logf("forward: udp %s -> %s: %v", key, target, err)
				continue
			}
			flows[key] = up
			go func() {
				rbuf := make([]byte, 65535)
				for {
					up.SetReadDeadline(time.Now().Add(UDPIdleTimeout))
					m, err := up.Read(rbuf)
					if err != nil {
						break
					}
					pc.WriteTo(rbuf[:m], from)
				}
				mu.Lock()
				delete(flows, key)
				mu.Unlock()
				up.Close()
			}()
		}
		mu.Unlock()
		up.Write(buf[:n])
	}
}

//go:build qnx

package daemon

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/netip"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/complex256/mib2q-wireguard/internal/backend"
	"github.com/complex256/mib2q-wireguard/internal/config"
)

// TestEndToEndTun needs a tun cloner in io-pkt: the MHI2q firmware has one,
// the SDK's io-pkt (what QEMU boots) does not, so this only runs on a unit.
func TestEndToEndTun(t *testing.T) {
	if _, err := os.Stat("/dev/tun0"); err != nil {
		t.Skip("no tun device on this io-pkt; gif is the fallback backend")
	}
	testKernelBackend(t, "tun", backend.TunIface)
}

// TestEndToEndGif runs everywhere io-pkt does.
func TestEndToEndGif(t *testing.T) {
	testKernelBackend(t, "gif", backend.GifIface)
}

// testKernelBackend drives a kernel-routed backend: no proxies, no forwards -
// ordinary sockets on the unit reach the server through io-pkt's routing
// table, and the server reaches ordinary listeners on the unit. Needs io-pkt
// and root (tests/qemu-net.sh with QEMU_TOOLS=route, or a unit).
func testKernelBackend(t *testing.T, backendName, iface string) {
	srv := newTestServer(t)
	tcpEcho := listenAny(t)
	cfgText := fmt.Sprintf(`
[Interface]
PrivateKey = %s
Address = %s/32
Backend = %s

[Peer]
PublicKey = %s
Endpoint = 127.0.0.1:%d
AllowedIPs = 10.77.0.0/24
PersistentKeepalive = 1
`, srv.unitKey, srv.unitAddr, backendName, srv.pub, srv.port)
	cfg, err := config.Parse(strings.NewReader(cfgText))
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	d, err := Start(ctx, cfg, Options{Logf: t.Logf})
	if err != nil {
		t.Fatal(err)
	}
	closed := false
	defer func() {
		if !closed {
			d.Close()
		}
	}()

	t.Run("out/tcp-plain-socket", func(t *testing.T) {
		// A plain HTTP client - no proxy. The kernel routes 10.77.0.0/24
		// into the tunnel interface.
		client := &http.Client{Timeout: 15 * time.Second}
		resp, err := client.Get("http://10.77.0.1/")
		if err != nil {
			t.Fatal(err)
		}
		body, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		if want := "you are 10.77.0.2:"; !strings.Contains(string(body), want) {
			t.Fatalf("body = %q, want %q", body, want)
		}
	})

	t.Run("in/tcp-plain-listener", func(t *testing.T) {
		// The server reaches a listener bound to 0.0.0.0 on the unit - how
		// sshd is reached, with no [Forward] involved.
		port := tcpEcho.Addr().(*net.TCPAddr).Port
		c, err := srv.net.DialContextTCPAddrPort(ctx, netip.AddrPortFrom(srv.unitAddr, uint16(port)))
		if err != nil {
			t.Fatal(err)
		}
		defer c.Close()
		roundTrip(t, c, "hello through gif\n")
	})

	t.Run("teardown", func(t *testing.T) {
		d.Close()
		closed = true
		if _, err := net.InterfaceByName(iface); err == nil {
			t.Errorf("%s still exists after Close", iface)
		}
		c, err := net.DialTimeout("tcp", "10.77.0.1:80", 2*time.Second)
		if err == nil {
			c.Close()
			t.Error("10.77.0.1 still reachable after Close - route left behind")
		}
	})
}

func listenAny(t *testing.T) net.Listener {
	ln, err := net.Listen("tcp4", "0.0.0.0:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func() { defer c.Close(); io.Copy(c, c) }()
		}
	}()
	return ln
}

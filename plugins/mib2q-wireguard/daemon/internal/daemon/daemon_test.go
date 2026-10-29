package daemon

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"strings"
	"testing"
	"time"

	"golang.zx2c4.com/wireguard/conn"
	"golang.zx2c4.com/wireguard/device"
	"golang.zx2c4.com/wireguard/tun/netstack"

	"github.com/complex256/mib2q-wireguard/internal/config"
	"github.com/complex256/mib2q-wireguard/internal/keys"
)

// TestEndToEnd runs the unit (a full Daemon) against a "server" WireGuard
// peer over real UDP on loopback, then drives traffic both ways:
//
//	remote access: server -> unit tunnel IP:port -> [Forward] -> local service
//	outbound:      local client -> [Proxy] -> tunnel -> service on the server
//
// It uses only loopback, so the same binary doubles as an on-target check:
// `go test -c` for GOOS=qnx and run it on the unit or under QEMU.
func TestEndToEnd(t *testing.T) {
	srv := newTestServer(t)
	unitKey, srvPub, srvPort, snet := srv.unitKey, srv.pub, srv.port, srv.net
	unitAddr := srv.unitAddr

	// Local services on the unit's host stack, reached through [Forward].
	tcpEcho := listenTCPEcho(t)
	udpEcho := listenUDPEcho(t)

	socks := fmt.Sprintf("127.0.0.1:%d", freePort(t, "tcp"))
	httpProxy := fmt.Sprintf("127.0.0.1:%d", freePort(t, "tcp"))
	cfgText := fmt.Sprintf(`
[Interface]
PrivateKey = %s
Address = %s/32

[Peer]
PublicKey = %s
Endpoint = 127.0.0.1:%d
AllowedIPs = 10.77.0.0/24
PersistentKeepalive = 1

[Forward]
TCP = 2222 -> %s
UDP = 5353 -> %s

[Proxy]
SOCKS5 = %s
HTTP = %s
`, unitKey, unitAddr, srvPub, srvPort, tcpEcho, udpEcho, socks, httpProxy)
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
	defer d.Close()

	t.Run("forward/tcp", func(t *testing.T) {
		c, err := snet.DialContextTCPAddrPort(ctx, netip.AddrPortFrom(unitAddr, 2222))
		if err != nil {
			t.Fatal(err)
		}
		defer c.Close()
		roundTrip(t, c, "ssh-ish hello\n")
	})

	t.Run("forward/udp", func(t *testing.T) {
		c, err := snet.DialUDPAddrPort(netip.AddrPort{}, netip.AddrPortFrom(unitAddr, 5353))
		if err != nil {
			t.Fatal(err)
		}
		defer c.Close()
		c.SetDeadline(time.Now().Add(10 * time.Second))
		if _, err := c.Write([]byte("dns-ish")); err != nil {
			t.Fatal(err)
		}
		buf := make([]byte, 64)
		n, err := c.Read(buf)
		if err != nil || string(buf[:n]) != "dns-ish" {
			t.Fatalf("udp echo = %q, %v", buf[:n], err)
		}
	})

	for _, proxy := range []string{"socks5://" + socks, "http://" + httpProxy} {
		t.Run("proxy/"+proxy[:strings.Index(proxy, ":")], func(t *testing.T) {
			u, _ := url.Parse(proxy)
			client := &http.Client{
				Transport: &http.Transport{Proxy: http.ProxyURL(u)},
				Timeout:   15 * time.Second,
			}
			resp, err := client.Get("http://10.77.0.1/")
			if err != nil {
				t.Fatal(err)
			}
			body, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			// The server must see the unit's tunnel address as the source.
			if want := "you are 10.77.0.2:"; !strings.Contains(string(body), want) {
				t.Fatalf("body = %q, want %q", body, want)
			}
		})
	}

	t.Run("status", func(t *testing.T) {
		st := d.PeerStatus()
		if len(st) != 1 || st[0].PublicKey != srvPub || st[0].LastHandshake.IsZero() || st[0].RxBytes == 0 {
			t.Fatalf("status = %+v", st)
		}
	})
}

type testServer struct {
	unitKey, unitPub, pub config.Key
	port                  int
	net                   *netstack.Net
	addr, unitAddr        netip.Addr
}

// newTestServer starts the "server" side: a plain wireguard-go device on its
// own netstack at 10.77.0.1, peered with a fresh unit key at 10.77.0.2.
func newTestServer(t *testing.T) *testServer {
	t.Helper()
	s := &testServer{addr: netip.MustParseAddr("10.77.0.1"), unitAddr: netip.MustParseAddr("10.77.0.2")}
	srvKey, _ := keys.NewPrivate()
	s.unitKey, _ = keys.NewPrivate()
	s.unitPub, _ = keys.Public(s.unitKey)
	s.pub, _ = keys.Public(srvKey)
	s.port = freePort(t, "udp")
	stun, snet, err := netstack.CreateNetTUN([]netip.Addr{s.addr}, nil, 1420)
	if err != nil {
		t.Fatal(err)
	}
	s.net = snet
	sdev := device.NewDevice(stun, conn.NewDefaultBind(), device.NewLogger(device.LogLevelError, "server: "))
	t.Cleanup(sdev.Close)
	err = sdev.IpcSet(fmt.Sprintf("private_key=%s\nlisten_port=%d\npublic_key=%s\nallowed_ip=%s/32\n",
		srvKey.Hex(), s.port, s.unitPub.Hex(), s.unitAddr))
	if err != nil {
		t.Fatal(err)
	}
	if err := sdev.Up(); err != nil {
		t.Fatal(err)
	}
	web, err := snet.ListenTCP(&net.TCPAddr{IP: s.addr.AsSlice(), Port: 80})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { web.Close() })
	go http.Serve(web, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintf(w, "hello from the server, you are %s", r.RemoteAddr)
	}))
	return s
}

func roundTrip(t *testing.T, c net.Conn, msg string) {
	t.Helper()
	c.SetDeadline(time.Now().Add(10 * time.Second))
	if _, err := io.WriteString(c, msg); err != nil {
		t.Fatal(err)
	}
	got, err := bufio.NewReader(c).ReadString('\n')
	if err != nil || got != msg {
		t.Fatalf("echo = %q, %v", got, err)
	}
}

func freePort(t *testing.T, network string) int {
	t.Helper()
	if network == "udp" {
		pc, err := net.ListenPacket("udp4", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		defer pc.Close()
		return pc.LocalAddr().(*net.UDPAddr).Port
	}
	ln, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	return ln.Addr().(*net.TCPAddr).Port
}

func listenTCPEcho(t *testing.T) string {
	ln, err := net.Listen("tcp4", "127.0.0.1:0")
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
	return ln.Addr().String()
}

func listenUDPEcho(t *testing.T) string {
	pc, err := net.ListenPacket("udp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { pc.Close() })
	go func() {
		buf := make([]byte, 2048)
		for {
			n, from, err := pc.ReadFrom(buf)
			if err != nil {
				return
			}
			pc.WriteTo(buf[:n], from)
		}
	}()
	return pc.LocalAddr().String()
}

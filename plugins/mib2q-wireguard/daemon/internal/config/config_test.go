package config

import (
	"strings"
	"testing"
	"time"
)

const good = `
# the unit
[Interface]
PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
Address = 10.66.0.2/32, fd00:66::2/128
DNS = 10.66.0.1
MTU = 1380

[Peer]
PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=
PresharedKey = FpCyhws9cxwWoV4xELtfJvjJN+zQVRPISllRWgeopVE=
Endpoint = vpn.example.com:51820
AllowedIPs = 10.66.0.0/24
AllowedIPs = 192.168.1.0/24   # repeated keys append
PersistentKeepalive = 25

[Forward]
TCP = 22
TCP = 2323 -> 127.0.0.1:23
UDP = 5353 -> 10.0.0.1:53

[Proxy]
SOCKS5 = 127.0.0.1:1080
HTTP = 0.0.0.0:8118

[Time]
NTP = pool.ntp.org, time.google.com
`

func TestParseGood(t *testing.T) {
	c, err := Parse(strings.NewReader(good))
	if err != nil {
		t.Fatal(err)
	}
	if c.Interface.MTU != 1380 || len(c.Interface.Addresses) != 2 || len(c.Interface.DNS) != 1 {
		t.Errorf("interface = %+v", c.Interface)
	}
	p := c.Peers[0]
	if p.Endpoint != "vpn.example.com:51820" || len(p.AllowedIPs) != 2 || p.PersistentKeepalive != 25*time.Second {
		t.Errorf("peer = %+v", p)
	}
	want := []Forward{
		{"tcp", 22, "127.0.0.1:22"},
		{"tcp", 2323, "127.0.0.1:23"},
		{"udp", 5353, "10.0.0.1:53"},
	}
	if len(c.Forwards) != len(want) {
		t.Fatalf("forwards = %+v", c.Forwards)
	}
	for i := range want {
		if c.Forwards[i] != want[i] {
			t.Errorf("forward %d = %+v, want %+v", i, c.Forwards[i], want[i])
		}
	}
	if c.Proxy.SOCKS5 != "127.0.0.1:1080" || c.Proxy.HTTP != "0.0.0.0:8118" {
		t.Errorf("proxy = %+v", c.Proxy)
	}
	if len(c.Time.NTP) != 2 {
		t.Errorf("ntp = %v", c.Time.NTP)
	}
}

func TestUAPI(t *testing.T) {
	c, err := Parse(strings.NewReader(good))
	if err != nil {
		t.Fatal(err)
	}
	u := c.UAPI()
	for _, line := range []string{
		"private_key=c809f3e5317e9575c9b5ed78b638b7ce530dabe85ddab614220241801ddf0669\n",
		"public_key=c5320103",
		"allowed_ip=10.66.0.0/24\n",
		"allowed_ip=192.168.1.0/24\n",
	} {
		if !strings.Contains(u, line) {
			t.Errorf("UAPI missing %q:\n%s", line, u)
		}
	}
	// Hostname endpoints are resolved at runtime, never baked in.
	if strings.Contains(u, "endpoint=") {
		t.Errorf("UAPI must not carry a hostname endpoint:\n%s", u)
	}
	// ...and neither is the keepalive, which would fire before resolution.
	if strings.Contains(u, "persistent_keepalive") {
		t.Errorf("UAPI must defer keepalive for a hostname endpoint:\n%s", u)
	}
	c.Peers[0].Endpoint = "192.0.2.7:51820"
	u = c.UAPI()
	for _, line := range []string{"endpoint=192.0.2.7:51820\n", "persistent_keepalive_interval=25\n"} {
		if !strings.Contains(u, line) {
			t.Errorf("UAPI missing %q for a literal endpoint:\n%s", line, u)
		}
	}
}

func TestParseErrors(t *testing.T) {
	const key = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk="
	iface := "[Interface]\nPrivateKey = " + key + "\nAddress = 10.0.0.2/32\n"
	peer := "[Peer]\nPublicKey = " + key + "\n"
	for name, tc := range map[string]struct{ in, want string }{
		"no key":        {"[Interface]\nAddress = 10.0.0.2\n" + peer, "PrivateKey is required"},
		"no address":    {"[Interface]\nPrivateKey = " + key + "\n" + peer, "Address is required"},
		"no peer":       {iface, "at least one [Peer]"},
		"bad key":       {"[Interface]\nPrivateKey = nope\n", "invalid key"},
		"short key":     {"[Interface]\nPrivateKey = AAAA\n", "want 32"},
		"bad section":   {"[Bogus]\n", "unknown section"},
		"wg-quick hook": {"[Interface]\nPostUp = iptables -A\n", "PostUp"},
		"no section":    {"PrivateKey = x\n", "outside of a section"},
		"bad endpoint":  {iface + peer + "Endpoint = example.com\n", "Endpoint"},
		"dup peer":      {iface + peer + peer, "duplicate PublicKey"},
		"bad forward":   {iface + peer + "[Forward]\nSCTP = 1\n", "want TCP or UDP"},
		"zero forward":  {iface + peer + "[Forward]\nTCP = 0\n", "nonzero"},
		"dup forward":   {iface + peer + "[Forward]\nTCP = 22\nTCP = 22 -> 127.0.0.1:2222\n", "listed twice"},
		"bad target":    {iface + peer + "[Forward]\nTCP = 22 -> localhost\n", "want host:port"},
		"bad proxy":     {iface + peer + "[Proxy]\nSOCKS5 = 1080\n", "want host:port"},
		"bad mtu":       {"[Interface]\nMTU = 100\n", "out of range"},
		"bad backend":   {"[Interface]\nBackend = utun\n", "want netstack, tun or gif"},
		"bad table":     {"[Interface]\nTable = 1234\n", "want auto or off"},
	} {
		_, err := Parse(strings.NewReader(tc.in))
		if err == nil || !strings.Contains(err.Error(), tc.want) {
			t.Errorf("%s: err = %v, want containing %q", name, err, tc.want)
		}
	}
}

func TestBackendAndRoutes(t *testing.T) {
	c, err := Parse(strings.NewReader(good))
	if err != nil {
		t.Fatal(err)
	}
	if c.Interface.Backend != "netstack" || c.Interface.Table != "auto" {
		t.Errorf("defaults = %q/%q", c.Interface.Backend, c.Interface.Table)
	}
	if r := c.Routes(); len(r) != 2 || r[0].String() != "10.66.0.0/24" {
		t.Errorf("routes = %v", r)
	}
	in := strings.Replace(good, "MTU = 1380", "MTU = 1380\nBackend = GIF\nTable = off", 1)
	if c, err = Parse(strings.NewReader(in)); err != nil {
		t.Fatal(err)
	}
	if c.Interface.Backend != "gif" || c.Routes() != nil {
		t.Errorf("backend %q, routes %v", c.Interface.Backend, c.Routes())
	}
}

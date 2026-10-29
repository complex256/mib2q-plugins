// Package config parses mibwg's configuration: a wg-quick style INI file
// ([Interface] + [Peer] sections, same keys as wg-quick) extended with the
// sections a tun-less QNX head unit needs - [Forward], [Proxy] and [Time].
package config

import (
	"bufio"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"strconv"
	"strings"
	"time"
)

// Key is a Curve25519 key (private, public or preshared).
type Key [32]byte

func ParseKey(s string) (Key, error) {
	var k Key
	b, err := base64.StdEncoding.DecodeString(strings.TrimSpace(s))
	if err != nil {
		return k, fmt.Errorf("invalid key %q: %w", s, err)
	}
	if len(b) != len(k) {
		return k, fmt.Errorf("invalid key %q: %d bytes, want 32", s, len(b))
	}
	copy(k[:], b)
	return k, nil
}

func (k Key) Hex() string    { return hex.EncodeToString(k[:]) }
func (k Key) String() string { return base64.StdEncoding.EncodeToString(k[:]) }
func (k Key) IsZero() bool   { return k == Key{} }

type Interface struct {
	PrivateKey Key
	Addresses  []netip.Prefix
	DNS        []netip.Addr
	ListenPort int // 0 = random
	MTU        int
	// Backend is where tunnel packets terminate: "netstack" (userspace stack,
	// reached via [Forward]/[Proxy]), or kernel-routed (QNX only): "tun" when
	// io-pkt has a tun cloner, else "gif".
	Backend string
	// Table = off keeps the kernel-routed backends from adding routes for AllowedIPs,
	// as in wg-quick.
	Table string
}

type Peer struct {
	PublicKey           Key
	PresharedKey        Key
	Endpoint            string // host:port, resolved at runtime (the unit often boots offline)
	AllowedIPs          []netip.Prefix
	PersistentKeepalive time.Duration
}

// Forward exposes a head-unit service on the tunnel: connections arriving on
// the unit's tunnel address at ListenPort are relayed to Target on the host
// network stack (usually 127.0.0.1).
type Forward struct {
	Proto      string // "tcp" or "udp"
	ListenPort uint16
	Target     string // host:port
}

type Proxy struct {
	SOCKS5 string // listen address on the host stack, "" = off
	HTTP   string
}

type Time struct {
	NTP []string // SNTP servers; empty = don't touch the clock
}

type Config struct {
	Interface Interface
	Peers     []Peer
	Forwards  []Forward
	Proxy     Proxy
	Time      Time
}

const DefaultMTU = 1420

func Load(path string) (*Config, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	return Parse(f)
}

func Parse(r io.Reader) (*Config, error) {
	c := &Config{Interface: Interface{MTU: DefaultMTU, Backend: "netstack", Table: "auto"}}
	var section string
	var peer *Peer
	sc := bufio.NewScanner(r)
	for n := 1; sc.Scan(); n++ {
		line := sc.Text()
		if i := strings.IndexByte(line, '#'); i >= 0 {
			line = line[:i]
		}
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		if strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]") {
			section = strings.ToLower(strings.TrimSpace(line[1 : len(line)-1]))
			switch section {
			case "peer":
				c.Peers = append(c.Peers, Peer{})
				peer = &c.Peers[len(c.Peers)-1]
			case "interface", "forward", "proxy", "time":
			default:
				return nil, fmt.Errorf("line %d: unknown section [%s]", n, section)
			}
			continue
		}
		key, val, ok := strings.Cut(line, "=")
		if !ok {
			return nil, fmt.Errorf("line %d: expected key = value", n)
		}
		key = strings.ToLower(strings.TrimSpace(key))
		val = strings.TrimSpace(val)
		var err error
		switch section {
		case "interface":
			err = c.Interface.set(key, val)
		case "peer":
			err = peer.set(key, val)
		case "forward":
			var fw Forward
			if fw, err = parseForward(key, val); err == nil {
				c.Forwards = append(c.Forwards, fw)
			}
		case "proxy":
			err = c.Proxy.set(key, val)
		case "time":
			err = c.Time.set(key, val)
		default:
			err = fmt.Errorf("key %q outside of a section", key)
		}
		if err != nil {
			return nil, fmt.Errorf("line %d: %w", n, err)
		}
	}
	if err := sc.Err(); err != nil {
		return nil, err
	}
	return c, c.validate()
}

func (in *Interface) set(key, val string) (err error) {
	switch key {
	case "privatekey":
		in.PrivateKey, err = ParseKey(val)
	case "address":
		for _, s := range splitList(val) {
			p, perr := parsePrefix(s)
			if perr != nil {
				return perr
			}
			in.Addresses = append(in.Addresses, p)
		}
	case "dns":
		for _, s := range splitList(val) {
			a, perr := netip.ParseAddr(s)
			if perr != nil {
				return fmt.Errorf("DNS: %w", perr)
			}
			in.DNS = append(in.DNS, a)
		}
	case "listenport":
		in.ListenPort, err = parsePort(val)
	case "mtu":
		in.MTU, err = strconv.Atoi(val)
		if err == nil && (in.MTU < 576 || in.MTU > 65535) {
			err = fmt.Errorf("MTU %d out of range", in.MTU)
		}
	case "backend":
		in.Backend = strings.ToLower(val)
		if in.Backend != "netstack" && in.Backend != "tun" && in.Backend != "gif" {
			err = fmt.Errorf("Backend %q: want netstack, tun or gif", val)
		}
	case "table":
		in.Table = strings.ToLower(val)
		if in.Table != "auto" && in.Table != "off" {
			err = fmt.Errorf("Table %q: want auto or off (custom tables aren't supported)", val)
		}
	default:
		err = fmt.Errorf("unknown [Interface] key %q (wg-quick keys like PostUp have no meaning here)", key)
	}
	return err
}

func (p *Peer) set(key, val string) (err error) {
	switch key {
	case "publickey":
		p.PublicKey, err = ParseKey(val)
	case "presharedkey":
		p.PresharedKey, err = ParseKey(val)
	case "endpoint":
		p.Endpoint = val
	case "allowedips":
		for _, s := range splitList(val) {
			pf, perr := parsePrefix(s)
			if perr != nil {
				return perr
			}
			p.AllowedIPs = append(p.AllowedIPs, pf.Masked())
		}
	case "persistentkeepalive":
		if val == "off" {
			return nil
		}
		var secs int
		secs, err = strconv.Atoi(val)
		if err == nil && (secs < 0 || secs > 65535) {
			err = fmt.Errorf("PersistentKeepalive %d out of range", secs)
		}
		p.PersistentKeepalive = time.Duration(secs) * time.Second
	default:
		err = fmt.Errorf("unknown [Peer] key %q", key)
	}
	return err
}

// parseForward accepts "TCP = 22" (tunnel port 22 -> 127.0.0.1:22) and
// "TCP = 2222 -> 127.0.0.1:22".
func parseForward(key, val string) (Forward, error) {
	fw := Forward{Proto: key}
	if key != "tcp" && key != "udp" {
		return fw, fmt.Errorf("unknown [Forward] key %q (want TCP or UDP)", key)
	}
	port, target, hasTarget := strings.Cut(val, "->")
	p, err := parsePort(strings.TrimSpace(port))
	if err != nil {
		return fw, err
	}
	if p == 0 {
		return fw, fmt.Errorf("forward port must be nonzero")
	}
	fw.ListenPort = uint16(p)
	fw.Target = "127.0.0.1:" + strconv.Itoa(p)
	if hasTarget {
		fw.Target = strings.TrimSpace(target)
		if _, tp, err := splitHostPort(fw.Target); err != nil || tp == 0 {
			return fw, fmt.Errorf("forward target %q: want host:port", fw.Target)
		}
	}
	return fw, nil
}

func (p *Proxy) set(key, val string) error {
	switch key {
	case "socks5":
		p.SOCKS5 = val
	case "http":
		p.HTTP = val
	default:
		return fmt.Errorf("unknown [Proxy] key %q (want SOCKS5 or HTTP)", key)
	}
	if _, _, err := splitHostPort(val); err != nil {
		return fmt.Errorf("%s listen address %q: want host:port", key, val)
	}
	return nil
}

func (t *Time) set(key, val string) error {
	if key != "ntp" {
		return fmt.Errorf("unknown [Time] key %q (want NTP)", key)
	}
	t.NTP = append(t.NTP, splitList(val)...)
	return nil
}

func (c *Config) validate() error {
	if c.Interface.PrivateKey.IsZero() {
		return fmt.Errorf("[Interface] PrivateKey is required")
	}
	if len(c.Interface.Addresses) == 0 {
		return fmt.Errorf("[Interface] Address is required - it is the unit's IP inside the tunnel")
	}
	if len(c.Peers) == 0 {
		return fmt.Errorf("at least one [Peer] is required")
	}
	seen := map[Key]bool{}
	for i, p := range c.Peers {
		if p.PublicKey.IsZero() {
			return fmt.Errorf("[Peer] #%d: PublicKey is required", i+1)
		}
		if seen[p.PublicKey] {
			return fmt.Errorf("[Peer] #%d: duplicate PublicKey", i+1)
		}
		seen[p.PublicKey] = true
		if p.Endpoint != "" {
			if _, port, err := splitHostPort(p.Endpoint); err != nil || port == 0 {
				return fmt.Errorf("[Peer] #%d: Endpoint %q: want host:port", i+1, p.Endpoint)
			}
		}
	}
	ports := map[string]bool{}
	for _, fw := range c.Forwards {
		k := fw.Proto + "/" + strconv.Itoa(int(fw.ListenPort))
		if ports[k] {
			return fmt.Errorf("[Forward] %s port %d listed twice", strings.ToUpper(fw.Proto), fw.ListenPort)
		}
		ports[k] = true
	}
	return nil
}

// Routes are the prefixes a kernel-routed backend should send into the
// tunnel: every peer's AllowedIPs, unless Table = off.
func (c *Config) Routes() []netip.Prefix {
	if c.Interface.Table == "off" {
		return nil
	}
	var out []netip.Prefix
	for _, p := range c.Peers {
		out = append(out, p.AllowedIPs...)
	}
	return out
}

// UAPI renders the device configuration in wireguard's cross-platform
// configuration protocol, as consumed by device.IpcSet.
func (c *Config) UAPI() string {
	var b strings.Builder
	fmt.Fprintf(&b, "private_key=%s\n", c.Interface.PrivateKey.Hex())
	if c.Interface.ListenPort != 0 {
		fmt.Fprintf(&b, "listen_port=%d\n", c.Interface.ListenPort)
	}
	b.WriteString("replace_peers=true\n")
	for _, p := range c.Peers {
		fmt.Fprintf(&b, "public_key=%s\n", p.PublicKey.Hex())
		// Literal endpoints go in with the peer: the netstack device is up
		// from creation, so a keepalive fires during this very IpcSet.
		// Hostnames are resolved later, off the startup path.
		if ap, err := netip.ParseAddrPort(p.Endpoint); err == nil {
			fmt.Fprintf(&b, "endpoint=%s\n", ap)
		}
		if !p.PresharedKey.IsZero() {
			fmt.Fprintf(&b, "preshared_key=%s\n", p.PresharedKey.Hex())
		}
		// A hostname endpoint isn't known yet; a keepalive now would only
		// log "no known endpoint", so the daemon sends it with the endpoint.
		if !p.hostnameEndpoint() {
			b.WriteString(KeepaliveUAPI(p))
		}
		b.WriteString("replace_allowed_ips=true\n")
		for _, a := range p.AllowedIPs {
			fmt.Fprintf(&b, "allowed_ip=%s\n", a)
		}
	}
	return b.String()
}

// KeepaliveUAPI is the peer's persistent_keepalive_interval line, if any.
func KeepaliveUAPI(p Peer) string {
	if p.PersistentKeepalive <= 0 {
		return ""
	}
	return fmt.Sprintf("persistent_keepalive_interval=%d\n", int(p.PersistentKeepalive/time.Second))
}

func (p Peer) hostnameEndpoint() bool {
	if p.Endpoint == "" {
		return false
	}
	_, err := netip.ParseAddrPort(p.Endpoint)
	return err != nil
}

func splitList(s string) []string {
	var out []string
	for _, f := range strings.Split(s, ",") {
		if f = strings.TrimSpace(f); f != "" {
			out = append(out, f)
		}
	}
	return out
}

// parsePrefix accepts "10.0.0.2/24" or a bare address (host prefix).
func parsePrefix(s string) (netip.Prefix, error) {
	if strings.Contains(s, "/") {
		return netip.ParsePrefix(s)
	}
	a, err := netip.ParseAddr(s)
	if err != nil {
		return netip.Prefix{}, err
	}
	return netip.PrefixFrom(a, a.BitLen()), nil
}

func parsePort(s string) (int, error) {
	p, err := strconv.Atoi(s)
	if err != nil || p < 0 || p > 65535 {
		return 0, fmt.Errorf("invalid port %q", s)
	}
	return p, nil
}

// splitHostPort splits host:port (with [v6]:port) and validates the port.
func splitHostPort(s string) (string, int, error) {
	host, port, err := net.SplitHostPort(s)
	if err != nil {
		return "", 0, err
	}
	p, err := parsePort(port)
	return host, p, err
}

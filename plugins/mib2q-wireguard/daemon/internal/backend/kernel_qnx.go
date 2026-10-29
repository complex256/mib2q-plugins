//go:build qnx

package backend

import (
	"fmt"
	"net/netip"
	"os"
	"os/exec"
	"strings"
	"sync"
	"sync/atomic"

	"golang.zx2c4.com/wireguard/tun"
)

// kernelIface is what the kernel-routed backends (tun, gif) share: an io-pkt
// interface carrying the unit's tunnel addresses, routes for AllowedIPs,
// endpoint pinning for full-tunnel mode, and the queue of packets read from
// the device. Each backend supplies only how packets get in and out.
type kernelIface struct {
	name      string
	opts      KernelOptions
	gw4, gw6  netip.Addr // the tunnel addresses routes point at (-interface)
	full4     bool
	defaults  []netip.Prefix // default-route halves, installed only while pinned
	defaultOn bool
	// endpoint is the peer's outer address. In full-tunnel mode, packets to it
	// that still reach the device (no pinned route yet) are dropped:
	// re-encrypting WireGuard's own UDP into the tunnel would loop forever.
	endpoint  atomic.Pointer[netip.Addr]
	pinMu     sync.Mutex
	pinned    netip.Addr
	pinGW     string
	pkts      chan []byte
	events    chan tun.Event
	closed    chan struct{}
	closeOnce sync.Once
}

func newKernelIface(name string, o KernelOptions) *kernelIface {
	return &kernelIface{
		name:   name,
		opts:   o,
		pkts:   make(chan []byte, 256),
		events: make(chan tun.Event, 4),
		closed: make(chan struct{}),
	}
}

// configure sets MTU and addresses, brings the interface up and installs
// routes. withV6 = false skips IPv6 (a device that can only carry IPv4).
func (k *kernelIface) configure(withV6 bool) error {
	if err := run("ifconfig", k.name, "mtu", fmt.Sprint(k.opts.MTU)); err != nil {
		return err
	}
	for _, p := range k.opts.Addrs {
		a := p.Addr()
		var err error
		switch {
		case a.Is4() && !k.gw4.IsValid():
			// Point-to-point needs a destination; the address itself will do,
			// since routes point at the interface via -interface.
			err = run("ifconfig", k.name, "inet", a.String(), a.String(), "netmask", "255.255.255.255")
			k.gw4 = a
		case a.Is4():
			err = run("ifconfig", k.name, "inet", a.String(), a.String(), "netmask", "255.255.255.255", "alias")
		case !withV6:
			k.opts.Logf("%s: skipping IPv6 address %s: this device carries IPv4 only", k.name, a)
		default:
			err = run("ifconfig", k.name, "inet6", a.String(), "prefixlen", "128", "alias")
			if !k.gw6.IsValid() {
				k.gw6 = a
			}
		}
		if err != nil {
			return err
		}
	}
	if err := run("ifconfig", k.name, "up"); err != nil {
		return err
	}
	for _, r := range expandRoutes(k.opts.Routes) {
		if r.Bits() == 1 {
			// Default route: held back until the endpoint is pinned outside
			// the tunnel - otherwise resolving the endpoint's own name would
			// have to go through the tunnel it is needed to bring up.
			k.defaults = append(k.defaults, r)
			k.full4 = k.full4 || r.Addr().Is4()
			continue
		}
		k.addRoute(r)
	}
	return nil
}

func (k *kernelIface) addRoute(r netip.Prefix) bool {
	gw, fam := k.gw4, "-inet"
	if r.Addr().Is6() {
		gw, fam = k.gw6, "-inet6"
	}
	if !gw.IsValid() {
		k.opts.Logf("%s: skipping route %s: no tunnel address of that family", k.name, r)
		return false
	}
	if err := run("route", "add", fam, "-net", r.String(), "-interface", gw.String()); err != nil {
		// An existing route (e.g. the uplink's own subnet) isn't fatal;
		// that prefix just stays where it was.
		k.opts.Logf("%s: route %s: %v", k.name, r, err)
		return false
	}
	k.opts.Logf("%s: route %s", k.name, r)
	return true
}

// setDefaults adds or removes the default-route halves (caller holds pinMu).
func (k *kernelIface) setDefaults(on bool) {
	if on == k.defaultOn {
		return
	}
	for _, r := range k.defaults {
		if on {
			k.addRoute(r)
			continue
		}
		fam := "-inet"
		if r.Addr().Is6() {
			fam = "-inet6"
		}
		run("route", "delete", fam, "-net", r.String())
	}
	k.defaultOn = on
	if on {
		k.opts.Logf("%s: full tunnel on - default route via %s", k.name, k.name)
	} else {
		k.opts.Logf("%s: full tunnel off - traffic goes direct until the endpoint is pinned again", k.name)
	}
}

// PinEndpoint keeps the WireGuard server itself reachable outside the tunnel
// in full-tunnel mode: a host route to it via the uplink's default gateway,
// and only then the default-route halves. Called periodically - the uplink
// (tethered phone, hotspot, LTE) changes.
func (k *kernelIface) PinEndpoint(ep netip.Addr) {
	k.endpoint.Store(&ep)
	if !k.full4 || !ep.Is4() {
		return
	}
	gw := defaultGateway()
	k.pinMu.Lock()
	defer k.pinMu.Unlock()
	if ep == k.pinned && gw == k.pinGW {
		return
	}
	if k.pinned.IsValid() {
		run("route", "delete", "-host", k.pinned.String())
	}
	k.pinned, k.pinGW = netip.Addr{}, ""
	// Fail open: without a pinned endpoint the default halves would swallow
	// WireGuard's own UDP, so the unit goes direct rather than dark.
	if gw == "" {
		k.opts.Logf("%s: no default route yet; endpoint %s not pinned", k.name, ep)
		k.setDefaults(false)
		return
	}
	if err := run("route", "add", "-host", ep.String(), gw); err != nil {
		k.opts.Logf("%s: pin endpoint %s via %s: %v", k.name, ep, gw, err)
		k.setDefaults(false)
		return
	}
	k.pinned, k.pinGW = ep, gw
	k.opts.Logf("%s: endpoint %s pinned via %s", k.name, ep, gw)
	k.setDefaults(true)
}

// deliver queues a packet read from the device for WireGuard. It reports
// false once the interface is closed.
func (k *kernelIface) deliver(inner []byte) bool {
	if ep := k.endpoint.Load(); ep != nil && k.full4 && innerDst(inner) == *ep {
		return true // WireGuard's own traffic leaking into the tunnel
	}
	select {
	case k.pkts <- append([]byte(nil), inner...):
		return true
	case <-k.closed:
		return false
	}
}

func (k *kernelIface) Read(bufs [][]byte, sizes []int, offset int) (int, error) {
	select {
	case p := <-k.pkts:
		sizes[0] = copy(bufs[0][offset:], p)
		return 1, nil
	case <-k.closed:
		return 0, os.ErrClosed
	}
}

func (k *kernelIface) File() *os.File           { return nil }
func (k *kernelIface) MTU() (int, error)        { return k.opts.MTU, nil }
func (k *kernelIface) Name() (string, error)    { return k.name, nil }
func (k *kernelIface) Events() <-chan tun.Event { return k.events }
func (k *kernelIface) BatchSize() int           { return 1 }

func (k *kernelIface) Net() Network {
	var addrs []netip.Addr
	for _, p := range k.opts.Addrs {
		addrs = append(addrs, p.Addr())
	}
	return &hostNetwork{addrs: addrs}
}

// shutdown destroys the interface (which drops every route through it) and
// the endpoint pin; extra releases the backend's own resources first.
func (k *kernelIface) shutdown(extra func()) {
	k.closeOnce.Do(func() {
		close(k.closed)
		extra()
		k.destroy()
		close(k.events)
	})
}

func (k *kernelIface) destroy() {
	run("ifconfig", k.name, "destroy")
	k.pinMu.Lock()
	if k.pinned.IsValid() {
		run("route", "delete", "-host", k.pinned.String())
	}
	k.pinMu.Unlock()
}

// defaultGateway parses `route -n get default`; "" when there is none.
func defaultGateway() string {
	out, err := output("route", "-n", "get", "default")
	if err != nil {
		return ""
	}
	for _, line := range strings.Split(out, "\n") {
		if k, v, ok := strings.Cut(strings.TrimSpace(line), ":"); ok && k == "gateway" {
			return strings.TrimSpace(v)
		}
	}
	return ""
}

// The unit's PATH is sparse and tools are scattered (ifconfig in /usr/sbin,
// route in /armle/sbin on the MHI2q; everything in /proc/boot in the QEMU
// test image), so they are looked up rather than taken from PATH.
var toolDirs = []string{"/sbin", "/usr/sbin", "/bin", "/usr/bin", "/armle/sbin", "/armle/bin", "/armle/usr/sbin", "/armle/usr/bin", "/proc/boot"}

func tool(name string) string {
	for _, d := range toolDirs {
		p := d + "/" + name
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	return name
}

func output(name string, args ...string) (string, error) {
	out, err := exec.Command(tool(name), args...).CombinedOutput()
	if err != nil {
		return string(out), fmt.Errorf("%s %s: %v: %s", name, strings.Join(args, " "), err, strings.TrimSpace(string(out)))
	}
	return string(out), nil
}

func run(name string, args ...string) error {
	_, err := output(name, args...)
	return err
}

// Package daemon assembles a running mibwg: WireGuard device, tunnel backend,
// endpoint resolution, clock sync, forwards and proxies.
package daemon

import (
	"bufio"
	"context"
	"encoding/hex"
	"fmt"
	"net"
	"net/netip"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"golang.zx2c4.com/wireguard/conn"
	"golang.zx2c4.com/wireguard/device"

	"github.com/complex256/mib2q-wireguard/internal/backend"
	"github.com/complex256/mib2q-wireguard/internal/config"
	"github.com/complex256/mib2q-wireguard/internal/forward"
	"github.com/complex256/mib2q-wireguard/internal/proxy"
	"github.com/complex256/mib2q-wireguard/internal/sntp"
)

type Options struct {
	Logf    func(format string, args ...any)
	Verbose bool   // WireGuard device debug logging
	Status  string // file to rewrite with peer status periodically; "" = off
}

type Daemon struct {
	cfg    *config.Config
	opts   Options
	dev    *device.Device
	be     backend.Backend
	cancel context.CancelFunc
	wg     sync.WaitGroup
}

// Timing knobs, variables so tests can shrink them.
var (
	ResolveRetry    = 10 * time.Second
	ReresolveAfter  = 3 * time.Minute // no handshake for this long -> re-resolve endpoint DNS
	CheckInterval   = 30 * time.Second
	ClockResync     = 6 * time.Hour
	ClockRetry      = 30 * time.Second
	ClockRetryUnset = 5 * time.Second // while the clock still reads 1970
	initialClockTry = 8 * time.Second
)

func Start(ctx context.Context, cfg *config.Config, opts Options) (*Daemon, error) {
	if opts.Logf == nil {
		opts.Logf = func(string, ...any) {}
	}
	logf := opts.Logf
	ctx, cancel := context.WithCancel(ctx)
	d := &Daemon{cfg: cfg, opts: opts, cancel: cancel}
	ok := false
	defer func() {
		if !ok {
			d.Close()
		}
	}()

	if len(cfg.Time.NTP) > 0 {
		// One bounded attempt before the first handshake, so a unit with a
		// working uplink never sends a 1970 timestamp; the loop keeps trying.
		if !sntp.ClockPlausible() {
			tctx, tcancel := context.WithTimeout(ctx, initialClockTry)
			d.syncClock(tctx)
			tcancel()
		}
		d.goLoop(ctx, d.clockLoop)
	} else if !sntp.ClockPlausible() {
		logf("warning: clock reads %s and [Time] NTP is not set; peers that have seen a newer handshake will reject this one", time.Now().UTC().Format(time.RFC3339))
	}

	var addrs []netip.Addr
	for _, p := range cfg.Interface.Addresses {
		addrs = append(addrs, p.Addr())
	}
	var be backend.Backend
	var err error
	kopts := backend.KernelOptions{
		Addrs: cfg.Interface.Addresses, Routes: cfg.Routes(), MTU: cfg.Interface.MTU, Logf: logf,
	}
	switch cfg.Interface.Backend {
	case "tun":
		be, err = backend.NewTun(kopts)
	case "gif":
		be, err = backend.NewGif(kopts)
	default:
		be, err = backend.NewNetstack(addrs, cfg.Interface.DNS, cfg.Interface.MTU)
	}
	if err != nil {
		return nil, err
	}
	d.be = be

	wglog := &device.Logger{
		Verbosef: func(f string, a ...any) { logf("wg: "+f, a...) },
		Errorf:   func(f string, a ...any) { logf("wg: error: "+f, a...) },
	}
	if !opts.Verbose {
		wglog.Verbosef = device.DiscardLogf
	}
	d.dev = device.NewDevice(be.TUN(), conn.NewDefaultBind(), wglog)
	if err := d.dev.IpcSet(cfg.UAPI()); err != nil {
		return nil, fmt.Errorf("configure device: %w", err)
	}
	if err := d.dev.Up(); err != nil {
		return nil, fmt.Errorf("device up: %w", err)
	}
	logf("wireguard up (%s backend): %d peer(s), addresses %v, mtu %d", cfg.Interface.Backend, len(cfg.Peers), addrs, cfg.Interface.MTU)

	for _, p := range cfg.Peers {
		if p.Endpoint != "" {
			d.goLoop(ctx, func(ctx context.Context) { d.endpointLoop(ctx, p) })
		}
	}

	if cfg.Interface.Backend != "netstack" {
		// The tunnel address is a real kernel address here, so services are
		// reachable directly and a forward is optional - one that clashes
		// with a real listener (sshd on *:22) is only worth a warning.
		for _, fw := range cfg.Forwards {
			if err := forward.Start(ctx, be.Net(), []config.Forward{fw}, logf); err != nil {
				logf("forward: %s %d skipped (the %s backend reaches unit services directly): %v", fw.Proto, fw.ListenPort, cfg.Interface.Backend, err)
			}
		}
	} else if err := forward.Start(ctx, be.Net(), cfg.Forwards, logf); err != nil {
		return nil, fmt.Errorf("forward: %w", err)
	}
	if a := cfg.Proxy.SOCKS5; a != "" {
		ln, err := net.Listen("tcp", a)
		if err != nil {
			return nil, fmt.Errorf("socks5 listen: %w", err)
		}
		go func() { <-ctx.Done(); ln.Close() }()
		go proxy.ServeSOCKS5(ctx, ln, be.Net(), logf)
		logf("socks5 proxy on %s", ln.Addr())
	}
	if a := cfg.Proxy.HTTP; a != "" {
		ln, err := net.Listen("tcp", a)
		if err != nil {
			return nil, fmt.Errorf("http proxy listen: %w", err)
		}
		go proxy.ServeHTTPProxy(ctx, ln, proxy.NewHTTP(be.Net(), logf))
		logf("http proxy on %s", ln.Addr())
	}
	if opts.Status != "" {
		d.goLoop(ctx, d.statusLoop)
	}
	ok = true
	return d, nil
}

func (d *Daemon) Close() {
	d.cancel()
	if d.dev != nil {
		d.dev.Close()
	}
	d.wg.Wait()
}

// Wait blocks until the device shuts down.
func (d *Daemon) Wait() <-chan struct{} { return d.dev.Wait() }

func (d *Daemon) goLoop(ctx context.Context, f func(context.Context)) {
	d.wg.Add(1)
	go func() {
		defer d.wg.Done()
		f(ctx)
	}()
}

func sleep(ctx context.Context, dur time.Duration) bool {
	t := time.NewTimer(dur)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-t.C:
		return true
	}
}

func (d *Daemon) syncClock(ctx context.Context) bool {
	delta, err := sntp.Sync(ctx, d.cfg.Time.NTP)
	if err != nil {
		d.opts.Logf("time: %v", err)
		return false
	}
	d.opts.Logf("time: clock set to %s (adjusted %s)", time.Now().UTC().Format(time.RFC3339), delta.Round(time.Millisecond))
	return true
}

func (d *Daemon) clockLoop(ctx context.Context) {
	for {
		// No need to kick the device after a jump: wireguard-go stamps every
		// handshake retry (each 5 s) with time.Now().
		wait := ClockResync
		if !d.syncClock(ctx) {
			wait = ClockRetry
			// At boot DNS comes up seconds after mibwg, and until the clock
			// is set every handshake is rejected as a replay - retry fast.
			if !sntp.ClockPlausible() {
				wait = ClockRetryUnset
			}
		}
		if !sleep(ctx, wait) {
			return
		}
	}
}

// endpointLoop resolves a peer's Endpoint (the unit usually boots before it
// has an uplink) and re-resolves it when handshakes stop, which follows a
// dynamic-DNS server to a new address.
func (d *Daemon) endpointLoop(ctx context.Context, p config.Peer) {
	logf := d.opts.Logf
	host, port, _ := net.SplitHostPort(p.Endpoint)
	current := ""
	for {
		st, _ := d.status(p.PublicKey)
		stale := st.LastHandshake.IsZero() || time.Since(st.LastHandshake) > ReresolveAfter
		if current == "" || stale {
			ep, err := resolve(ctx, host, port)
			switch {
			case err != nil:
				if current == "" {
					logf("peer %s: resolve %s: %v (retrying)", short(p.PublicKey), p.Endpoint, err)
				}
			case ep != current:
				err := d.dev.IpcSet(fmt.Sprintf("public_key=%s\nupdate_only=true\nendpoint=%s\n%s",
					p.PublicKey.Hex(), ep, config.KeepaliveUAPI(p)))
				if err != nil {
					logf("peer %s: set endpoint %s: %v", short(p.PublicKey), ep, err)
				} else {
					logf("peer %s: endpoint %s (%s)", short(p.PublicKey), ep, p.Endpoint)
					current = ep
				}
			}
		}
		// Re-pinned every tick: the uplink's gateway changes under us.
		if pin, ok := d.be.(backend.EndpointPinner); ok && current != "" {
			pin.PinEndpoint(netip.MustParseAddrPort(current).Addr())
		}
		wait := CheckInterval
		if current == "" {
			wait = ResolveRetry
		}
		if !sleep(ctx, wait) {
			return
		}
	}
}

// resolve prefers IPv4: head-unit uplinks (tethered phone, hotspot) rarely
// have working IPv6.
func resolve(ctx context.Context, host, port string) (string, error) {
	if a, err := netip.ParseAddr(host); err == nil {
		return netip.AddrPortFrom(a, mustPort(port)).String(), nil
	}
	rctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	ips, err := net.DefaultResolver.LookupNetIP(rctx, "ip", host)
	if err != nil {
		return "", err
	}
	best := ips[0]
	for _, ip := range ips {
		if ip.Unmap().Is4() {
			best = ip
			break
		}
	}
	return netip.AddrPortFrom(best.Unmap(), mustPort(port)).String(), nil
}

func mustPort(s string) uint16 {
	p, _ := strconv.ParseUint(s, 10, 16)
	return uint16(p)
}

type PeerStatus struct {
	PublicKey     config.Key
	Endpoint      string
	LastHandshake time.Time
	RxBytes       uint64
	TxBytes       uint64
}

func (d *Daemon) status(pub config.Key) (PeerStatus, bool) {
	for _, st := range d.peerStatus() {
		if st.PublicKey == pub {
			return st, true
		}
	}
	return PeerStatus{}, false
}

// peerStatus parses the device's UAPI "get" dump.
func (d *Daemon) peerStatus() []PeerStatus {
	raw, err := d.dev.IpcGet()
	if err != nil {
		return nil
	}
	var out []PeerStatus
	var cur *PeerStatus
	var hsSec, hsNsec int64
	flush := func() {
		if cur != nil {
			if hsSec != 0 || hsNsec != 0 {
				cur.LastHandshake = time.Unix(hsSec, hsNsec)
			}
			out = append(out, *cur)
		}
		hsSec, hsNsec = 0, 0
	}
	sc := bufio.NewScanner(strings.NewReader(raw))
	for sc.Scan() {
		k, v, _ := strings.Cut(sc.Text(), "=")
		switch k {
		case "public_key":
			flush()
			cur = &PeerStatus{}
			b, _ := hex.DecodeString(v)
			copy(cur.PublicKey[:], b)
		case "endpoint":
			if cur != nil {
				cur.Endpoint = v
			}
		case "last_handshake_time_sec":
			hsSec, _ = strconv.ParseInt(v, 10, 64)
		case "last_handshake_time_nsec":
			hsNsec, _ = strconv.ParseInt(v, 10, 64)
		case "rx_bytes":
			if cur != nil {
				cur.RxBytes, _ = strconv.ParseUint(v, 10, 64)
			}
		case "tx_bytes":
			if cur != nil {
				cur.TxBytes, _ = strconv.ParseUint(v, 10, 64)
			}
		}
	}
	flush()
	return out
}

// PeerStatus reports every peer's endpoint, last handshake and traffic.
func (d *Daemon) PeerStatus() []PeerStatus { return d.peerStatus() }

// statusLoop rewrites opts.Status so `cat` over SSH shows tunnel health -
// there is no `wg show` on the unit.
func (d *Daemon) statusLoop(ctx context.Context) {
	for {
		var b strings.Builder
		fmt.Fprintf(&b, "mibwg pid %d, updated %s\n", os.Getpid(), time.Now().UTC().Format(time.RFC3339))
		for _, st := range d.peerStatus() {
			hs := "never"
			if !st.LastHandshake.IsZero() {
				hs = time.Since(st.LastHandshake).Round(time.Second).String() + " ago"
			}
			fmt.Fprintf(&b, "peer %s\n  endpoint: %s\n  latest handshake: %s\n  transfer: %d B received, %d B sent\n",
				st.PublicKey, orNone(st.Endpoint), hs, st.RxBytes, st.TxBytes)
		}
		// Written in place: rename(2) fails on the unit's /dev/shmem (/tmp).
		os.WriteFile(d.opts.Status, []byte(b.String()), 0o644)
		if !sleep(ctx, CheckInterval) {
			os.Remove(d.opts.Status)
			return
		}
	}
}

func orNone(s string) string {
	if s == "" {
		return "(none)"
	}
	return s
}

func short(k config.Key) string { return k.String()[:8] + "…" }

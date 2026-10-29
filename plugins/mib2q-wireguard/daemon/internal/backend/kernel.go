package backend

import "net/netip"

// KernelOptions configures the kernel-routed backends (tun, gif; QNX only).
type KernelOptions struct {
	Addrs  []netip.Prefix // the unit's tunnel addresses
	Routes []netip.Prefix // prefixes to route into the tunnel (AllowedIPs); nil = none
	MTU    int
	Logf   func(string, ...any)
}

// EndpointPinner is implemented by backends that route traffic through the
// kernel and must keep the WireGuard server itself reachable outside the
// tunnel.
type EndpointPinner interface {
	PinEndpoint(netip.Addr)
}

// expandRoutes turns a default route into two halves, so the uplink's own
// default stays in place (and remains what `route get default` reports).
func expandRoutes(in []netip.Prefix) []netip.Prefix {
	var out []netip.Prefix
	for _, r := range in {
		r = r.Masked()
		switch {
		case r.Bits() == 0 && r.Addr().Is4():
			out = append(out, netip.MustParsePrefix("0.0.0.0/1"), netip.MustParsePrefix("128.0.0.0/1"))
		case r.Bits() == 0 && r.Addr().Is6():
			out = append(out, netip.MustParsePrefix("::/1"), netip.MustParsePrefix("8000::/1"))
		default:
			out = append(out, r)
		}
	}
	return out
}

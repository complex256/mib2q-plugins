package backend

import (
	"context"
	"net"
	"net/netip"
)

// hostNetwork is the tunnel-side network for kernel-routed backends: the
// kernel already routes AllowedIPs into the tunnel, so it's just the host's
// own sockets.
type hostNetwork struct {
	addrs []netip.Addr
	d     net.Dialer
}

func (h *hostNetwork) Addrs() []netip.Addr { return h.addrs }

func (h *hostNetwork) DialContext(ctx context.Context, network, address string) (net.Conn, error) {
	return h.d.DialContext(ctx, network, address)
}

func (h *hostNetwork) ListenTCP(addr netip.AddrPort) (net.Listener, error) {
	return net.ListenTCP("tcp", net.TCPAddrFromAddrPort(addr))
}

func (h *hostNetwork) ListenUDP(addr netip.AddrPort) (net.PacketConn, error) {
	return net.ListenUDP("udp", net.UDPAddrFromAddrPort(addr))
}

func (h *hostNetwork) LookupHost(ctx context.Context, host string) ([]netip.Addr, error) {
	return net.DefaultResolver.LookupNetIP(ctx, "ip", host)
}

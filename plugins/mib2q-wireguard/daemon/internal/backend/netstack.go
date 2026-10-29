package backend

import (
	"context"
	"fmt"
	"net"
	"net/netip"

	"golang.zx2c4.com/wireguard/tun"
	"golang.zx2c4.com/wireguard/tun/netstack"
)

// Netstack terminates the tunnel in gVisor's userspace TCP/IP stack.
type Netstack struct {
	dev   tun.Device
	tnet  *netstack.Net
	addrs []netip.Addr
	dns   bool
}

func NewNetstack(addrs, dns []netip.Addr, mtu int) (*Netstack, error) {
	dev, tnet, err := netstack.CreateNetTUN(addrs, dns, mtu)
	if err != nil {
		return nil, fmt.Errorf("netstack: %w", err)
	}
	return &Netstack{dev: dev, tnet: tnet, addrs: addrs, dns: len(dns) > 0}, nil
}

func (n *Netstack) TUN() tun.Device { return n.dev }
func (n *Netstack) Net() Network    { return n }

func (n *Netstack) Addrs() []netip.Addr { return n.addrs }

// DialContext resolves names itself (see LookupHost) so a config without
// tunnel DNS still dials hostnames, via the head unit's own resolver.
func (n *Netstack) DialContext(ctx context.Context, network, address string) (net.Conn, error) {
	host, port, err := net.SplitHostPort(address)
	if err != nil {
		return nil, err
	}
	if _, err := netip.ParseAddr(host); err != nil {
		ips, err := n.LookupHost(ctx, host)
		if err != nil {
			return nil, err
		}
		address = net.JoinHostPort(ips[0].String(), port)
	}
	return n.tnet.DialContext(ctx, network, address)
}

func (n *Netstack) ListenTCP(addr netip.AddrPort) (net.Listener, error) {
	return n.tnet.ListenTCPAddrPort(addr)
}

func (n *Netstack) ListenUDP(addr netip.AddrPort) (net.PacketConn, error) {
	return n.tnet.ListenUDPAddrPort(addr)
}

// LookupHost uses the tunnel's DNS servers when [Interface] DNS is set, else
// the host resolver. Results are ordered to prefer the families the tunnel has.
func (n *Netstack) LookupHost(ctx context.Context, host string) ([]netip.Addr, error) {
	var names []string
	var err error
	if n.dns {
		names, err = n.tnet.LookupContextHost(ctx, host)
	} else {
		names, err = net.DefaultResolver.LookupHost(ctx, host)
	}
	if err != nil {
		return nil, err
	}
	var has4, has6 bool
	for _, a := range n.addrs {
		has4 = has4 || a.Is4()
		has6 = has6 || a.Is6()
	}
	var preferred, other []netip.Addr
	for _, s := range names {
		a, err := netip.ParseAddr(s)
		if err != nil {
			continue
		}
		a = a.Unmap()
		if (a.Is4() && has4) || (a.Is6() && has6) {
			preferred = append(preferred, a)
		} else {
			other = append(other, a)
		}
	}
	out := append(preferred, other...)
	if len(out) == 0 {
		return nil, fmt.Errorf("lookup %s: no addresses", host)
	}
	return out, nil
}

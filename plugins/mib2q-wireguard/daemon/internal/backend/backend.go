// Package backend abstracts where tunnel packets enter and leave the head
// unit. QNX 6.5's io-pkt has no tun/tap cloner, so there are two stand-ins:
//
//   - Netstack: a userspace TCP/IP stack (gVisor) inside mibwg; the unit
//     reaches it only through [Forward] and [Proxy].
//   - Gif: gif(4) IP-in-IP over lo0 plus raw sockets act as a tun, so the
//     kernel routes AllowedIPs into the tunnel for every program on the unit.
package backend

import (
	"context"
	"net"
	"net/netip"

	"golang.zx2c4.com/wireguard/tun"
)

// Network is the socket API on the tunnel side of the device.
type Network interface {
	DialContext(ctx context.Context, network, address string) (net.Conn, error)
	// Addrs are the unit's own addresses inside the tunnel.
	Addrs() []netip.Addr
	ListenTCP(addr netip.AddrPort) (net.Listener, error)
	ListenUDP(addr netip.AddrPort) (net.PacketConn, error)
	// LookupHost resolves names for dialing through the tunnel.
	LookupHost(ctx context.Context, host string) ([]netip.Addr, error)
}

type Backend interface {
	// TUN is the packet device handed to the WireGuard device.
	TUN() tun.Device
	Net() Network
}

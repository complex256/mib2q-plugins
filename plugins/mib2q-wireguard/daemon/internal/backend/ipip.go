package backend

import (
	"encoding/binary"
	"net/netip"
)

// IP-in-IPv4 framing between mibwg and the gif interface. gif0's tunnel runs
// TunLocal -> TunPeer, both lo0 aliases: packets the kernel routes into gif0
// come out encapsulated local->peer (gif never claims that direction, so a
// raw socket gets them), and packets mibwg wraps peer->local are claimed by
// gif0 and delivered as if they arrived on it.
//
// Not 127/8: gif's input validation drops encapsulated packets with a martian
// outer source. 10.255.255.0/30 is a host route on lo0, so it wins over any
// AllowedIPs prefix that happens to contain it.
var (
	TunLocal = netip.AddrFrom4([4]byte{10, 255, 255, 1})
	TunPeer  = netip.AddrFrom4([4]byte{10, 255, 255, 2})
)

const (
	protoIPIP = 4  // IPv4 in IPv4
	protoIPv6 = 41 // IPv6 in IPv4
)

// decap validates an outer packet read from the raw socket and returns the
// inner packet, or nil if it isn't gif traffic heading out of the tunnel.
// The outer ip_len is not trusted: 4.4BSD raw sockets hand it over in host
// order with the header length already subtracted; the read size is the truth.
func decap(p []byte) []byte {
	if len(p) < 20 || p[0]>>4 != 4 {
		return nil
	}
	ihl := int(p[0]&0x0f) * 4
	if ihl < 20 || len(p) < ihl+1 {
		return nil
	}
	if netip.AddrFrom4([4]byte(p[12:16])) != TunLocal || netip.AddrFrom4([4]byte(p[16:20])) != TunPeer {
		return nil
	}
	inner := p[ihl:]
	switch {
	case p[9] == protoIPIP && inner[0]>>4 == 4 && len(inner) >= 20:
	case p[9] == protoIPv6 && inner[0]>>4 == 6 && len(inner) >= 40:
	default:
		return nil
	}
	return inner
}

// encap wraps an inner IPv4 or IPv6 packet for delivery into gif0
// (TunPeer -> TunLocal). ip_len/ip_off go in network order: io-pkt's raw
// output takes IP_HDRINCL headers as-is (verified under QEMU).
func encap(inner []byte) []byte {
	proto := byte(protoIPIP)
	if len(inner) > 0 && inner[0]>>4 == 6 {
		proto = protoIPv6
	}
	p := make([]byte, 20+len(inner))
	p[0] = 0x45
	binary.BigEndian.PutUint16(p[2:], uint16(len(p)))
	p[8] = 64
	p[9] = proto
	src, dst := TunPeer.As4(), TunLocal.As4()
	copy(p[12:], src[:])
	copy(p[16:], dst[:])
	binary.BigEndian.PutUint16(p[10:], checksum(p[:20]))
	copy(p[20:], inner)
	return p
}

// innerDst is the destination address of an inner IPv4/IPv6 packet.
func innerDst(inner []byte) netip.Addr {
	switch inner[0] >> 4 {
	case 4:
		return netip.AddrFrom4([4]byte(inner[16:20]))
	case 6:
		return netip.AddrFrom16([16]byte(inner[24:40]))
	}
	return netip.Addr{}
}

func checksum(b []byte) uint16 {
	var s uint32
	for i := 0; i+1 < len(b); i += 2 {
		s += uint32(b[i])<<8 | uint32(b[i+1])
	}
	for s > 0xffff {
		s = s>>16 + s&0xffff
	}
	return ^uint16(s)
}

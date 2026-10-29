package backend

import (
	"encoding/binary"
	"net/netip"
	"testing"
)

func udp4Packet(src, dst string) []byte {
	p := make([]byte, 28)
	p[0] = 0x45
	binary.BigEndian.PutUint16(p[2:], 28)
	p[9] = 17
	s, d := netip.MustParseAddr(src).As4(), netip.MustParseAddr(dst).As4()
	copy(p[12:], s[:])
	copy(p[16:], d[:])
	return p
}

// outbound builds what io-pkt hands the raw socket for a packet leaving via
// gif: outer local->peer, ip_len mangled the 4.4BSD way (host order, header
// length subtracted).
func outbound(inner []byte, proto byte) []byte {
	p := make([]byte, 20+len(inner))
	p[0] = 0x45
	binary.LittleEndian.PutUint16(p[2:], uint16(len(inner)))
	p[9] = proto
	l, r := TunLocal.As4(), TunPeer.As4()
	copy(p[12:], l[:])
	copy(p[16:], r[:])
	copy(p[20:], inner)
	return p
}

func TestDecap(t *testing.T) {
	inner := udp4Packet("10.6.0.2", "10.6.0.1")
	got := decap(outbound(inner, protoIPIP))
	if string(got) != string(inner) {
		t.Fatalf("decap = %x, want %x", got, inner)
	}
	if innerDst(got) != netip.MustParseAddr("10.6.0.1") {
		t.Errorf("innerDst = %v", innerDst(got))
	}

	v6 := make([]byte, 40)
	v6[0] = 0x60
	if decap(outbound(v6, protoIPv6)) == nil {
		t.Error("IPv6-in-IPv4 rejected")
	}

	for name, p := range map[string][]byte{
		"proto/version mismatch": outbound(v6, protoIPIP),
		"not gif traffic":        udp4Packet("10.255.255.1", "10.255.255.2"),
		"our own inbound":        encap(inner), // peer->local: gif claims these
		"short":                  {0x45, 0},
	} {
		if decap(p) != nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestEncap(t *testing.T) {
	inner := udp4Packet("10.6.0.1", "10.6.0.2")
	p := encap(inner)
	if p[9] != protoIPIP || binary.BigEndian.Uint16(p[2:]) != uint16(len(p)) {
		t.Fatalf("outer header %x", p[:20])
	}
	if src, dst := netip.AddrFrom4([4]byte(p[12:16])), netip.AddrFrom4([4]byte(p[16:20])); src != TunPeer || dst != TunLocal {
		t.Errorf("outer %v -> %v, want peer -> local", src, dst)
	}
	if checksum(p[:20]) != 0 {
		t.Error("bad outer header checksum")
	}
	v6 := make([]byte, 40)
	v6[0] = 0x60
	if encap(v6)[9] != protoIPv6 {
		t.Error("IPv6 inner not marked proto 41")
	}
}

func TestExpandRoutes(t *testing.T) {
	got := expandRoutes([]netip.Prefix{
		netip.MustParsePrefix("0.0.0.0/0"),
		netip.MustParsePrefix("::/0"),
		netip.MustParsePrefix("10.6.0.9/24"),
	})
	want := []string{"0.0.0.0/1", "128.0.0.0/1", "::/1", "8000::/1", "10.6.0.0/24"}
	if len(got) != len(want) {
		t.Fatalf("got %v", got)
	}
	for i := range want {
		if got[i].String() != want[i] {
			t.Errorf("route %d = %v, want %s", i, got[i], want[i])
		}
	}
}

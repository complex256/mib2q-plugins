// gifprobe answers the phase-2 feasibility questions (docs/gif-backend.md)
// on a real io-pkt: can a gif(4) tunnel over loopback, plus raw IPPROTO_IPIP
// sockets, stand in for a tun device?
//
// Expects this setup (tests/qemu-net.sh QEMU_PRE, or by hand on a unit):
//
//	ifconfig lo0 10.255.255.1 netmask 255.255.255.255 alias
//	ifconfig lo0 10.255.255.2 netmask 255.255.255.255 alias
//	ifconfig gif0 create
//	ifconfig gif0 tunnel 10.255.255.1 10.255.255.2
//	ifconfig gif0 10.99.0.1 10.99.0.2 up
//
// Checks:
//
//	OUT: a UDP datagram routed to gif0 reaches the raw socket, encapsulated
//	     local -> peer (gif doesn't claim its own reversed packet)
//	IN:  a hand-built IPIP packet peer -> local written with
//	     IP_HDRINCL is decapsulated by gif0 and delivered to a normal socket
package main

import (
	"encoding/binary"
	"fmt"
	"net"
	"os"
	"os/exec"
	"syscall"
	"time"
)

const (
	ipprotoIPIP = 4 // <netinet/in.h>
	ipHdrincl   = 2
)

// Outer (tunnel) addresses, both aliased on lo0. Not 127/8: gif's input
// validation drops encapsulated packets with a martian outer source.
var tunLocal, tunPeer = [4]byte{10, 255, 255, 1}, [4]byte{10, 255, 255, 2}

func main() {
	ok := true
	if len(os.Args) > 1 && os.Args[1] == "-setup" {
		ifconfig := "/sbin/ifconfig"
		if _, err := os.Stat(ifconfig); err != nil {
			ifconfig = "/proc/boot/ifconfig" // QEMU boot image
		}
		for _, args := range [][]string{
			{"lo0", "10.255.255.1", "netmask", "255.255.255.255", "alias"},
			{"lo0", "10.255.255.2", "netmask", "255.255.255.255", "alias"},
			{"gif0", "create"},
			{"gif0", "tunnel", "10.255.255.1", "10.255.255.2"},
			{"gif0", "10.99.0.1", "10.99.0.2", "up"},
			{"gif0"},
		} {
			out, err := exec.Command(ifconfig, args...).CombinedOutput()
			fmt.Printf("     ifconfig %v: err=%v\n%s", args, err, out)
		}
	}
	fd, err := syscall.Socket(syscall.AF_INET, syscall.SOCK_RAW, ipprotoIPIP)
	if err != nil {
		fmt.Println("FAIL raw socket(AF_INET, SOCK_RAW, IPIP):", err)
		os.Exit(1)
	}
	if err := syscall.SetsockoptInt(fd, syscall.IPPROTO_IP, ipHdrincl, 1); err != nil {
		fmt.Println("FAIL setsockopt IP_HDRINCL:", err)
		os.Exit(1)
	}
	fmt.Println("ok   raw IPIP socket with IP_HDRINCL")

	seen := make(chan []byte, 16)
	go func() {
		buf := make([]byte, 65535)
		for {
			n, _, err := syscall.Recvfrom(fd, buf, 0)
			if err != nil {
				fmt.Println("     raw read error:", err)
				return
			}
			p := append([]byte(nil), buf[:n]...)
			fmt.Printf("     raw read %d bytes: %s\n", n, describe(p))
			seen <- p
		}
	}()

	// OUT: kernel -> raw socket.
	c, err := net.Dial("udp4", "10.99.0.2:7000")
	if err != nil {
		fmt.Println("FAIL dial via gif0:", err)
		os.Exit(1)
	}
	c.Write([]byte("hello-out"))
	if p := waitFor(seen, 3*time.Second, func(p []byte) bool {
		return containsInner(p, "hello-out")
	}); p != nil {
		fmt.Println("PASS OUT: datagram routed to gif0 arrived on the raw socket, encapsulated")
	} else {
		fmt.Println("FAIL OUT: nothing encapsulated reached the raw socket")
		ok = false
	}

	// IN: raw socket -> gif0 -> normal socket.
	ln, err := net.ListenPacket("udp4", "10.99.0.1:6000")
	if err != nil {
		fmt.Println("FAIL listen on inner address:", err)
		os.Exit(1)
	}
	got := make(chan string, 4)
	go func() {
		buf := make([]byte, 2048)
		for {
			n, from, err := ln.ReadFrom(buf)
			if err != nil {
				return
			}
			got <- fmt.Sprintf("%q from %s", buf[:n], from)
		}
	}()
	inner := udp4([4]byte{10, 99, 0, 2}, [4]byte{10, 99, 0, 1}, 5000, 6000, []byte("hello-in"))
	delivered := false
	for _, order := range []string{"network", "host"} {
		// Inbound = as if from the tunnel peer: outer src = peer, dst = local.
		pkt := ipv4(tunPeer, tunLocal, ipprotoIPIP, inner, order == "host")
		to := &syscall.SockaddrInet4{Addr: tunLocal}
		if err := syscall.Sendto(fd, pkt, 0, to); err != nil {
			fmt.Printf("     inject (%s-order ip_len): sendto: %v\n", order, err)
			continue
		}
		select {
		case s := <-got:
			fmt.Printf("PASS IN: injected packet delivered by gif0 (%s-order ip_len/ip_off): %s\n", order, s)
			delivered = true
		case <-time.After(3 * time.Second):
			fmt.Printf("     inject (%s-order ip_len): nothing delivered\n", order)
		}
		if delivered {
			break
		}
	}
	if !delivered {
		fmt.Println("FAIL IN: gif0 never delivered an injected packet")
		ok = false
	}
	if !ok {
		os.Exit(1)
	}
	fmt.Println("gif backend: FEASIBLE")
}

func waitFor(ch <-chan []byte, d time.Duration, match func([]byte) bool) []byte {
	deadline := time.After(d)
	for {
		select {
		case p := <-ch:
			if match(p) {
				return p
			}
		case <-deadline:
			return nil
		}
	}
}

func containsInner(p []byte, s string) bool {
	for i := 0; i+len(s) <= len(p); i++ {
		if string(p[i:i+len(s)]) == s {
			return true
		}
	}
	return false
}

func describe(p []byte) string {
	if len(p) < 20 {
		return "short"
	}
	ihl := int(p[0]&0x0f) * 4
	s := fmt.Sprintf("outer %v -> %v proto %d ihl %d len-field %d",
		net.IP(p[12:16]), net.IP(p[16:20]), p[9], ihl, binary.BigEndian.Uint16(p[2:4]))
	if p[9] == ipprotoIPIP && len(p) >= ihl+20 {
		q := p[ihl:]
		s += fmt.Sprintf(" | inner %v -> %v proto %d", net.IP(q[12:16]), net.IP(q[16:20]), q[9])
	}
	return s
}

// ipv4 builds a 20-byte-header IPv4 packet. hostOrder writes ip_len/ip_off
// in host byte order, which 4.4BSD-era raw sockets expected with IP_HDRINCL.
func ipv4(src, dst [4]byte, proto byte, payload []byte, hostOrder bool) []byte {
	p := make([]byte, 20+len(payload))
	p[0] = 0x45
	total := uint16(len(p))
	if hostOrder {
		binary.LittleEndian.PutUint16(p[2:], total) // ARM is little-endian
	} else {
		binary.BigEndian.PutUint16(p[2:], total)
	}
	p[8] = 64
	p[9] = proto
	copy(p[12:], src[:])
	copy(p[16:], dst[:])
	binary.BigEndian.PutUint16(p[10:], csum(p[:20]))
	copy(p[20:], payload)
	return p
}

func udp4(src, dst [4]byte, sport, dport uint16, data []byte) []byte {
	u := make([]byte, 8+len(data))
	binary.BigEndian.PutUint16(u[0:], sport)
	binary.BigEndian.PutUint16(u[2:], dport)
	binary.BigEndian.PutUint16(u[4:], uint16(len(u)))
	copy(u[8:], data) // checksum 0 = none, legal for UDP over IPv4
	return ipv4(src, dst, syscall.IPPROTO_UDP, u, false)
}

func csum(b []byte) uint16 {
	var s uint32
	for i := 0; i+1 < len(b); i += 2 {
		s += uint32(b[i])<<8 | uint32(b[i+1])
	}
	for s > 0xffff {
		s = s>>16 + s&0xffff
	}
	return ^uint16(s)
}

// tunprobe checks whether a unit's io-pkt tun(4) works as a plain packet
// device from Go: create tun9, send a datagram out through it (read from
// /dev/tun9), write a datagram in (delivered to a normal socket), destroy.
// The SDK's io-pkt has no tun cloner; some unit firmware does
// (`ifconfig -C` lists it), so this only runs on a unit.
package main

import (
	"encoding/binary"
	"fmt"
	"net"
	"os"
	"os/exec"
	"strings"
	"time"
)

const dev = "tun9"

func ifconfig(args ...string) error {
	bin := "ifconfig"
	for _, p := range []string{"/usr/sbin/ifconfig", "/sbin/ifconfig", "/proc/boot/ifconfig"} {
		if _, err := os.Stat(p); err == nil {
			bin = p
			break
		}
	}
	out, err := exec.Command(bin, args...).CombinedOutput()
	fmt.Printf("     ifconfig %s: err=%v %s\n", strings.Join(args, " "), err, strings.TrimSpace(string(out)))
	return err
}

func main() {
	ok := run()
	ifconfig(dev, "destroy")
	if !ok {
		os.Exit(1)
	}
	fmt.Println("tun backend: FEASIBLE")
}

func run() bool {
	if ifconfig(dev, "create") != nil || ifconfig(dev, "10.99.0.1", "10.99.0.2", "up") != nil {
		return false
	}
	f, err := os.OpenFile("/dev/"+dev, os.O_RDWR, 0)
	if err != nil {
		fmt.Println("FAIL open /dev/"+dev+":", err)
		return false
	}
	defer f.Close()
	fmt.Println("ok   opened /dev/" + dev)

	reads := make(chan []byte, 8)
	go func() {
		buf := make([]byte, 65535)
		for {
			n, err := f.Read(buf)
			if err != nil {
				fmt.Println("     read error:", err)
				return
			}
			reads <- append([]byte(nil), buf[:n]...)
		}
	}()

	ok := true
	c, err := net.Dial("udp4", "10.99.0.2:7000")
	if err != nil {
		fmt.Println("FAIL dial via tun:", err)
		return false
	}
	c.Write([]byte("hello-out"))
	select {
	case p := <-reads:
		hdr := "none (raw IP)"
		if len(p) > 0 && p[0]>>4 != 4 {
			hdr = fmt.Sprintf("% x", p[:min(4, len(p))])
		}
		fmt.Printf("PASS OUT: read %d bytes from /dev/%s, leading header: %s, contains payload: %v\n",
			len(p), dev, hdr, strings.Contains(string(p), "hello-out"))
	case <-time.After(3 * time.Second):
		fmt.Println("FAIL OUT: nothing read from the tun device")
		ok = false
	}

	ln, err := net.ListenPacket("udp4", "10.99.0.1:6000")
	if err != nil {
		fmt.Println("FAIL listen:", err)
		return false
	}
	defer ln.Close()
	got := make(chan string, 2)
	go func() {
		buf := make([]byte, 2048)
		n, from, err := ln.ReadFrom(buf)
		if err == nil {
			got <- fmt.Sprintf("%q from %s", buf[:n], from)
		}
	}()
	pkt := udp4([4]byte{10, 99, 0, 2}, [4]byte{10, 99, 0, 1}, 5000, 6000, []byte("hello-in"))
	for _, framing := range []string{"raw", "af-header"} {
		out := pkt
		if framing == "af-header" {
			out = append([]byte{0, 0, 0, 2}, pkt...) // AF_INET, TUNSIFHEAD-style
		}
		if _, err := f.Write(out); err != nil {
			fmt.Printf("     write (%s): %v\n", framing, err)
			continue
		}
		select {
		case s := <-got:
			fmt.Printf("PASS IN: written packet delivered (%s framing): %s\n", framing, s)
			return ok
		case <-time.After(3 * time.Second):
			fmt.Printf("     write (%s): nothing delivered\n", framing)
		}
	}
	fmt.Println("FAIL IN: no written packet was delivered")
	return false
}

func udp4(src, dst [4]byte, sport, dport uint16, data []byte) []byte {
	p := make([]byte, 28+len(data))
	p[0] = 0x45
	binary.BigEndian.PutUint16(p[2:], uint16(len(p)))
	p[8] = 64
	p[9] = 17
	copy(p[12:], src[:])
	copy(p[16:], dst[:])
	var s uint32
	for i := 0; i < 20; i += 2 {
		s += uint32(p[i])<<8 | uint32(p[i+1])
	}
	for s > 0xffff {
		s = s>>16 + s&0xffff
	}
	binary.BigEndian.PutUint16(p[10:], ^uint16(s))
	binary.BigEndian.PutUint16(p[20:], sport)
	binary.BigEndian.PutUint16(p[22:], dport)
	binary.BigEndian.PutUint16(p[24:], uint16(8+len(data)))
	copy(p[28:], data)
	return p
}

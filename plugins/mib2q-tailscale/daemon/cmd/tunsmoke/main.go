// tunsmoke proves wireguard-go's QNX CreateTUN works on a unit: create tun9,
// address it, push a packet out, read it back off the device, then clean up.
// QEMU can't run this - the SDK's io-pkt has no tun cloner.
package main

import (
	"fmt"
	"net"
	"os"
	"os/exec"
	"strings"
	"time"

	"golang.zx2c4.com/wireguard/tun"
)

const iface = "tun9"

func main() {
	dev, err := tun.CreateTUN(iface, 1420)
	if err != nil {
		fmt.Println("FAIL CreateTUN:", err)
		os.Exit(1)
	}
	defer dev.Close()
	fmt.Println("ok   CreateTUN", iface)

	if out, err := exec.Command("/usr/sbin/ifconfig", iface, "10.99.0.1", "10.99.0.2", "up").CombinedOutput(); err != nil {
		fmt.Printf("FAIL ifconfig: %v: %s\n", err, out)
		os.Exit(1)
	}

	got := make(chan string, 4)
	go func() {
		bufs := [][]byte{make([]byte, 2048)}
		sizes := make([]int, 1)
		for {
			n, err := dev.Read(bufs, sizes, 0)
			if err != nil {
				return
			}
			if n > 0 {
				got <- string(bufs[0][:sizes[0]])
			}
		}
	}()

	c, err := net.Dial("udp4", "10.99.0.2:7000")
	if err != nil {
		fmt.Println("FAIL dial:", err)
		os.Exit(1)
	}
	c.Write([]byte("hello-tun"))
	select {
	case p := <-got:
		if !strings.Contains(p, "hello-tun") {
			fmt.Println("FAIL packet did not contain the payload")
			os.Exit(1)
		}
		fmt.Printf("PASS read %d bytes off %s containing the payload\n", len(p), iface)
	case <-time.After(5 * time.Second):
		fmt.Println("FAIL nothing read from the device")
		os.Exit(1)
	}
	fmt.Println("wireguard-go QNX tun: OK")
}

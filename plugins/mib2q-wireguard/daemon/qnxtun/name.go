// Package qnxtun is io-pkt's tun(4) as a packet device, for QNX 6.5 units
// whose io-pkt has the tun cloner (`ifconfig -C` lists it; the SDK's does
// not). Shared by mibwg's tun backend and the patched wireguard-go.
package qnxtun

import "fmt"

// DevPath is the character device for an interface name.
func DevPath(name string) string { return "/dev/" + name }

// ValidName accepts "tun" followed by digits - the only names io-pkt's tun
// cloner creates, and the only ones safe to paste into a device path.
func ValidName(name string) error {
	if len(name) < 4 || name[:3] != "tun" {
		return fmt.Errorf("tun name %q: want tunN", name)
	}
	for _, c := range name[3:] {
		if c < '0' || c > '9' {
			return fmt.Errorf("tun name %q: want tunN", name)
		}
	}
	return nil
}

// Package qnxplat holds the QNX platform logic tailscaled needs: routing and
// DNS. The logic lives here, in a normal testable module; the files copied
// into the Tailscale tree are thin adapters that call it.
package qnxplat

import "os"

// toolDirs covers both the unit (ifconfig in /usr/sbin, route in
// /armle/sbin, neither on the default PATH) and the QEMU image
// (everything in /proc/boot).
var toolDirs = []string{"/sbin", "/usr/sbin", "/bin", "/usr/bin", "/armle/sbin", "/armle/bin", "/armle/usr/sbin", "/armle/usr/bin", "/proc/boot"}

// Tool returns the full path of a system tool, or the bare name if not found.
func Tool(name string) string {
	for _, d := range toolDirs {
		p := d + "/" + name
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	return name
}

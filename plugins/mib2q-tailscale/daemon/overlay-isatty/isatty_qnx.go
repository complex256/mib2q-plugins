//go:build qnx

// This mirrors isatty_others.go (built for appengine/js/nacl/tinygo/wasm):
// tailscale.com's only caller (util/prompt) uses IsTerminal solely to decide
// whether to block on an interactive y/n prompt or fall back to a default -
// tailscaled on the unit never has a controlling terminal attached, so
// "false" is the correct answer here, not a placeholder standing in for one.
package isatty

// IsTerminal returns true if the file descriptor is a terminal, which is
// always false on the unit: tailscaled has no controlling terminal.
func IsTerminal(fd uintptr) bool {
	return false
}

// IsCygwinTerminal returns true if the file descriptor is a cygwin or msys2
// terminal. Always false here too - qnx has no such thing.
func IsCygwinTerminal(fd uintptr) bool {
	return false
}

package qnxtun

import "testing"

func TestDevPath(t *testing.T) {
	if got := DevPath("tun9"); got != "/dev/tun9" {
		t.Errorf("DevPath = %q, want /dev/tun9", got)
	}
}

func TestValidName(t *testing.T) {
	for _, ok := range []string{"tun0", "tun9", "tun42"} {
		if err := ValidName(ok); err != nil {
			t.Errorf("ValidName(%q) = %v, want nil", ok, err)
		}
	}
	for _, bad := range []string{"", "tun", "gif9", "tun9/../etc", "tun9 x", "TUN9"} {
		if err := ValidName(bad); err == nil {
			t.Errorf("ValidName(%q) = nil, want error", bad)
		}
	}
}

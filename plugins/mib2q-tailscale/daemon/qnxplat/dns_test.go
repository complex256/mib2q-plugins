package qnxplat

import (
	"net/netip"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// DnsmasqConf must stay empty even for the input that used to produce a
// server= line, because dnsmasq reads its conf-file only at startup: a line
// written at SetDNS time would sit unread until the next boot. The input
// below is the fully-populated case, so this fails the moment someone
// reintroduces rendering here rather than at boot, where it belongs.
//
// This is a guard on a deliberate no-op, not a behavioral test - it cannot
// fail for any implementation that returns "". It is here so the intent
// survives someone reading only the code.
func TestDnsmasqConfNeverEmitsServerLine(t *testing.T) {
	got := DnsmasqConf(DNSConfig{
		Nameservers:  []netip.Addr{netip.MustParseAddr("100.100.100.100")},
		MatchDomains: []string{"tail1234.ts.net"},
	})
	if got != "" {
		t.Errorf("DnsmasqConf = %q, want \"\": split DNS has to be written before dnsmasq starts, not from SetDNS", got)
	}
}

func TestHostsBlockIsDelimited(t *testing.T) {
	got := HostsBlock(DNSConfig{Hosts: map[string][]netip.Addr{
		"stash.tail1234.ts.net": {netip.MustParseAddr("100.99.1.2")},
	}})
	if !strings.Contains(got, "100.99.1.2\tstash.tail1234.ts.net") {
		t.Errorf("HostsBlock = %q, missing the entry", got)
	}
	if !strings.Contains(got, hostsBegin) || !strings.Contains(got, hostsEnd) {
		t.Errorf("HostsBlock = %q, want begin/end markers so it can be replaced later", got)
	}
}

func TestWriteDnsmasqIncludeReportsChange(t *testing.T) {
	p := filepath.Join(t.TempDir(), "sdis.conf")
	if changed, err := WriteDnsmasqInclude(p, "server=/a/1.2.3.4\n"); err != nil || !changed {
		t.Fatalf("first write: changed=%v err=%v, want true/nil", changed, err)
	}
	if changed, err := WriteDnsmasqInclude(p, "server=/a/1.2.3.4\n"); err != nil || changed {
		t.Fatalf("identical write: changed=%v err=%v, want false/nil", changed, err)
	}
	b, _ := os.ReadFile(p)
	if string(b) != "server=/a/1.2.3.4\n" {
		t.Errorf("file = %q", b)
	}
}

// Writing "" to a path that does not exist yet must not be reported as a
// change, and must not create the file: a missing include file already
// behaves as if it were empty, so this would otherwise be a no-op that
// still trips a dnsmasq reload on every startup.
func TestWriteDnsmasqIncludeEmptyToMissingFileIsNotAChange(t *testing.T) {
	p := filepath.Join(t.TempDir(), "sdis.conf")
	changed, err := WriteDnsmasqInclude(p, "")
	if err != nil || changed {
		t.Fatalf("WriteDnsmasqInclude(missing, \"\") = changed=%v err=%v, want false/nil", changed, err)
	}
	if _, statErr := os.Stat(p); !os.IsNotExist(statErr) {
		t.Errorf("file was created at %q, want it left absent", p)
	}
}

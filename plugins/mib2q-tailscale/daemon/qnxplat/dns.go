package qnxplat

import (
	"fmt"
	"net/netip"
	"os"
	"os/exec"
	"sort"
	"strings"
)

// The unit runs dnsmasq natively and its /etc/dnsmasq.conf ends with
// `conf-file=/tmp/dnsmasq.sdis.conf`, an include file that is empty and
// recreated on every boot - a ready-made hook for split DNS.
const DnsmasqInclude = "/tmp/dnsmasq.sdis.conf"

const (
	hostsBegin = "# BEGIN mib2q-tailscale"
	hostsEnd   = "# END mib2q-tailscale"
)

// DNSConfig is the part of tailscale's dns.OSConfig this platform acts on.
type DNSConfig struct {
	Nameservers  []netip.Addr
	MatchDomains []string
	Hosts        map[string][]netip.Addr
}

// DnsmasqConf used to render one split-DNS "server=/domain/100.100.100.100"
// line per matched domain, pointing dnsmasq at Tailscale's resolver
// (quad-100). It always returns "" now, for a reason that has nothing to do
// with whether quad-100 answers - it does, netstack is in this build.
//
// The obstacle is when dnsmasq reads its config. It parses conf-file once,
// at startup, and does not re-read it on SIGHUP, so a line written here at
// runtime would sit unread until the next boot. Emitting it anyway would be
// harmless-but-useless at best; the honest thing is to emit nothing and let
// the caller log why.
//
// There is a way in, found by reading the unit's own startup.sh: it touches
// the empty /tmp/dnsmasq.sdis.conf and starts dnsmasq several hundred lines
// later, both well after our autostart hook. Writing the include at boot,
// before dnsmasq starts, gets split DNS with no reload and no restart. That
// is a boot-time concern, not a SetDNS-time one, which is why this function
// stays empty rather than growing a special case.
func DnsmasqConf(cfg DNSConfig) string {
	return ""
}

// HostsBlock renders peer names for /etc/hosts, between markers so an old
// block can be replaced. dnsmasq re-reads /etc/hosts on SIGHUP, which makes
// this the fallback when the include file cannot be picked up.
func HostsBlock(cfg DNSConfig) string {
	if len(cfg.Hosts) == 0 {
		return ""
	}
	names := make([]string, 0, len(cfg.Hosts))
	for n := range cfg.Hosts {
		names = append(names, n)
	}
	sort.Strings(names)
	var b strings.Builder
	b.WriteString(hostsBegin + "\n")
	for _, n := range names {
		for _, a := range cfg.Hosts[n] {
			if a.Is4() {
				fmt.Fprintf(&b, "%s\t%s\n", a, strings.TrimSuffix(n, "."))
			}
		}
	}
	b.WriteString(hostsEnd + "\n")
	return b.String()
}

// WriteDnsmasqInclude writes contents if they differ, reporting whether it
// changed anything (a reload is only worth it when they did).
func WriteDnsmasqInclude(path, contents string) (bool, error) {
	old, err := os.ReadFile(path)
	if err == nil && string(old) == contents {
		return false, nil
	}
	if err != nil && contents == "" {
		// The file is missing or unreadable, and there is nothing to
		// write: creating an empty file here would report changed=true
		// for a no-op and trigger a pointless dnsmasq reload.
		return false, nil
	}
	if err := os.WriteFile(path, []byte(contents), 0o644); err != nil {
		return false, err
	}
	return true, nil
}

// ReloadDnsmasq makes dnsmasq pick up a changed include file.
//
// SIGHUP re-reads /etc/hosts and the lease files but NOT the config, so the
// include only takes effect on restart. Restarting is only safe if something
// respawns dnsmasq - the unit also serves DHCP and DNS for its Wi-Fi hotspot
// from it. Until that is confirmed on a unit, this only sends SIGHUP and
// reports what it did.
func ReloadDnsmasq(logf func(string, ...any)) error {
	out, err := exec.Command(Tool("slay"), "-s", "SIGHUP", "-f", "dnsmasq").CombinedOutput()
	if err != nil {
		return fmt.Errorf("slay -s SIGHUP -f dnsmasq: %v: %s", err, strings.TrimSpace(string(out)))
	}
	logf("qnx dns: SIGHUP sent to dnsmasq (re-reads /etc/hosts; config needs a restart)")
	return nil
}

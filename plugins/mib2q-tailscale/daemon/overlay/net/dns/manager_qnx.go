//go:build qnx

package dns

import (
	"sync"

	"github.com/complex256/mib2q-tailscale/qnxplat"

	"tailscale.com/control/controlknobs"
	"tailscale.com/health"
	"tailscale.com/types/logger"
	"tailscale.com/util/eventbus"
	"tailscale.com/util/syspolicy/policyclient"
)

// NewOSConfigurator drives the unit's own dnsmasq for MagicDNS.
func NewOSConfigurator(logf logger.Logf, _ *health.Tracker, _ *eventbus.Bus, _ policyclient.Client, _ *controlknobs.Knobs, _ string) (OSConfigurator, error) {
	return &qnxManager{logf: logf}, nil
}

type qnxManager struct {
	logf     logger.Logf
	warnOnce sync.Once
}

// SetDNS cannot configure the unit's resolver at runtime, and the reason is
// dnsmasq, not Tailscale. tailscaled does answer 100.100.100.100 in this
// build - netstack is compiled in - and `tailscale dns query <name>` resolves
// correctly on the unit. But the only resolver the rest of the system talks
// to is dnsmasq, which reads its conf-file exactly once, at startup, and
// does not re-read it on SIGHUP. Rewriting the include from here would
// change nothing until the next boot, and restarting dnsmasq would drop DHCP
// and DNS for the car's Wi-Fi hotspot while someone is driving.
//
// So this is a deliberate no-op that says so once. Peers are reachable by
// IP; see the README's "Known gaps" for the boot-time route that does work.
func (m *qnxManager) SetDNS(cfg OSConfig) error {
	m.warnOnce.Do(func() {
		m.logf("qnx dns: not configuring the OS resolver: dnsmasq only reads its config at startup; peers are reachable by IP")
	})
	return nil
}

// SupportsSplitDNS reports false: nothing here can install a split-DNS route
// that takes effect, for the dnsmasq reason above. It is not a statement
// about whether a resolver exists to point at - one does.
func (m *qnxManager) SupportsSplitDNS() bool { return false }

func (m *qnxManager) GetBaseConfig() (OSConfig, error) {
	return OSConfig{}, ErrGetBaseConfigNotSupported
}

// Close clears the include file. WriteDnsmasqInclude reports whether it
// actually changed anything, and that answer is the whole point of it: when
// there is nothing to clear, this must not SIGHUP the process serving DHCP
// and DNS to the car's hotspot just to have it re-read a file we didn't
// touch.
func (m *qnxManager) Close() error {
	changed, err := qnxplat.WriteDnsmasqInclude(qnxplat.DnsmasqInclude, "")
	if err != nil {
		return err
	}
	if !changed {
		return nil
	}
	return qnxplat.ReloadDnsmasq(m.logf)
}

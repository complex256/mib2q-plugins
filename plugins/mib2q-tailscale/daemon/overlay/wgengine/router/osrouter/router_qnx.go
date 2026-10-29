//go:build qnx

package osrouter

import (
	"github.com/complex256/mib2q-tailscale/qnxplat"

	"tailscale.com/wgengine/router"
)

func init() {
	router.HookNewUserspaceRouter.Set(newQNXRouter)
}

func newQNXRouter(opts router.NewOpts) (router.Router, error) {
	name, err := opts.Tun.Name()
	if err != nil {
		return nil, err
	}
	return &qnxRouter{iface: name, logf: opts.Logf}, nil
}

type qnxRouter struct {
	iface string
	logf  func(string, ...any)
	cur   qnxplat.RouteConfig
}

// Up brings the interface up. There is nothing to do here: CreateTUN itself
// runs `ifconfig <name> up` (see tun_qnx.go) because the tun.Device exists
// and is already open well before this Router is constructed - by the time
// Up is called, the interface has been up since CreateTUN returned.
func (r *qnxRouter) Up() error { return nil }

func (r *qnxRouter) Set(cfg *router.Config) error {
	next := qnxplat.RouteConfig{}
	if cfg != nil {
		next = qnxplat.RouteConfig{Addrs: cfg.LocalAddrs, Routes: cfg.Routes}
		if len(cfg.LocalRoutes) > 0 {
			r.logf("qnx router: ignoring %d LocalRoutes %v: QNX has no equivalent of Linux's `throw` routes to exclude them from the tunnel", len(cfg.LocalRoutes), cfg.LocalRoutes)
		}
	}
	cmds := qnxplat.PlanRoutes(r.iface, r.cur, next, r.logf)
	if err := qnxplat.Apply(cmds, r.logf); err != nil {
		// Apply has no rollback, so a plan that died partway leaves the unit
		// somewhere between r.cur and next and we cannot tell where. Record
		// the union of the two: everything that might now be installed, so
		// that a later Set can still diff it away. Keeping r.cur as it was
		// used to strand the prefixes the partial plan did install - they
		// appeared in no later oldCfg, so nothing ever deleted them, and a
		// /32 for a peer that had since left kept blackholing that address.
		//
		// There is no retry timer behind this: tailscale re-calls Set on the
		// next netmap or prefs change, which on a parked car may be days
		// away, so the state we leave here has to be one the next Set can
		// work from rather than a bet on being called again soon.
		r.cur = qnxplat.PossiblyApplied(r.cur, next)
		r.logf("qnx router: apply failed, recording what may be installed so a later Set can clean it up: %v", err)
		return err
	}
	r.cur = next
	return nil
}

func (r *qnxRouter) Close() error {
	// Destroying the interface (wireguard-go's tun Close) drops its routes,
	// so the route deletes it now plans are belt-and-braces: whichever happens
	// first, the loser exits benignly with "not in table". The addresses do
	// need the explicit removal.
	cmds := qnxplat.PlanRoutes(r.iface, r.cur, qnxplat.RouteConfig{}, r.logf)
	err := qnxplat.Apply(cmds, r.logf)
	r.cur = qnxplat.RouteConfig{}
	return err
}

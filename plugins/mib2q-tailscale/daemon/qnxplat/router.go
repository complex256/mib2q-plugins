package qnxplat

import (
	"context"
	"fmt"
	"net/netip"
	"os"
	"os/exec"
	"strings"
	"time"
)

// RouteConfig is the part of tailscale's router.Config this platform can act
// on: the node's own addresses and the prefixes routed into the tunnel.
type RouteConfig struct {
	Addrs  []netip.Prefix
	Routes []netip.Prefix
}

// PlanRoutes returns the commands that move the OS from oldCfg to newCfg.
//
// IPv4 only (the QNX tun carries IPv4), and never a default route: a default
// route through the tunnel also captures the traffic the unit forwards for
// its Wi-Fi hotspot, whose NAT only applies on the uplink - it takes the
// hotspot's internet down.
//
// Any IPv6 prefixes in newCfg are dropped (there is nothing to install them
// on) and logged via logf, which may be nil to suppress that logging - the
// same logf later passed to Apply is fine to reuse here.
func PlanRoutes(iface string, oldCfg, newCfg RouteConfig, logf func(string, ...any)) [][]string {
	var cmds [][]string
	oldAddrs, newAddrs := v4Set(oldCfg.Addrs, "address", nil), v4Set(newCfg.Addrs, "address", logf)
	oldGW, gw := primary(oldCfg.Addrs), primary(newCfg.Addrs)

	// A route on this platform is a prefix AND the `-interface <addr>` it
	// hangs off, so the gateway belongs in the diff key. When the unit's
	// Tailscale IPv4 changes - re-auth with a new node key, or an
	// admin-forced address - the peer prefixes usually do not change at all,
	// yet every installed route still points at an address this same plan
	// removes. A diff keyed on the prefix alone emitted nothing for those
	// routes and every peer went silently unreachable, so a changed gateway
	// re-plans the whole route set instead of diffing it.
	sameGW := oldGW.IsValid() && oldGW == gw

	var addrDeletes [][]string
	for a := range newAddrs {
		if !oldAddrs[a] {
			ip := a.Addr().String()
			args := []string{"ifconfig", iface, "inet", ip, ip, "netmask", "255.255.255.255"}
			if gw.IsValid() && a.Addr() != gw {
				args = append(args, "alias")
			}
			cmds = append(cmds, args)
		}
	}
	for a := range oldAddrs {
		if !newAddrs[a] {
			addrDeletes = append(addrDeletes, []string{"ifconfig", iface, "inet", a.Addr().String(), "delete"})
		}
	}

	oldRoutes, newRoutes := v4Set(oldCfg.Routes, "route", nil), v4Set(newCfg.Routes, "route", logf)

	// Deletes come before the address deletes below and need no gateway of
	// their own, so they are planned even when newCfg has no usable IPv4.
	// That case is a transient netmap (empty or IPv6-only LocalAddrs), not a
	// Close: the interface stays up and its routes with it. Skipping the
	// deletes here left them pointing at an address the same plan removed,
	// and the caller then recorded the empty config as applied, so no later
	// Set ever computed those deletes again.
	for r := range oldRoutes {
		if !newRoutes[r] || !sameGW {
			cmds = append(cmds, []string{"route", "delete", "-inet", "-net", r.String()})
		}
	}
	if gw.IsValid() {
		for r := range newRoutes {
			if !oldRoutes[r] || !sameGW {
				cmds = append(cmds, []string{"route", "add", "-inet", "-net", r.String(), "-interface", gw.String()})
			}
		}
	}

	// Last: an address must outlive the routes that still point at it, and
	// the replacement address is already in place by now.
	return append(cmds, addrDeletes...)
}

// v4Set keeps the IPv4 prefixes, dropping IPv6 and any prefix that would
// amount to a default route. IPv6 prefixes are reported via logf (which may
// be nil) so they are dropped loudly rather than silently; kind labels them
// ("address" or "route") in that message. The default-route drop is not
// logged - see PlanRoutes.
//
// "Amount to a default route" is deliberately wider than 0.0.0.0/0. The
// usual way a VPN takes over all traffic while leaving the existing default
// route in the table is the pair 0.0.0.0/1 + 128.0.0.0/1, which together
// cover the whole address space but have Bits() == 1 and so would slip past
// a == 0 test. On this unit a full tunnel captured the Wi-Fi hotspot's
// forwarded traffic, whose NAT only applies on the uplink, and took the
// hotspot's internet down until someone physically rebooted the car. Both
// encodings are refused here, at the one place that emits route commands.
func v4Set(in []netip.Prefix, kind string, logf func(string, ...any)) map[netip.Prefix]bool {
	out := map[netip.Prefix]bool{}
	var skippedV6 []netip.Prefix
	for _, p := range in {
		p = p.Masked()
		if !p.Addr().Is4() {
			skippedV6 = append(skippedV6, p)
			continue
		}
		if p.Bits() <= 1 {
			continue
		}
		out[p] = true
	}
	if len(skippedV6) > 0 && logf != nil {
		logf("qnx router: dropping %d IPv6 %s(es), this tun is IPv4-only: %v", len(skippedV6), kind, skippedV6)
	}
	return out
}

// primary picks the address that routes hang off as `-interface <addr>`. It
// must apply the same filter v4Set does, or an address v4Set refused can
// still come back as a gateway: a 0.0.0.0/0 entry in Addrs produced
// `route add ... -interface 0.0.0.0` before this skipped it. Bits are only
// used to test the prefix; the address itself is returned unmasked, since
// that is what ifconfig was given.
func primary(addrs []netip.Prefix) netip.Addr {
	for _, p := range addrs {
		if !p.Addr().Is4() || p.Bits() <= 1 || p.Addr().IsUnspecified() {
			continue
		}
		return p.Addr()
	}
	return netip.Addr{}
}

// cmdTimeout bounds how long Apply waits on a single command. route(8) and
// ifconfig(8) normally return in milliseconds, so 10s is far beyond any
// legitimate run - and still short enough that a full reconfiguration
// completes quickly even if one or two commands each wedge once.
//
// It is a var rather than a const only so a test can shrink it instead of
// actually sitting through a hang; it is deliberately not exported.
var cmdTimeout = 10 * time.Second

// cmdAttempts bounds how many times a command that timed out is retried. A
// transient spawn wedge may clear on a later attempt, and each attempt is
// separately bounded by cmdTimeout - so the worst case for one command is
// cmdAttempts * cmdTimeout.
const cmdAttempts = 3

// runOnce runs one command under two independent bounds:
//
//   - exec.CommandContext's context kills the CHILD process when it fires.
//     That covers a child that starts and then hangs.
//   - The select against time.After bounds this call regardless of whether
//     the exec machinery ever returns. Context cancellation cannot help if
//     the hang is on the PARENT side - inside Go's forkExec, before or during
//     posix_spawn, before any child exists yet to kill - which is plausible
//     for the failure this guards against: every command here is a
//     posix_spawn plus a SIGCHLD on child exit, the same machinery behind a
//     historical "fatal: bad g in signal handler" crash on this Go port. If
//     that is where it wedges, CombinedOutput() itself never returns and no
//     context rescues the goroutine running it - only the timer does.
//
// completed is false only in that second case. When it is false, the
// goroutine below leaks: there is no child process to kill, and Go has no way
// to abort a goroutine blocked in a syscall. That is an accepted trade, not
// an oversight - the leak is bounded by the number of commands in one
// reconfiguration, and the alternative is a node that never comes up again
// until a human power-cycles it.
func runOnce(c []string) (out string, err error, completed bool) {
	ctx, cancel := context.WithTimeout(context.Background(), cmdTimeout)
	defer cancel()

	type result struct {
		out []byte
		err error
	}
	ch := make(chan result, 1) // buffered: a leaked goroutine must still be able to send without blocking forever
	go func() {
		cmd := exec.CommandContext(ctx, Tool(c[0]), c[1:]...)
		cmd.Env = childEnv()
		o, e := cmd.CombinedOutput()
		ch <- result{o, e}
	}()

	select {
	case r := <-ch:
		return strings.TrimSpace(string(r.out)), r.err, true
	case <-time.After(cmdTimeout):
		return "", nil, false
	}
}

// childEnv is the environment every command runOnce spawns is given: the
// parent's environment minus the variables that exist only for tailscaled's
// own benefit, not for route(8) or ifconfig(8).
//
// tailscaled is started (deploy/tailscale.sh) with LD_PRELOAD pointing at a
// shim (qnxplat/clockshim/tsclock.c) that interposes clock_gettime so
// tailscaled sees corrected time without the OS clock itself being stepped -
// stepping it restarted the car's navigation domain, which is why the shim
// exists instead. TS_CLOCK_OFFSET carries the correction it applies. Neither
// exec.CommandContext call above set Env, so every route/ifconfig child was
// also inheriting both and loading that shim, running its
// __attribute__((constructor)) - which does dlsym(RTLD_NEXT, "clock_gettime")
// - on every single spawn. route(8)/ifconfig(8) have no use for corrected
// time, so that constructor call is pure overhead on this path, and an
// LD_PRELOAD'd constructor doing dlsym across repeated fork/exec is a
// well-known dynamic-loader-lock hazard - sitting precisely on the path that
// has been observed to wedge: on some cold boots tailscaled adds exactly one
// route and then hangs forever inside the second.
//
// To be clear about what this is: the first observed hang predates the
// shim's existence, so this is NOT a fix for that deadlock and must not be
// described as one. It removes a real hazard from the failing path and takes
// one variable off the table for whoever debugs the next hang.
//
// The filter matches on the variable NAME - the part of each os.Environ()
// entry before its first '=' - not on a substring of the whole entry. A
// naive strings.Contains(e, "LD_PRELOAD") would also drop an unrelated
// variable whose VALUE merely happens to mention LD_PRELOAD.
func childEnv() []string {
	drop := map[string]bool{"LD_PRELOAD": true, "TS_CLOCK_OFFSET": true}
	parent := os.Environ()
	out := make([]string, 0, len(parent))
	for _, e := range parent {
		name, _, _ := strings.Cut(e, "=")
		if drop[name] {
			continue
		}
		out = append(out, e)
	}
	return out
}

// runWithRetries runs c up to cmdAttempts times, retrying only when runOnce
// timed out (completed == false); a genuine result, success or a real exit
// error, returns on the first attempt. If every attempt times out, the
// returned error carries no output for benign to match against, so it is
// correctly treated as a real failure rather than laundered by benign's
// text-matching.
func runWithRetries(c []string, logf func(string, ...any)) (string, error) {
	name := strings.Join(c, " ")
	for attempt := 1; attempt <= cmdAttempts; attempt++ {
		out, err, completed := runOnce(c)
		if completed {
			return out, err
		}
		if attempt < cmdAttempts {
			logf("qnx router: %s: attempt %d/%d timed out after %s, retrying", name, attempt, cmdAttempts, cmdTimeout)
		}
	}
	return "", fmt.Errorf("timed out after %d attempts of %s each, command abandoned", cmdAttempts, cmdTimeout)
}

// Apply runs planned commands, logging each. A failing command is logged and
// the rest still run: a stale route left behind must not stop the node from
// coming up. The error reports only the commands that genuinely failed - a
// command that exited non-zero because the box was already in the state we
// asked for is logged and otherwise ignored, see benign.
func Apply(cmds [][]string, logf func(string, ...any)) error {
	var failed []string
	for _, c := range cmds {
		msg, err := runWithRetries(c, logf)
		if err != nil {
			if benign(c, msg) {
				logf("qnx router: %s: already in the desired state: %s", strings.Join(c, " "), msg)
				continue
			}
			logf("qnx router: %s: %v: %s", strings.Join(c, " "), err, msg)
			failed = append(failed, strings.Join(c, " "))
			continue
		}
		logf("qnx router: %s", strings.Join(c, " "))
	}
	if len(failed) > 0 {
		return fmt.Errorf("%d command(s) failed: %s", len(failed), strings.Join(failed, "; "))
	}
	return nil
}

// benign reports whether a non-zero exit means the command's effect was
// already in place, rather than that it did not happen.
//
// route(8) and ifconfig(8) on QNX 6.5 are NetBSD-derived and have no
// idempotent mode: they report the errno from the routing socket and exit
// non-zero, so "add a route that is already there" is indistinguishable from
// "the add failed" by exit status alone. Counting those as failures was what
// froze the caller's applied-state at an old config: it re-planned from it on
// every later Set, re-issued the same adds, got the same EEXIST and never
// advanced. There is nothing to retry when the table already says what we
// want it to say.
//
// The errno text is matched per verb rather than globally, so that a message
// that is benign for one command cannot launder an unrelated failure of
// another:
//
//   - route add / EEXIST "File exists" - the prefix is already in the table.
//   - route delete / ESRCH - route(8) prints the raw strerror ("No such
//     process") on its "writing to routing socket" line and its own "not in
//     table" on the summary line; either identifies a route already gone.
//   - ifconfig ... delete / EADDRNOTAVAIL "Can't assign requested address" -
//     the address is not on the interface. This one is routine rather than
//     exceptional: assigning the primary address without `alias` replaces the
//     one already there, so the plan's own delete of the old address
//     regularly finds it gone. The message is also spelled "Cannot assign
//     requested address" by some strerror tables, so both are accepted.
//
// Anything else - a missing tun, an unreachable gateway, a tool that is not
// on the unit at all - is a real failure and reported as one.
//
// All three were confirmed on a unit (2026-09-21) by provoking them by hand,
// rather than read off NetBSD's source (addresses below are examples; the
// message text is verbatim):
//
//	route: writing to routing socket: File exists
//	add net 100.64.0.2: gateway 100.64.0.1: File exists
//	route: writing to routing socket: not in table / delete net ...: not in table
//	ifconfig: SIOCDIFADDR: Can't assign requested address
//
// That exercise also showed route(8)'s two streams interleaving mid-word
// ("routing socke" + "delete net ..." + "t: not in table") because Apply
// captures them combined. A splice could in principle land inside the phrase
// matched here - but route(8) prints its phrase on both streams, so there are
// two chances at it, and ifconfig prints once from a single writer and cannot
// interleave with itself. Worth knowing before trusting a single match.
func benign(cmd []string, out string) bool {
	if len(cmd) < 2 {
		return false
	}
	switch {
	case cmd[0] == "route" && cmd[1] == "add":
		return strings.Contains(out, "File exists")
	case cmd[0] == "route" && cmd[1] == "delete":
		return strings.Contains(out, "not in table") || strings.Contains(out, "No such process")
	case cmd[0] == "ifconfig" && cmd[len(cmd)-1] == "delete":
		return strings.Contains(out, "Can't assign requested address") ||
			strings.Contains(out, "Cannot assign requested address")
	}
	return false
}

// PossiblyApplied returns the config a caller should record as applied when
// Apply reported a genuine failure partway through a plan.
//
// Apply has no rollback - the commands before the failure already took - so
// neither prev nor attempted describes the unit. Recording prev (what the
// router used to do) loses every prefix the partial plan installed: it is in
// no later oldCfg, so it is never diffed away, and a /32 for a peer that has
// since left keeps blackholing that address until the process dies, which on
// a parked car can be weeks. Recording attempted is the mirror image and
// loses whatever prev had that the plan was about to delete.
//
// The union is deliberately the pessimistic set: everything that might be
// installed. A delete planned for something that never landed exits benignly
// and costs one logged line, whereas a prefix dropped from the set can never
// be cleaned up again. The set collapses back to the plain config on the next
// Set that applies cleanly.
func PossiblyApplied(prev, attempted RouteConfig) RouteConfig {
	return RouteConfig{
		Addrs:  union(prev.Addrs, attempted.Addrs),
		Routes: union(prev.Routes, attempted.Routes),
	}
}

// union concatenates a and b without duplicates, a's order first. Order
// matters for Addrs: primary takes the first usable one, so the address
// already on the interface stays the gateway until a Set applies cleanly.
func union(a, b []netip.Prefix) []netip.Prefix {
	seen := make(map[netip.Prefix]bool, len(a)+len(b))
	out := make([]netip.Prefix, 0, len(a)+len(b))
	for _, list := range [][]netip.Prefix{a, b} {
		for _, p := range list {
			if seen[p] {
				continue
			}
			seen[p] = true
			out = append(out, p)
		}
	}
	return out
}

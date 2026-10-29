package qnxplat

import (
	"fmt"
	"net/netip"
	"strings"
	"testing"
	"time"
)

func pfx(ss ...string) []netip.Prefix {
	var out []netip.Prefix
	for _, s := range ss {
		out = append(out, netip.MustParsePrefix(s))
	}
	return out
}

func joined(cmds [][]string) []string {
	var out []string
	for _, c := range cmds {
		out = append(out, strings.Join(c, " "))
	}
	return out
}

func has(t *testing.T, cmds [][]string, want string) {
	t.Helper()
	for _, c := range joined(cmds) {
		if c == want {
			return
		}
	}
	t.Errorf("missing command %q in:\n  %s", want, strings.Join(joined(cmds), "\n  "))
}

func hasNo(t *testing.T, cmds [][]string, substr string) {
	t.Helper()
	for _, c := range joined(cmds) {
		if strings.Contains(c, substr) {
			t.Errorf("unexpected command containing %q: %s", substr, c)
		}
	}
}

// before asserts a plan's ordering. The unit executes the plan top to bottom
// with no rollback, so "delete then re-add" is the difference between a route
// that ends up installed and one that does not.
func before(t *testing.T, cmds [][]string, first, second string) {
	t.Helper()
	all := joined(cmds)
	fi, si := -1, -1
	for i, c := range all {
		if c == first && fi < 0 {
			fi = i
		}
		if c == second && si < 0 {
			si = i
		}
	}
	if fi < 0 || si < 0 || fi > si {
		t.Errorf("want %q before %q, got:\n  %s", first, second, strings.Join(all, "\n  "))
	}
}

func TestPlanRoutesFirstConfig(t *testing.T) {
	cmds := PlanRoutes("tun9", RouteConfig{}, RouteConfig{
		Addrs:  pfx("100.101.102.103/32"),
		Routes: pfx("100.64.0.0/10", "100.99.1.2/32"),
	}, nil)
	has(t, cmds, "ifconfig tun9 inet 100.101.102.103 100.101.102.103 netmask 255.255.255.255")
	has(t, cmds, "route add -inet -net 100.64.0.0/10 -interface 100.101.102.103")
	has(t, cmds, "route add -inet -net 100.99.1.2/32 -interface 100.101.102.103")
}

func TestPlanRoutesIsIncremental(t *testing.T) {
	old := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10", "100.99.1.2/32")}
	new := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10", "100.99.9.9/32")}
	cmds := PlanRoutes("tun9", old, new, nil)
	has(t, cmds, "route delete -inet -net 100.99.1.2/32")
	has(t, cmds, "route add -inet -net 100.99.9.9/32 -interface 100.101.102.103")
	hasNo(t, cmds, "100.64.0.0/10") // unchanged: not touched
	hasNo(t, cmds, "ifconfig")      // address unchanged
}

func TestPlanRoutesSkipsIPv6AndDefault(t *testing.T) {
	var logged []string
	logf := func(format string, args ...any) { logged = append(logged, fmt.Sprintf(format, args...)) }
	cmds := PlanRoutes("tun9", RouteConfig{}, RouteConfig{
		Addrs:  pfx("100.101.102.103/32", "fd7a:115c:a1e0::1/128"),
		Routes: pfx("100.64.0.0/10", "0.0.0.0/0", "::/0"),
	}, logf)
	hasNo(t, cmds, "fd7a")      // IPv6 address skipped
	hasNo(t, cmds, "::/0")      // IPv6 default skipped
	hasNo(t, cmds, "0.0.0.0/0") // never a default route: it broke the unit's hotspot NAT

	var sawV6Addr, sawV6Route bool
	for _, l := range logged {
		if strings.Contains(l, "fd7a") {
			sawV6Addr = true
		}
		if strings.Contains(l, "::/0") {
			sawV6Route = true
		}
	}
	if !sawV6Addr {
		t.Errorf("dropped IPv6 address was not logged; logged=%v", logged)
	}
	if !sawV6Route {
		t.Errorf("dropped IPv6 route was not logged; logged=%v", logged)
	}
}

// The pair 0.0.0.0/1 + 128.0.0.0/1 covers the whole address space, which is
// how a VPN usually takes over all traffic without replacing the existing
// default route. Bits() == 1, so a guard that only rejects /0 lets it
// through, and the result on this unit is the hotspot outage that required a
// physical reboot. Both halves must be refused wherever they appear.
func TestPlanRoutesRefusesSplitDefault(t *testing.T) {
	cmds := PlanRoutes("tun9", RouteConfig{}, RouteConfig{
		Addrs:  pfx("100.101.102.103/32"),
		Routes: pfx("0.0.0.0/1", "128.0.0.0/1", "100.64.0.0/10"),
	}, nil)
	hasNo(t, cmds, "0.0.0.0/1")
	hasNo(t, cmds, "128.0.0.0/1")
	has(t, cmds, "route add -inet -net 100.64.0.0/10 -interface 100.101.102.103")
}

// A default route hidden in Addrs rather than Routes must not become an
// ifconfig command either.
func TestPlanRoutesRefusesDefaultInAddrs(t *testing.T) {
	cmds := PlanRoutes("tun9", RouteConfig{}, RouteConfig{
		Addrs:  pfx("0.0.0.0/0"),
		Routes: pfx("100.64.0.0/10"),
	}, nil)
	hasNo(t, cmds, "0.0.0.0")
	// With no usable address there is no -interface argument to hang routes
	// off, so nothing is planned at all rather than something half-formed.
	hasNo(t, cmds, "route add")
}

// route(8) and ifconfig(8) on QNX 6.5 are NetBSD-derived: they exit non-zero
// for commands whose effect is already the state we wanted. Treating those as
// failures pinned r.cur at an old config forever, so every later Set re-planned
// from it, re-issued the same adds, got "File exists" again and never advanced.
// Only an error that leaves the box in a state we did not ask for counts.
func TestBenignExitsAreNotFailures(t *testing.T) {
	cases := []struct {
		name string
		cmd  []string
		out  string
		want bool
	}{
		// EEXIST. The route is already in the table pointing where we want it.
		{"route add existing", []string{"route", "add", "-inet", "-net", "100.64.0.0/10", "-interface", "100.101.102.103"},
			"route: writing to routing socket: File exists\nadd net 100.64.0.0: gateway 100.101.102.103: File exists", true},
		// ESRCH. route(8) prints the raw strerror on the socket line and its
		// own wording on the summary line; either one identifies the case.
		{"route delete missing", []string{"route", "delete", "-inet", "-net", "100.99.1.2/32"},
			"route: writing to routing socket: No such process\ndelete net 100.99.1.2: not in table", true},
		// EADDRNOTAVAIL. The address is not on the interface, which is what
		// the delete was for.
		{"ifconfig delete missing", []string{"ifconfig", "tun9", "inet", "100.101.102.103", "delete"},
			"ifconfig: SIOCDIFADDR: Can't assign requested address", true},
		// Genuinely wrong: the tun is gone, or the gateway address is not on
		// any interface. Nothing we asked for happened and re-planning from
		// the old config is the right response.
		{"route add unreachable", []string{"route", "add", "-inet", "-net", "100.64.0.0/10", "-interface", "100.101.102.103"},
			"route: writing to routing socket: Network is unreachable", false},
		{"ifconfig no such interface", []string{"ifconfig", "tun9", "inet", "100.101.102.103", "100.101.102.103", "netmask", "255.255.255.255"},
			"ifconfig: SIOCGIFFLAGS tun9: Device not configured", false},
		// "File exists" is only benign for the verb that can legitimately
		// produce it; it must not launder an unrelated delete failure.
		{"file exists on delete", []string{"route", "delete", "-inet", "-net", "100.64.0.0/10"},
			"route: writing to routing socket: File exists", false},
	}
	for _, c := range cases {
		if got := benign(c.cmd, c.out); got != c.want {
			t.Errorf("%s: benign(%q) = %v, want %v", c.name, strings.Join(c.cmd, " "), got, c.want)
		}
	}
}

// A plan that dies partway still installed everything before the failure. If
// the router forgets those, they are never in a later oldCfg and so are never
// deleted: 100.99.1.2/32 keeps blackholing that address for the life of the
// process, long after the peer left. Recording the union of what was already
// there and what we just tried keeps every possibly-installed prefix
// deletable; the deletes for the ones that never landed exit benignly.
func TestPlanRoutesAfterPartialApplyDeletesWhatMayBeInstalled(t *testing.T) {
	c0 := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10")}
	c1 := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10", "100.99.1.2/32")}
	c2 := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10")}

	cur := PossiblyApplied(c0, c1)
	cmds := PlanRoutes("tun9", cur, c2, nil)
	has(t, cmds, "route delete -inet -net 100.99.1.2/32")
	hasNo(t, cmds, "100.64.0.0/10") // unchanged across all three: not touched

	// The recorded set is raw config, not a plan, so a default route that a
	// failed Set carried still has to die at v4Set on the way back out.
	bad := RouteConfig{Addrs: pfx("100.101.102.103/32", "0.0.0.0/0"), Routes: pfx("0.0.0.0/1", "128.0.0.0/1", "0.0.0.0/0")}
	cmds = PlanRoutes("tun9", PossiblyApplied(c0, bad), bad, nil)
	hasNo(t, cmds, "0.0.0.0")
	hasNo(t, cmds, "128.0.0.0/1")
}

// A transient netmap with empty or IPv6-only LocalAddrs is a Set, not a
// Close: the interface stays up and so do its routes. Skipping the deletes
// left every route pointing at an address this same plan removes, and r.cur
// then advanced to the empty config, so no later Set ever computed those
// deletes again. Deleting a route needs no gateway, so the missing address is
// no reason to skip it.
func TestPlanRoutesDeletesRoutesWhenNewConfigHasNoAddress(t *testing.T) {
	old := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10", "100.99.1.2/32")}
	new := RouteConfig{Addrs: pfx("fd7a:115c:a1e0::1/128")}
	cmds := PlanRoutes("tun9", old, new, nil)
	has(t, cmds, "route delete -inet -net 100.64.0.0/10")
	has(t, cmds, "route delete -inet -net 100.99.1.2/32")
	has(t, cmds, "ifconfig tun9 inet 100.101.102.103 delete")
	hasNo(t, cmds, "route add")
	// The routes have to go before the address they hang off.
	before(t, cmds, "route delete -inet -net 100.64.0.0/10", "ifconfig tun9 inet 100.101.102.103 delete")
}

// Re-auth with a new node key, or an admin-forced IP change, hands the unit a
// different Tailscale IPv4 while the peer set is unchanged. Every route was
// installed as `-interface <old addr>` and that address is removed by this
// same plan, so a diff keyed on the prefix alone emits nothing for them and
// every peer goes silently unreachable. The gateway is part of the route, so
// it has to be part of the key.
func TestPlanRoutesReinstallsRoutesWhenAddressChanges(t *testing.T) {
	old := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10", "100.99.1.2/32")}
	new := RouteConfig{Addrs: pfx("100.101.102.200/32"), Routes: pfx("100.64.0.0/10", "100.99.1.2/32")}
	cmds := PlanRoutes("tun9", old, new, nil)
	has(t, cmds, "ifconfig tun9 inet 100.101.102.200 100.101.102.200 netmask 255.255.255.255")
	has(t, cmds, "ifconfig tun9 inet 100.101.102.103 delete")
	has(t, cmds, "route delete -inet -net 100.64.0.0/10")
	has(t, cmds, "route delete -inet -net 100.99.1.2/32")
	has(t, cmds, "route add -inet -net 100.64.0.0/10 -interface 100.101.102.200")
	has(t, cmds, "route add -inet -net 100.99.1.2/32 -interface 100.101.102.200")
	hasNo(t, cmds, "-interface 100.101.102.103") // nothing left pointing at the old address
	before(t, cmds, "route delete -inet -net 100.64.0.0/10", "route add -inet -net 100.64.0.0/10 -interface 100.101.102.200")
	// The new address must exist before a route is hung off it, and the old
	// one must outlive the routes that still point at it.
	before(t, cmds, "ifconfig tun9 inet 100.101.102.200 100.101.102.200 netmask 255.255.255.255", "route add -inet -net 100.64.0.0/10 -interface 100.101.102.200")
	before(t, cmds, "route delete -inet -net 100.99.1.2/32", "ifconfig tun9 inet 100.101.102.103 delete")
}

// withShortTimeout shrinks cmdTimeout for the duration of a test so a test
// exercising the hang path does not actually wait out the real 10s bound.
// cmdTimeout is unexported on purpose - this is the one thing a test needs
// from it, not a public knob for callers.
func withShortTimeout(t *testing.T, d time.Duration) {
	t.Helper()
	orig := cmdTimeout
	cmdTimeout = d
	t.Cleanup(func() { cmdTimeout = orig })
}

// A command that never returns must not wedge Apply forever: the real
// failure this mitigates is tailscaled hanging dead on a `route add` that
// never comes back from posix_spawn. `sleep 30` stands in for it - Apply
// must give up and report it, in far less than 30s.
func TestApplyAbandonsHangingCommand(t *testing.T) {
	withShortTimeout(t, 30*time.Millisecond)

	var logged []string
	logf := func(format string, args ...any) { logged = append(logged, fmt.Sprintf(format, args...)) }

	start := time.Now()
	err := Apply([][]string{{"sleep", "30"}}, logf)
	elapsed := time.Since(start)

	if err == nil {
		t.Fatal("want an error for a command that never returns, got nil")
	}
	if !strings.Contains(err.Error(), "sleep 30") {
		t.Errorf("error %q does not name the hung command", err.Error())
	}
	if elapsed > 5*time.Second {
		t.Errorf("Apply took %s to give up on a command that never returns; the timeout is not bounding it", elapsed)
	}
}

// This is the property that matters most for the real failure: a wedged
// `route add` must not stop the routes planned after it from being
// attempted. Apply must move on to later commands, not stall on the first
// hang.
func TestApplyContinuesAfterTimeout(t *testing.T) {
	withShortTimeout(t, 30*time.Millisecond)

	var logged []string
	logf := func(format string, args ...any) { logged = append(logged, fmt.Sprintf(format, args...)) }

	err := Apply([][]string{
		{"sleep", "30"},
		{"echo", "ran-after-timeout"},
	}, logf)

	if err == nil {
		t.Fatal("want an error naming the hung command")
	}

	var sawSecondSucceed bool
	for _, l := range logged {
		if strings.Contains(l, "echo ran-after-timeout") && !strings.Contains(l, "timed out") {
			sawSecondSucceed = true
		}
	}
	if !sawSecondSucceed {
		t.Errorf("second command was not run after the first timed out; logged=%v", logged)
	}
}

// No regression in the happy path: a normal fast command still succeeds and
// is logged exactly as before, timeout/retry machinery notwithstanding.
func TestApplyFastCommandLogsSuccessUnchanged(t *testing.T) {
	var logged []string
	logf := func(format string, args ...any) { logged = append(logged, fmt.Sprintf(format, args...)) }

	if err := Apply([][]string{{"true"}}, logf); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	want := "qnx router: true"
	if len(logged) != 1 || logged[0] != want {
		t.Errorf("logged = %v, want [%q]", logged, want)
	}
}

// parseEnvOutput turns the line-per-entry output of the `env` command into a
// name->value map, splitting each line on its first '='.
func parseEnvOutput(t *testing.T, out string) map[string]string {
	t.Helper()
	m := map[string]string{}
	for _, line := range strings.Split(out, "\n") {
		if line == "" {
			continue
		}
		name, value, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		m[name] = value
	}
	return m
}

// TestApplySanitizesChildEnvironment is the reason this file exists: Apply's
// children must not inherit LD_PRELOAD or TS_CLOCK_OFFSET from tailscaled,
// but must inherit everything else - including a variable whose VALUE
// happens to mention LD_PRELOAD, which a naive substring filter would drop.
//
// The command run through Apply prints its own environment and then exits
// non-zero (`env; exit 1`), which routes its output through Apply's failure
// log line - the one place Apply hands back what a child actually saw.
func TestApplySanitizesChildEnvironment(t *testing.T) {
	if Tool("sh") == "sh" {
		t.Skip("no sh found on any of the configured tool dirs")
	}

	t.Setenv("LD_PRELOAD", "/tmp/libtsclock.so")
	t.Setenv("TS_CLOCK_OFFSET", "12345")
	t.Setenv("QNXPLAT_TEST_KEEP", "keep-me-please")
	t.Setenv("QNXPLAT_TEST_VALUE_MENTIONS_LD_PRELOAD", "the value mentions LD_PRELOAD but the name does not")

	var capturedOut string
	logf := func(format string, args ...any) {
		// Apply's failure line is logf("qnx router: %s: %v: %s", cmd, err, msg);
		// pull the raw msg argument rather than re-parsing the formatted
		// string, since msg itself contains newlines.
		if len(args) == 3 {
			if msg, ok := args[2].(string); ok {
				capturedOut = msg
			}
		}
	}

	err := Apply([][]string{{"sh", "-c", "env; exit 1"}}, logf)
	if err == nil {
		t.Fatal("want an error: the command exits non-zero on purpose so its output reaches logf")
	}
	if capturedOut == "" {
		t.Fatal("did not capture the child's output via logf")
	}

	env := parseEnvOutput(t, capturedOut)

	if v, ok := env["LD_PRELOAD"]; ok {
		t.Errorf("child received LD_PRELOAD=%q, want it dropped", v)
	}
	if v, ok := env["TS_CLOCK_OFFSET"]; ok {
		t.Errorf("child received TS_CLOCK_OFFSET=%q, want it dropped", v)
	}
	if got, want := env["QNXPLAT_TEST_KEEP"], "keep-me-please"; got != want {
		t.Errorf("QNXPLAT_TEST_KEEP = %q, want %q (environment must be sanitized, not blanked)", got, want)
	}
	if got, want := env["QNXPLAT_TEST_VALUE_MENTIONS_LD_PRELOAD"], "the value mentions LD_PRELOAD but the name does not"; got != want {
		t.Errorf("QNXPLAT_TEST_VALUE_MENTIONS_LD_PRELOAD = %q, want %q (must match on name, not substring)", got, want)
	}
}

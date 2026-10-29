# A full Tailscale node on the MHI2q head unit

**Date:** 2026-09-20
**Target:** Audi MHI2q (`QNX mmx 6.5.0 APQ8064 armle`), QNX 6.5.0 / ARMv7
**Status:** implemented and running on a unit (2026-09-21). Goals 1-3 verified
on hardware; success criteria 1, 2, 4 and 5 met. Criterion 3 is half met - a
plain TCP connection to a peer's IP succeeds, MagicDNS name resolution does
not, for the dnsmasq reload reason flagged as an open question below. See
[README](../../../README.md) for the as-built system.

## Goal

`tailscaled` running on the head unit as an ordinary Tailscale node:

1. **Reachable from the tailnet** - ssh and other unit services answer on the
   node's Tailscale IP from any device on the tailnet.
2. **The unit reaches the tailnet** - any program on the unit, not just ones
   that speak SOCKS, can reach tailnet peers by IP and by MagicDNS name.
3. **Survives reboots** - state persists in flash and the daemon autostarts,
   so the unit rejoins by itself.

Tailscale **replaces** mibwg on the unit. mibwg stays in its repo, still
builds, and can be switched back at any time.

### Non-goals

- Exit node or subnet router (no route advertisement in either direction).
- IPv6 inside the tunnel (the QNX tun device is IPv4-only until the Go port
  grows an `ioctl` binding for `TUNSIFHEAD`).
- Tailscale SSH, Taildrop, Serve, the web client - all compiled out.

## Background

What already exists and is proven on this unit:

| Piece | Where | State |
|---|---|---|
| `GOOS=qnx GOARCH=arm` Go port (1.26.4) | `qnx65-armv7-toolchain/go` | builds and runs; spawn-crash fixed 2026-09-20 |
| QNX tun device (`/dev/tunN`, blocking read) | `mib2q-wireguard/internal/backend/tun_qnx.go` | verified on the unit, both directions |
| Interface/route/pin logic (`ifconfig`, `route`) | `mib2q-wireguard/internal/backend/kernel_qnx.go` | verified on the unit |
| SNTP clock set | `mib2q-wireguard/internal/sntp` | verified on the unit (RTC boots at 1970) |
| Persistent install + autostart hook | `mib2q-wireguard/deploy`, `startup.sh` | verified across reboots |
| wireguard-go + x/sys QNX patches | `mib2q-wireguard/third_party` | in use |

An earlier attempt (recorded in `qnx65-armv7-toolchain/go/README.md`,
milestones 7-9) ran `tailscaled --tun=userspace-networking` on this unit and
reached the control plane, issuing a login URL. Its build recipe was lost, and
it never had a real tun device. This design supersedes it.

## Version pin

**Tailscale v1.100.0**, whose `go.mod` requires exactly `go 1.26.4` - the
port's version. Upstream `main` requires 1.27.1, so tracking main would first
need the Go port moved to 1.27 (~40 hand-edited upstream files, notably 8 in
`cmd/link`). That is a separate project and is explicitly out of scope here.

## Architecture

```
  /mnt/persist/tailscale/            flash: tailscaled.state, authkey
  /mnt/app/root/tailscale/           flash: tailscaled, tailscale, tailscale.sh, sntp
        |  (start: copy to /tmp, which is RAM and the only exec-capable fs)
        v
  tailscaled --tun=tun9 --state=/mnt/persist/tailscale/tailscaled.state
        |
        +-- tun device      -> qnxtun (shared package) via patched wireguard-go
        +-- router          -> registered through router.HookNewUserspaceRouter
        +-- DNS (MagicDNS)  -> dnsmasq include file + reload
        +-- netmon          -> upstream polling fallback
```

### Components

**1. `qnxtun` - the shared tun device**
Moves out of mibwg's `internal/backend` into `mib2q-wireguard/qnxtun` (an
importable package; the logic exists once, used by mibwg's backend and by the
patched wireguard-go). Public surface:

```go
package qnxtun
func Open(name string) (*Device, error)   // /dev/<name>, after the interface exists
func (*Device) Read([]byte) (int, error)  // one bare IPv4 packet, blocking
func (*Device) Write([]byte) (int, error)
func (*Device) Close() error
```

Load-bearing detail, learned the hard way on the unit: **`poll(2)` never
reports `/dev/tunN` readable on QNX**, so reads must block. A poll-driven
reader sees nothing while io-pkt's `Opkts` counter happily increments, and
traffic dies silently in one direction.

**2. wireguard-go patch - `tun/tun_qnx.go`**
Implements `CreateTUN(name, mtu)` over `qnxtun`, plus `Name`, `MTU`, `Events`,
`BatchSize`, `File` (nil). Creates the interface (`ifconfig <name> create`)
before opening the device; destroys it on `Close`. Lives in
`mib2q-wireguard/third_party/wireguard-go`, which the Tailscale build points at
with a `replace` directive.

**3. Router - `router_qnx.go`**
Registers itself with `router.HookNewUserspaceRouter` (no upstream dispatch
edit needed), and implements `Set(*router.Config)` by diffing against the
current state and shelling out:

- addresses: `ifconfig <tun> inet <addr> <addr> netmask 255.255.255.255`
- routes: `route add -inet -net <prefix> -interface <tun addr>`, removals via
  `route delete`
- `Close()`: destroy the interface, which drops its routes with it

IPv6 entries are skipped with a log line rather than attempted. Tool lookup
mirrors mibwg: `ifconfig` is `/usr/sbin/ifconfig`, `route` is
`/armle/sbin/route`, neither on the default PATH.

**4. DNS - `net/dns/manager_qnx.go` (MagicDNS)**
The unit runs dnsmasq natively, and `/etc/dnsmasq.conf` already ends with
`conf-file=/tmp/dnsmasq.sdis.conf` - an include file that is empty and
recreated on each boot. `SetDNS` writes the split-DNS line into it:

```
server=/<tailnet>.ts.net/100.100.100.100
```

then reloads dnsmasq. **Open question to resolve in implementation:** dnsmasq
re-reads `--conf-file` only on restart, not on SIGHUP, so this needs either a
supervised dnsmasq (HAM may respawn it - to be verified *before* any kill is
attempted on a unit someone is relying on) or the fallback below.

*Fallback:* write tailnet peer names into `/etc/hosts` (on the read-only
`/mnt/system`, remounted for the write) and SIGHUP dnsmasq, which *does*
re-read `/etc/hosts`. Gives working names without touching dnsmasq's config;
loses wildcard/split behavior.

The manager reports `SupportsSplitDNS() = true` only in the `conf-file` case.

**5. netmon**
Upstream's polling monitor (`netmon_polling.go`) covers platforms without
event-based monitoring; `net.Interfaces()` works in the port via `getifaddrs`.
No new code expected - to be confirmed at first build.

## Build

`scripts/build.sh` in the new repo:

1. Fetch Tailscale v1.100.0 into `build/tailscale` (shallow clone, pinned tag,
   not vendored - the repo stays small).
2. Apply `patches/*.patch` (the DNS manager tag edit and anything else upstream
   needs) and add our new files.
3. Add `replace` directives: wireguard-go and x/sys to
   `../mib2q-wireguard/third_party/*`, plus `qnxtun`.
4. Build in the toolchain image (`qnx65-armv7-toolchain:8.5-go`):
   `GOOS=qnx GOARCH=arm GOARM=7 CGO_ENABLED=0`, `-ldflags "-s -w -B none
   -buildid="` (QNX's loader rejects Go's build-id NOTE phdr).
5. Feature tags to fit the ~15 MB exec ceiling, starting with:
   `ts_omit_netstack` (no gVisor - we have a real tun), `ts_omit_ssh`,
   <!-- Implementation note: ts_omit_netstack did not survive contact with
   the unit. Without netstack, tstun.invertGSOChecksum panics "unreachable"
   on the first injected packet, killing tailscaled. netstack is in the
   shipped build, costing 2,670,592 bytes. -->
   `ts_omit_drive`, `ts_omit_taildrop`, `ts_omit_webclient`, `ts_omit_serve`,
   `ts_omit_kube`, `ts_omit_tpm`, `ts_omit_systray`, `ts_omit_capture`.
   The build fails loudly if either binary exceeds 15 MB.

Also built: `tailscale` (CLI, for `tailscale up`/`status` on the unit) and a
small `sntp` helper reusing mibwg's package.

## Deployment

`deploy/tailscale.sh`, modeled on the working `mibwg.sh` and constrained the
same way (the unit has no `dirname`, `tail`, `nohup`, `date` or `kill` binary;
ksh predefines `stop` as an alias, so functions are prefixed):

- `start`: copy binaries to `/tmp`, run `sntp` (the RTC boots at 1970 and TLS
  to the control plane fails without it), start `tailscaled` with
  `--tun=tun9 --state=/mnt/persist/tailscale/tailscaled.state
  --socket=/tmp/tailscaled.sock`, then `tailscale up` with the pre-auth key
  from `/mnt/persist/tailscale/authkey` if the node isn't already authorized.
- `stop`, `restart`, `status` (`tailscale status`), `log`.
- Autostart: the `startup.sh` hook line switches from `mibwg.sh` to
  `tailscale.sh`; the same fire-and-forget form (detached subshell, no stdio,
  guarded by `-x`) so boot can never block on it.

Secrets: the auth key lives only on the unit, in flash, `chmod 600`, and is
gitignored in the repo.

## Testing

| Level | Covers | Notes |
|---|---|---|
| Host (`go test`) | route/DNS command construction, config file handling, anything build-tag-free | QNX files excluded by tags |
| QEMU (`mib2q-wireguard/tests/qemu-net.sh`) | binary builds, starts, doesn't crash, cleans up | no tun cloner and no internet in the SDK's io-pkt, so no tailnet |
| Unit | the actual goals | join tailnet with the key; `ssh` in from `stash` over Tailscale; reach a tailnet peer by IP and MagicDNS name from the unit; reboot and confirm it rejoins unattended |

The unit is the only place goals 1-3 can be verified, as with the tun backend -
where every QEMU test passed while the device was silently dead.

## Risks and rollback

| Risk | Mitigation |
|---|---|
| Binary exceeds the ~15 MB exec ceiling | feature tags above; then fall back to `--tun=userspace-networking` (inbound works, outbound only via proxy) |
| dnsmasq won't reload config | `/etc/hosts` + SIGHUP fallback; verify supervision before killing dnsmasq on a live unit |
| Upstream doesn't compile for an unsupported GOOS | expected to be small (the earlier attempt called it "small" *with* gVisor); each gap is a new build-tagged file |
| Tailscale's router fights the unit's own routes | split-tunnel by nature: only tailnet CGNAT space (100.64.0.0/10) is routed. **Never a default route** - a full tunnel already broke the unit's Wi-Fi hotspot NAT once |
| Something goes wrong on the car | mibwg's binary, config and control script stay installed; rollback is restoring one line in `startup.sh` and starting `mibwg.sh` |

## Success criteria

1. `tailscale status` on the unit shows the node online, and the admin console
   lists it.
2. `ssh root@<unit tailscale ip>` from `stash` works.
3. From the unit, a plain TCP connection to a tailnet peer's IP succeeds, and a
   MagicDNS name resolves.
4. After a reboot, 1-3 hold with no human action.
5. `tailscaled` and `tailscale` are each under 15 MB.

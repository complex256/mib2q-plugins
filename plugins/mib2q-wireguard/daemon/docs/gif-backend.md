# Phase 2: kernel-routed backends (tun, gif)

**Status:**
- `Backend = gif`: implemented, end-to-end under QEMU on `io-pkt-v6-hc`;
  not yet run on a unit.
- `Backend = tun`: implemented; the device itself was verified on an MHI2q
  (`tests/tunprobe`), the backend has not run yet - the SDK's io-pkt has no
  tun, so QEMU can't test it.
- Full tunnel (`0.0.0.0/0`) is implemented for both but untested - it needs a
  real uplink and default route.

## Problem

With the netstack backend only programs using mibwg's [Proxy] reach the
tunnel. To route the unit's own traffic, the kernel must route into mibwg.

The SDK's `io-pkt-v6-hc` has no tun/tap (its cloners are
`bridge gif gre loop pppoe ppp vlan`), which is what this design started
from. **The MHI2q's own io-pkt does:** `ifconfig -C` on the unit lists
`tap tun bridge vlan gif gre pppoe ppp lo`, `/dev/tun0`-`tun3` exist from
boot, and a created `tun9` reads and writes bare IP packets from Go (no
header, both directions - `tests/tunprobe`). So:

- **tun** (`tun9`) is the preferred kernel-routed backend where io-pkt has
  the cloner: one pass through io-pkt, no framing. IPv4 only until the Go
  port grows an ioctl binding for `TUNSIFHEAD` (without it tun treats every
  written packet as AF_INET).
- **gif** (`gif9`, below) works on any io-pkt and carries IPv6 too.

Both share `internal/backend/kernel_qnx.go`: addresses, routes, endpoint
pinning and full-tunnel handling are identical.

## How it works

gif(4) is io-pkt's IP-in-IP interface. With its *outer* addresses on lo0 it
becomes a packet pipe between the routing table and a raw socket:

```
ifconfig lo0 10.255.255.1 netmask 255.255.255.255 alias    # TunLocal
ifconfig lo0 10.255.255.2 netmask 255.255.255.255 alias    # TunPeer
ifconfig gif9 create
ifconfig gif9 tunnel 10.255.255.1 10.255.255.2
ifconfig gif9 inet 10.6.0.2 10.6.0.2 netmask 255.255.255.255
route add -net 10.6.0.0/24 -interface 10.6.0.2               # per AllowedIPs
```

- **Kernel -> mibwg.** A packet routed to gif9 is encapsulated
  `TunLocal -> TunPeer` and looped through lo0. gif only claims encapsulated
  packets coming *from* its tunnel destination, so this one falls through to
  a raw `IPPROTO_IPIP` (or `41` for IPv6 inner) socket in mibwg.
- **mibwg -> kernel.** mibwg writes the inner packet under an outer header
  `TunPeer -> TunLocal` with `IP_HDRINCL`. gif9 claims it, decapsulates, and
  the inner packet is received on gif9 like any other.

Code: `internal/backend/gif_qnx.go` (device, setup/teardown, routes, endpoint
pinning), `internal/backend/ipip.go` (framing, unit-tested on any OS).

## What QEMU established (the risk list, answered)

| Question | Answer |
|---|---|
| Loopback tunnel endpoints accepted? | Yes, 127/8 and lo0 aliases alike |
| Outbound reaches `rip_input`? | Yes |
| Inbound `IP_HDRINCL` accepted? | Yes - but **not with a 127/8 outer source**: gif's input validation drops it as a martian. Hence the 10.255.255.1/.2 lo0 aliases |
| Raw header byte order | Output: network order works. Input: 4.4BSD mangling - `ip_len` arrives in host order minus the header length, so the read size is used, not the field |
| `route` on a p2p gif | `route add [-inet6] -net X -interface <tunnel addr>`; `ifconfig gif9 destroy` removes every route through it |

Tools on the unit: `ifconfig` is `/usr/sbin/ifconfig`, `route` is
`/armle/sbin/route` (not on the default PATH); the backends search the usual
directories. Spawning them needed a Go-port runtime fix - new threads now
publish g to TLS before running (qnx65-armv7-toolchain `go/README.md`,
"bad g in signal handler").

Harness notes (tests/qemu-net.sh): `pipe` must run for `os/exec` (pipes are
a resource manager on QNX), and the image needs `[data=copy]` - mkifs's
default `data=uip` lets each boot-image executable start only once.

## Full tunnel (AllowedIPs = 0.0.0.0/0)

- Routed as `0.0.0.0/1` + `128.0.0.0/1`, so the uplink's own default stays
  and `route -n get default` still names the real gateway.
- The halves go in **only after** the endpoint is pinned: a host route to the
  server via the uplink gateway. Otherwise resolving the endpoint's name would
  need the tunnel it is about to bring up.
- Re-pinned every 30 s as the uplink changes (tether, hotspot, LTE).
- **Fails open:** with no default route/gateway, the halves are removed and
  traffic goes direct instead of into a dead tunnel. The car keeps working;
  traffic leaves outside the VPN meanwhile.
- Loop guard: packets to the endpoint that still reach gif are dropped
  instead of re-encrypted.

## Still to verify on a unit

1. **pf** - checked on an MHI2q: the active ruleset only names physical
   interfaces (`uap0`, `mlan0`, `ecm0/1`, `en0`, `carplay0`, `mmx0`) and pf
   passes by default, so `lo0`, `gif*` and `tun*` traffic is untouched. The
   `dns_data`/`mmi_data` anchors on `ecm0` already carry WireGuard's UDP in
   netstack mode.
2. Tun backend end to end, then full tunnel: endpoint pinning against the unit's real gateway, DNS
   (dnsmasq upstream through the tunnel), and CarPlay/Android Auto still
   working with a default route via gif.
3. Throughput: every packet crosses io-pkt twice.

## Alternative: a real tun driver

io-pkt's driver API headers ship in the sysroot (`usr/include/io-pkt/`). A
`devnp-tun.so` is the clean fix, but a driver bug takes down the unit's whole
network stack; gif needs no kernel code.

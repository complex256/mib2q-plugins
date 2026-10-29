# mib2q-tailscale

A full Tailscale node on an Audi MHI2q (MIB2) head unit: `QNX mmx 6.5.0
APQ8064 armle`, QNX 6.5.0 on ARMv7.

`tailscaled` runs on the unit as an ordinary tailnet member, routing through a
real kernel `tun` device rather than userspace-networking. It replaces
[mib2q-wireguard](../mib2q-wireguard)'s `mibwg` on the unit; mibwg stays
installed and one line in `startup.sh` switches back.

**Status:** running on a unit. All three goals verified on hardware
(2026-09-21). One piece of the design is unfinished: the unit itself can't
resolve MagicDNS names outbound - see [Known gaps](#known-gaps).

| Goal | How it was verified |
|---|---|
| Reachable from the tailnet | `ssh root@<node's tailscale IP>` from a laptop on the tailnet |
| The unit reaches the tailnet | plain TCP dial from the unit to a peer's Tailscale IP, kernel-routed, no proxy |
| Survives reboots | cold boot with no human action: node online, `tun9` up with its address |

## Layout

Nothing upstream is vendored. `scripts/fetch.sh` clones the pinned sources
into `build/`, and `scripts/build.sh` lays this repo's files over them.

| Path | What it is |
|---|---|
| `scripts/fetch.sh` | clones tailscale v1.102.4, `tailscale/wireguard-go`, go-isatty into `build/` |
| `scripts/build.sh` | resets the fetched trees, applies overlays and patches, builds all three binaries in Docker, enforces the size gate |
| `overlay/` | new files dropped into the tailscale tree (`router_qnx.go`, `manager_qnx.go`) |
| `overlay-wireguard-go/` | `tun/tun_qnx.go` (`CreateTUN` over `qnxtun`) and `ipc/uapi_qnx.go` |
| `overlay-isatty/` | `isatty_qnx.go` |
| `patches/` | the two edits that must change existing upstream lines rather than add files |
| `qnxplat/` | the platform logic worth testing on a host: route planning, dnsmasq config, tool lookup |
| `cmd/sntp/` | clock *reporter* (`-print`); the RTC boots at 1970 and control-plane TLS fails without a correction |
| `cmd/tsstatus/` | status sidecar: polls tailscaled's LocalAPI and serves the result on `127.0.0.1:19850` (see [Status endpoint](#status-endpoint)) |
| `qnxplat/clockshim/` | `libtsclock.so`, LD_PRELOADed into tailscaled so only it sees corrected time |
| `cmd/tunsmoke/` | standalone tun read/write smoke test |
| `deploy/` | `tailscale.sh` control script, its host test `tailscale_test.sh`, and the `startup.sh` autostart line |

`qnxplat` holds the decisions, so they can be tested without a QNX box:
`PlanRoutes` turns a `router.Config` diff into `ifconfig`/`route` argv, and
`v4Set` drops IPv6 and - deliberately, permanently - any prefix with
`Bits() == 0`. A default route through the tunnel once captured the car's
Wi-Fi hotspot traffic, whose NAT only applies on the uplink, and killed
internet for everything on it. **This node never installs a default route.**

## Building

Requires Docker and the `qnx65-armv7-toolchain:8.5-go127` image (see
[qnx65-armv7-toolchain](../qnx65-armv7-toolchain); `QNX_IMAGE` overrides it),
which carries the `GOOS=qnx GOARCH=arm` Go 1.27.1 port. The older `8.5-go`
image (Go 1.26.4) cannot build the pinned release, whose `go.mod` wants 1.26.6.

```sh
./scripts/fetch.sh
./scripts/build.sh          # -> build/tailscaled, build/tailscale, build/sntp, build/tsstatus, build/libtsclock.so
```

Expect this to take a while: the toolchain image is amd64, so on an Apple
Silicon host every compiler invocation runs under qemu-x86_64 user-mode
emulation. `build.sh` wraps the build in `timeout 1800` because that emulation
has a deadlock mode where the `compile` child exits and the driver waits
forever on a zombie.

Two constraints the build enforces:

- **Version pin.** Tailscale **v1.102.4**, a release rather than `main`. Its
  `go.mod` wants `go 1.26.6` and pins `tailscale/wireguard-go 2e01ba5b00f0`,
  whose `tun.Device` still takes `Read([][]byte, []int, int)` - the shape
  `overlay-wireguard-go/tun/tun_qnx.go` implements. The newer slab/`ReadPacket`
  API is on upstream `main` only (wireguard-go `383e864`); **no released tag
  carries it**, so adopting it would mean shipping unreleased code to the car.
  The port to that API is kept on the `wireguard-readpacket-api` branch until a
  release ships it.
- **Size gate.** The unit's loader fails to exec large binaries; ~21 MB failed
  and 15,941,857 bytes ran, so the gate is 16 MiB (the observations are listed
  in `scripts/build.sh`). The deployed `tailscaled` is 15,634,657 bytes and
  `tailscale` 8,863,969. There is not much room left - `TS_TAGS` overrides the
  omit-tag list if you need to buy some.

`ts_omit_netstack` looks like an easy 2.6 MB, since we have a real tun. It
isn't: `tstun.invertGSOChecksum` panics `unreachable` on the first injected
packet, because upstream assumes injected packets only happen when netstack is
present and doesn't guard that path on the tag. It killed `tailscaled` on the
unit the moment anything used the tunnel.

## Deploying

`/mnt/app` is mounted read-only, so make it writable first, copy, then put it
back:

```sh
ssh audi 'mount -uw /mnt/app'
scp build/tailscaled build/tailscale build/sntp build/tsstatus build/libtsclock.so deploy/tailscale.sh audi:/mnt/app/root/tailscale/
ssh audi 'mount -ur /mnt/app'
```

Then on the unit:

```sh
/mnt/app/root/tailscale/tailscale.sh start | stop | restart | status | log
/mnt/app/root/tailscale/tailscale.sh debug on [DIR [MOUNTPOINT]] | off | status | snapshot
```

Authenticate headlessly by writing a pre-auth key to
`/mnt/persist/tailscale/authkey` (mode 600; gitignored here, never in the
repo). `ts_start` passes it to `tailscale up` when present. Without one,
`tailscale.sh start` leaves the node logged out and `tailscale.sh status`
prints a `Log in at: https://login.tailscale.com/a/...` URL to approve.
`tailscale up` passes no `--hostname`, so a name set in the admin console
sticks across restarts. The first start after a boot prints `cannot open
/tmp/<binary>` once per file: that is the size check on the not-yet-copied
RAM copy, and it is harmless.

**`--accept-routes` and the unit's LAN.** `tailscale up` accepts subnet
routes by default; that is how the car reaches a home broker on 192.168.x
through the tailnet's subnet router. If an advertised range also covers the
hosts that talk to the unit over its LAN (for example a `192.168.0.0/16` home
route while the bench laptop and Pi are on 192.168.x), the unit answers them
through the tunnel and drops off its LAN once logged in (seen on the bench
unit 2026-10-25; it stays reachable at its tailnet address). Such a unit opts
out on its own flash, without changing the default:

```sh
echo 0 >/mnt/persist/tailscale/accept-routes     # next start: --accept-routes=false
/mnt/app/root/tailscale/tailscale.sh restart      # or live, no restart:
/tmp/tailscale --socket=/tmp/tailscaled.sock set --accept-routes=false
```

`0`, `accept-routes=0`, `false`, `no` and `off` turn routes off; anything else,
or no file, keeps them on. `TS_ACCEPT_ROUTES` overrides the file. With routes
off the unit no longer reaches anything behind a subnet router, only tailnet
nodes and its own LAN.

Autostart is one line in `/mnt/system/etc/boot/startup.sh`, inside
`start_late_drivers()` right after `inetd`:

```sh
( sleep 30; [ -x /mnt/app/root/tailscale/tailscale.sh ] && /mnt/app/root/tailscale/tailscale.sh start ) </dev/null >/dev/null 2>&1 &
```

Detached, no stdio, guarded by `-x`: boot continues immediately and a missing
or crashing `tailscaled` changes nothing about the boot. On a unit that also
runs mibpkg, `inetd` is followed by mibpkg's own two-line hook; put this line
directly after `inetd` (above mibpkg's `##` comment), which leaves mibpkg's
anchor and its removal untouched. **Verify any edit to
this file with `ksh -n` before rebooting** - a syntax error here is a bricked
unit, not a failed service. `startup.sh.pre-mibwg` (pristine) and
`startup.sh.mibwg` (the WireGuard variant) sit beside it as rollbacks.

## Status endpoint

`tsstatus` (`cmd/tsstatus`) is a small sidecar that `tailscale.sh` starts and
stops with `tailscaled` (pid in `/tmp/tsstatus.pid`; optional - a deploy without
the binary still brings the node up). Every 10 s it asks the LocalAPI on
`/tmp/tailscaled.sock` for `/localapi/v0/status` (5 s timeout) and caches a
reduced result. A TCP client on `127.0.0.1:19850` is sent the cache and the
connection is closed:

```
v=1
daemon=tailscale
state=<BackendState>|down
ip=<first TailscaleIP or empty>
tailnet=<CurrentTailnet.Name or empty>
peers=<n>
peers_online=<n>
last_handshake_age_s=<min over peers>|-1
age_s=<seconds since the cached poll>
```

`state=down` means the socket was unreachable or the poll timed out. All times
are ages in seconds, because the unit's clock sits at 1970 and only
`tailscaled` sees corrected time. The handshake age therefore uses
`time.Now()` plus the offset in `/tmp/ts_clock_offset`; a handshake that would
be in the future (offset not learned yet) reads as `-1`. It imports nothing from
tailscale: the reply is decoded into a local struct of just these fields.

```sh
go test ./cmd/tsstatus/ && bash deploy/tailscale_test.sh
```

## Debug mode: a watchdog and a log that survives power-off

Off by default, and off means off: without the flag, `tailscale.sh` behaves
exactly as before - no watchdog, nothing written outside `/tmp`. It exists to
catch the intermittent boot hang (see [Known gaps](#known-gaps)) in the act,
since `/tmp` is RAM and the evidence is gone the moment the car powers down.

```sh
/mnt/app/root/tailscale/tailscale.sh debug on            # first writable of /fs/sda0, /fs/sdb0
/mnt/app/root/tailscale/tailscale.sh debug on /fs/sdb0/tailscale-debug /fs/sdb0
/mnt/app/root/tailscale/tailscale.sh restart             # applies it now; otherwise next boot
/mnt/app/root/tailscale/tailscale.sh debug status        # flag, watchdog, card, last events
/mnt/app/root/tailscale/tailscale.sh debug off
```

The flag is `TS_DEBUG=1` in `/mnt/persist/tailscale/debug.conf`, so it
survives a redeploy of `/mnt/app` and no install ever turns it on. The file is
sourced; any `TS_DEBUG_*`/`TS_WD_*` knob at the top of `tailscale.sh` can go
in it. If the card isn't writable it is remounted with `mount -uw` and left
that way. Both SD slots are FAT32 at `/fs/sda0` and `/fs/sdb0`; navigation
lives on eMMC (`/mnt/navdb`), so neither is the map card.

With it on:

- **Watchdog.** A shell loop, outside tailscaled on purpose: in the known
  wedge the Go scheduler is what's stuck, so nothing inside the process can
  notice. Every 30 s it asks the LocalAPI for status with a 20 s timeout - a
  wedged daemon's socket still accepts, so "the CLI never finished" is the
  signal. Two misses in a row is a wedge: it records `pidin` thread states and
  CPU times (twice, 3 s apart - the wedge is all threads in `SEM`, none in
  `REPLY`, CPU barely moving), sends SIGQUIT for a full goroutine dump
  (`GOTRACEBACK=system`, so runtime frames are in it), and restarts
  tailscaled. An unexplained exit is captured and restarted too. At most 3
  restarts per boot (`TS_WD_MAX_RESTARTS`; `TS_WD_RESTART=0` captures only).
- **A record per boot on the card**, `/fs/sdX0/tailscale-debug/NNNNN/` (or
  `DIR/NNNNN/`), numbered by a counter in flash, newest 30 kept:
  `events.log` (the timeline), `tailscaled.<n>.log` for every tailscaled
  started that boot (a restart never loses one), `capture.<n>.txt` for each
  wedge or death, and `bootinfo.txt` (`pidin info`, mounts, interfaces,
  process list). Everything is written to RAM first and copied over at each
  event and every 5 minutes, so a card that mounts late or not at all can
  never hold up a start.
- **Boot vs. sleep.** Events carry the unit's uptime (its clock is never
  stepped, so `ls -l` of a fresh file reads 1970 + uptime) and a
  `boot.instance` number. A start with no marker in `/tmp` is logged as a
  real power-on. A 30 s watchdog sleep that took far longer is logged as
  "suspended and resumed without a reboot" - which is how to tell whether
  stop/restart-the-car is a reboot at all.

`debug snapshot` saves the same `pidin` capture on demand without touching
tailscaled. `deploy/tailscale_test.sh` runs all of this on the host against
fake binaries.

## What the unit is like

Hard-won, and the reason several things here look odd:

- **Never step the unit's clock while the HMI is starting.** The RTC boots at
  1970 (there is no usable RTC chip - `rtc hw` reports "Unable to determine RTC
  chip type"), but setting `CLOCK_REALTIME` expires every pending realtime wait
  on the box at once. In the HMI that fires `OperationManager`'s
  `NavInitializationChecker`, which concludes navigation never initialised and
  restarts the whole navigation domain about a minute into every boot -
  navigation is already fully operable by then. Measured across four boots: the
  timer fired within a second of the step each time, once only 48 ms after it,
  while the gap between navigation becoming ready and the timer firing varied
  from 7.6 s to 26 s. So `sntp -print` only *reports* the offset and
  `libtsclock.so` applies it inside `tailscaled` alone.

  The checker is only armed around nav initialisation: a step at ~5 minutes
  uptime caused **no** restart at all. A delayed step (set the clock once, a few
  minutes in) would therefore give a correct system clock *and* no nav restart,
  and is the recommended next experiment - leaving the clock at 1970 has its own
  costs for anything else on the unit that wants wall time. See
  `mib2q-hmi-lab/docs/unit-findings.md` for the full evidence and for two
  chronic firmware faults (an IconProviderApp crash loop and an MMPM log flood)
  that are **not** related to this and predate it.

- **`poll(2)` never reports `/dev/tunN` readable.** Reads must block. A
  poll-driven reader sees nothing while io-pkt's `Opkts` counter climbs, and
  traffic dies silently in one direction with no error anywhere.
- **`tun` reads return `EHOSTDOWN` (errno 264) until the interface has an
  address.** `qnxtun.Read` retries rather than treating it as fatal; without
  that, `tailscaled` dies seconds after start.
- **`/tmp` is RAM (`/dev/shmem`) and the only exec-capable filesystem**, so
  binaries are copied there at start. It's also why the log is truncated each
  run - nothing else bounds it.
- **`rename(2)` fails on `/dev/shmem`**, so no write-temp-then-rename.
- **`grep` is POSIX, not GNU.** `grep "a\|b"` is a GNU extension: here it
  matches nothing at all, silently, which reads exactly like "the thing you
  searched for never happened". Use `grep -E 'a|b'`. Assume the same of every
  other tool's GNU-only flags.
- **Missing binaries:** no `dirname`, `tail`, `nohup`, `date`, `kill`, `df`,
  `tr`, `wc` or `cksum`. `deploy/tailscale.sh` works around each one it
  needs. Assume any coreutil is absent until you have watched it run; to
  compare a file against a local copy, `scp` it back and diff here.
- **A failed redirection on `:` exits the whole shell.** `:` is a special
  builtin, so `: >file` on a read-only filesystem kills the script (or the
  `$(...)` it's in) rather than returning non-zero. Wrap it: `( : >file )`.
  This bit for real: the SD cards (`/fs/sda0`, `/fs/sdb0`, FAT32) come up
  **read-only at every boot** and need `mount -uw`.
- **`ksh` predefines `stop` as an alias** for `kill -STOP`, and aliases expand
  inside function definitions - so every function in the control script is
  prefixed `ts_`.
- **No `nohup`:** closing the ssh session SIGHUPs `tailscaled`. The script
  `trap '' HUP` in a subshell and `exec`s, so the disposition survives into
  `tailscaled` and `$!` is still its real pid.

## Known gaps

- **Intermittent hang on boot - root cause found 2026-09-26, fix not yet
  verified on the unit.** It was never the router's exec path. It is a Go
  scheduler deadlock caused by this repo's own clock shim.

  `runtime.semasleep` (`os_qnx.go`) builds an **absolute CLOCK_REALTIME**
  deadline for `sem_timedwait` - the only clock QNX's `sem_timedwait` accepts -
  by calling libc `clock_gettime`, which is exactly the symbol
  `libtsclock.so` interposes. The deadline therefore left the process shifted
  forward by the offset (~1.79e9 s) while procnto judged it against the real,
  unshifted clock, so every deadline landed ~56.7 years out and **no timed park
  ever timed out.** The one that matters is `stopTheWorldWithSema`, which
  re-preempts straggler Ps via `notetsleep(&sched.stopnote, 100*1000)`; that
  100 us retry never fired, so any GC stop-the-world with a straggler P never
  completed and the process froze permanently - wherever GC happened to land,
  which is why the stall point moved between boots and looked intermittent.

  Evidence: a SIGQUIT dump of a live wedge shows the stop-the-world signature -
  `[runnable]` goroutines that nobody runs, **no M in `poll()`**, one goroutine
  `_Grunning` on a parked M, sysmon alive, ~4 ms of CPU per minute. `GOGC=off`
  with the shim still active ran clean (tun configured, login, netmap, live
  peers, 1.149 s CPU) where the same binary froze at 0.065 s. The control was
  already in the logs: the run just before it had offset 0, so the shim was
  inert, and it ran fine for 90 s.

  Two corrections to what this section used to claim. The stall was **not**
  inside `exec()` of the second `route`: that reading came from the log's last
  line, but the `ifconfig` child had in fact completed (`tun9` was
  `UP,POINTOPOINT,RUNNING` with its address) and no child process or exec pipe
  remained. And the `cmdTimeout`/`cmdAttempts` mitigation cannot ever catch
  this - firing a Go timer needs the scheduler, which is the wedged thing, so a
  real occurrence logs no `attempt 1/3 timed out` line. Keep it, since it is
  still right for a genuinely slow child, but it is not the fix.

  **Fix:** `libtsclock.so` now also interposes `sem_timedwait` and shifts the
  absolute deadline back before passing it to libc, so the process and the
  kernel agree again (`qnxplat/clockshim/tsclock.c`). `tsclock_test.sh` covers
  it, including the unsigned-`time_t` wrap that the QNX compiler's
  `-Wtype-limits` warning exposed - on QNX 6.5 `time_t` is unsigned, so doing
  the subtraction in `time_t` wraps to ~136 years out and reintroduces the
  freeze.

  **Verified on the unit 2026-09-28/29.** Cold boot with the rebuilt shim:
  `tun9` `UP,POINTOPOINT,RUNNING` with its address (so the router reconfigure
  that used to wedge completed), 11 threads with one in `REPLY`, CPU climbing
  past 2.3 s, live log - netcheck reports, wireguard keepalives, magicsock
  disco. Compare a wedge: 9 threads all in `SEM`, nothing in `REPLY`, 0.065 s
  of CPU, log stopped dead. The boot also restarted tailscaled once, but that
  was the routine sntp path (first attempt lost the DNS race, background
  attempt 1 learned the offset), and `.prev` ends in an orderly `ts_stop`, not
  a freeze - note the first instance runs with offset 0, so only the second
  instance is a real test of the fix.

  **Two clean boots is encouraging, not conclusive.** The freeze needed a GC
  stop-the-world to land on a straggler P, so it was probabilistic: at the old
  ~2-in-3 failure rate, two clean boots in a row happen by luck about 11% of
  the time. The stronger evidence is that the offset-carrying instance has now
  completed many GC cycles (minutes of uptime, seconds of CPU) where the bug
  froze the process within 0.065 s.

  Still open upstream of this: `semasleep` lets a userspace `LD_PRELOAD`
  corrupt the scheduler at all, and its `mp.ts.tv_sec = int32(sec)` truncates.
  That truncation is **not** a practical risk here - it overflows in January
  2038, and QNX 6.5's own 32-bit `time_t` breaks system-wide at the same
  instant, so this line is not the binding constraint. (An earlier revision of
  this section said "~11% below the ceiling", which was wrong: with the offset
  applied `tv_sec` reads ~1.79e9 against an `int32` ceiling of ~2.15e9, so
  ~17% of the range is left, which is the ~11.3 years to 2038. The 11 was the
  years, not the percent.) What it does mean today is that a deliberately large
  synthetic `TS_CLOCK_OFFSET` - a test fixture, say - wraps immediately and
  silently, so keep test offsets small.

  The cleaner end state is to drop the shim entirely via a delayed clock step
  past the nav-init window, which deletes this whole class of bug.


- **MagicDNS names resolve, and needed no work.** Tailscale publishes
  `<node>.<tailnet>.ts.net` in public DNS, so the unit's ordinary resolver
  finds them: `ping <peer>.<tailnet>.ts.net` answers on that peer's
  100.x address. `net/dns/manager_qnx.go` is a deliberate logged no-op and
  `tailscale up` runs `--accept-dns=false`; neither needs to change.
  What is *not* configured is the 100.100.100.100 resolver, which would
  matter only if tailnet connectivity outlived public DNS. An attempt to
  point dnsmasq at it from `start_network()` was reverted: the include came
  back empty at boot, and the feature it bought was one we already had. If
  you pick this up, know that something outside `startup.sh` manages
  `/tmp/dnsmasq.sdis.conf` - the script only touches it.
- **IPv6 inside the tunnel** is unimplemented: the Go port has no `ioctl`
  binding for `TUNSIFHEAD`, so the QNX tun is IPv4-only. IPv6 entries in a
  `router.Config` are skipped with a log line.
- Not built, by design: exit node / subnet router, Tailscale SSH, Taildrop,
  Serve, the web client.

## Docs

- [Design spec](docs/superpowers/specs/2026-09-20-tailscale-qnx-node-design.md)
- [Implementation plan](docs/superpowers/plans/2026-09-20-tailscale-qnx-node.md)

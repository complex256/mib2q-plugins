# mib2q-wireguard

**WireGuard for the Audi/VW MHI2q (MIB2) head unit**: QNX 6.5.0, ARMv7
(`QNX mmx 6.5.0 APQ8064 armle`).

`mibwg` is one static-ish Go binary (~9 MB stripped) built on
[wireguard-go](https://git.zx2c4.com/wireguard-go) and cross-compiled with the
`GOOS=qnx` port from [qnx65-armv7-toolchain](../qnx65-armv7-toolchain). Its
config file is standard wg-quick format, so the server is any ordinary
WireGuard peer.

**Disclaimer:** Use at your own risk. You're running your own code on a car's
infotainment unit. Back up anything you change.

## Why it's not "just wg-quick"

QNX 6.5 has no wg-quick, and the SDK's network stack (`io-pkt-v6-hc`) has no
tun device, so the default backend makes no assumptions: the tunnel ends in a
**userspace TCP/IP stack** (gVisor netstack) inside mibwg, which bridges it
to the unit:

```
             ┌──────────────────────── mibwg ─────────────────────────┐
 server ◄─UDP─► wireguard-go ◄─► netstack (10.66.0.2)                  │
             │                     ▲   │                              │
             │         [Forward]   │   │   [Proxy]                    │
             │  tunnel:22 ─────────┘   └──── SOCKS5 / HTTP ◄───────── programs on
             │        │                                               │ the unit / hotspot
             └────────┼───────────────────────────────────────────────┘
                      ▼
               127.0.0.1:22 (unit's sshd, telnet, ...)
```

- **Remote access** (`[Forward]`): connections to the unit's tunnel address
  are relayed to services on the unit, e.g. `ssh root@10.66.0.2` from anywhere
  on your WireGuard network. TCP and UDP.
- **Outbound** (`[Proxy]`): SOCKS5 and HTTP proxies send traffic from the
  unit, or from phones on its Wi-Fi hotspot, out through the tunnel.
- **Clock** (`[Time]`): the RTC boots at 1970, and WireGuard servers drop
  handshakes older than the last one they saw. mibwg sets the clock via SNTP.
- **Roaming**: endpoints are resolved in the background (the unit boots
  offline) and re-resolved when handshakes stop (for dynamic-DNS servers).

Stock head-unit software won't use the proxy on its own. For that there are
kernel-routed backends, which make the kernel route AllowedIPs into the
tunnel for every program on the unit:

- `Backend = tun`: io-pkt's tun device. The MHI2q's io-pkt has one (the SDK's
  doesn't); IPv4 only for now. Its device was verified on a unit; the backend
  itself hasn't run on one yet.
- `Backend = gif`: gif(4) IP-in-IP over lo0 plus raw sockets, for io-pkt
  builds without tun. Verified under QEMU.

See [docs/gif-backend.md](docs/gif-backend.md).

## Build

Needs the toolchain image from `../qnx65-armv7-toolchain`:

```sh
(cd ../qnx65-armv7-toolchain && host-scripts/qnx-run.sh build 8.5-go)
scripts/build.sh                 # -> build/mibwg  (ELF 32-bit ARM, interp /usr/lib/ldqnx.so.2)
```

No Docker: build the port's Go natively once (`cp -r ../qnx65-armv7-toolchain/go
/tmp/goqnx && cd /tmp/goqnx/src && ./make.bash`), then
`QNX_GOROOT=/tmp/goqnx scripts/build.sh`. mibwg links internally
(`CGO_ENABLED=0`), so no QNX gcc is involved.

## Set up

```sh
go run ./cmd/mibwg genkey > unit.key            # or: wg genkey
go run ./cmd/mibwg pubkey < unit.key            # -> unit public key for the server
cp deploy/mibwg.conf.example mibwg.conf         # fill in keys, addresses, endpoint
go run ./cmd/mibwg check mibwg.conf
```

Server side (any WireGuard implementation):

```ini
[Peer]
# MHI2q
PublicKey = <unit public key>
AllowedIPs = 10.66.0.2/32
```

## Deploy

Everything lives in `/mnt/app/root/wireguard/` (`mibwg`, `mibwg.sh`,
`mibwg.conf`). `/mnt/app` is mounted read-only; remount it only for writes:

```sh
mount -uw /mnt/app
mkdir -p /mnt/app/root/wireguard
cp mibwg mibwg.sh mibwg.conf /mnt/app/root/wireguard/
chmod 755 /mnt/app/root/wireguard/mibwg /mnt/app/root/wireguard/mibwg.sh
chmod 600 /mnt/app/root/wireguard/mibwg.conf
sync; mount -ur /mnt/app
```

Day to day, over SSH:

```sh
W=/mnt/app/root/wireguard
$W/mibwg.sh status     # pid + peer handshake/transfer
$W/mibwg.sh log
$W/mibwg.sh edit       # vi the config (remounts rw), validate, restart if valid
$W/mibwg.sh restart
```

`edit` never restarts on an invalid config: the running tunnel keeps the old
settings until the file passes `mibwg check`.

### Autostart

One line in `/mnt/system/etc/boot/startup.sh`, right after `inetd` starts
(the point where `sshd` becomes reachable). The exact text is in
[deploy/startup-hook.txt](deploy/startup-hook.txt):

```sh
( sleep 30; [ -x /mnt/app/root/wireguard/mibwg.sh ] && /mnt/app/root/wireguard/mibwg.sh start ) </dev/null >/dev/null 2>&1 &
```

It's a detached background subshell with no stdio, so boot continues
immediately. If mibwg is missing or fails, nothing happens. The original is
kept as `startup.sh.pre-mibwg`. To roll back:

```sh
mount -uw /mnt/system
cp /mnt/system/etc/boot/startup.sh.pre-mibwg /mnt/system/etc/boot/startup.sh
sync; mount -ur /mnt/system
```

(At boot, `/etc/boot/startup.sh` is the same file as
`/mnt/system/etc/boot/startup.sh`; `/etc` only moves to a ramdisk in
software-update mode.)

### Unit quirks the script handles

- Persistent storage isn't mmap-capable for exec, so binaries run from `/tmp`
  (`/dev/shmem`, RAM, cleared between sessions). `start` re-copies each time.
- Log and status go to `/tmp` by default (no flash wear). Set `MIBWG_LOG`
  for a persistent log.
- The userland is much sparser than the SDP sysroot: no `dirname`, `tail`,
  `nohup`, `date`, `df` or `kill` binary. The script uses only ksh builtins
  plus `cat cp chmod rm sed sleep mount sync vi`. `mibwg up` ignores SIGHUP
  itself, so it keeps running after the SSH session that started it ends.
- ksh predefines aliases such as `stop='kill -STOP'`, and aliases expand even
  in function definitions. That's why the script's functions are `mibwg_*`.
- `rename(2)` fails on `/dev/shmem` (plain `mv` works), so the status file is
  written in place.
- Run `sync` and wait a few seconds after copying files before any forced
  reboot, or the copies can be truncated.

## Verified on the unit

On a real MHI2q (`QNX mmx 6.5.0 APQ8064 armle`), over an LTE uplink:

- the clock was set from 1970 by SNTP
- the endpoint hostname resolved, and the handshake completed with the server
- `curl` through the unit's SOCKS5 and HTTP proxies reported the *server's*
  public IP, while a direct request reported the car's carrier IP

## Test

```sh
go test ./...                    # on the host: config + full end-to-end
scripts/build.sh test            # build the same tests for QNX
QEMU_TOOLS=route tests/qemu-net.sh build/daemon.test -test.v   # ...on QNX 6.5 in QEMU
```

On QNX the daemon tests include the gif backend (`TestEndToEndGif`): plain
sockets on the unit reach the server with no proxy, the server reaches a
plain `0.0.0.0` listener on the unit, and teardown leaves no interface or
route behind. `tests/gifprobe` is the standalone feasibility probe.

The end-to-end test runs a complete mibwg against a second WireGuard peer over
real UDP on loopback. It checks both `[Forward]` directions (TCP and UDP), both
proxies, that the server sees the unit's tunnel address as the source, and
peer status. It only uses loopback, so the same binary runs on QNX under QEMU
(`tests/qemu-net.sh` boots real `io-pkt-v6-hc`) or on the unit itself.

## Layout

| Path | |
|---|---|
| `cmd/mibwg/` | CLI: `up`, `check`, `genkey`, `pubkey` |
| `internal/config/` | wg-quick parser + mibwg sections; renders the WireGuard UAPI config |
| `internal/daemon/` | assembles a running tunnel; endpoint resolution, clock loop, status file |
| `internal/backend/` | where tunnel packets terminate: `Netstack`, or kernel-routed `Tun` / `Gif` (QNX) |
| `internal/forward/` | `[Forward]`: tunnel port -> unit service (TCP, UDP) |
| `internal/proxy/` | `[Proxy]`: SOCKS5, HTTP CONNECT / absolute-URI |
| `internal/sntp/` | SNTP client + `ClockSettime` on QNX |
| `third_party/` | wireguard-go and x/sys with small QNX patches ([list](third_party/README.md)) |
| `deploy/` | on-unit control script, example config |
| `scripts/build.sh` | cross-build in the toolchain image |
| `tests/qemu-net.sh` | run a QNX binary under QEMU with io-pkt up |
| `tests/gifprobe/` | standalone gif feasibility probe (`-setup` configures gif itself) |
| `tests/tunprobe/` | tun device probe for a unit (creates and destroys `tun9`) |

## Tailscale

The toolchain's Go port has run `tailscaled` on this unit, with the same
netstack approach and the same kind of wireguard-go/x/sys patches as here. But
tailscaled only fits under the unit's ~15 MB exec ceiling with trimming, and
needs the same clock handling. Plain WireGuard is a third of the size and
works with any server.

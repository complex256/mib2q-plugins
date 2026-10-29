# Tailscale QNX Node Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run upstream `tailscaled` as a real Tailscale node on the Audi MHI2q head unit (QNX 6.5, ARMv7), using a kernel tun device, so any program on the unit reaches the tailnet and the unit is reachable from it.

**Architecture:** Pin Tailscale v1.100.0 and fetch it at build time. Our own module holds the QNX platform logic as pure, host-testable functions (`qnxplat`); thin adapter files are added into the fetched tree to register that logic through upstream's existing hooks. The tun device itself moves into a shared `qnxtun` package in `mib2q-wireguard`, used by both mibwg and a patched wireguard-go.

**Tech Stack:** Go 1.26.4 (`GOOS=qnx GOARCH=arm`, the port in `qnx65-armv7-toolchain/go`), Tailscale v1.100.0, wireguard-go (vendored in `mib2q-wireguard/third_party`), Docker image `qnx65-armv7-toolchain:8.5-go`, QEMU for smoke tests, the unit for acceptance.

**Spec:** `docs/superpowers/specs/2026-09-20-tailscale-qnx-node-design.md`

## Global Constraints

- **Go toolchain:** the QNX port only. Build inside `qnx65-armv7-toolchain:8.5-go`, or with `QNX_GOROOT` pointing at a native `make.bash` build of `qnx65-armv7-toolchain/go`. Never the host's stock Go for QNX targets.
- **Build flags, every QNX binary:** `GOOS=qnx GOARCH=arm GOARM=7 CGO_ENABLED=0`, `-trimpath`, `-ldflags "-s -w -B none -buildid="`. `-B none -buildid=` is load-bearing: QNX's loader chokes on Go's build-id NOTE phdr placed before `.text`.
- **Size ceiling:** every binary that must run on the unit is **under 15 MB**. 32-bit QNX `exec` fails above roughly that.
- **Never install a default route.** Only tailnet CGNAT space (`100.64.0.0/10`) and peer routes. A full tunnel already broke the unit's Wi-Fi hotspot NAT once.
- **IPv4 only inside the tunnel.** The QNX tun device carries IPv4; IPv6 config entries are skipped with a log line, never attempted.
- **Unit tool paths** (not on the default PATH): `ifconfig` = `/usr/sbin/ifconfig`, `route` = `/armle/sbin/route`, `netstat` = `/armle/usr/bin/netstat`, `ping` = `/armle/usr/bin/ping`. Search `/sbin /usr/sbin /bin /usr/bin /armle/sbin /armle/bin /armle/usr/sbin /armle/usr/bin /proc/boot`.
- **Unit shell constraints:** no `dirname`, `tail`, `nohup`, `date`, `kill` binary, `df`, `tr`. ksh predefines aliases (`stop='kill -STOP'`), so shell functions are prefixed (`ts_start`, not `start`). Available: `cat cp chmod rm sed sleep mount sync vi ksh slay pidin head`.
- **Exec from `/tmp` only** (`/dev/shmem`, RAM, cleared between sessions). Persistent code in `/mnt/app/root/tailscale/` (remount `/mnt/app` rw to write); persistent state in `/mnt/persist/tailscale/` (writable without remount).
- **Secrets:** the pre-auth key lives only on the unit at `/mnt/persist/tailscale/authkey`, `chmod 600`. Never commit it; `.gitignore` covers `authkey` and `*.key`.
- **Authorship:** commits as `complex256 <me@complex256.dev>`, ending with:
  `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`
- **Honesty rule for this plan:** QEMU cannot test tun (the SDK's io-pkt has no tun cloner) and has no internet. A task whose acceptance needs the tailnet says so and is verified on the unit (`ssh audi`), not faked in QEMU.

## File Structure

**`mib2q-wireguard` (existing repo, additive):**
- `qnxtun/qnxtun.go` — NEW. Importable QNX tun device: create/open/read/write/close. Build-tagged `qnx`.
- `qnxtun/qnxtun_other.go` — NEW. Non-QNX stub so the package compiles everywhere.
- `qnxtun/name.go` — NEW. Pure helpers (device path, name validation), no build tag, host-testable.
- `qnxtun/name_test.go` — NEW. Host tests for the pure helpers.
- `internal/backend/tun_qnx.go` — MODIFY. Use `qnxtun` instead of its own open/read/write.
- `third_party/wireguard-go/tun/tun_qnx.go` — NEW. `CreateTUN` over `qnxtun`.

**`mib2q-tailscale` (new repo):**
- `qnxplat/router.go` — NEW. Pure route/address planning + an exec runner.
- `qnxplat/router_test.go` — NEW. Host tests for planning.
- `qnxplat/dns.go` — NEW. Pure rendering of the dnsmasq line and hosts entries + apply.
- `qnxplat/dns_test.go` — NEW. Host tests for rendering.
- `qnxplat/tool.go` — NEW. Tool lookup shared by router and dns.
- `qnxplat/tool_test.go` — NEW. Host tests for lookup order.
- `overlay/wgengine/router/osrouter/router_qnx.go` — NEW. Thin adapter: registers `qnxplat` with `HookNewUserspaceRouter`.
- `overlay/net/dns/manager_qnx.go` — NEW. Thin adapter implementing `dns.OSConfigurator` over `qnxplat`.
- `patches/0001-dns-manager-default-exclude-qnx.patch` — NEW. One build-tag edit upstream.
- `scripts/build.sh` — NEW. Fetch, overlay, patch, build, size-gate.
- `scripts/fetch.sh` — NEW. Shallow clone of the pinned tag.
- `cmd/tunsmoke/main.go` — NEW. On-unit smoke test for the wireguard-go tun patch.
- `cmd/sntp/main.go` — NEW. Clock setter reusing mibwg's `internal/sntp` logic.
- `deploy/tailscale.sh` — NEW. Unit control script.
- `deploy/startup-hook.txt` — NEW. The autostart line.
- `README.md` — NEW. Build, deploy, verify, rollback.

---

### Task 1: Extract the QNX tun device into a shared `qnxtun` package

**Files:**
- Create: `mib2q-wireguard/qnxtun/name.go`
- Create: `mib2q-wireguard/qnxtun/name_test.go`
- Create: `mib2q-wireguard/qnxtun/qnxtun.go`
- Create: `mib2q-wireguard/qnxtun/qnxtun_other.go`
- Modify: `mib2q-wireguard/internal/backend/tun_qnx.go`

**Interfaces:**
- Consumes: nothing.
- Produces: `qnxtun.DevPath(name string) string`; `qnxtun.ValidName(name string) error`; `qnxtun.Open(name string) (*qnxtun.Device, error)`; `(*Device).Read([]byte) (int, error)`; `(*Device).Write([]byte) (int, error)`; `(*Device).Close() error`; `(*Device).Fd() int`.

- [ ] **Step 1: Write the failing test**

Create `mib2q-wireguard/qnxtun/name_test.go`:

```go
package qnxtun

import "testing"

func TestDevPath(t *testing.T) {
	if got := DevPath("tun9"); got != "/dev/tun9" {
		t.Errorf("DevPath = %q, want /dev/tun9", got)
	}
}

func TestValidName(t *testing.T) {
	for _, ok := range []string{"tun0", "tun9", "tun42"} {
		if err := ValidName(ok); err != nil {
			t.Errorf("ValidName(%q) = %v, want nil", ok, err)
		}
	}
	for _, bad := range []string{"", "tun", "gif9", "tun9/../etc", "tun9 x", "TUN9"} {
		if err := ValidName(bad); err == nil {
			t.Errorf("ValidName(%q) = nil, want error", bad)
		}
	}
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/complex-projects/mib2q-wireguard && go test ./qnxtun/`
Expected: FAIL — `undefined: DevPath`, `undefined: ValidName`.

- [ ] **Step 3: Write minimal implementation**

Create `mib2q-wireguard/qnxtun/name.go`:

```go
// Package qnxtun is io-pkt's tun(4) as a packet device, for QNX 6.5 units
// whose io-pkt has the tun cloner (`ifconfig -C` lists it; the SDK's does
// not). Shared by mibwg's tun backend and the patched wireguard-go.
package qnxtun

import "fmt"

// DevPath is the character device for an interface name.
func DevPath(name string) string { return "/dev/" + name }

// ValidName accepts "tun" followed by digits - the only names io-pkt's tun
// cloner creates, and the only ones safe to paste into a device path.
func ValidName(name string) error {
	if len(name) < 4 || name[:3] != "tun" {
		return fmt.Errorf("tun name %q: want tunN", name)
	}
	for _, c := range name[3:] {
		if c < '0' || c > '9' {
			return fmt.Errorf("tun name %q: want tunN", name)
		}
	}
	return nil
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ~/complex-projects/mib2q-wireguard && go test ./qnxtun/`
Expected: PASS.

- [ ] **Step 5: Add the QNX device, moving the logic out of the backend**

Create `mib2q-wireguard/qnxtun/qnxtun.go`:

```go
//go:build qnx

package qnxtun

import (
	"fmt"
	"sync"
	"syscall"
)

// Device is an open /dev/tunN. Safe for one reader and one writer
// concurrently, which is how wireguard-go uses a tun.
type Device struct {
	fd     int
	name   string
	closeM sync.Mutex
	closed bool
}

// Open opens the device for an interface that already exists.
func Open(name string) (*Device, error) {
	if err := ValidName(name); err != nil {
		return nil, err
	}
	fd, err := syscall.Open(DevPath(name), syscall.O_RDWR, 0)
	if err != nil {
		return nil, fmt.Errorf("open %s: %w", DevPath(name), err)
	}
	return &Device{fd: fd, name: name}, nil
}

// Read returns one bare IPv4 packet (no address-family header).
//
// It blocks. poll(2) never reports /dev/tunN readable on QNX: io-pkt queues
// the packet and counts it in the interface's Opkts, but the poll wakeup
// never comes, so a poll-driven reader reads nothing at all and traffic dies
// silently in one direction. Close unblocks this by closing the fd.
func (d *Device) Read(p []byte) (int, error) { return syscall.Read(d.fd, p) }

// Write injects one bare IPv4 packet.
func (d *Device) Write(p []byte) (int, error) { return syscall.Write(d.fd, p) }

func (d *Device) Fd() int      { return d.fd }
func (d *Device) Name() string { return d.name }

func (d *Device) Close() error {
	d.closeM.Lock()
	defer d.closeM.Unlock()
	if d.closed {
		return nil
	}
	d.closed = true
	return syscall.Close(d.fd)
}
```

Create `mib2q-wireguard/qnxtun/qnxtun_other.go`:

```go
//go:build !qnx

package qnxtun

import "errors"

// Device exists on every platform so callers compile; only QNX can open one.
type Device struct{}

func Open(string) (*Device, error)      { return nil, errors.New("qnxtun: QNX only") }
func (*Device) Read([]byte) (int, error)  { return 0, errors.New("qnxtun: QNX only") }
func (*Device) Write([]byte) (int, error) { return 0, errors.New("qnxtun: QNX only") }
func (*Device) Close() error              { return nil }
func (*Device) Fd() int                   { return -1 }
func (*Device) Name() string              { return "" }
```

- [ ] **Step 6: Point mibwg's backend at the package**

In `mib2q-wireguard/internal/backend/tun_qnx.go`: replace the `syscall.Open` in `setup` with `qnxtun.Open(TunIface)` storing a `*qnxtun.Device`, replace `syscall.Read(t.fd, buf)` in `reader` with `t.dev.Read(buf)`, replace `syscall.Write(t.fd, p)` in `Write` with `t.dev.Write(p)`, and replace `syscall.Close(t.fd)` in `Close` with `t.dev.Close()`. Keep the blocking-read comment with the code that blocks. Import `github.com/complex256/mib2q-wireguard/qnxtun`; drop `syscall` if now unused.

- [ ] **Step 7: Verify both targets still build and host tests pass**

Run:
```bash
cd ~/complex-projects/mib2q-wireguard
go vet ./... && go test ./...
G=$SCRATCH/goqnx   # the extracted qnx/arm GOROOT (see Task 1)
GOROOT=$G GOTOOLCHAIN=local GOOS=qnx GOARCH=arm $G/bin/go vet ./...
```
Expected: all PASS, both vets clean.

- [ ] **Step 8: Verify on the unit that mibwg's tun backend still works**

Run:
```bash
cd ~/complex-projects/mib2q-wireguard && scripts/build.sh test
scp build/daemon.test audi:/tmp/daemon.test
ssh audi 'chmod 755 /tmp/daemon.test && cd /tmp && ./daemon.test -test.run TestEndToEndTun -test.v -test.timeout=120s'
```
Expected: `PASS`, including `teardown`. This is the only proof the extraction didn't break the device — QEMU has no tun.

- [ ] **Step 9: Commit**

```bash
cd ~/complex-projects/mib2q-wireguard
git add qnxtun internal/backend/tun_qnx.go
git commit -m "qnxtun: extract the QNX tun device into an importable package

The blocking-read behaviour (poll never reports /dev/tunN readable) now
lives in one place, so the patched wireguard-go that tailscaled builds
against can use the same code as mibwg's backend.

TestEndToEndTun still passes on the unit.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Teach wireguard-go to create a QNX tun, and smoke-test it on the unit

**Files:**
- Create: `mib2q-wireguard/third_party/wireguard-go/tun/tun_qnx.go`
- Modify: `mib2q-wireguard/third_party/README.md`
- Create: `mib2q-tailscale/cmd/tunsmoke/main.go`
- Create: `mib2q-tailscale/go.mod`

**Interfaces:**
- Consumes: `qnxtun.Open`, `qnxtun.ValidName`, `qnxtun.DevPath` (Task 1).
- Produces: `tun.CreateTUN(name string, mtu int) (tun.Device, error)` for `GOOS=qnx`, satisfying wireguard-go's `tun.Device` (`Read(bufs [][]byte, sizes []int, offset int)`, `Write(bufs [][]byte, offset int)`, `MTU`, `Name`, `Events`, `Close`, `BatchSize`, `File`).

- [ ] **Step 1: Write the QNX CreateTUN**

Create `mib2q-wireguard/third_party/wireguard-go/tun/tun_qnx.go`:

```go
//go:build qnx

package tun

import (
	"fmt"
	"os"
	"os/exec"
	"sync"

	"github.com/complex256/mib2q-wireguard/qnxtun"
)

// CreateTUN creates interface `name` with io-pkt's tun cloner and opens its
// device. IPv4 only: without the TUNSIFHEAD ioctl (the QNX Go port has no
// ioctl binding) tun treats every written packet as AF_INET.
func CreateTUN(name string, mtu int) (Device, error) {
	if err := qnxtun.ValidName(name); err != nil {
		return nil, err
	}
	ifconfig := lookupTool("ifconfig")
	exec.Command(ifconfig, name, "destroy").Run() // leftover from a crashed run
	if out, err := exec.Command(ifconfig, name, "create").CombinedOutput(); err != nil {
		return nil, fmt.Errorf("ifconfig %s create: %v: %s (does this io-pkt have a tun cloner? `ifconfig -C`)", name, err, out)
	}
	if out, err := exec.Command(ifconfig, name, "mtu", fmt.Sprint(mtu)).CombinedOutput(); err != nil {
		exec.Command(ifconfig, name, "destroy").Run()
		return nil, fmt.Errorf("ifconfig %s mtu: %v: %s", name, err, out)
	}
	dev, err := qnxtun.Open(name)
	if err != nil {
		exec.Command(ifconfig, name, "destroy").Run()
		return nil, err
	}
	t := &qnxDevice{dev: dev, name: name, mtu: mtu, events: make(chan Event, 4)}
	t.events <- EventUp
	return t, nil
}

func lookupTool(name string) string {
	for _, d := range []string{"/sbin", "/usr/sbin", "/bin", "/usr/bin", "/armle/sbin", "/armle/bin", "/armle/usr/sbin", "/armle/usr/bin", "/proc/boot"} {
		p := d + "/" + name
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	return name
}

type qnxDevice struct {
	dev       *qnxtun.Device
	name      string
	mtu       int
	events    chan Event
	closeOnce sync.Once
}

func (t *qnxDevice) Read(bufs [][]byte, sizes []int, offset int) (int, error) {
	n, err := t.dev.Read(bufs[0][offset:])
	if err != nil {
		return 0, err
	}
	sizes[0] = n
	return 1, nil
}

func (t *qnxDevice) Write(bufs [][]byte, offset int) (int, error) {
	for i, b := range bufs {
		p := b[offset:]
		if len(p) == 0 || p[0]>>4 != 4 {
			continue // IPv6 can't be expressed without TUNSIFHEAD
		}
		if _, err := t.dev.Write(p); err != nil {
			return i, err
		}
	}
	return len(bufs), nil
}

func (t *qnxDevice) MTU() (int, error)     { return t.mtu, nil }
func (t *qnxDevice) Name() (string, error) { return t.name, nil }
func (t *qnxDevice) Events() <-chan Event  { return t.events }
func (t *qnxDevice) BatchSize() int        { return 1 }
func (t *qnxDevice) File() *os.File        { return nil }

func (t *qnxDevice) Close() error {
	var err error
	t.closeOnce.Do(func() {
		err = t.dev.Close()
		exec.Command(lookupTool("ifconfig"), t.name, "destroy").Run()
		close(t.events)
	})
	return err
}
```

- [ ] **Step 2: Create the new repo's module and the smoke test**

Create `mib2q-tailscale/go.mod`:

```
module github.com/complex256/mib2q-tailscale

go 1.26.4

require (
	github.com/complex256/mib2q-wireguard v0.0.0
	golang.zx2c4.com/wireguard v0.0.0
)

replace github.com/complex256/mib2q-wireguard => ../mib2q-wireguard

replace golang.zx2c4.com/wireguard => ../mib2q-wireguard/third_party/wireguard-go

replace golang.org/x/sys => ../mib2q-wireguard/third_party/x-sys
```

Create `mib2q-tailscale/cmd/tunsmoke/main.go`:

```go
// tunsmoke proves wireguard-go's QNX CreateTUN works on a unit: create tun9,
// address it, push a packet out, read it back off the device, then clean up.
// QEMU can't run this - the SDK's io-pkt has no tun cloner.
package main

import (
	"fmt"
	"net"
	"os"
	"os/exec"
	"strings"
	"time"

	"golang.zx2c4.com/wireguard/tun"
)

const iface = "tun9"

func main() {
	dev, err := tun.CreateTUN(iface, 1420)
	if err != nil {
		fmt.Println("FAIL CreateTUN:", err)
		os.Exit(1)
	}
	defer dev.Close()
	fmt.Println("ok   CreateTUN", iface)

	if out, err := exec.Command("/usr/sbin/ifconfig", iface, "10.99.0.1", "10.99.0.2", "up").CombinedOutput(); err != nil {
		fmt.Printf("FAIL ifconfig: %v: %s\n", err, out)
		os.Exit(1)
	}

	got := make(chan string, 4)
	go func() {
		bufs := [][]byte{make([]byte, 2048)}
		sizes := make([]int, 1)
		for {
			n, err := dev.Read(bufs, sizes, 0)
			if err != nil {
				return
			}
			if n > 0 {
				got <- string(bufs[0][:sizes[0]])
			}
		}
	}()

	c, err := net.Dial("udp4", "10.99.0.2:7000")
	if err != nil {
		fmt.Println("FAIL dial:", err)
		os.Exit(1)
	}
	c.Write([]byte("hello-tun"))
	select {
	case p := <-got:
		if !strings.Contains(p, "hello-tun") {
			fmt.Println("FAIL packet did not contain the payload")
			os.Exit(1)
		}
		fmt.Printf("PASS read %d bytes off %s containing the payload\n", len(p), iface)
	case <-time.After(5 * time.Second):
		fmt.Println("FAIL nothing read from the device")
		os.Exit(1)
	}
	fmt.Println("wireguard-go QNX tun: OK")
}
```

- [ ] **Step 3: Build the smoke test for QNX**

Run:
```bash
cd ~/complex-projects/mib2q-tailscale
docker run --rm --platform=linux/amd64 -v "$PWD":/src -v ~/complex-projects/mib2q-wireguard:/mib2q-wireguard \
  -w /src -e GOOS=qnx -e GOARCH=arm -e GOARM=7 -e CGO_ENABLED=0 -e GOFLAGS=-buildvcs=false \
  qnx65-armv7-toolchain:8.5-go go build -trimpath -ldflags='-s -w -B none -buildid=' -o build/tunsmoke ./cmd/tunsmoke
```
Expected: builds. If the mounted relative `replace` path fails, run the build with both repos under one mounted parent (`-v ~/complex-projects:/src -w /src/mib2q-tailscale`) and record that in `scripts/build.sh`.

- [ ] **Step 4: Run it on the unit**

Run:
```bash
scp build/tunsmoke audi:/tmp/tunsmoke
ssh audi 'chmod 755 /tmp/tunsmoke && /tmp/tunsmoke; echo "exit=$?"; ifconfig tun9 2>&1 | head -1'
```
Expected: `PASS read ... containing the payload`, `wireguard-go QNX tun: OK`, `exit=0`, and `tun9` gone afterwards (`No such device`).

- [ ] **Step 5: Document the patch**

Add a row to the `wireguard-go` table in `mib2q-wireguard/third_party/README.md`:
`| tun/tun_qnx.go (new) | CreateTUN over the shared qnxtun package, so tailscaled gets a kernel tun on QNX. IPv4 only (no TUNSIFHEAD ioctl in the Go port). |`

- [ ] **Step 6: Commit both repos**

```bash
cd ~/complex-projects/mib2q-wireguard
git add third_party/wireguard-go/tun/tun_qnx.go third_party/README.md
git commit -m "wireguard-go: CreateTUN for QNX over qnxtun

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
cd ~/complex-projects/mib2q-tailscale
git add go.mod cmd/tunsmoke
git commit -m "tunsmoke: prove wireguard-go's QNX tun on a unit

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Build pipeline — fetch, overlay, compile tailscaled for QNX

**Files:**
- Create: `mib2q-tailscale/scripts/fetch.sh`
- Create: `mib2q-tailscale/scripts/build.sh`
- Create: `mib2q-tailscale/.gitignore` (if absent: `/build/`, `authkey`, `*.key`)

**Interfaces:**
- Consumes: the `replace` targets from Task 2's `go.mod`.
- Produces: `scripts/build.sh` leaving `build/tailscaled` and `build/tailscale`; `scripts/fetch.sh` leaving a checkout at `build/tailscale-src` pinned to `TS_VERSION` (default `v1.100.0`).

- [ ] **Step 1: Write the fetch script**

Create `mib2q-tailscale/scripts/fetch.sh`:

```sh
#!/bin/sh
# Fetch the pinned Tailscale source. Not vendored: the tree is ~200 MB and we
# only add a handful of files to it.
#
# v1.100.0's go.mod requires exactly go 1.26.4 - the QNX port's version.
# Upstream main wants 1.27.1, which the port does not have yet.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TS_VERSION=${TS_VERSION:-v1.100.0}
SRC=$ROOT/build/tailscale-src

if [ -d "$SRC/.git" ] && [ "$(git -C "$SRC" describe --tags 2>/dev/null)" = "$TS_VERSION" ]; then
    echo "tailscale $TS_VERSION already fetched"
    exit 0
fi
rm -rf "$SRC"
mkdir -p "$(dirname "$SRC")"
git clone -q --depth 1 --branch "$TS_VERSION" https://github.com/tailscale/tailscale.git "$SRC"
echo "fetched tailscale $TS_VERSION -> $SRC"
grep -m1 '^go ' "$SRC/go.mod"
```

- [ ] **Step 2: Write the build script**

Create `mib2q-tailscale/scripts/build.sh`:

```sh
#!/bin/sh
# Build tailscaled + tailscale for the MHI2q (QNX 6.5 armle-v7).
#
#   scripts/build.sh
#
# Fetches the pinned source, copies our overlay files in, applies patches,
# points the module at our QNX forks, and builds in the toolchain image.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PARENT=$(cd "$ROOT/.." && pwd)
SRC=$ROOT/build/tailscale-src
IMG=${QNX_IMAGE:-qnx65-armv7-toolchain:8.5-go}
MAXBYTES=15728640   # 15 MiB: 32-bit QNX exec fails above roughly this

sh "$ROOT/scripts/fetch.sh"

# Overlay: our platform files land inside the fetched tree at the same paths.
( cd "$ROOT/overlay" && find . -type f -name '*.go' ) | while read -r f; do
    mkdir -p "$SRC/$(dirname "$f")"
    cp "$ROOT/overlay/$f" "$SRC/$f"
done

# Patches: edits to upstream files (build tags and the like).
for p in "$ROOT"/patches/*.patch; do
    [ -e "$p" ] || continue
    git -C "$SRC" apply --check "$p" 2>/dev/null && git -C "$SRC" apply "$p" && echo "applied $(basename "$p")"
done

# Our module + the QNX forks of wireguard-go and x/sys.
( cd "$SRC"
  go mod edit -require=github.com/complex256/mib2q-tailscale@v0.0.0 \
      -replace=github.com/complex256/mib2q-tailscale=/work/mib2q-tailscale \
      -replace=github.com/complex256/mib2q-wireguard=/work/mib2q-wireguard \
      -replace=golang.zx2c4.com/wireguard=/work/mib2q-wireguard/third_party/wireguard-go \
      -replace=golang.org/x/sys=/work/mib2q-wireguard/third_party/x-sys )

# gVisor goes with ts_omit_netstack: we have a real tun, and it is the
# biggest single chunk of the binary.
TAGS=${TS_TAGS:-ts_omit_netstack,ts_omit_ssh,ts_omit_drive,ts_omit_taildrop,ts_omit_webclient,ts_omit_serve,ts_omit_kube,ts_omit_tpm,ts_omit_systray,ts_omit_capture,ts_omit_aws,ts_omit_bird,ts_omit_relayserver}

docker run --rm --platform=linux/amd64 \
    -v "$PARENT":/work -w /work/mib2q-tailscale/build/tailscale-src \
    -v tailscale-gomod:/root/go/pkg/mod \
    -e GOOS=qnx -e GOARCH=arm -e GOARM=7 -e CGO_ENABLED=0 -e GOTOOLCHAIN=local \
    -e GOFLAGS=-buildvcs=false \
    "$IMG" sh -c "
        go mod tidy >/dev/null 2>&1 || true
        go build -tags '$TAGS' -trimpath -ldflags='-s -w -B none -buildid=' -o ../tailscaled ./cmd/tailscaled &&
        go build -tags '$TAGS' -trimpath -ldflags='-s -w -B none -buildid=' -o ../tailscale  ./cmd/tailscale"

for b in tailscaled tailscale; do
    sz=$(wc -c < "$ROOT/build/$b")
    printf '%-12s %s bytes\n' "$b" "$sz"
    if [ "$sz" -gt "$MAXBYTES" ]; then
        echo "ERROR: $b is over the unit's ~15 MB exec ceiling" >&2
        exit 1
    fi
done
echo "built for qnx/arm; exec ceiling OK"
```

- [ ] **Step 3: Run the build and record what breaks**

Run: `cd ~/complex-projects/mib2q-tailscale && chmod +x scripts/*.sh && scripts/build.sh 2>&1 | tail -40`
Expected: a list of compile errors for `GOOS=qnx` (missing router, DNS, or netmon pieces). **This step is expected to fail** — its deliverable is the error list, which Tasks 4 and 5 fix. Save it: `scripts/build.sh > build/first-build.log 2>&1 || true`.

- [ ] **Step 4: Fix only the mechanical breakage**

For each error that is *not* the router or DNS (Tasks 4-5), add the smallest fix:
- a missing build-tagged file for `qnx` → add it under `overlay/` at the same path;
- an upstream file whose build tag excludes `qnx` wrongly → a patch in `patches/`.
Re-run `scripts/build.sh` after each. Do not stub anything that would silently do nothing at runtime: if a piece genuinely needs implementing, note it and let Task 4/5 own it.

- [ ] **Step 5: Commit the pipeline plus whatever mechanical fixes were needed**

```bash
cd ~/complex-projects/mib2q-tailscale
git add scripts patches overlay .gitignore
git commit -m "build: fetch, overlay and compile tailscale v1.100.0 for qnx/arm

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3 addendum (ruled during execution, 2026-09-20)

Tailscale v1.100.0 does not import `golang.zx2c4.com/wireguard`; it imports
its own fork, `github.com/tailscale/wireguard-go`, pinned in its `go.mod`.
The `replace` in this plan therefore pointed at a module the build never
loads, and Task 2's QNX `CreateTUN` was invisible to it.

Task 3 also covers, before the build can reach the router and DNS layers:

1. Fetch `github.com/tailscale/wireguard-go` at the version tailscale's
   `go.mod` pins, overlay a QNX `tun/tun_qnx.go` (same shape as Task 2's,
   reusing the shared `qnxtun` package) plus whatever its `ipc` package needs
   for `qnx`, and `replace` to it.
2. Add `unix.Access` and `unix.O_RDWR` to `mib2q-wireguard/third_party/x-sys`.
3. Give `github.com/mattn/go-isatty` a `qnx` implementation (`IsTerminal` =
   false) through the same fetch-and-overlay mechanism.

---

### Task 4: QNX router

**Files:**
- Create: `mib2q-tailscale/qnxplat/tool.go`
- Create: `mib2q-tailscale/qnxplat/tool_test.go`
- Create: `mib2q-tailscale/qnxplat/router.go`
- Create: `mib2q-tailscale/qnxplat/router_test.go`
- Create: `mib2q-tailscale/overlay/wgengine/router/osrouter/router_qnx.go`

**Interfaces:**
- Consumes: `router.NewOpts`, `router.Router`, `router.Config` (upstream, shown below); `tun.Device.Name()`.
- Produces: `qnxplat.Tool(name string) string`; `qnxplat.PlanRoutes(iface string, oldCfg, newCfg qnxplat.RouteConfig) [][]string`; `qnxplat.RouteConfig{Addrs, Routes []netip.Prefix}`; `qnxplat.Apply(cmds [][]string, logf func(string, ...any)) error`.

Upstream types this task must satisfy (do not redefine):

```go
type Router interface {
	Up() error
	Set(*Config) error
	Close() error
}
type NewOpts struct {
	Logf   logger.Logf
	Tun    tun.Device
	NetMon *netmon.Monitor
	Health *health.Tracker
	Bus    *eventbus.Bus
}
type Config struct {
	LocalAddrs  []netip.Prefix
	Routes      []netip.Prefix
	LocalRoutes []netip.Prefix
	NewMTU      int
	// ... Linux-only fields, ignored here
}
var HookNewUserspaceRouter feature.Hook[func(NewOpts) (Router, error)]
```

- [ ] **Step 1: Write the failing tests**

Create `mib2q-tailscale/qnxplat/router_test.go`:

```go
package qnxplat

import (
	"net/netip"
	"strings"
	"testing"
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

func TestPlanRoutesFirstConfig(t *testing.T) {
	cmds := PlanRoutes("tun9", RouteConfig{}, RouteConfig{
		Addrs:  pfx("100.101.102.103/32"),
		Routes: pfx("100.64.0.0/10", "100.99.1.2/32"),
	})
	has(t, cmds, "ifconfig tun9 inet 100.101.102.103 100.101.102.103 netmask 255.255.255.255")
	has(t, cmds, "route add -inet -net 100.64.0.0/10 -interface 100.101.102.103")
	has(t, cmds, "route add -inet -net 100.99.1.2/32 -interface 100.101.102.103")
}

func TestPlanRoutesIsIncremental(t *testing.T) {
	old := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10", "100.99.1.2/32")}
	new := RouteConfig{Addrs: pfx("100.101.102.103/32"), Routes: pfx("100.64.0.0/10", "100.99.9.9/32")}
	cmds := PlanRoutes("tun9", old, new)
	has(t, cmds, "route delete -inet -net 100.99.1.2/32")
	has(t, cmds, "route add -inet -net 100.99.9.9/32 -interface 100.101.102.103")
	hasNo(t, cmds, "100.64.0.0/10") // unchanged: not touched
	hasNo(t, cmds, "ifconfig")      // address unchanged
}

func TestPlanRoutesSkipsIPv6AndDefault(t *testing.T) {
	cmds := PlanRoutes("tun9", RouteConfig{}, RouteConfig{
		Addrs:  pfx("100.101.102.103/32", "fd7a:115c:a1e0::1/128"),
		Routes: pfx("100.64.0.0/10", "0.0.0.0/0", "::/0"),
	})
	hasNo(t, cmds, "fd7a")   // IPv6 address skipped
	hasNo(t, cmds, "::/0")   // IPv6 default skipped
	hasNo(t, cmds, "0.0.0.0/0") // never a default route: it broke the unit's hotspot NAT
}
```

Create `mib2q-tailscale/qnxplat/tool_test.go`:

```go
package qnxplat

import "testing"

func TestToolFallsBackToBareName(t *testing.T) {
	if got := Tool("definitely-not-a-real-tool"); got != "definitely-not-a-real-tool" {
		t.Errorf("Tool = %q, want the bare name when nothing is found", got)
	}
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd ~/complex-projects/mib2q-tailscale && go test ./qnxplat/`
Expected: FAIL — `undefined: PlanRoutes`, `undefined: RouteConfig`, `undefined: Tool`.

- [ ] **Step 3: Implement the pure planner and the runner**

Create `mib2q-tailscale/qnxplat/tool.go`:

```go
// Package qnxplat holds the QNX platform logic tailscaled needs: routing and
// DNS. The logic lives here, in a normal testable module; the files copied
// into the Tailscale tree are thin adapters that call it.
package qnxplat

import "os"

// toolDirs covers both the unit (ifconfig in /usr/sbin, route in
// /armle/sbin, neither on the default PATH) and the QEMU image
// (everything in /proc/boot).
var toolDirs = []string{"/sbin", "/usr/sbin", "/bin", "/usr/bin", "/armle/sbin", "/armle/bin", "/armle/usr/sbin", "/armle/usr/bin", "/proc/boot"}

// Tool returns the full path of a system tool, or the bare name if not found.
func Tool(name string) string {
	for _, d := range toolDirs {
		p := d + "/" + name
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	return name
}
```

Create `mib2q-tailscale/qnxplat/router.go`:

```go
package qnxplat

import (
	"fmt"
	"net/netip"
	"os/exec"
	"strings"
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
func PlanRoutes(iface string, oldCfg, newCfg RouteConfig) [][]string {
	var cmds [][]string
	oldAddrs, newAddrs := v4Set(oldCfg.Addrs), v4Set(newCfg.Addrs)
	gw := primary(newCfg.Addrs)

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
			cmds = append(cmds, []string{"ifconfig", iface, "inet", a.Addr().String(), "delete"})
		}
	}
	if !gw.IsValid() {
		return cmds
	}
	oldRoutes, newRoutes := v4Set(oldCfg.Routes), v4Set(newCfg.Routes)
	for r := range oldRoutes {
		if !newRoutes[r] {
			cmds = append(cmds, []string{"route", "delete", "-inet", "-net", r.String()})
		}
	}
	for r := range newRoutes {
		if !oldRoutes[r] {
			cmds = append(cmds, []string{"route", "add", "-inet", "-net", r.String(), "-interface", gw.String()})
		}
	}
	return cmds
}

// v4Set keeps the IPv4 prefixes, dropping IPv6 and any default route.
func v4Set(in []netip.Prefix) map[netip.Prefix]bool {
	out := map[netip.Prefix]bool{}
	for _, p := range in {
		p = p.Masked()
		if !p.Addr().Is4() || p.Bits() == 0 {
			continue
		}
		out[p] = true
	}
	return out
}

func primary(addrs []netip.Prefix) netip.Addr {
	for _, p := range addrs {
		if p.Addr().Is4() {
			return p.Addr()
		}
	}
	return netip.Addr{}
}

// Apply runs planned commands, logging each. A failing command is logged and
// the rest still run: a stale route left behind must not stop the node from
// coming up.
func Apply(cmds [][]string, logf func(string, ...any)) error {
	var failed []string
	for _, c := range cmds {
		out, err := exec.Command(Tool(c[0]), c[1:]...).CombinedOutput()
		if err != nil {
			logf("qnx router: %s: %v: %s", strings.Join(c, " "), err, strings.TrimSpace(string(out)))
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd ~/complex-projects/mib2q-tailscale && go test ./qnxplat/ -v`
Expected: PASS for `TestPlanRoutesFirstConfig`, `TestPlanRoutesIsIncremental`, `TestPlanRoutesSkipsIPv6AndDefault`, `TestToolFallsBackToBareName`.

- [ ] **Step 5: Write the adapter that registers with upstream**

Create `mib2q-tailscale/overlay/wgengine/router/osrouter/router_qnx.go`:

```go
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

func (r *qnxRouter) Up() error { return nil } // CreateTUN already brought the interface up

func (r *qnxRouter) Set(cfg *router.Config) error {
	next := qnxplat.RouteConfig{}
	if cfg != nil {
		next = qnxplat.RouteConfig{Addrs: cfg.LocalAddrs, Routes: cfg.Routes}
	}
	cmds := qnxplat.PlanRoutes(r.iface, r.cur, next)
	err := qnxplat.Apply(cmds, r.logf)
	r.cur = next
	return err
}

func (r *qnxRouter) Close() error {
	// Destroying the interface (wireguard-go's tun Close) drops its routes,
	// so only the addresses need explicit removal here.
	cmds := qnxplat.PlanRoutes(r.iface, r.cur, qnxplat.RouteConfig{})
	err := qnxplat.Apply(cmds, r.logf)
	r.cur = qnxplat.RouteConfig{}
	return err
}
```

- [ ] **Step 6: Build for QNX**

Run: `cd ~/complex-projects/mib2q-tailscale && scripts/build.sh 2>&1 | tail -20`
Expected: the router errors from Task 3's log are gone. DNS errors may remain (Task 5).

- [ ] **Step 7: Commit**

```bash
cd ~/complex-projects/mib2q-tailscale
git add qnxplat overlay/wgengine
git commit -m "qnxplat: router for tailscaled (addresses + routes via ifconfig/route)

Incremental: plans only the difference against the previous config. IPv4
only, and never a default route - a full tunnel takes down the NAT the
unit's Wi-Fi hotspot depends on.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: MagicDNS through the unit's dnsmasq

**Files:**
- Create: `mib2q-tailscale/qnxplat/dns.go`
- Create: `mib2q-tailscale/qnxplat/dns_test.go`
- Create: `mib2q-tailscale/overlay/net/dns/manager_qnx.go`
- Create: `mib2q-tailscale/patches/0001-dns-manager-default-exclude-qnx.patch`

**Interfaces:**
- Consumes: `dns.OSConfigurator`, `dns.OSConfig{Hosts []*HostEntry, Nameservers []netip.Addr, SearchDomains, MatchDomains []dnsname.FQDN}` (upstream).
- Produces: `qnxplat.DnsmasqConf(cfg qnxplat.DNSConfig) string`; `qnxplat.HostsBlock(cfg qnxplat.DNSConfig) string`; `qnxplat.DNSConfig{Nameservers []netip.Addr, MatchDomains []string, Hosts map[string][]netip.Addr}`; `qnxplat.WriteDnsmasqInclude(path, contents string) (changed bool, err error)`; `qnxplat.ReloadDnsmasq(logf func(string, ...any)) error`.

- [ ] **Step 1: Write the failing tests**

Create `mib2q-tailscale/qnxplat/dns_test.go`:

```go
package qnxplat

import (
	"net/netip"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDnsmasqConfSplitDNS(t *testing.T) {
	got := DnsmasqConf(DNSConfig{
		Nameservers:  []netip.Addr{netip.MustParseAddr("100.100.100.100")},
		MatchDomains: []string{"tail1234.ts.net"},
	})
	want := "server=/tail1234.ts.net/100.100.100.100\n"
	if !strings.Contains(got, want) {
		t.Errorf("DnsmasqConf = %q, want it to contain %q", got, want)
	}
}

func TestDnsmasqConfEmptyWhenNoMatchDomains(t *testing.T) {
	// No MatchDomains means Tailscale wants to be the *primary* resolver.
	// Doing that here would break the unit's DNS whenever tailscaled is down,
	// so it is deliberately not configured.
	if got := DnsmasqConf(DNSConfig{Nameservers: []netip.Addr{netip.MustParseAddr("100.100.100.100")}}); strings.Contains(got, "server=") {
		t.Errorf("DnsmasqConf = %q, want no server= line without MatchDomains", got)
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd ~/complex-projects/mib2q-tailscale && go test ./qnxplat/ -run TestDnsmasq -v`
Expected: FAIL — `undefined: DnsmasqConf`, `undefined: DNSConfig`, etc.

- [ ] **Step 3: Implement**

Create `mib2q-tailscale/qnxplat/dns.go`:

```go
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

// DnsmasqConf renders the include file: one split-DNS server line per
// matched domain, pointing at Tailscale's resolver.
//
// With no MatchDomains, Tailscale is asking to be the primary resolver for
// everything. We deliberately render nothing then: pointing the unit's only
// resolver at 100.100.100.100 breaks all DNS whenever tailscaled is down,
// and the unit needs DNS for navigation and online services.
func DnsmasqConf(cfg DNSConfig) string {
	if len(cfg.MatchDomains) == 0 || len(cfg.Nameservers) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteString("# written by mib2q-tailscale; recreated each boot\n")
	doms := append([]string(nil), cfg.MatchDomains...)
	sort.Strings(doms)
	for _, d := range doms {
		d = strings.TrimSuffix(d, ".")
		for _, ns := range cfg.Nameservers {
			if ns.Is4() {
				fmt.Fprintf(&b, "server=/%s/%s\n", d, ns)
			}
		}
	}
	return b.String()
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
	if old, err := os.ReadFile(path); err == nil && string(old) == contents {
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
		return fmt.Errorf("slay -s SIGHUP dnsmasq: %v: %s", err, strings.TrimSpace(string(out)))
	}
	logf("qnx dns: SIGHUP sent to dnsmasq (re-reads /etc/hosts; config needs a restart)")
	return nil
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd ~/complex-projects/mib2q-tailscale && go test ./qnxplat/ -v`
Expected: all PASS.

- [ ] **Step 5: Write the OSConfigurator adapter**

Create `mib2q-tailscale/overlay/net/dns/manager_qnx.go`:

```go
//go:build qnx

package dns

import (
	"net/netip"

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
	logf logger.Logf
}

func (m *qnxManager) SetDNS(cfg OSConfig) error {
	c := qnxplat.DNSConfig{Nameservers: cfg.Nameservers, Hosts: map[string][]netip.Addr{}}
	for _, d := range cfg.MatchDomains {
		c.MatchDomains = append(c.MatchDomains, d.WithoutTrailingDot())
	}
	// HostEntry is {Addr netip.Addr; Hosts []string} - one address, many names.
	for _, h := range cfg.Hosts {
		for _, name := range h.Hosts {
			c.Hosts[name] = append(c.Hosts[name], h.Addr)
		}
	}
	changed, err := qnxplat.WriteDnsmasqInclude(qnxplat.DnsmasqInclude, qnxplat.DnsmasqConf(c))
	if err != nil {
		return err
	}
	if changed {
		m.logf("qnx dns: wrote %s (%d match domain(s))", qnxplat.DnsmasqInclude, len(c.MatchDomains))
		return qnxplat.ReloadDnsmasq(m.logf)
	}
	return nil
}

func (m *qnxManager) SupportsSplitDNS() bool { return true }

func (m *qnxManager) GetBaseConfig() (OSConfig, error) { return OSConfig{}, ErrGetBaseConfigNotSupported }

func (m *qnxManager) Close() error {
	if _, err := qnxplat.WriteDnsmasqInclude(qnxplat.DnsmasqInclude, ""); err != nil {
		return err
	}
	return qnxplat.ReloadDnsmasq(m.logf)
}
```

- [ ] **Step 6: Patch the default manager's build tag to exclude qnx**

Create `mib2q-tailscale/patches/0001-dns-manager-default-exclude-qnx.patch` by editing the file in the fetched tree and capturing the diff:

```bash
cd ~/complex-projects/mib2q-tailscale/build/tailscale-src
sed -i '' 's|^//go:build (!linux \|\| android) && !freebsd|//go:build (!linux \|\| android) \&\& !qnx \&\& !freebsd|' net/dns/manager_default.go
git diff net/dns/manager_default.go > ../../patches/0001-dns-manager-default-exclude-qnx.patch
git checkout net/dns/manager_default.go
```
Verify the patch file is non-empty and mentions `!qnx`.

- [ ] **Step 7: Build for QNX**

Run: `cd ~/complex-projects/mib2q-tailscale && scripts/build.sh 2>&1 | tail -20`
Expected: both binaries build, each under 15 MB, and the script prints `built for qnx/arm; exec ceiling OK`. If over the ceiling, add feature tags to `TS_TAGS` and re-run; if it still will not fit, stop and report — that is the trigger for the userspace-networking fallback in the spec.

- [ ] **Step 8: Commit**

```bash
cd ~/complex-projects/mib2q-tailscale
git add qnxplat/dns.go qnxplat/dns_test.go overlay/net patches
git commit -m "qnxplat: MagicDNS via the unit's dnsmasq

Writes split-DNS server= lines into the include file dnsmasq already
reads, and never makes Tailscale the primary resolver: the unit needs
working DNS when tailscaled is down.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Deploy — clock, control script, autostart

**Files:**
- Create: `mib2q-tailscale/cmd/sntp/main.go`
- Create: `mib2q-tailscale/deploy/tailscale.sh`
- Create: `mib2q-tailscale/deploy/startup-hook.txt`
- Modify: `mib2q-tailscale/scripts/build.sh` (also build `sntp`)

**Interfaces:**
- Consumes: `github.com/complex256/mib2q-wireguard/internal/sntp` is internal, so this task uses its *approach*, not the package: a `cmd/sntp` that queries an NTP server and calls `syscall.ClockSettime`.
- Produces: `/mnt/app/root/tailscale/tailscale.sh` with `start|stop|restart|status|log`.

- [ ] **Step 1: Write the clock setter**

Create `mib2q-tailscale/cmd/sntp/main.go`:

```go
// sntp sets the unit's clock from network time. The MHI2q's RTC boots at
// 1970, and TLS to the Tailscale control plane fails against a 1970 clock.
package main

import (
	"encoding/binary"
	"fmt"
	"net"
	"os"
	"syscall"
	"time"
)

const ntpEpochOffset = 2208988800

func main() {
	servers := os.Args[1:]
	if len(servers) == 0 {
		servers = []string{"pool.ntp.org", "time.google.com"}
	}
	for _, s := range servers {
		t, err := query(s)
		if err != nil {
			fmt.Printf("sntp: %s: %v\n", s, err)
			continue
		}
		ts := syscall.NsecToTimespec(t.UnixNano())
		if err := syscall.ClockSettime(0, &ts); err != nil { // 0 = CLOCK_REALTIME
			fmt.Println("sntp: set clock:", err)
			os.Exit(1)
		}
		fmt.Println("sntp: clock set to", t.UTC().Format(time.RFC3339))
		return
	}
	fmt.Println("sntp: no server answered")
	os.Exit(1)
}

func query(server string) (time.Time, error) {
	if _, _, err := net.SplitHostPort(server); err != nil {
		server = net.JoinHostPort(server, "123")
	}
	c, err := net.DialTimeout("udp", server, 5*time.Second)
	if err != nil {
		return time.Time{}, err
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(5 * time.Second))
	req := make([]byte, 48)
	req[0] = 0x23 // LI=0, VN=4, Mode=3 (client)
	sent := time.Now()
	binary.BigEndian.PutUint64(req[40:], uint64(sent.Unix()+ntpEpochOffset)<<32)
	if _, err := c.Write(req); err != nil {
		return time.Time{}, err
	}
	resp := make([]byte, 48)
	n, err := c.Read(resp)
	if err != nil {
		return time.Time{}, err
	}
	if n < 48 {
		return time.Time{}, fmt.Errorf("short reply (%d bytes)", n)
	}
	if mode := resp[0] & 7; mode != 4 {
		return time.Time{}, fmt.Errorf("reply mode %d, want 4", mode)
	}
	if resp[1] == 0 {
		return time.Time{}, fmt.Errorf("kiss-of-death")
	}
	secs := int64(binary.BigEndian.Uint32(resp[40:44])) - ntpEpochOffset
	frac := int64(binary.BigEndian.Uint32(resp[44:48])) * 1e9 >> 32
	return time.Unix(secs, frac).Add(time.Since(sent) / 2), nil
}
```

- [ ] **Step 2: Build it and verify it sets the clock on the unit**

Add `sntp` to `scripts/build.sh`'s build step (same flags, `-o ../sntp ./cmd/sntp` — note it lives in *our* module, so build it from the repo root rather than the tailscale tree).

Run:
```bash
cd ~/complex-projects/mib2q-tailscale && scripts/build.sh
scp build/sntp audi:/tmp/sntp
ssh audi 'chmod 755 /tmp/sntp && /tmp/sntp'
```
Expected: `sntp: clock set to 2026-...`.

- [ ] **Step 3: Write the control script**

Create `mib2q-tailscale/deploy/tailscale.sh`:

```sh
#!/bin/sh
# tailscaled control script for the MHI2q head unit.
#
#   tailscale.sh start | stop | restart | status | log
#
# Constraints this script works within (all verified on a unit): no dirname,
# tail, nohup, date or kill binary; ksh predefines aliases such as
# stop='kill -STOP', so every function here is prefixed; persistent storage
# is not mmap-capable for exec, so binaries run from /tmp (RAM, cleared
# between sessions), while state lives in /mnt/persist (flash).
case $0 in */*) here=${0%/*} ;; *) here=. ;; esac
DIR=${TS_DIR:-$(cd "$here" && pwd)}
STATE=${TS_STATE:-/mnt/persist/tailscale}
SOCK=/tmp/tailscaled.sock
PID=/tmp/tailscaled.pid
LOG=${TS_LOG:-/tmp/tailscaled.log}

ts_lastlines() {
    [ -f "$2" ] && sed -e :a -e "\$q;N;$(($1 + 1)),\$D;ba" "$2"
}

ts_running() {
    [ -f "$PID" ] && kill -0 "$(cat "$PID")" 2>/dev/null
}

ts_install() {
    for b in tailscaled tailscale sntp; do
        if [ ! "$DIR/$b" -ef "/tmp/$b" ]; then
            cp "$DIR/$b" "/tmp/$b" || return 1
        fi
        chmod 755 "/tmp/$b"
    done
}

ts_start() {
    if ts_running; then
        echo "tailscaled already running (pid $(cat "$PID"))"
        return 0
    fi
    ts_install || return 1
    mkdir -p "$STATE"
    /tmp/sntp || echo "warning: clock not set; control-plane TLS will fail"
    /tmp/tailscaled --tun=tun9 --state="$STATE/tailscaled.state" --socket="$SOCK" \
        --statedir="$STATE" </dev/null >>"$LOG" 2>&1 &
    echo $! > "$PID"
    sleep 5
    if ! ts_running; then
        echo "tailscaled exited during startup:" >&2
        ts_lastlines 10 "$LOG" >&2
        return 1
    fi
    if [ -f "$STATE/authkey" ]; then
        /tmp/tailscale --socket="$SOCK" up --authkey="$(cat "$STATE/authkey")" --accept-dns=true || true
    else
        /tmp/tailscale --socket="$SOCK" up --accept-dns=true || true
    fi
    echo "tailscaled started (pid $(cat "$PID")), log $LOG"
}

ts_stop() {
    if ! ts_running; then
        echo "tailscaled not running"
        rm -f "$PID"
        return 0
    fi
    kill "$(cat "$PID")"
    i=0
    while ts_running && [ $i -lt 10 ]; do sleep 1; i=$((i + 1)); done
    ts_running && kill -9 "$(cat "$PID")"
    rm -f "$PID"
    echo "tailscaled stopped"
}

case $1 in
    start) ts_start ;;
    stop) ts_stop ;;
    restart) ts_stop; ts_start ;;
    status)
        if ts_running; then
            echo "tailscaled running (pid $(cat "$PID"))"
            /tmp/tailscale --socket="$SOCK" status
        else
            echo "tailscaled not running"
            exit 1
        fi ;;
    log) ts_lastlines 50 "$LOG" ;;
    *) echo "usage: $0 start|stop|restart|status|log" >&2; exit 2 ;;
esac
```

- [ ] **Step 4: Check the script parses under the unit's own ksh**

Run:
```bash
scp deploy/tailscale.sh audi:/tmp/tailscale.sh.new
ssh audi 'ksh -n /tmp/tailscale.sh.new && echo "ksh -n: ok"'
```
Expected: `ksh -n: ok`. (This is exactly how the `stop()` alias bug was caught before.)

- [ ] **Step 5: Write the autostart hook text**

Create `mib2q-tailscale/deploy/startup-hook.txt`:

```
# Replaces the mibwg line in /mnt/system/etc/boot/startup.sh, inserted in
# start_late_drivers() right after `    inetd > /dev/null 2>&1`.
# Backup of the original: startup.sh.pre-mibwg in the same directory.
#
# Best effort by construction: a detached background subshell with no stdio,
# so startup.sh continues immediately and a missing or failing tailscaled
# changes nothing about the boot.

    ## tailscaled autostart. Best effort only: detached, no stdio.
    ( sleep 30; [ -x /mnt/app/root/tailscale/tailscale.sh ] && /mnt/app/root/tailscale/tailscale.sh start ) </dev/null >/dev/null 2>&1 &
```

- [ ] **Step 6: Commit**

```bash
cd ~/complex-projects/mib2q-tailscale
git add cmd/sntp deploy scripts/build.sh
git commit -m "deploy: clock setter, control script and autostart hook

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Bring the node up on the unit and verify the goals

**Files:**
- Create: `mib2q-tailscale/README.md`
- Modify: `mib2q-tailscale/docs/superpowers/specs/2026-09-20-tailscale-qnx-node-design.md` (status line only)

**Interfaces:**
- Consumes: everything above.
- Produces: a node on the tailnet, and a README stating exactly what was verified and what was not.

- [ ] **Step 1: Ask the human for a pre-auth key**

Ask for a Tailscale pre-auth key (admin console → Settings → Keys). Do not proceed without one; do not put it in any file in the repo. Write it to the unit only:

```bash
ssh audi 'mkdir -p /mnt/persist/tailscale && cat > /mnt/persist/tailscale/authkey && chmod 600 /mnt/persist/tailscale/authkey'
```

- [ ] **Step 2: Check what supervises dnsmasq before anything touches it**

Run (read-only):
```bash
ssh audi 'grep -rl dnsmasq /mnt/system/etc/eso /etc/eso 2>/dev/null | head; /bin/pidin ar | sed -n "/dnsmasq/p" | head -3'
```
Record whether dnsmasq appears in a supervisor/HAM config. **Do not kill dnsmasq on a unit someone is using.** If unsupervised, note in the README that split-DNS config needs a reboot to take effect and the `/etc/hosts` fallback is the live path.

- [ ] **Step 3: Install and start**

```bash
cd ~/complex-projects/mib2q-tailscale && scripts/build.sh
scp build/tailscaled build/tailscale build/sntp deploy/tailscale.sh audi:/tmp/
ssh audi 'set -e; W=/mnt/app/root/tailscale; mount -uw /mnt/app; mkdir -p $W;
  cp /tmp/tailscaled /tmp/tailscale /tmp/sntp /tmp/tailscale.sh $W/;
  chmod 755 $W/tailscaled $W/tailscale $W/sntp $W/tailscale.sh; sync; mount -ur /mnt/app;
  $W/tailscale.sh start'
```
Expected: `tailscaled started`. If it exits during startup, read `/tmp/tailscaled.log` — do not loop blindly.

- [ ] **Step 4: Verify goal 1 — the node is on the tailnet**

```bash
ssh audi '/mnt/app/root/tailscale/tailscale.sh status'
```
Expected: the node listed with a `100.x.y.z` address and its peers. Confirm it also appears in the admin console.

- [ ] **Step 5: Verify goal 2 — reachable from the tailnet, and reaching it**

Ask the human to run, from `stash`:
```
ssh -o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedAlgorithms=+ssh-rsa root@<unit tailscale ip>
```
And from the unit (reuse the reach probe from the mibwg work, or `tailscale ping`):
```bash
ssh audi '/tmp/tailscale --socket=/tmp/tailscaled.sock ping <a peer>; /tmp/reach <peer-ip>:22'
```
Expected: ssh in succeeds; `tailscale ping` gets replies; the TCP dial connects. Also check a MagicDNS name resolves; if it does not, apply the `/etc/hosts` fallback and say so plainly.

- [ ] **Step 6: Verify goal 3 — survives a reboot**

Switch the autostart line in `/mnt/system/etc/boot/startup.sh` from `mibwg.sh` to `tailscale.sh` (keep the existing backup; verify with `ksh -n` before rebooting), then ask the human to reboot. After it comes back:
```bash
ssh audi '/mnt/app/root/tailscale/tailscale.sh status | head -5'
```
Expected: online with no human action. Confirm the hotspot still has internet (`curl` from a device on it) — the full-tunnel incident makes this a required check, not an optional one.

- [ ] **Step 7: Write the README**

Cover: what this is, the version pin and why, build, deploy, the control script, rollback to mibwg (its files are still installed), and a "Verified on the unit / Not verified" section that is honest about MagicDNS and anything else outstanding.

- [ ] **Step 8: Commit**

```bash
cd ~/complex-projects/mib2q-tailscale
git add README.md docs
git commit -m "A Tailscale node on the MHI2q: verified on the unit

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

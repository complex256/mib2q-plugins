#!/bin/sh
# Build tailscaled + tailscale for the MHI2q (QNX 6.5 armle-v7).
#
#   scripts/build.sh
#
# Fetches the pinned source, copies our overlay files in, applies patches,
# points the module at our QNX forks, and builds in the toolchain image.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PARENT=$(cd "$ROOT/../.." && pwd)
SRC=$ROOT/build/tailscale-src
WGSRC=$ROOT/build/wireguard-go-src
ISATTYSRC=$ROOT/build/go-isatty-src
# 8.5-go127 (Go 1.27.1), not 8.5-go (Go 1.26.4): the pinned tailscale v1.102.4
# declares `go 1.26.6`, and GOTOOLCHAIN=local means the container's toolchain
# has to satisfy that on its own - 1.26.4 fails the build outright rather than
# downloading a newer one. 1.27.1 is also the toolchain whose output is proven
# on the unit (see the sizes below).
IMG=${QNX_IMAGE:-qnx65-armv7-toolchain:8.5-go127}
# 16 MiB. This gate is evidence-based, not a guess - every figure below was
# observed on QNX, not inferred:
#   14,717,155  runs on the unit (the v1.100.0/Go 1.26.4 binary deployed today)
#   15,200,481  exec'd on the unit, 2026-09-23 (v1.100.0 built with Go 1.27.1)
#   15,941,857  RUNS ON THE UNIT, 2026-09-23 - Tailscale main built with Go
#               1.27.1: joined the tailnet and parsed a netmap with the full
#               peer list. Also exec'd under QEMU in a 256 MB VM beforehand.
#   ~21 MB      failed to exec on the unit - the ONLY observed failure
# The previous 15 MiB value was a conservative guess made before any of the
# upper data points existed, and it rejected a binary since shown to load.
# Raise this again only on a new observation, and record that observation here.
MAXBYTES=16777216

sh "$ROOT/scripts/fetch.sh"

# fetch.sh skips re-cloning a tree that's already at the pinned
# version/commit, so each of these can carry state left over from a
# previous run of this script: last run's overlay copies, its applied
# patches, and go.mod/go.sum edits from `go mod edit`/`go mod tidy` below
# (the docker steps bind-mount this checkout, so those edits land on disk
# here, not just in the container). Left alone, that makes the patch loop
# below non-deterministic - a patch already applied on a previous run
# looks identical to a genuinely broken one ("does not apply" either way).
# Reset every fetched tree to pristine first so this script's own output
# is the same whether the tree was just cloned or has been built a dozen
# times.
for t in "$SRC" "$WGSRC" "$ISATTYSRC"; do
    [ -d "$t/.git" ] || continue
    git -C "$t" reset --hard -q
    git -C "$t" clean -fdq
done

# Overlay: our platform files land inside a fetched tree at the same paths.
# overlay*/ directories are populated incrementally by later tasks - guard so
# this pipeline runs clean before their files exist (a `find` over a
# nonexistent dir would abort the script under set -e).
overlay_into() {
    ov=$1
    dest=$2
    [ -d "$ROOT/$ov" ] || return 0
    ( cd "$ROOT/$ov" && find . -type f -name '*.go' ) | while read -r f; do
        mkdir -p "$dest/$(dirname "$f")"
        cp "$ROOT/$ov/$f" "$dest/$f"
    done
}
overlay_into overlay "$SRC"
overlay_into overlay-wireguard-go "$WGSRC"
overlay_into overlay-isatty "$ISATTYSRC"

# Patches: edits to upstream files (build tags and the like). Strict: the
# reset above means $SRC starts pristine every run, so a patch that doesn't
# apply now is genuinely broken (drifted from the pinned tailscale version)
# rather than merely already-applied-from-last-time - stop the build rather
# than silently ship a binary missing a patch it was supposed to have.
for p in "$ROOT"/patches/*.patch; do
    [ -e "$p" ] || continue
    name=$(basename "$p")
    if git -C "$SRC" apply --check "$p" 2>/dev/null; then
        git -C "$SRC" apply "$p"
        echo "applied $name"
    elif git -C "$SRC" apply --reverse --check "$p" 2>/dev/null; then
        echo "ERROR: patch $name is already applied to what should have been a pristine tree - the reset/clean above did not clear it. This is a bug in this script (or something outside it modified build/tailscale-src), not a broken patch." >&2
        exit 1
    else
        echo "ERROR: patch $name does not apply (forward or reverse) to the pinned tailscale source - it has drifted from upstream and needs to be regenerated against the current pinned version." >&2
        exit 1
    fi
done

# Our module + the QNX forks of wireguard-go, x/sys, tailscale's own
# wireguard-go fork (a different module from golang.zx2c4.com/wireguard, and
# the one tailscaled actually imports), and go-isatty.
( cd "$SRC"
  go mod edit -require=github.com/complex256/mib2q-tailscale@v0.0.0 \
      -replace=github.com/complex256/mib2q-tailscale=/work/mib2q-tailscale/daemon \
      -replace=github.com/complex256/mib2q-wireguard=/work/mib2q-wireguard/daemon \
      -replace=golang.zx2c4.com/wireguard=/work/mib2q-wireguard/daemon/third_party/wireguard-go \
      -replace=golang.org/x/sys=/work/mib2q-wireguard/daemon/third_party/x-sys \
      -replace=github.com/tailscale/wireguard-go=/work/mib2q-tailscale/daemon/build/wireguard-go-src \
      -replace=github.com/mattn/go-isatty=/work/mib2q-tailscale/daemon/build/go-isatty-src )

# ts_omit_acme: ipn/ipnlocal's
# cert.go (ACME cert refresh) is only gated on !ts_omit_acme, but its
# refreshApplicableCerts reads LocalBackend.ServeConfig(), which ts_omit_serve
# removes (ipn/ipnlocal/serve_disabled.go has no stub for it) - acme has its
# own cert_disabled.go stub, so omitting it is the correct fix, not a patch.
#
# ts_omit_clientupdate: self-update support (downloads and installs new
# tailscaled binaries at runtime). We deploy binaries ourselves through this
# pipeline; letting tailscaled fetch and swap its own binary on a locked-down
# embedded unit is both undesired and unnecessary weight.
#
# The rest below are also genuinely inapplicable to a single-purpose,
# non-interactive node on the head unit, not just picked to hit the size
# ceiling: ts_omit_appconnectors (App Connectors, a subnet-router extension
# we don't run), ts_omit_desktop_sessions (desktop multi-user session
# tracking - qnx here is headless), ts_omit_oauthkey (OAuth-to-authkey
# exchange - we provision with a static authkey), ts_omit_webbrowser
# (opening a system browser for interactive login - no browser on the unit).
#
# The rest of this list (ruled 2026-09-20) is a further one-shot pass of
# features that plainly don't apply to this deployment either: portlist,
# portmapper/debugportmapper (we don't need NAT-PMP/PCP/UPnP port mapping
# diagnostics), captiveportal, wakeonlan, advertiseexitnode/advertiseroutes/
# useexitnode/useroutes (this node neither advertises nor uses exit
# nodes/subnet routes), routecheck, peerapiserver/peerapiclient,
# outboundproxy/useproxy, tailnetlock, netlog, doctor, debugeventbus,
# clientmetrics/usermetrics/runtimemetrics, linkspeed, listenrawdisco,
# qrcodes, colorable, completion, cliconndiag, hujsonconf,
# identityfederation, cloud, ace, c2n, syspolicy, logtail, synology, dbus,
# iptables, networkmanager, resolved, sdnotify. Deliberately NOT omitted:
# dns (MagicDNS is a project goal), bakedroots (the unit has no CA store,
# so TLS to the control plane needs the embedded roots), osrouter (Task 4
# registers through it), cli (we need `tailscale up`/`status` on the unit),
# and anything DERP-related (the car is behind carrier NAT and needs
# relays).
# netstack is NOT omitted: with ts_omit_netstack, tstun.invertGSOChecksum
# panics "unreachable" on the first injected packet (upstream assumes injected
# packets only occur with netstack), which killed tailscaled on the unit.
# gro depends on netstack, so it cannot be omitted alone either.
#
# ts_omit_flashappliance (added when moving to `main`, which introduced this
# feature after v1.100.0): `tailscale configure flash-appliance` writes a
# gokrazy disk image and pulls in github.com/diskfs/go-diskfs's ext4 package,
# which needs unix.Stat_t/Stat/Major/Minor/Mkdev - none of which the QNX
# golang.org/x/sys shim (mib2q-wireguard/third_party/x-sys) implements, so it
# fails to compile. It is also meaningless on this deployment: the unit isn't
# reflashed by writing a gokrazy image to a disk from within tailscale itself.
# Upstream ships the matching _omit.go stub, so this is the supported way to
# drop it rather than a patch against unix_qnx.go.
TAGS=${TS_TAGS:-ts_omit_acme,ts_omit_ssh,ts_omit_drive,ts_omit_taildrop,ts_omit_webclient,ts_omit_serve,ts_omit_kube,ts_omit_tpm,ts_omit_systray,ts_omit_capture,ts_omit_aws,ts_omit_bird,ts_omit_relayserver,ts_omit_clientupdate,ts_omit_appconnectors,ts_omit_desktop_sessions,ts_omit_oauthkey,ts_omit_webbrowser,ts_omit_portlist,ts_omit_portmapper,ts_omit_captiveportal,ts_omit_wakeonlan,ts_omit_advertiseexitnode,ts_omit_advertiseroutes,ts_omit_useexitnode,ts_omit_useroutes,ts_omit_routecheck,ts_omit_peerapiserver,ts_omit_peerapiclient,ts_omit_outboundproxy,ts_omit_useproxy,ts_omit_tailnetlock,ts_omit_netlog,ts_omit_doctor,ts_omit_debugportmapper,ts_omit_debugeventbus,ts_omit_clientmetrics,ts_omit_usermetrics,ts_omit_runtimemetrics,ts_omit_linkspeed,ts_omit_listenrawdisco,ts_omit_qrcodes,ts_omit_colorable,ts_omit_completion,ts_omit_cliconndiag,ts_omit_hujsonconf,ts_omit_identityfederation,ts_omit_cloud,ts_omit_ace,ts_omit_c2n,ts_omit_syspolicy,ts_omit_logtail,ts_omit_synology,ts_omit_dbus,ts_omit_iptables,ts_omit_networkmanager,ts_omit_resolved,ts_omit_sdnotify,ts_omit_flashappliance}

# The toolchain image is amd64 and this build commonly runs under
# qemu-x86_64 user-mode emulation (e.g. on Apple Silicon docker hosts). That
# emulation has a known deadlock mode: the go build driver's `compile` child
# goes zombie and the step then sits at 0% CPU forever - it never times out
# on its own. Bound each docker build step so that failure mode is loud
# instead of an indefinite hang; a real build takes a few minutes, so 1800s
# only trips on a genuine hang. Deliberately no automatic retry here (same
# reasoning as the documented "case-insensitive file name collision" flake):
# a masked, silently-retried hang is worse than a loud one.
TIMEOUT_BIN=""
for c in timeout gtimeout; do
    if command -v "$c" >/dev/null 2>&1; then
        TIMEOUT_BIN=$c
        break
    fi
done
if [ -z "$TIMEOUT_BIN" ]; then
    echo "WARNING: no timeout(1)/gtimeout(1) on this host; docker build steps are NOT time-bounded and could hang indefinitely (see the qemu emulation deadlock note above)." >&2
fi

# run_docker_build CMD...: runs one docker build step, bounded to 1800s
# when a timeout binary is available; reports the emulation deadlock by name
# on a timeout instead of leaving a bare "docker: ..." failure to puzzle out.
run_docker_build() {
    set +e
    if [ -n "$TIMEOUT_BIN" ]; then
        "$TIMEOUT_BIN" 1800 "$@"
    else
        "$@"
    fi
    rc=$?
    set -e
    if [ "$rc" -eq 124 ]; then
        echo "ERROR: docker build timed out after 1800s. This looks like the qemu-x86_64 user-mode emulation deadlock (a zombie 'compile' child the go build driver never reaps - seen on Apple Silicon docker hosts running this amd64 toolchain image), not a real build failure. Remove the stuck container (docker ps / docker kill) and re-run scripts/build.sh; it has reliably succeeded on retry so far." >&2
        exit 1
    elif [ "$rc" -ne 0 ]; then
        exit "$rc"
    fi
}

# GOCACHE is a named volume, not just the module cache: without it every run
# recompiles the whole dependency graph from scratch, which under qemu-x86_64
# emulation is the difference between a rebuild and a coffee break. Both docker
# runs share it. `docker volume rm tailscale-gocache` forces a cold build.
run_docker_build docker run --rm --platform=linux/amd64 \
    -v "$PARENT":/work -w /work/mib2q-tailscale/daemon/build/tailscale-src \
    -v tailscale-gomod:/root/go/pkg/mod \
    -v tailscale-gocache:/root/.cache/go-build \
    -e GOOS=qnx -e GOARCH=arm -e GOARM=7 -e CGO_ENABLED=0 -e GOTOOLCHAIN=local \
    -e GOFLAGS=-buildvcs=false \
    "$IMG" sh -c "
        # Non-fatal on purpose: tidy can fail for reasons the cross-build
        # doesn't care about, and the go build below is the real gate. But
        # it rewrites go.mod/go.sum in a bind-mounted tree, so discarding
        # its output entirely meant a partial rewrite left no trace at all.
        # Warn and keep going.
        go mod tidy || echo 'warning: go mod tidy failed; go.mod/go.sum may be stale' >&2
        go build -tags '$TAGS' -trimpath -ldflags='-s -w -B none -buildid=' -o ../tailscaled ./cmd/tailscaled &&
        go build -tags '$TAGS' -trimpath -ldflags='-s -w -B none -buildid=' -o ../tailscale  ./cmd/tailscale"

# sntp lives in our own module (cmd/sntp), not the fetched tailscale tree, so
# it builds from the repo root in its own docker run rather than piggybacking
# on the tailscale-src build above.
run_docker_build docker run --rm --platform=linux/amd64 \
    -v "$PARENT":/work -w /work/mib2q-tailscale/daemon \
    -v tailscale-gomod:/root/go/pkg/mod \
    -v tailscale-gocache:/root/.cache/go-build \
    -e GOOS=qnx -e GOARCH=arm -e GOARM=7 -e CGO_ENABLED=0 -e GOTOOLCHAIN=local \
    -e GOFLAGS=-buildvcs=false \
    "$IMG" sh -c "
        go build -trimpath -ldflags='-s -w -B none -buildid=' -o build/sntp ./cmd/sntp &&
        go build -trimpath -ldflags='-s -w -B none -buildid=' -o build/tsstatus ./cmd/tsstatus"

# libtsclock.so is C, not Go: it is LD_PRELOADed into tailscaled so that one
# process sees corrected wall time while the unit's clock stays at 1970. The
# clock must not be stepped - doing so fires the HMI's pending CLOCK_REALTIME
# waits and restarts the navigation domain (see ts_clock in deploy/tailscale.sh).
run_docker_build docker run --rm --platform=linux/amd64 \
    -v "$PARENT":/work -w /work/mib2q-tailscale/daemon \
    "$IMG" sh -c "
        arm-unknown-nto-qnx6.5.0eabi-gcc -shared -fPIC -O2 -Wall -Wextra \
            -o build/libtsclock.so qnxplat/clockshim/tsclock.c"

# Three things make this shim work, and all are silent when broken: it has to
# EXPORT clock_gettime (that is the interposition), EXPORT sem_timedwait (that
# is what keeps the Go runtime's absolute deadlines consistent with the kernel -
# without it any GC stop-the-world freezes tailscaled forever; see the header of
# qnxplat/clockshim/tsclock.c), and IMPORT dlsym (that is how it reaches the
# real ones through RTLD_NEXT). Note it does not import either wrapped symbol -
# dlsym takes the name as a string, so there is no link-time reference to check.
# A shim missing the clock half would hand tailscaled a 1970 clock and every
# handshake would be rejected as a replay; missing the sem_timedwait half it
# wedges on the first GC. Neither says anything in any log.
syms=$(docker run --rm --platform=linux/amd64 \
    -v "$PARENT":/work -w /work/mib2q-tailscale/daemon "$IMG" sh -c \
    "arm-unknown-nto-qnx6.5.0eabi-nm build/libtsclock.so" || true)
if [ "$(printf '%s\n' "$syms" | grep -c ' T clock_gettime$')" -lt 1 ]; then
    echo "ERROR: libtsclock.so does not export clock_gettime; it would never interpose" >&2
    exit 1
fi
if [ "$(printf '%s\n' "$syms" | grep -c ' T sem_timedwait$')" -lt 1 ]; then
    echo "ERROR: libtsclock.so does not export sem_timedwait; timed parks would never expire and tailscaled would freeze on its first GC stop-the-world" >&2
    exit 1
fi
if [ "$(printf '%s\n' "$syms" | grep -c ' U dlsym$')" -lt 1 ]; then
    echo "ERROR: libtsclock.so does not import dlsym; it cannot reach the real clock_gettime" >&2
    exit 1
fi

for b in tailscaled tailscale sntp tsstatus; do
    sz=$(wc -c < "$ROOT/build/$b")
    printf '%-12s %s bytes\n' "$b" "$sz"
    if [ "$sz" -gt "$MAXBYTES" ]; then
        echo "ERROR: $b is over the unit's ~15 MB exec ceiling" >&2
        exit 1
    fi
done
printf '%-12s %s bytes\n' libtsclock.so "$(wc -c < "$ROOT/build/libtsclock.so")"
echo "built for qnx/arm; exec ceiling OK"

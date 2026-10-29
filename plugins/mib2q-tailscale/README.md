# mib2q-tailscale

The Tailscale product for the MIB2Q head unit.

- `plugin/` — the `tailscale-status` Java plugin: the icon-only status item
  (bright running, flickering connecting, dim otherwise) and its detail page.
  It reads the local `tsstatus` endpoint on `127.0.0.1:19850`.
- `daemon/` — the native Tailscale build: `tailscaled`, `tailscale`, `sntp`,
  `tsstatus`, `libtsclock.so`, the `tailscale.sh` control/service script and
  the `mibpkg` package manifest.

See `daemon/README.md` for the pinned Tailscale version, overlays, patches,
QNX toolchain and deploy details.

## Build

```sh
# Java status plugin
./gradlew :plugins:mib2q-tailscale:plugin:test \
          :plugins:mib2q-tailscale:plugin:mibpkgPackage \
          -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo \
          -Pmibsdk.mibpkg=/path/to/mibpkg

# Native binaries (Docker + the private QNX image)
docker login ghcr.io
export QNX_GO_IMAGE=ghcr.io/complex256/qnx65-armv7-toolchain:8.5-go127
./gradlew :plugins:mib2q-tailscale:daemon:buildDaemon \
          :plugins:mib2q-tailscale:daemon:packageDaemon \
          -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo \
          -Pmibsdk.mibpkg=/path/to/mibpkg
```

The mibpkg package is `tailscale` (version from `daemon/VERSION`). Its `@config`
directory holds `authkey` (optional, mode 600), `accept-routes` and
`tailscaled.state`; the service runs `tailscale.sh serve @config`.

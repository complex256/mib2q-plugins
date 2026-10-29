# mib2q-wireguard

The `mibwg` WireGuard tunnel for the MIB2Q head unit.

- `daemon/` — the native Go `mibwg` daemon and the QNX wireguard-go/x-sys
  forks, plus packaging and config example. `mibwg` is used by the unit's
  network setup and is independent of the Tailscale product.

See `daemon/README.md` for the tunnel backends, config and deploy details.

## Build

```sh
docker login ghcr.io
export QNX_GO_IMAGE=ghcr.io/complex256/qnx65-armv7-toolchain:8.5-go127
./gradlew :plugins:mib2q-wireguard:daemon:buildDaemon \
          :plugins:mib2q-wireguard:daemon:packageDaemon \
          -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo \
          -Pmibsdk.mibpkg=/path/to/mibpkg
```

The mibpkg package is `wireguard`; its `@config/mibwg.conf` is required and the
service runs the daemon in the foreground after copying the binary to `/tmp`.

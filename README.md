# mib2q-plugins

The MIB2Q plugin and daemon monorepo for the MIB2 High head unit. It holds
both halves of each product: the Java/Kotlin plugins and the native daemons
they talk to.

| Product | Plugin | Native daemon | mibpkg package |
|---|---|---|---|
| `mib2q-tailscale` | `tailscale-status` icon/page | `tailscaled`, `tailscale`, `sntp`, `tsstatus`, `libtsclock.so` | `tailscale` |
| `mib2q-carmqtt` | `carmqtt-status` icon/page | `carmqtt` | `carmqtt` |
| `mib2q-wireguard` | — | `mibwg` | `wireguard` |
| `bike-rack` | bike-rack plugin | — | `bike-rack` |
| `hello-menu` | hello-menu plugin | — | `hello-menu` |
| `inputs-demo` | inputs-demo plugin | — | `inputs-demo` |

The SDK itself (`api`, runtime, adapter) lives in `mib2q-plugin-sdk`; the
package manager and host `mibpkg` binary live in `mibpkg`.

## Layout

```text
plugins/
  bike-rack/                  Gradle plugin
  hello-menu/                 Gradle plugin
  inputs-demo/                Gradle plugin
  mib2q-tailscale/
    plugin/                   Gradle + mibsdk.plugin: tailscale-status
    daemon/                   Gradle base: native Go sources, scripts, packaging
  mib2q-carmqtt/
    plugin/                   Gradle + mibsdk.plugin: carmqtt-status
    daemon/                   Gradle base: native Rust sources, scripts, packaging
  mib2q-wireguard/
    daemon/                   Gradle base: native Go sources, scripts, packaging
```

Only the `plugin/` projects and the root-level plugins are SDK plugin builds;
the `daemon/` projects use the `base` plugin and wrap the Docker/QNX scripts
with `buildDaemon` / `packageDaemon` tasks.

## Prerequisites

- JDK 17.
- A checkout of `mib2q-plugin-sdk` at the version this repo's `mibsdk.plugin`
  asks for, after `./gradlew publishLocal` (or a hosted copy of its
  `build/repo`).
- A host `mibpkg` binary for the `.mpk` tasks.
- Docker plus the two private QNX toolchain images:
  ```text
  ghcr.io/complex256/qnx65-armv7-toolchain:8.5-go127
  ghcr.io/complex256/qnx65-armv7-toolchain:8.5-rust
  ```
  `docker login ghcr.io` with a token that can read the packages.

`gradle.properties` leaves `mibsdk.repo` and `mibsdk.mibpkg` blank so no
machine-specific paths are committed. Pass them with `-P` (or set them in
`~/.gradle/gradle.properties`):

```sh
./gradlew :plugins:mib2q-tailscale:plugin:test \
  -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo

./gradlew :plugins:mib2q-tailscale:daemon:buildDaemon \
  -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo
```

The root `mibsdk.plugin` marker is resolved for every Gradle invocation, so
`mibsdk.repo` is currently needed even for daemon-only tasks; the daemon
projects themselves do not use the SDK artifacts.

## Build examples

```sh
# Java status plugins
./gradlew :plugins:mib2q-tailscale:plugin:test \
          :plugins:mib2q-carmqtt:plugin:test

# Native binaries (Docker images required)
export QNX_GO_IMAGE=ghcr.io/complex256/qnx65-armv7-toolchain:8.5-go127
export QNX_RUST_IMAGE=ghcr.io/complex256/qnx65-armv7-toolchain:8.5-rust
./gradlew :plugins:mib2q-tailscale:daemon:buildDaemon \
          :plugins:mib2q-wireguard:daemon:buildDaemon \
          :plugins:mib2q-carmqtt:daemon:buildDaemon

# .mpk packages
./gradlew :plugins:mib2q-tailscale:daemon:packageDaemon \
          :plugins:mib2q-wireguard:daemon:packageDaemon \
          :plugins:mib2q-carmqtt:daemon:packageDaemon \
          :plugins:mib2q-tailscale:plugin:mibpkgPackage \
          :plugins:mib2q-carmqtt:plugin:mibpkgPackage \
          -Pmibsdk.mibpkg=/path/to/mibpkg
```

The native packages install mibpkg-managed config:

- `tailscale`: `@config/authkey`, `@config/accept-routes`,
  `@config/tailscaled.state`; service `tailscale`.
- `wireguard`: `@config/mibwg.conf`; service `mibwg`.
- `carmqtt`: `@config/carmqtt.toml`; service `carmqtt` (waits for `tailnet`).

## CI

`.github/workflows/plugins.yml` runs two matrices:

- Java: SDK `publishLocal`, host mibpkg, plugin tests and plugin `.mpk`s.
- Native: host mibpkg, private GHCR login, daemon `buildDaemon` /
  `packageDaemon`, native `.mpk`s.

Set the `MIBSDK_MIBPKG_URL` repository variable to the host (Linux) mibpkg
binary and `GHCR_TOKEN` to a token with `read:packages` for the private QNX
images. The SDK repo is public, so no `SDK_REPO_TOKEN` is needed.

Native CI is gated on the repository variable `NATIVE_CI=true` so a checkout
without the private image token stays green. Set it once `GHCR_TOKEN` and the
two images exist; the native matrix then runs on push (or use
`workflow_dispatch`).

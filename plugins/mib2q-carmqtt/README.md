# mib2q-carmqtt

The carmqtt product for the MIB2Q head unit.

- `plugin/` — the `carmqtt-status` Java plugin: the MQTT plug status item and
  its detail page. It reads the local carmqtt status endpoint on
  `127.0.0.1:19851`.
- `daemon/` — the native Rust `carmqtt` daemon: ExLAP/UBX sources, MQTT
  publisher, packaging and config example.

See `daemon/README.md` for the daemon, config and MQTT layout.

## Build

```sh
# Java status plugin
./gradlew :plugins:mib2q-carmqtt:plugin:test \
          -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo \
          -Pmibsdk.mibpkg=/path/to/mibpkg

# Native daemon package (Docker + the private QNX image)
docker login ghcr.io
export QNX_RUST_IMAGE=ghcr.io/complex256/qnx65-armv7-toolchain:8.5-rust
./gradlew :plugins:mib2q-carmqtt:daemon:buildDaemon \
          :plugins:mib2q-carmqtt:daemon:packageDaemon \
          -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo \
          -Pmibsdk.mibpkg=/path/to/mibpkg
```

The mibpkg package is `carmqtt`; it contains the native daemon and the `carmqtt-status` plugin, so one package installs both. its `@config/carmqtt.toml` is required and the
service waits for the `tailnet` start condition (Tailscale brings it up).

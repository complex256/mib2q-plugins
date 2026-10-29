# mib2q-plugins

The MIB2Q plugin SDK plugins for the head unit: `bike-rack`, `hello-menu`, `inputs-demo`, `tailscale-status` (the
Tailscale icon: bright running, flickering connecting, dim otherwise) and `carmqtt-status` (the MQTT plug: bright
connected, dim otherwise). The SDK itself (api/runtime/adapter) lives in `mib2q-plugin-sdk`; the daemons
(`mib2q-tailscale`, `mib2q-carmqtt`) live in their own repositories and expose `127.0.0.1:19850`/`19851`.

## Build

Point `mibsdk.repo` at an SDK checkout's `build/repo` (after `./gradlew publishLocal`, with
`mibsdk.version` set to the version this repo's `mibsdk.plugin` asks for), then:

```
./gradlew :plugins:hello-menu:test
./gradlew :plugins:tailscale-status:mibpkgPackage -Pmibsdk.mibpkg=/path/to/mibpkg
```

`mibsdk.mibpkg` in `gradle.properties` is the host mibpkg used by `mibpkgPackage`.

## CI

`.github/workflows/plugins.yml` runs a matrix over `plugins/*`: it checks out the SDK, runs `publishLocal`,
tests and packages each plugin with the host mibpkg, and uploads the `.mpk`s. Set the `MIBSDK_MIBPKG_URL`
repository variable to the host (Linux) mibpkg binary and, if the SDK is not checked out per job, point
`mibsdk.repo` at a hosted copy of the SDK's `build/repo`.

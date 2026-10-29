# mib2q-plugins

The MIB2Q plugin SDK plugins for the head unit: `bike-rack`, `hello-menu`, `inputs-demo`, `tailscale-status` (the
Tailscale icon: bright running, flickering connecting, dim otherwise) and `carmqtt-status` (the MQTT plug: bright
connected, dim otherwise). The SDK itself (api/runtime/adapter) lives in `mib2q-plugin-sdk`; the daemons
(`mib2q-tailscale`, `mib2q-carmqtt`) live in their own repositories and expose `127.0.0.1:19850`/`19851`.

## Build

`gradle.properties` leaves `mibsdk.repo` and `mibsdk.mibpkg` blank, so the repository carries no machine-specific
paths. Pass them with `-P` (or set them in `~/.gradle/gradle.properties`):

```
./gradlew :plugins:hello-menu:test \
  -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo
./gradlew :plugins:tailscale-status:mibpkgPackage \
  -Pmibsdk.repo=file:///path/to/mib2q-plugin-sdk/build/repo \
  -Pmibsdk.mibpkg=/path/to/mibpkg
```

The SDK checkout must be at the version this repo's `mibsdk.plugin` asks for, after `./gradlew publishLocal`.

## CI

`.github/workflows/plugins.yml` runs a matrix over `plugins/*`: it checks out the SDK, runs `publishLocal`,
tests and packages each plugin with the host mibpkg, and uploads the `.mpk`s. Set the `MIBSDK_MIBPKG_URL`
repository variable to the host (Linux) mibpkg binary and, if the SDK is not checked out per job, point
`mibsdk.repo` at a hosted copy of the SDK's `build/repo`.

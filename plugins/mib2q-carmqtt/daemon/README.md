# carmqtt

Publishes the status of an Audi with a MIB2 High head unit (MU1316 / MHI2Q,
QNX 6.5) to Home Assistant over MQTT, from a daemon running on the head unit
itself.

v1 reads the unit's own ExLAP server (`127.0.0.1:25010`). It needs FeCs
`00060100` (vehicle data) and optionally `00060A00` (high-performance data)
in the unit's FeC container; without them ExLAP only exposes navigation.
Door, tyre, temperature and battery data are not exposed by ExLAP on the
development car (`EC_01, not existing`) and show as unavailable until another
source provides them.

Design: `docs/superpowers/specs/2026-09-25-carmqtt-design.md`.

## Build and test

```sh
cargo test                  # host; the end-to-end test uses mosquitto if installed
scripts/build.sh            # cross-build in the qnx65-armv7-toolchain:8.5-rust image
```

## Install

```sh
cp deploy/carmqtt.toml.example deploy/carmqtt.toml   # fill in [mqtt]
deploy/install.sh audi                               # files + boot hook (backs up startup.sh)
ssh audi /mnt/app/root/carmqtt/carmqtt.sh start
ssh audi /mnt/app/root/carmqtt/carmqtt.sh log
```

`deploy/uninstall.sh audi` stops it, removes the boot hook and deletes the files.

### As a mibpkg package

carmqtt also ships as a mibpkg package (`packaging/manifest.toml.in`):
mibpkg owns the files, starts the service (after the tailnet, or anyway after
5 min) and keeps the config at
`/mnt/app/root/mibpkg/state/config/carmqtt/carmqtt.toml` (mode 600). The
hand-install above is then not used. Change the config through mibpkg, never by
editing `/mnt/app` directly:

- the mibpkg web interface (config files), or
- over SSH: upload the new file to `/tmp/mibpkg.text.<id>` (`<id>` = 32 random
  lowercase hex characters), then `/tmp/mibpkg gem config-set carmqtt 0 <id>`.
  mibpkg keeps the old file as `carmqtt.toml.prev`, deletes the upload and
  restarts the service.

Keep the broker password only in that file on the unit (not in this repo, a
commit or a log).

**No MQTT connection until a VIN is known.** The first connect needs the VIN
for the topic names. It comes from ExLAP, or from `[device] vin` in the config.
Without either (e.g. a bench unit with no vehicle) the log says `waiting for VIN
before connecting to MQTT` and the status endpoint stays at `mqtt=0`,
`last_publish_age_s=-1`, even when the broker is reachable.

## MQTT layout

| Topic | Payload |
|---|---|
| `mib2q/<vin>/status` | `online` / `offline` (last will); drives the "Online" entity |
| `mib2q/<vin>/state/<signal/path>` | value, e.g. `69760`, `ON`, `position_P` |
| `mib2q/<vin>/avail/<signal/path>` | `online` / `offline` per signal |
| `mib2q/<vin>/location` | JSON for the device tracker |
| `homeassistant/<component>/mib2q_<vin>/<signal_id>/config` | discovery |

All retained, QoS 1. Signal ids and default rates are in `src/catalog.rs`;
per-signal overrides go in `[signals]` in the config.

## Status endpoint

The daemon listens on `127.0.0.1:19851` (`[status] port`). Each connection
receives one snapshot and is closed; a bind failure is logged and ignored.

```
v=1
daemon=carmqtt
version=0.3.3
mqtt=0|1
last_publish_age_s=<n>|-1      # -1 until the first publish; monotonic seconds
signals=<n>
exlap=0|1
exlap_err=<short text or empty>
uptime_s=<n>
```

`mqtt` is the last-known connection state (it changes on connect, publish
failure and shutdown), not a live probe. Consumers should treat a large
`last_publish_age_s` as stale. `signals` counts values currently available.

## Adding a data source

Implement `sources::Source` (see `src/sources/exlap/`), map your data onto
catalog signal ids, add a `[sources.<name>]` config section, and start it in
`app::start`. The router merges sources by `priority`.

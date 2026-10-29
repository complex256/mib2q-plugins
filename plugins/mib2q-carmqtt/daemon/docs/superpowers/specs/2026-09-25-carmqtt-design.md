# carmqtt: car status → Home Assistant daemon for the MIB2 High (MHI2Q)

Date: 2026-09-25
Status: approved design, pending implementation plan

## 1. Goal

A Rust daemon that starts at boot on the Audi MHI2Q head unit (MU1316, QNX 6.5.0
armle-v7), reads vehicle status from the unit's own services, and publishes it to
Home Assistant over MQTT with auto-discovery, so the car's status can be seen and
recorded over time.

### Requirements (agreed)

| # | Requirement |
|---|---|
| R1 | Publish to an MQTT broker reached over Tailscale, plain MQTT 3.1.1 on TCP (no TLS in the daemon). |
| R2 | Home Assistant MQTT discovery: one device for the car, one entity per signal. |
| R3 | Offline behaviour is **latest state only**: while the broker is unreachable, keep only the newest value per signal in RAM; publish that snapshot on reconnect. Nothing is persisted to flash. |
| R4 | Slow status signals publish on change; fast signals are throttled per signal (minimum interval + deadband), configurable. |
| R5 | Multiple data sources may run at once. Each maps its data onto a shared canonical signal catalog; a router merges them with config-defined priority. |
| R6 | v1 ships one source: ExLAP on `127.0.0.1:25010`. The design must let a native eso-comm/DSI source and a Java HMI bridge source be added later without touching the router or publisher. |
| R7 | Starts at boot via the same `startup.sh` hook pattern as mib2q-tailscale, runs from `/tmp`, restarts itself on crash. |

### Non-goals (v1)

- Store-and-forward history with original timestamps.
- TLS, MQTT 5, QoS 2.
- Any write/control path into the car (ExLAP `Call` URLs such as media, guidance, volume are never used).
- The DSI and HMI-bridge sources themselves (each gets its own spec).

## 2. Background: what ExLAP gives on this unit

Verified live on 2026-09-25 against the user's car (after adding FeCs 00060100 and
00060A00 and rebooting):

- `rdiserver` + `libexlap_plugin.so` listens on `127.0.0.1:25010` (also hotspot/uap0/mlan0,
  firewalled by the pf `EXLAP` anchor; loopback is unfiltered). Server identifies as
  "VW SAI-Server HIGH", API level 5, protocol 1.3.
- Auth: `<Authenticate phase="challenge" useHash="sha256"/>` → `<Challenge nonce="…"/>` →
  `<Authenticate phase="response" user cnonce digest/>` where
  `digest = base64(sha256(user:pass:nonce:cnonce))`, each field truncated to 44 chars.
  Credential `RSE_3-DE1400` sees 149 of the 152 URLs, including every vehicle URL used below.
- 52 URLs return live values. Door/lock, tyres, outside/coolant/oil temperature, battery
  voltage, lights and seat heaters return `state="error" msg="EC_01, not existing"` (the
  car-side view option reports the function absent). `Nav_*` and other HMI-backed URLs
  return `state="nodata"`; `Context_States` reports Navigation/Media/Radio/System/Sound
  `Unavailable`, and a reboot did not change this.
- `Subscribe url ival timeStamp="true"` pushes `<Dat timeStamp="…" url="…">` messages;
  subscribing does not turn `EC_01` into data.

## 3. Architecture

```
┌─ source threads ────────────┐  Update{signal, value, ts, source}
│ ExlapSource  (v1)           │──┐
│ DsiSource    (future)       │──┼──► Router ──────────► Publisher ──► MQTT broker
│ HmiBridge    (future)       │──┘   last-value table     discovery       (Tailscale)
└─────────────────────────────┘      priority merge       availability
         ▲ supervisor restarts        throttle/deadband    reconnect + snapshot
```

Runtime model: plain `std` threads and `std::sync::mpsc` channels, blocking I/O. No async
runtime (tokio/mio are not ported to QNX 6.5, which lacks `eventfd`, `pipe2`, `accept4`).

Threads:
- one per enabled source, run by a supervisor that restarts `Source::run` with backoff;
- one router thread (owns the last-value table);
- one publisher thread (owns the MQTT connection).

### 3.1 Signal catalog (`catalog.rs`)

A single static table defines every canonical signal once:

```rust
pub struct SignalDef {
    pub id: &'static str,            // "odometer", "service.oil.distance"
    pub name: &'static str,          // HA friendly name
    pub kind: Kind,                  // Sensor | BinarySensor | Tracker
    pub device_class: Option<&'static str>,
    pub state_class: Option<&'static str>,
    pub default_unit: Option<Unit>,  // used when the source gives no unit
    pub default_rate: Rate,          // enabled, min_interval, deadband, max_interval (default 15 min)
    pub icon: Option<&'static str>,
}
```

The router and publisher know only catalog ids. Signals with no current provider stay in
the catalog (e.g. `door.driver`) so a future source only adds mappings.

### 3.2 Values

```rust
pub enum Value {
    Num { v: f64, unit: Option<Unit> },
    Bool(bool),
    Enum(String),        // passed through verbatim, e.g. "position_P"
    Text(String),
    Geo { lat: f64, lon: f64, heading: Option<f64>, alt: Option<f64> },
    Unavailable,         // source is connected but has no valid value (EC_xx, nodata)
}

pub struct Update {
    pub signal: SignalId,
    pub value: Value,
    pub ts: Option<SystemTime>,   // source timestamp if given (ExLAP Dat timeStamp)
    pub source: SourceId,
}
```

`Unit` is a closed enum (`Mi, Km, Mph, Kmh, MpgUs, L100km, GalUs, L, Psi, Bar, KPa, DegF,
DegC, Percent, Rpm, Days, Seconds, Volt, …`) with a `ha_str()` giving Home Assistant's unit
string (`mi`, `mph`, `mpg`, `gal`, `°F`, `psi`, `%`, `rpm`, `d`, `s`, `V`, …). Sources convert
their own unit names into `Unit`; values are never converted, HA does display conversion.

### 3.3 The `Source` trait

```rust
pub trait Source: Send {
    fn id(&self) -> SourceId;
    fn provides(&self) -> &[SignalId];
    /// Blocks until `stop` is set or the source fails. Owns its own
    /// connect/auth/reconnect loop; returning Err makes the supervisor
    /// restart it after backoff.
    fn run(&mut self, tx: &Sender<Update>, stop: &StopFlag) -> Result<(), SourceError>;
}
```

- On losing its upstream (socket closed, auth failure) a source sends `Lost` for all of its
  signals before reconnecting. The router drops that source's inputs and falls back to
  another source, or keeps the last forwarded value if there is none (changed 2026-09-26:
  the head unit stops ExLAP before carmqtt at shutdown, and the user wants last-known
  values in HA while the car sleeps). `Unavailable` now means only "the source is up but
  has no value".
- Protocol code is generic over `Read + Write` so it can be driven by an in-memory or fake
  server in tests.
- Adding a source = one module implementing `Source` plus its own mapping table into
  catalog ids, plus a `[sources.<name>]` config section. Nothing else changes.

### 3.4 Router

- Keeps, per signal: the last value from each source, and the value last forwarded.
- Selection: the highest-priority source that currently has a non-`Unavailable` value; if
  none, `Unavailable`. Priority comes from `sources.<name>.priority` (higher wins).
- Throttle: forward when the selected value changes, subject to the signal's
  `min_interval` and numeric `deadband` (change smaller than deadband is suppressed, but the
  latest value is still published once `max_interval` elapses so HA converges). Transitions
  to/from `Unavailable` and all `Bool`/`Enum` changes bypass throttling.
- The reconnect snapshot is the publisher's own table of the last value it received from
  the router for each signal (the router forwards throttled values once their interval
  elapses, so that table converges to the current selection).

### 3.5 Publisher (MQTT)

Built-in minimal MQTT 3.1.1 client (`mqtt/`): CONNECT (with username/password and last
will), CONNACK, PUBLISH QoS 0/1 with PUBACK, SUBSCRIBE/SUBACK, PINGREQ/PINGRESP,
DISCONNECT. Sans-IO packet codec + a thin blocking `TcpStream` driver. No external MQTT
crate (existing ones pull in tokio or C libraries).

Topics (`<vin>` = VIN, lower-case; signal id dots become `/`):

| Topic | Payload | Retained | QoS |
|---|---|---|---|
| `mib2q/<vin>/status` | `online` / `offline` (LWT is `offline`) | yes | 1 |
| `mib2q/<vin>/state/<signal>` | plain value: `69760`, `ON`/`OFF`, `position_P` | yes | 1 |
| `mib2q/<vin>/avail/<signal>` | `online` / `offline` | yes | 1 |
| `mib2q/<vin>/location` | JSON `{latitude, longitude, gps_accuracy, heading, altitude}` | yes | 1 |
| `homeassistant/<component>/mib2q_<vin>/<signal_underscored>/config` | discovery JSON | yes | 1 |

Discovery JSON per signal: `name`, `unique_id` (`mib2q_<vin>_<signal>`), `state_topic`,
`availability` (the signal's own `avail` topic only, so entities keep their last
value while the car sleeps; changed 2026-09-26 at the user's request — a separate
"Online" `binary_sensor` (device_class connectivity) is driven by `mib2q/<vin>/status`), `device_class`, `state_class`, `unit_of_measurement`, `icon`,
and a shared `device` block (`identifiers: [mib2q_<vin>]`, `manufacturer: Audi`,
`model` from config, `name` from config). Location uses `device_tracker` with
`json_attributes_topic`.

Connection sequence (every connect): CONNECT with LWT → subscribe `homeassistant/status`
→ publish discovery for every signal provided by an enabled source → `status=online` →
snapshot of all current values and availabilities. When `homeassistant/status` becomes
`online`, repeat discovery + snapshot. Disconnect → reconnect with backoff 1 s → 60 s;
nothing is queued (the router's table is the only offline state, satisfying R3).

The VIN is required for topic names; the publisher does not connect until the router has a
`vin` value (ExLAP provides it without auth). `device.vin` in config overrides it.

## 4. ExLAP source (v1)

Module `sources/exlap/`: `xml.rs` (tolerant tag/attribute scanner for the small ExLAP
vocabulary, no DOM), `auth.rs` (SHA-256 digest, own implementation, test-vectored),
`session.rs` (state machine over `Read + Write`), `map.rs` (URL/field → signal table).

Session: connect → read `<Status><Init/>` → `Protocol version="1"` → Authenticate
challenge/response → `Heartbeat ival="10"` (best effort; whether the server then sends
anything unprompted is unverified) → one `Subscribe url ival timeStamp="true"` per mapped URL
(ival = the smallest `min_interval` among its enabled signals in ms, 1000 ms floor) → loop
reading `Dat` and `Rsp`. Liveness: after 20 s with nothing received the source sends
`<Get url="vehicleIdenticationNumber"/>`; no answer within 10 s = dead link. Credentials: the four known public pairs are
built in; `sources.exlap.user` picks one (default `RSE_3-DE1400`); `password` may override.

Field handling: `val` attributes parse by element type (`Abs`/`Rel` → number, `Act` →
bool, `Enm` → enum string, `Txt` → text). `state="error"` or `state="nodata"` →
`Unavailable`. A sibling `unit`/`distanceUnit`/`speedUnit`/`refillVolumeUnit` enum supplies
the `Unit`. Unknown enum values pass through as text; unparseable elements are logged once
per URL and skipped.

### 4.1 v1 mapping (ExLAP URL.field → catalog signal)

Enabled by default:

| Signal | ExLAP URL . field | HA | Notes |
|---|---|---|---|
| `vin` | `vehicleIdenticationNumber.VIN` | sensor (text) | also device id |
| `odometer` | `totalDistance.distanceValue` (+`unit`) | sensor, distance, total_increasing | |
| `fuel.level` | `tankLevelPrimary.level` ×100 | sensor, %, measurement | `state=valid` gate |
| `fuel.low` | `fuelWarningPrimaryTank.warningPrimaryTank` | binary_sensor, problem | |
| `oil.level` | `oilLevel.level` | sensor, measurement | raw bargraph step |
| `oil.status` | `oilLevel.warnings` | sensor (enum) | e.g. `oil_OK` |
| `service.inspection.distance` | `serviceInspection.distance` (+`distanceUnit`) | sensor, distance | |
| `service.inspection.days` | `serviceInspection.time` | sensor, duration, d | |
| `service.oil.distance` | `serviceOil.distance` (+`distanceUnit`) | sensor, distance | |
| `service.oil.days` | `serviceOil.time` | sensor, duration, d | |
| `ignition.on` | `clampState.clamp_15` | binary_sensor, power | |
| `ignition.key_in` | `clampState.clamp_S` | binary_sensor | |
| `parking_brake` | `parkingBrake.engaged` | binary_sensor | |
| `gear.selector` | `gearTransmissionMode.mode` | sensor (enum) | `position_P` etc. |
| `gear.current` | `currentGear.currentGear` | sensor (enum) | |
| `drive_mode` | `driveMode.activeProfile` | sensor (enum) | |
| `vehicle.speed` | `vehicleSpeed.speed` (+`unit`) | sensor, speed | 5 s, deadband 1 |
| `engine.rpm` | `engineSpeed.engineSpeed` | sensor, rpm | 5 s, deadband 50 |
| `transmission.oil_temp` | `gearboxOilTemperature.gearboxOilTemp` | sensor, temperature, °C | 30 s, deadband 1; unit assumed °C (URL carries no unit) and checked on-car in the plan |
| `economy.long_term` | `longTermConsumptionPrimary.value` (+`unit`) | sensor, measurement | |
| `economy.short_term` | `shortTermConsumptionPrimary.value` (+`unit`) | sensor, measurement | |
| `trip.long_term.distance` | `consumptionLongTermGeneral.distanceValue` (+`distanceUnit`) | sensor, distance | |
| `trip.long_term.avg_speed` | `consumptionLongTermGeneral.speedValue` (+`speedUnit`) | sensor, speed | |
| `trip.long_term.minutes` | `consumptionLongTermGeneral.time` | sensor, duration, min | |
| `trip.short_term.distance` / `.avg_speed` / `.minutes` | `consumptionShortTermGeneral.*` | as above | |

Mapped but disabled by default (enable per signal in config): `accel.lateral`,
`accel.longitudinal`, `yaw_rate`, `steering_angle` (`wheelAngle`), `brake_pressure`,
`accelerator_position`, `engine.torque`, `engine.power`, `reverse` (`reverseGear.engaged`).

Mapped, currently unavailable on this car (published as unavailable, so they light up if a
coding change or another source fixes them): `location` (`Nav_GeoPosition` +
`Nav_Heading` + `Nav_Altitude`), `temp.outside`, `temp.coolant`, `temp.oil`,
`battery.voltage`, `door.*`/`lock` (`doorState`), `tyre.pressure.{fl,fr,rl,rr}`
(`tyrePressures`).

## 5. Configuration

`/mnt/app/root/carmqtt/carmqtt.toml`, parsed with `serde` + `toml`. Every field except
`[mqtt].host` has a default.

```toml
[mqtt]
host = "homeassistant.tailnet-xxxx.ts.net"
port = 1883
username = "carmqtt"
password = "…"
client_id = "mib2q-carmqtt"
keepalive_s = 30
topic_prefix = "mib2q"
discovery_prefix = "homeassistant"

[device]
name = "Audi"
model = "A4"
# vin = "…"          # optional override

[log]
level = "info"        # error|warn|info|debug|trace

[sources.exlap]
enabled = true
addr = "127.0.0.1:25010"
user = "RSE_3-DE1400"
priority = 10

[signals]
"vehicle.speed"  = { min_interval_s = 5, deadband = 1, max_interval_s = 900 }
"accel.lateral"  = { enabled = true, min_interval_s = 1 }
```

Invalid config → log the error and exit non-zero (the wrapper retries after 10 s, so a fix
via `scp` takes effect without a reboot).

## 6. Deployment

Repo layout:

```
mib2q-carmqtt/
  Cargo.toml
  src/{main.rs, config.rs, catalog.rs, value.rs, router.rs, supervisor.rs, log.rs}
  src/mqtt/{codec.rs, client.rs}
  src/publisher.rs, src/discovery.rs
  src/sources/{mod.rs, exlap/{mod.rs, xml.rs, auth.rs, session.rs, map.rs}}
  tests/fixtures/exlap/…   (captured responses, VIN replaced with a fake)
  deploy/{carmqtt.sh, carmqtt.toml.example, install.sh, uninstall.sh}
  scripts/build.sh          (docker qnx65-armv7-toolchain :8.5-rust, build_std.sh)
```

On the unit:
- `/mnt/app/root/carmqtt/{carmqtt, carmqtt.sh, carmqtt.toml}` (persistent, not exec-capable).
- `carmqtt.sh start` copies the binary to `/tmp/carmqtt/` and runs it in a loop: on exit,
  sleep 10 s and restart. `carmqtt.sh stop` stops loop and daemon.
- Boot hook, one line in `start_late_drivers()` of `/mnt/system/etc/boot/startup.sh`:
  `( sleep 45; [ -x /mnt/app/root/carmqtt/carmqtt.sh ] && /mnt/app/root/carmqtt/carmqtt.sh start ) </dev/null >/dev/null 2>&1 &`
- `install.sh`: scp files, back up `startup.sh` to `startup.sh.pre-carmqtt`, insert the hook
  only if absent, verify with `ksh -n` and restore the backup on failure. `uninstall.sh`
  reverses it.
- Logs: `/tmp/carmqtt/carmqtt.log` (RAM), capped at 1 MB, rotated once to `.1`.

Build: nightly Rust via the toolchain's `build_std.sh` (`-Z build-std=std,panic_abort`),
target `armv7-unknown-nto-qnx650` (already `+strict-align`, softfp VFPv3-D16),
`panic = "abort"`. Dependencies are limited to pure-Rust crates that need no OS support
beyond what the 6.5 std port provides (`serde`, `toml`); no `getrandom` (cnonce comes from
`/dev/urandom`, which the unit has).

## 7. Error handling

| Failure | Behaviour |
|---|---|
| ExLAP TCP drop / auth fail / unanswered liveness probe | source sends `Unavailable` for its signals, reconnects with backoff 1 s → 60 s |
| Source `run` returns Err | supervisor logs it, restarts after backoff; other sources unaffected |
| Panic anywhere | release builds use `panic = "abort"`, so the process exits and `carmqtt.sh` restarts it after 10 s |
| Broker unreachable / drop | reconnect with backoff 1 s → 60 s; router keeps updating the in-RAM latest-value table; snapshot on reconnect |
| Unknown enum value | passed through as text |
| Unparseable XML element | logged once per URL, skipped |
| Invalid config | log + exit non-zero; wrapper retries every 10 s |
| Daemon crash | `carmqtt.sh` loop restarts it after 10 s |

## 8. Testing

Host (`cargo test` on macOS/Linux):
- `auth`: SHA-256 known vectors (`abc`, empty) and a digest computed from a fixed
  nonce/cnonce, including the 44-char truncation of the 45-char `==` passwords.
- `xml` + `map`: every captured response from 2026-09-25 (directory, the 52 live
  values, the `EC_01` and `nodata` shapes) parses to the expected `Update`s.
- `router`: priority merge across two fake sources, deadband/min-interval/max-interval,
  `Unavailable` fallback and recovery, snapshot contents.
- `mqtt::codec`: byte-exact encode/decode of each packet type against MQTT 3.1.1 examples,
  including remaining-length boundaries (127/128, 16383/16384).
- `discovery`: snapshot tests of generated config JSON for a sensor, a binary_sensor and the
  tracker.
- Integration: a fake ExLAP server thread replays fixtures to the real `ExlapSource`; the
  publisher talks to a local `mosquitto`; the test asserts the retained topics and payloads.

Target:
- Cross-build and run `carmqtt --self-test` in the toolchain's QEMU image (exercises std
  threads, TCP loopback, config parsing).
- On the car: run from `/tmp` by hand, compare HA entities against the 2026-09-25 sweep,
  confirm the `gearboxOilTemperature` unit by comparing with the value after a cold start
  (ambient) vs. warm, then run `install.sh` and verify it comes up after a reboot.

## 9. Future sources (out of scope, shaping constraints only)

- **DSI / eso comm (`DsiSource`)**: native client of MIBRoot's car DSI services (or
  `asi.dsi_spy.*`) via TCP `10.0.0.16:2140x` or `/dev/eso/*`; payload layouts from
  `ems_tables/dsi.*.ems_table.json`; handshake still to be reverse-engineered from
  `libcomm.so`. Would supply doors/locks, tyres, temperatures, battery voltage. Default
  priority above ExLAP.
- **HMI bridge (`HmiBridgeSource`)**: a Java listener injected into lsd via the class-override
  workflow, pushing JSON lines over a localhost socket that this source reads.

Both only need a `Source` impl, a mapping table into existing catalog ids, and a config
section.

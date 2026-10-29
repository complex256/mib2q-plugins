//! The canonical signal catalog. Router and publisher only know these ids;
//! each source maps its own data onto them.

use std::collections::HashMap;
use std::time::Duration;

use crate::value::{SignalId, Unit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sensor,
    BinarySensor,
    Tracker,
}

/// How often a signal may be forwarded.
///
/// A numeric change smaller than `deadband` is held back until `max_interval`
/// has passed since the last forward; any forward waits at least
/// `min_interval`. Bool/enum/text changes and availability changes ignore both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub enabled: bool,
    pub min_interval: Duration,
    pub deadband: f64,
    pub max_interval: Duration,
}

const MAX: Duration = Duration::from_secs(900);

impl Rate {
    pub const STATUS: Rate = Rate { enabled: true, min_interval: Duration::ZERO, deadband: 0.0, max_interval: MAX };

    pub const fn throttled(min_s: u64, deadband: f64) -> Rate {
        Rate { enabled: true, min_interval: Duration::from_secs(min_s), deadband, max_interval: MAX }
    }

    pub const fn off(min_s: u64, deadband: f64) -> Rate {
        Rate { enabled: false, min_interval: Duration::from_secs(min_s), deadband, max_interval: MAX }
    }
}

#[derive(Debug)]
pub struct SignalDef {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    pub device_class: Option<&'static str>,
    pub state_class: Option<&'static str>,
    pub default_unit: Option<Unit>,
    pub default_rate: Rate,
    pub icon: Option<&'static str>,
}

const fn sensor(
    id: &'static str,
    name: &'static str,
    device_class: Option<&'static str>,
    state_class: Option<&'static str>,
    default_unit: Option<Unit>,
    default_rate: Rate,
    icon: Option<&'static str>,
) -> SignalDef {
    SignalDef { id, name, kind: Kind::Sensor, device_class, state_class, default_unit, default_rate, icon }
}

const fn binary(
    id: &'static str,
    name: &'static str,
    device_class: Option<&'static str>,
    default_rate: Rate,
    icon: Option<&'static str>,
) -> SignalDef {
    SignalDef { id, name, kind: Kind::BinarySensor, device_class, state_class: None, default_unit: None, default_rate, icon }
}

const M: Option<&str> = Some("measurement");

pub static CATALOG: &[SignalDef] = &[
    // --- status, enabled ---
    sensor("vin", "VIN", None, None, None, Rate::STATUS, Some("mdi:card-text")),
    sensor("odometer", "Odometer", Some("distance"), Some("total_increasing"), Some(Unit::Mi), Rate::STATUS, Some("mdi:counter")),
    sensor("fuel.level", "Fuel level", None, M, Some(Unit::Percent), Rate::throttled(60, 1.0), Some("mdi:gas-station")),
    binary("fuel.low", "Fuel low", Some("problem"), Rate::STATUS, Some("mdi:gas-station-off")),
    sensor("oil.level", "Oil level", None, M, None, Rate::STATUS, Some("mdi:oil-level")),
    sensor("oil.status", "Oil status", None, None, None, Rate::STATUS, Some("mdi:oil")),
    sensor("service.inspection.distance", "Inspection due in", Some("distance"), M, Some(Unit::Mi), Rate::STATUS, Some("mdi:wrench-clock")),
    sensor("service.inspection.days", "Inspection due in (days)", Some("duration"), M, Some(Unit::Days), Rate::STATUS, Some("mdi:calendar-clock")),
    sensor("service.oil.distance", "Oil change due in", Some("distance"), M, Some(Unit::Mi), Rate::STATUS, Some("mdi:oil")),
    sensor("service.oil.days", "Oil change due in (days)", Some("duration"), M, Some(Unit::Days), Rate::STATUS, Some("mdi:calendar-clock")),
    binary("ignition.on", "Ignition", Some("power"), Rate::STATUS, Some("mdi:key")),
    binary("ignition.key_in", "Key present", None, Rate::STATUS, Some("mdi:key-variant")),
    binary("parking_brake", "Parking brake", None, Rate::STATUS, Some("mdi:car-brake-parking")),
    sensor("gear.selector", "Gear selector", None, None, None, Rate::STATUS, Some("mdi:car-shift-pattern")),
    sensor("gear.current", "Current gear", None, None, None, Rate::STATUS, Some("mdi:car-shift-pattern")),
    sensor("drive_mode", "Drive mode", None, None, None, Rate::STATUS, Some("mdi:car-cog")),
    // --- fast, enabled but throttled ---
    sensor("vehicle.speed", "Speed", Some("speed"), M, Some(Unit::Mph), Rate::throttled(5, 1.0), Some("mdi:speedometer")),
    sensor("engine.rpm", "Engine speed", None, M, Some(Unit::Rpm), Rate::throttled(5, 50.0), Some("mdi:engine")),
    sensor("transmission.oil_temp", "Transmission oil temperature", Some("temperature"), M, Some(Unit::DegC), Rate::throttled(30, 1.0), Some("mdi:thermometer")),
    sensor("economy.long_term", "Fuel economy (long term)", None, M, Some(Unit::MpgUs), Rate::throttled(60, 0.1), Some("mdi:fuel")),
    sensor("economy.short_term", "Fuel economy (since start)", None, M, Some(Unit::MpgUs), Rate::throttled(60, 0.1), Some("mdi:fuel")),
    sensor("trip.long_term.distance", "Trip distance (long term)", Some("distance"), M, Some(Unit::Mi), Rate::throttled(60, 0.1), Some("mdi:map-marker-distance")),
    sensor("trip.long_term.avg_speed", "Average speed (long term)", Some("speed"), M, Some(Unit::Mph), Rate::throttled(60, 1.0), Some("mdi:speedometer-medium")),
    sensor("trip.long_term.minutes", "Driving time (long term)", Some("duration"), M, Some(Unit::Minutes), Rate::throttled(60, 1.0), Some("mdi:timer-outline")),
    sensor("trip.short_term.distance", "Trip distance (since start)", Some("distance"), M, Some(Unit::Mi), Rate::throttled(60, 0.1), Some("mdi:map-marker-distance")),
    sensor("trip.short_term.avg_speed", "Average speed (since start)", Some("speed"), M, Some(Unit::Mph), Rate::throttled(60, 1.0), Some("mdi:speedometer-medium")),
    sensor("trip.short_term.minutes", "Driving time (since start)", Some("duration"), M, Some(Unit::Minutes), Rate::throttled(60, 1.0), Some("mdi:timer-outline")),
    // --- driving telemetry, disabled by default ---
    sensor("accel.lateral", "Lateral acceleration", None, M, None, Rate::off(1, 0.05), Some("mdi:axis-x-arrow")),
    sensor("accel.longitudinal", "Longitudinal acceleration", None, M, None, Rate::off(1, 0.05), Some("mdi:axis-y-arrow")),
    sensor("yaw_rate", "Yaw rate", None, M, None, Rate::off(1, 0.5), Some("mdi:rotate-3d-variant")),
    sensor("steering_angle", "Steering angle", None, M, None, Rate::off(1, 1.0), Some("mdi:steering")),
    sensor("brake_pressure", "Brake pressure", None, M, None, Rate::off(1, 0.5), Some("mdi:car-brake-alert")),
    sensor("accelerator_position", "Accelerator position", None, M, None, Rate::off(1, 1.0), Some("mdi:car-speed-limiter")),
    sensor("engine.torque", "Engine torque", None, M, None, Rate::off(1, 1.0), Some("mdi:engine")),
    sensor("engine.power", "Engine power", None, M, None, Rate::off(1, 1.0), Some("mdi:engine")),
    binary("reverse", "Reverse gear", None, Rate { enabled: false, ..Rate::STATUS }, Some("mdi:car-back")),
    // --- mapped, currently unavailable from ExLAP on this car ---
    SignalDef { id: "location", name: "Location", kind: Kind::Tracker, device_class: None, state_class: None, default_unit: None, default_rate: Rate::throttled(2, 0.0), icon: Some("mdi:car") },
    sensor("temp.outside", "Outside temperature", Some("temperature"), M, None, Rate::throttled(60, 0.5), Some("mdi:thermometer")),
    sensor("temp.coolant", "Coolant temperature", Some("temperature"), M, None, Rate::throttled(30, 1.0), Some("mdi:coolant-temperature")),
    sensor("temp.oil", "Oil temperature", Some("temperature"), M, None, Rate::throttled(30, 1.0), Some("mdi:oil-temperature")),
    sensor("battery.voltage", "Battery voltage", Some("voltage"), M, Some(Unit::Volt), Rate::throttled(60, 0.1), Some("mdi:car-battery")),
    sensor("lock", "Lock state", None, None, None, Rate::STATUS, Some("mdi:car-key")),
    sensor("door.driver", "Driver door", None, None, None, Rate::STATUS, Some("mdi:car-door")),
    sensor("door.passenger", "Passenger door", None, None, None, Rate::STATUS, Some("mdi:car-door")),
    sensor("door.rear_left", "Rear left door", None, None, None, Rate::STATUS, Some("mdi:car-door")),
    sensor("door.rear_right", "Rear right door", None, None, None, Rate::STATUS, Some("mdi:car-door")),
    sensor("door.hood", "Hood", None, None, None, Rate::STATUS, Some("mdi:car-door")),
    sensor("door.trunk", "Trunk", None, None, None, Rate::STATUS, Some("mdi:car-back")),
    sensor("tyre.pressure.fl", "Tyre pressure front left", Some("pressure"), M, None, Rate::throttled(60, 0.5), Some("mdi:car-tire-alert")),
    sensor("tyre.pressure.fr", "Tyre pressure front right", Some("pressure"), M, None, Rate::throttled(60, 0.5), Some("mdi:car-tire-alert")),
    sensor("tyre.pressure.rl", "Tyre pressure rear left", Some("pressure"), M, None, Rate::throttled(60, 0.5), Some("mdi:car-tire-alert")),
    sensor("tyre.pressure.rr", "Tyre pressure rear right", Some("pressure"), M, None, Rate::throttled(60, 0.5), Some("mdi:car-tire-alert")),
];

pub fn lookup(id: &str) -> Option<&'static SignalDef> {
    CATALOG.iter().find(|d| d.id == id)
}

/// Every catalog signal with its built-in rate (before config overrides).
pub fn default_rates() -> HashMap<SignalId, Rate> {
    CATALOG.iter().map(|d| (d.id, d.default_rate)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_well_formed() {
        let mut seen = HashSet::new();
        for d in CATALOG {
            assert!(seen.insert(d.id), "duplicate id {}", d.id);
            assert!(
                !d.id.is_empty()
                    && d.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
                "bad id {}",
                d.id
            );
        }
    }

    #[test]
    fn odometer_is_total_increasing_distance() {
        let d = lookup("odometer").unwrap();
        assert_eq!(d.kind, Kind::Sensor);
        assert_eq!(d.device_class, Some("distance"));
        assert_eq!(d.state_class, Some("total_increasing"));
        assert_eq!(d.default_unit, Some(Unit::Mi));
        assert!(lookup("nope").is_none());
    }

    #[test]
    fn telemetry_is_off_by_default() {
        let off: HashSet<&str> = CATALOG.iter().filter(|d| !d.default_rate.enabled).map(|d| d.id).collect();
        let expected: HashSet<&str> = [
            "accel.lateral", "accel.longitudinal", "yaw_rate", "steering_angle", "brake_pressure",
            "accelerator_position", "engine.torque", "engine.power", "reverse",
        ]
        .into_iter()
        .collect();
        assert_eq!(off, expected);
    }

    #[test]
    fn only_location_is_a_tracker_and_binaries_have_no_unit() {
        for d in CATALOG {
            assert_eq!(d.kind == Kind::Tracker, d.id == "location", "{}", d.id);
            if d.kind == Kind::BinarySensor {
                assert!(d.default_unit.is_none() && d.state_class.is_none(), "{}", d.id);
            }
        }
        assert_eq!(default_rates().len(), CATALOG.len());
    }
}

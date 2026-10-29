//! ExLAP URL/field -> catalog signal table and the decoder that applies it.

use super::xml::Element;
use crate::value::{SignalId, Unit, Value};
use crate::warn;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Conv {
    Num,
    /// Numeric, multiplied by the factor (ExLAP fuel level is 0..1).
    Scale(f64),
    Bool,
    Enum,
    Text,
    GeoLat,
    GeoLon,
    GeoHeading,
    GeoAltitude,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldMap {
    pub url: &'static str,
    pub field: &'static str,
    pub signal: SignalId,
    pub conv: Conv,
    /// Sibling field holding the unit enum, e.g. `"unit"` or `"distanceUnit"`.
    pub unit_field: Option<&'static str>,
    /// Fixed unit when the URL carries none.
    pub unit: Option<Unit>,
}

const fn f(url: &'static str, field: &'static str, signal: SignalId, conv: Conv) -> FieldMap {
    FieldMap { url, field, signal, conv, unit_field: None, unit: None }
}

const fn fu(url: &'static str, field: &'static str, signal: SignalId, unit_field: &'static str) -> FieldMap {
    FieldMap { url, field, signal, conv: Conv::Num, unit_field: Some(unit_field), unit: None }
}

const fn fx(url: &'static str, field: &'static str, signal: SignalId, unit: Unit) -> FieldMap {
    FieldMap { url, field, signal, conv: Conv::Num, unit_field: None, unit: Some(unit) }
}

pub static MAP: &[FieldMap] = &[
    f("vehicleIdenticationNumber", "VIN", "vin", Conv::Text),
    fu("totalDistance", "distanceValue", "odometer", "unit"),
    FieldMap { url: "tankLevelPrimary", field: "level", signal: "fuel.level", conv: Conv::Scale(100.0), unit_field: None, unit: Some(Unit::Percent) },
    f("fuelWarningPrimaryTank", "warningPrimaryTank", "fuel.low", Conv::Bool),
    f("oilLevel", "level", "oil.level", Conv::Num),
    f("oilLevel", "warnings", "oil.status", Conv::Enum),
    fu("serviceInspection", "distance", "service.inspection.distance", "distanceUnit"),
    fx("serviceInspection", "time", "service.inspection.days", Unit::Days),
    fu("serviceOil", "distance", "service.oil.distance", "distanceUnit"),
    fx("serviceOil", "time", "service.oil.days", Unit::Days),
    f("clampState", "clamp_15", "ignition.on", Conv::Bool),
    f("clampState", "clamp_S", "ignition.key_in", Conv::Bool),
    f("parkingBrake", "engaged", "parking_brake", Conv::Bool),
    f("gearTransmissionMode", "mode", "gear.selector", Conv::Enum),
    f("currentGear", "currentGear", "gear.current", Conv::Enum),
    f("driveMode", "activeProfile", "drive_mode", Conv::Enum),
    fu("vehicleSpeed", "speed", "vehicle.speed", "unit"),
    fx("engineSpeed", "engineSpeed", "engine.rpm", Unit::Rpm),
    fx("gearboxOilTemperature", "gearboxOilTemp", "transmission.oil_temp", Unit::DegC),
    fu("longTermConsumptionPrimary", "value", "economy.long_term", "unit"),
    fu("shortTermConsumptionPrimary", "value", "economy.short_term", "unit"),
    fu("consumptionLongTermGeneral", "distanceValue", "trip.long_term.distance", "distanceUnit"),
    fu("consumptionLongTermGeneral", "speedValue", "trip.long_term.avg_speed", "speedUnit"),
    fx("consumptionLongTermGeneral", "time", "trip.long_term.minutes", Unit::Minutes),
    fu("consumptionShortTermGeneral", "distanceValue", "trip.short_term.distance", "distanceUnit"),
    fu("consumptionShortTermGeneral", "speedValue", "trip.short_term.avg_speed", "speedUnit"),
    fx("consumptionShortTermGeneral", "time", "trip.short_term.minutes", Unit::Minutes),
    f("lateralAcceleration", "lateralAcceleration", "accel.lateral", Conv::Num),
    f("longitudinalAcceleration", "longitudinalAcceleration", "accel.longitudinal", Conv::Num),
    f("yawRate", "yawRate", "yaw_rate", Conv::Num),
    f("wheelAngle", "wheelAngle", "steering_angle", Conv::Num),
    f("brakePressure", "brakePressure", "brake_pressure", Conv::Num),
    f("acceleratorPosition", "acceleratorPosition", "accelerator_position", Conv::Num),
    f("currentTorque", "currentTorque", "engine.torque", Conv::Num),
    f("currentOutputPower", "currentOutputPower", "engine.power", Conv::Num),
    f("reverseGear", "engaged", "reverse", Conv::Bool),
    fu("outsideTemperature", "temperature", "temp.outside", "unit"),
    fu("coolantTemperature", "temperature", "temp.coolant", "unit"),
    fu("oilTemperature", "temperature", "temp.oil", "unit"),
    fx("batteryVoltage", "voltage", "battery.voltage", Unit::Volt),
    f("doorState", "lockstate", "lock", Conv::Enum),
    f("doorState", "driverDoorState", "door.driver", Conv::Enum),
    f("doorState", "coDriverDoorState", "door.passenger", Conv::Enum),
    f("doorState", "fondDoorLeftState", "door.rear_left", Conv::Enum),
    f("doorState", "fondDoorRightState", "door.rear_right", Conv::Enum),
    f("doorState", "hoodState", "door.hood", Conv::Enum),
    f("doorState", "trunkState", "door.trunk", Conv::Enum),
    fu("tyrePressures", "pressureFrontLeft", "tyre.pressure.fl", "pressureUnit"),
    fu("tyrePressures", "pressureFrontRight", "tyre.pressure.fr", "pressureUnit"),
    fu("tyrePressures", "pressureRearLeft", "tyre.pressure.rl", "pressureUnit"),
    fu("tyrePressures", "pressureRearRight", "tyre.pressure.rr", "pressureUnit"),
    f("Nav_GeoPosition", "Latitude", "location", Conv::GeoLat),
    f("Nav_GeoPosition", "Longitude", "location", Conv::GeoLon),
    f("Nav_Heading", "Heading", "location", Conv::GeoHeading),
    f("Nav_Altitude", "Altitude", "location", Conv::GeoAltitude),
];

pub fn unit_from_exlap(s: &str) -> Option<Unit> {
    Some(match s {
        "mi" => Unit::Mi,
        "km" => Unit::Km,
        "mph" => Unit::Mph,
        "kmh" | "km/h" | "km_h" => Unit::Kmh,
        "mpg_US" => Unit::MpgUs,
        "mpg_UK" => Unit::MpgUk,
        "l_100km" | "l/100km" | "L/100km" => Unit::L100km,
        "km_l" | "km/l" => Unit::KmL,
        "gallon_US" | "gal_US" => Unit::GalUs,
        "gallon_UK" | "gal_UK" => Unit::GalUk,
        "liter" | "l" | "L" => Unit::L,
        "psi" => Unit::Psi,
        "bar" => Unit::Bar,
        "kPa" | "kpa" => Unit::KPa,
        "°F" | "F" | "degF" => Unit::DegF,
        "°C" | "C" | "degC" => Unit::DegC,
        _ => return None,
    })
}

pub fn urls() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for m in MAP {
        if !out.contains(&m.url) {
            out.push(m.url);
        }
    }
    out
}

pub fn signals_for(url: &str) -> Vec<SignalId> {
    let mut out: Vec<SignalId> = Vec::new();
    for m in MAP.iter().filter(|m| m.url == url) {
        if !out.contains(&m.signal) {
            out.push(m.signal);
        }
    }
    out
}

pub fn all_signals() -> Vec<SignalId> {
    let mut out: Vec<SignalId> = Vec::new();
    for m in MAP {
        if !out.contains(&m.signal) {
            out.push(m.signal);
        }
    }
    out
}

fn field<'a>(fields: &'a [Element], name: &str) -> Option<&'a Element> {
    fields.iter().find(|e| e.attr("name") == Some(name))
}

/// The element's value, or `None` if it reports `state="error"`/`"nodata"`.
fn val(e: &Element) -> Option<&str> {
    if e.attr("state").is_some() {
        None
    } else {
        e.attr("val")
    }
}

fn num(e: Option<&Element>) -> Option<f64> {
    e.and_then(val).and_then(|v| v.parse::<f64>().ok())
}

#[derive(Default)]
pub struct Decoder {
    heading: Option<f64>,
    alt: Option<f64>,
    geo: Option<(f64, f64)>,
    warned: HashSet<(&'static str, &'static str)>,
}

impl Decoder {
    pub fn decode(&mut self, url: &str, fields: &[Element]) -> Vec<(SignalId, Value)> {
        let invalid = field(fields, "state")
            .and_then(|e| e.attr("val"))
            .map(|v| v != "valid")
            .unwrap_or(false);
        let mut out = Vec::new();
        for m in MAP.iter().filter(|m| m.url == url) {
            match m.conv {
                Conv::GeoLat => out.push((m.signal, self.geo_position(fields))),
                Conv::GeoLon => {}
                Conv::GeoHeading => {
                    self.heading = num(field(fields, m.field));
                    if let Some(g) = self.geo_value() {
                        out.push((m.signal, g));
                    }
                }
                Conv::GeoAltitude => {
                    self.alt = num(field(fields, m.field));
                    if let Some(g) = self.geo_value() {
                        out.push((m.signal, g));
                    }
                }
                _ => {
                    let Some(e) = field(fields, m.field) else { continue };
                    let value = if invalid { Value::Unavailable } else { self.convert(m, e, fields) };
                    out.push((m.signal, value));
                }
            }
        }
        out
    }

    fn geo_position(&mut self, fields: &[Element]) -> Value {
        match (num(field(fields, "Latitude")), num(field(fields, "Longitude"))) {
            (Some(lat), Some(lon)) => {
                self.geo = Some((lat, lon));
                self.geo_value().expect("just set")
            }
            _ => {
                self.geo = None;
                Value::Unavailable
            }
        }
    }

    fn geo_value(&self) -> Option<Value> {
        self.geo.map(|(lat, lon)| Value::Geo { lat, lon, heading: self.heading, alt: self.alt, speed: None })
    }

    fn convert(&mut self, m: &FieldMap, e: &Element, fields: &[Element]) -> Value {
        let Some(raw) = val(e) else { return Value::Unavailable };
        match m.conv {
            Conv::Num | Conv::Scale(_) => {
                let Ok(v) = raw.parse::<f64>() else {
                    if self.warned.insert((m.url, m.field)) {
                        warn!("exlap {}.{}: not a number: {raw:?}", m.url, m.field);
                    }
                    return Value::Unavailable;
                };
                let v = if let Conv::Scale(k) = m.conv { v * k } else { v };
                let unit = match m.unit_field.and_then(|uf| field(fields, uf)).and_then(val) {
                    Some(u) => {
                        let parsed = unit_from_exlap(u);
                        if parsed.is_none() && self.warned.insert((m.url, m.field)) {
                            warn!("exlap {}.{}: unknown unit {u:?}", m.url, m.field);
                        }
                        parsed
                    }
                    None => m.unit,
                };
                Value::Num { v, unit }
            }
            Conv::Bool => match raw {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                other => Value::Enum(other.to_string()),
            },
            Conv::Enum => Value::Enum(raw.to_string()),
            Conv::Text => Value::Text(raw.to_string()),
            Conv::GeoLat | Conv::GeoLon | Conv::GeoHeading | Conv::GeoAltitude => Value::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::xml::{parse, start_tags, Msg};
    use super::*;
    use crate::catalog;
    use std::collections::{HashMap, HashSet};

    fn fixtures() -> HashMap<String, String> {
        serde_json::from_str(include_str!("../../../tests/fixtures/exlap/get_responses.json")).unwrap()
    }

    fn decode_fixture(d: &mut Decoder, url: &str) -> Vec<(SignalId, Value)> {
        let fx = fixtures();
        match parse(&fx[url]) {
            Msg::Rsp { children, .. } => d.decode(url, &children),
            other => panic!("{other:?}"),
        }
    }

    fn one(d: &mut Decoder, url: &str) -> HashMap<SignalId, Value> {
        decode_fixture(d, url).into_iter().collect()
    }

    #[test]
    fn every_mapped_signal_is_in_the_catalog_and_every_url_exists() {
        for s in all_signals() {
            assert!(catalog::lookup(s).is_some(), "{s} missing from catalog");
        }
        let dir: HashMap<String, Vec<String>> =
            serde_json::from_str(include_str!("../../../tests/fixtures/exlap/directory.json")).unwrap();
        let known: HashSet<&str> = dir.values().flatten().map(String::as_str).collect();
        for u in urls() {
            assert!(known.contains(u), "{u} is not an ExLAP URL on this car");
        }
    }

    #[test]
    fn decodes_live_values() {
        let mut d = Decoder::default();
        assert_eq!(one(&mut d, "totalDistance")["odometer"], Value::Num { v: 69760.0, unit: Some(Unit::Mi) });
        match &one(&mut d, "tankLevelPrimary")["fuel.level"] {
            Value::Num { v, unit } => {
                assert!((v - 66.0).abs() < 1e-9);
                assert_eq!(*unit, Some(Unit::Percent));
            }
            other => panic!("{other:?}"),
        }
        let oil = one(&mut d, "oilLevel");
        assert_eq!(oil["oil.level"], Value::Num { v: 6.0, unit: None });
        assert_eq!(oil["oil.status"], Value::Enum("oil_OK".into()));
        let insp = one(&mut d, "serviceInspection");
        assert_eq!(insp["service.inspection.distance"], Value::Num { v: 10600.0, unit: Some(Unit::Mi) });
        assert_eq!(insp["service.inspection.days"], Value::Num { v: 251.0, unit: Some(Unit::Days) });
        let clamp = one(&mut d, "clampState");
        assert_eq!(clamp["ignition.on"], Value::Bool(true));
        assert_eq!(clamp["ignition.key_in"], Value::Bool(true));
        assert_eq!(one(&mut d, "gearTransmissionMode")["gear.selector"], Value::Enum("position_P".into()));
        assert_eq!(one(&mut d, "vehicleSpeed")["vehicle.speed"], Value::Num { v: 0.0, unit: Some(Unit::Mph) });
        assert_eq!(one(&mut d, "vehicleIdenticationNumber")["vin"], Value::Text("TESTVIN0000000000".into()));
        let trip = one(&mut d, "consumptionLongTermGeneral");
        assert_eq!(trip["trip.long_term.distance"], Value::Num { v: 100.8, unit: Some(Unit::Mi) });
        assert_eq!(trip["trip.long_term.avg_speed"], Value::Num { v: 10.0, unit: Some(Unit::Mph) });
        assert!(matches!(trip["trip.long_term.minutes"], Value::Num { unit: Some(Unit::Minutes), .. }));
        assert_eq!(one(&mut d, "lateralAcceleration")["accel.lateral"], Value::Num { v: -0.02, unit: None });
    }

    #[test]
    fn errors_and_nodata_are_unavailable() {
        let mut d = Decoder::default();
        let doors = one(&mut d, "doorState");
        for s in ["lock", "door.driver", "door.passenger", "door.rear_left", "door.rear_right", "door.hood", "door.trunk"] {
            assert_eq!(doors[s], Value::Unavailable, "{s}");
        }
        assert_eq!(one(&mut d, "coolantTemperature")["temp.coolant"], Value::Unavailable);
        assert_eq!(one(&mut d, "tyrePressures")["tyre.pressure.fl"], Value::Unavailable);
        assert_eq!(one(&mut d, "Nav_GeoPosition")["location"], Value::Unavailable);
    }

    #[test]
    fn invalid_state_field_marks_url_unavailable() {
        let mut d = Decoder::default();
        let fields = start_tags(r#"<Enm name="state" val="invalid"/><Rel name="level" val=".5"/>"#);
        assert_eq!(d.decode("tankLevelPrimary", &fields), vec![("fuel.level", Value::Unavailable)]);
    }

    #[test]
    fn location_combines_position_heading_altitude() {
        let mut d = Decoder::default();
        assert!(d.decode("Nav_Heading", &start_tags(r#"<Abs name="Heading" val="90"/>"#)).is_empty());
        let got = d.decode("Nav_GeoPosition", &start_tags(r#"<Abs name="Longitude" val="11.5"/><Abs name="Latitude" val="48.1"/>"#));
        assert_eq!(got, vec![("location", Value::Geo { lat: 48.1, lon: 11.5, heading: Some(90.0), alt: None, speed: None })]);
        let got = d.decode("Nav_Altitude", &start_tags(r#"<Abs name="Altitude" val="500"/>"#));
        assert_eq!(got, vec![("location", Value::Geo { lat: 48.1, lon: 11.5, heading: Some(90.0), alt: Some(500.0), speed: None })]);
    }

    #[test]
    fn unknown_url_and_units() {
        let mut d = Decoder::default();
        assert!(d.decode("Media_Track", &[]).is_empty());
        assert_eq!(unit_from_exlap("mpg_US"), Some(Unit::MpgUs));
        assert_eq!(unit_from_exlap("furlong"), None);
        let got = d.decode("totalDistance", &start_tags(r#"<Abs name="distanceValue" val="5"/><Enm name="unit" val="km"/>"#));
        assert_eq!(got, vec![("odometer", Value::Num { v: 5.0, unit: Some(Unit::Km) })]);
    }

    #[test]
    fn warns_once_per_field() {
        let mut d = Decoder::default();
        let fields = start_tags(r#"<Abs name="distanceValue" val="not_a_number"/><Enm name="unit" val="mi"/>"#);
        let got1 = d.decode("totalDistance", &fields);
        assert_eq!(got1, vec![("odometer", Value::Unavailable)]);
        assert!(d.warned.contains(&("totalDistance", "distanceValue")));

        let got2 = d.decode("totalDistance", &fields);
        assert_eq!(got2, vec![("odometer", Value::Unavailable)]);
    }
}

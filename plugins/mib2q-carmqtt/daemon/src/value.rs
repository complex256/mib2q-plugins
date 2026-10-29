//! Canonical values that flow from sources through the router to the publisher.

use std::time::SystemTime;

/// A catalog signal id such as `"odometer"` or `"service.oil.distance"`.
pub type SignalId = &'static str;
/// A source id such as `"exlap"`.
pub type SourceId = &'static str;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Mi,
    Km,
    Mph,
    Kmh,
    MpgUs,
    MpgUk,
    L100km,
    KmL,
    GalUs,
    GalUk,
    L,
    Psi,
    Bar,
    KPa,
    DegF,
    DegC,
    Percent,
    Rpm,
    Days,
    Minutes,
    Seconds,
    Volt,
}

impl Unit {
    /// The unit string Home Assistant expects in `unit_of_measurement`.
    pub fn ha_str(self) -> &'static str {
        match self {
            Unit::Mi => "mi",
            Unit::Km => "km",
            Unit::Mph => "mph",
            Unit::Kmh => "km/h",
            Unit::MpgUs | Unit::MpgUk => "mpg",
            Unit::L100km => "L/100km",
            Unit::KmL => "km/L",
            Unit::GalUs | Unit::GalUk => "gal",
            Unit::L => "L",
            Unit::Psi => "psi",
            Unit::Bar => "bar",
            Unit::KPa => "kPa",
            Unit::DegF => "°F",
            Unit::DegC => "°C",
            Unit::Percent => "%",
            Unit::Rpm => "rpm",
            Unit::Days => "d",
            Unit::Minutes => "min",
            Unit::Seconds => "s",
            Unit::Volt => "V",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num { v: f64, unit: Option<Unit> },
    Bool(bool),
    /// Passed through verbatim, e.g. `"position_P"`.
    Enum(String),
    Text(String),
    Geo { lat: f64, lon: f64, heading: Option<f64>, alt: Option<f64>, speed: Option<f64> },
    /// The source is up but has no valid value (ExLAP `EC_xx` / `nodata`).
    Unavailable,
    /// The source lost its upstream connection. Never published: the router
    /// drops that source's input and keeps the last value unless another
    /// source has one (so HA keeps last-known values while the car sleeps).
    Lost,
}

impl Value {
    pub fn is_available(&self) -> bool {
        !matches!(self, Value::Unavailable | Value::Lost)
    }

    pub fn unit(&self) -> Option<Unit> {
        match self {
            Value::Num { unit, .. } => *unit,
            _ => None,
        }
    }

    /// The MQTT state payload; `None` for `Unavailable`.
    pub fn payload(&self) -> Option<String> {
        match self {
            Value::Num { v, .. } => Some(format_num(*v)),
            Value::Bool(b) => Some(if *b { "ON" } else { "OFF" }.to_string()),
            Value::Enum(s) | Value::Text(s) => Some(s.clone()),
            Value::Geo { lat, lon, heading, alt, speed } => {
                let mut m = serde_json::Map::new();
                m.insert("latitude".into(), serde_json::json!(lat));
                m.insert("longitude".into(), serde_json::json!(lon));
                m.insert("gps_accuracy".into(), serde_json::json!(10));
                if let Some(h) = heading {
                    m.insert("heading".into(), serde_json::json!(h));
                }
                if let Some(a) = alt {
                    m.insert("altitude".into(), serde_json::json!(a));
                }
                // mph, rides with each fix so HA records speed per GPS point.
                if let Some(s) = speed {
                    m.insert("speed".into(), serde_json::json!(s));
                }
                Some(serde_json::Value::Object(m).to_string())
            }
            Value::Unavailable | Value::Lost => None,
        }
    }
}

/// Format a number with at most 3 decimals and no trailing zeros:
/// `69760.0 -> "69760"`, `66.00000000000001 -> "66"`, `-0.02 -> "-0.02"`.
pub fn format_num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    if s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub signal: SignalId,
    pub value: Value,
    /// Source-side timestamp if the source has one (unused by v1 publishing).
    pub ts: Option<SystemTime>,
    pub source: SourceId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(format_num(69760.0), "69760");
        assert_eq!(format_num(66.00000000000001), "66");
        assert_eq!(format_num(-0.02), "-0.02");
        assert_eq!(format_num(16.7), "16.7");
        assert_eq!(format_num(-0.0001), "0");
        assert_eq!(format_num(1234.5), "1234.5");
    }

    #[test]
    fn payloads() {
        assert_eq!(Value::Bool(true).payload().as_deref(), Some("ON"));
        assert_eq!(Value::Bool(false).payload().as_deref(), Some("OFF"));
        assert_eq!(Value::Enum("position_P".into()).payload().as_deref(), Some("position_P"));
        assert_eq!(Value::Unavailable.payload(), None);
        let n = Value::Num { v: 8200.0, unit: Some(Unit::Mi) };
        assert_eq!(n.payload().as_deref(), Some("8200"));
        assert_eq!(n.unit(), Some(Unit::Mi));
        assert!(n.is_available());
        assert!(!Value::Unavailable.is_available());
    }

    #[test]
    fn geo_payload_is_json() {
        let g = Value::Geo { lat: 48.1, lon: 11.5, heading: None, alt: None, speed: None };
        assert_eq!(
            g.payload().unwrap(),
            r#"{"gps_accuracy":10,"latitude":48.1,"longitude":11.5}"#
        );
        let g = Value::Geo { lat: 1.0, lon: 2.0, heading: Some(90.0), alt: Some(500.0), speed: Some(42.5) };
        let v: serde_json::Value = serde_json::from_str(&g.payload().unwrap()).unwrap();
        assert_eq!(v["heading"], 90.0);
        assert_eq!(v["altitude"], 500.0);
        assert_eq!(v["speed"], 42.5);
    }

    #[test]
    fn ha_units() {
        assert_eq!(Unit::Mi.ha_str(), "mi");
        assert_eq!(Unit::DegF.ha_str(), "°F");
        assert_eq!(Unit::MpgUs.ha_str(), "mpg");
        assert_eq!(Unit::Days.ha_str(), "d");
    }
}

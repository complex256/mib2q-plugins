//! MQTT topic layout and Home Assistant discovery payloads.

use serde_json::{json, Map, Value as J};

use crate::catalog::{Kind, SignalDef};
use crate::value::Unit;

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct Topics {
    prefix: String,
    discovery_prefix: String,
    vin: String,
    vin_upper: String,
}

impl Topics {
    pub fn new(prefix: &str, discovery_prefix: &str, vin: &str) -> Topics {
        Topics {
            prefix: prefix.trim_end_matches('/').to_string(),
            discovery_prefix: discovery_prefix.trim_end_matches('/').to_string(),
            vin: vin.to_ascii_lowercase(),
            vin_upper: vin.to_ascii_uppercase(),
        }
    }

    pub fn vin(&self) -> &str {
        &self.vin
    }

    pub fn status(&self) -> String {
        format!("{}/{}/status", self.prefix, self.vin)
    }

    pub fn state(&self, signal: &str) -> String {
        format!("{}/{}/state/{}", self.prefix, self.vin, signal.replace('.', "/"))
    }

    pub fn avail(&self, signal: &str) -> String {
        format!("{}/{}/avail/{}", self.prefix, self.vin, signal.replace('.', "/"))
    }

    pub fn location(&self) -> String {
        format!("{}/{}/location", self.prefix, self.vin)
    }

    pub fn ha_status(&self) -> String {
        format!("{}/status", self.discovery_prefix)
    }

    pub fn object_id(&self, signal: &str) -> String {
        format!("mib2q_{}_{}", self.vin, signal.replace('.', "_"))
    }

    pub fn config(&self, def: &SignalDef) -> String {
        format!("{}/{}/mib2q_{}/{}/config", self.discovery_prefix, component(def.kind), self.vin, def.id.replace('.', "_"))
    }

    pub fn online_config(&self) -> String {
        format!("{}/binary_sensor/mib2q_{}/online/config", self.discovery_prefix, self.vin)
    }

    /// Where the value of `def` is published.
    pub fn value_topic(&self, def: &SignalDef) -> String {
        match def.kind {
            Kind::Tracker => self.location(),
            _ => self.state(def.id),
        }
    }
}

pub fn component(kind: Kind) -> &'static str {
    match kind {
        Kind::Sensor => "sensor",
        Kind::BinarySensor => "binary_sensor",
        Kind::Tracker => "device_tracker",
    }
}

fn device_json(t: &Topics, dev: &DeviceInfo) -> J {
    json!({
        "identifiers": [format!("mib2q_{}", t.vin)],
        "manufacturer": "Audi",
        "model": dev.model,
        "name": dev.name,
        "serial_number": t.vin_upper,
    })
}

/// The "Online" connectivity entity, driven by the status topic (which the
/// broker sets to `offline` via the last will when the head unit sleeps).
pub fn online_config_payload(t: &Topics, dev: &DeviceInfo) -> String {
    json!({
        "name": "Online",
        "unique_id": t.object_id("online"),
        "state_topic": t.status(),
        "payload_on": "online",
        "payload_off": "offline",
        "device_class": "connectivity",
        "device": device_json(t, dev),
    })
    .to_string()
}

pub fn config_payload(t: &Topics, def: &SignalDef, unit: Option<Unit>, dev: &DeviceInfo) -> String {
    let mut m = Map::new();
    m.insert("name".into(), json!(def.name));
    m.insert("unique_id".into(), json!(t.object_id(def.id)));
    // Availability follows only this signal's own flag, not the car's online
    // status: when the head unit sleeps, HA keeps showing the last value. The
    // separate "Online" entity (online_config_payload) shows connectivity.
    m.insert("availability".into(), json!([{ "topic": t.avail(def.id) }]));
    m.insert("device".into(), device_json(t, dev));
    if let Some(icon) = def.icon {
        m.insert("icon".into(), json!(icon));
    }
    match def.kind {
        Kind::Sensor => {
            m.insert("state_topic".into(), json!(t.state(def.id)));
            let effective_unit = unit.or(def.default_unit);
            // HA warns on a device_class with no unit_of_measurement, so
            // omit it rather than announce a value it will reject.
            if let (Some(dc), Some(_)) = (def.device_class, effective_unit) {
                m.insert("device_class".into(), json!(dc));
            }
            if let Some(sc) = def.state_class {
                m.insert("state_class".into(), json!(sc));
            }
            if let Some(u) = effective_unit {
                m.insert("unit_of_measurement".into(), json!(u.ha_str()));
            }
        }
        Kind::BinarySensor => {
            m.insert("state_topic".into(), json!(t.state(def.id)));
            m.insert("payload_on".into(), json!("ON"));
            m.insert("payload_off".into(), json!("OFF"));
            if let Some(dc) = def.device_class {
                m.insert("device_class".into(), json!(dc));
            }
        }
        Kind::Tracker => {
            m.insert("json_attributes_topic".into(), json!(t.location()));
            m.insert("source_type".into(), json!("gps"));
        }
    }
    J::Object(m).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::lookup;

    fn t() -> Topics {
        Topics::new("mib2q", "homeassistant", "TESTVIN0000000000")
    }

    fn dev() -> DeviceInfo {
        DeviceInfo { name: "Audi".into(), model: "A4".into() }
    }

    #[test]
    fn topic_layout() {
        let t = t();
        assert_eq!(t.vin(), "wauzzz8w0ja000001");
        assert_eq!(t.status(), "mib2q/wauzzz8w0ja000001/status");
        assert_eq!(t.state("service.oil.distance"), "mib2q/wauzzz8w0ja000001/state/service/oil/distance");
        assert_eq!(t.avail("fuel.level"), "mib2q/wauzzz8w0ja000001/avail/fuel/level");
        assert_eq!(t.ha_status(), "homeassistant/status");
        assert_eq!(t.config(lookup("parking_brake").unwrap()), "homeassistant/binary_sensor/mib2q_wauzzz8w0ja000001/parking_brake/config");
        assert_eq!(t.config(lookup("service.oil.days").unwrap()), "homeassistant/sensor/mib2q_wauzzz8w0ja000001/service_oil_days/config");
        assert_eq!(t.value_topic(lookup("location").unwrap()), "mib2q/wauzzz8w0ja000001/location");
    }

    #[test]
    fn sensor_payload_uses_value_unit_over_default() {
        let t = t();
        let j: J = serde_json::from_str(&config_payload(&t, lookup("odometer").unwrap(), Some(Unit::Km), &dev())).unwrap();
        assert_eq!(j["unit_of_measurement"], "km");
        assert_eq!(j["device_class"], "distance");
        assert_eq!(j["state_class"], "total_increasing");
        assert_eq!(j["state_topic"], "mib2q/wauzzz8w0ja000001/state/odometer");
        assert_eq!(j["unique_id"], "mib2q_wauzzz8w0ja000001_odometer");
        assert!(j.get("availability_mode").is_none());
        assert_eq!(j["availability"].as_array().unwrap().len(), 1, "car status must not gate sensors");
        assert_eq!(j["availability"][0]["topic"], "mib2q/wauzzz8w0ja000001/avail/odometer");
        assert_eq!(j["device"]["identifiers"][0], "mib2q_wauzzz8w0ja000001");
        assert_eq!(j["device"]["serial_number"], "TESTVIN0000000000");
        assert_eq!(j["device"]["model"], "A4");
        let j: J = serde_json::from_str(&config_payload(&t, lookup("odometer").unwrap(), None, &dev())).unwrap();
        assert_eq!(j["unit_of_measurement"], "mi");
        let j: J = serde_json::from_str(&config_payload(&t, lookup("gear.selector").unwrap(), None, &dev())).unwrap();
        assert!(j.get("unit_of_measurement").is_none());
    }

    #[test]
    fn online_entity_tracks_status_topic() {
        let t = t();
        assert_eq!(t.online_config(), "homeassistant/binary_sensor/mib2q_wauzzz8w0ja000001/online/config");
        let j: J = serde_json::from_str(&online_config_payload(&t, &dev())).unwrap();
        assert_eq!(j["state_topic"], "mib2q/wauzzz8w0ja000001/status");
        assert_eq!(j["payload_on"], "online");
        assert_eq!(j["payload_off"], "offline");
        assert_eq!(j["device_class"], "connectivity");
        assert_eq!(j["unique_id"], "mib2q_wauzzz8w0ja000001_online");
        assert_eq!(j["device"]["identifiers"][0], "mib2q_wauzzz8w0ja000001");
        assert!(j.get("availability").is_none());
    }

    #[test]
    fn sensor_with_device_class_but_no_unit_omits_device_class() {
        let t = t();
        // temp.coolant has a device_class ("temperature") but no default_unit,
        // and ExLAP never reports a per-value unit for it; HA warns on
        // device_class without unit_of_measurement, so it must be omitted.
        let def = lookup("temp.coolant").unwrap();
        assert!(def.device_class.is_some());
        assert!(def.default_unit.is_none());
        let j: J = serde_json::from_str(&config_payload(&t, def, None, &dev())).unwrap();
        assert!(j.get("device_class").is_none(), "{j}");
        assert!(j.get("unit_of_measurement").is_none(), "{j}");
        // once a unit is known (from the value itself), device_class comes back
        let j: J = serde_json::from_str(&config_payload(&t, def, Some(Unit::DegC), &dev())).unwrap();
        assert_eq!(j["device_class"], "temperature");
        assert_eq!(j["unit_of_measurement"], "°C");
    }

    #[test]
    fn binary_and_tracker_payloads() {
        let t = t();
        let j: J = serde_json::from_str(&config_payload(&t, lookup("ignition.on").unwrap(), None, &dev())).unwrap();
        assert_eq!(j["payload_on"], "ON");
        assert_eq!(j["device_class"], "power");
        let j: J = serde_json::from_str(&config_payload(&t, lookup("location").unwrap(), None, &dev())).unwrap();
        assert_eq!(j["json_attributes_topic"], "mib2q/wauzzz8w0ja000001/location");
        assert_eq!(j["source_type"], "gps");
        assert!(j.get("state_topic").is_none());
    }
}

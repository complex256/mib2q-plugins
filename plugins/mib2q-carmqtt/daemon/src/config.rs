//! `carmqtt.toml`. Everything except `[mqtt].host` has a default.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::catalog::{self, Rate};
use crate::log::Level;
use crate::value::SignalId;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mqtt: MqttCfg,
    #[serde(default)]
    pub device: DeviceCfg,
    #[serde(default)]
    pub log: LogCfg,
    #[serde(default)]
    pub sources: SourcesCfg,
    #[serde(default)]
    pub status: StatusCfg,
    #[serde(default)]
    pub signals: BTreeMap<String, SignalOverride>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MqttCfg {
    pub host: String,
    #[serde(default = "d_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "d_client_id")]
    pub client_id: String,
    #[serde(default = "d_keepalive")]
    pub keepalive_s: u64,
    #[serde(default = "d_topic_prefix")]
    pub topic_prefix: String,
    #[serde(default = "d_discovery_prefix")]
    pub discovery_prefix: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceCfg {
    #[serde(default = "d_device_name")]
    pub name: String,
    #[serde(default = "d_device_model")]
    pub model: String,
    #[serde(default)]
    pub vin: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogCfg {
    #[serde(default = "d_log_level")]
    pub level: String,
    #[serde(default = "d_log_file")]
    pub file: String,
    #[serde(default = "d_log_max")]
    pub max_bytes: u64,
}

/// Local status endpoint (127.0.0.1 only) read by the HMI plugin.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusCfg {
    #[serde(default = "d_status_port")]
    pub port: u16,
}

impl Default for StatusCfg {
    fn default() -> Self {
        StatusCfg { port: d_status_port() }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcesCfg {
    #[serde(default)]
    pub exlap: ExlapCfg,
    #[serde(default)]
    pub ubx: UbxCfg,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExlapCfg {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_exlap_addr")]
    pub addr: String,
    #[serde(default = "d_exlap_user")]
    pub user: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "d_exlap_priority")]
    pub priority: i32,
}

/// UBX GPS source: streams ublox UBX from the RCC's nav device and emits `location`.
/// Disabled by default (needs the car + QNX `on`). The command must stream raw UBX to
/// stdout; the default runs `cat` on the RCC node over Qnet.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UbxCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "d_ubx_command")]
    pub command: String,
    #[serde(default = "d_ubx_args")]
    pub args: Vec<String>,
    #[serde(default = "d_ubx_priority")]
    pub priority: i32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SignalOverride {
    pub enabled: Option<bool>,
    pub min_interval_s: Option<f64>,
    pub deadband: Option<f64>,
    pub max_interval_s: Option<f64>,
}

fn d_port() -> u16 { 1883 }
fn d_client_id() -> String { "mib2q-carmqtt".into() }
fn d_keepalive() -> u64 { 30 }
fn d_topic_prefix() -> String { "mib2q".into() }
fn d_discovery_prefix() -> String { "homeassistant".into() }
fn d_device_name() -> String { "Audi".into() }
fn d_device_model() -> String { "MIB2 High".into() }
fn d_log_level() -> String { "info".into() }
fn d_log_file() -> String { "/tmp/carmqtt.log".into() }
fn d_log_max() -> u64 { 1_048_576 }
fn d_status_port() -> u16 { 19851 }
fn d_true() -> bool { true }
fn d_exlap_addr() -> String { "127.0.0.1:25010".into() }
fn d_exlap_user() -> String { "RSE_3-DE1400".into() }
fn d_exlap_priority() -> i32 { 10 }
fn d_ubx_command() -> String { "on".into() }
fn d_ubx_args() -> Vec<String> { ["-f", "rcc", "cat", "/dev/nav/gps"].iter().map(|s| s.to_string()).collect() }
fn d_ubx_priority() -> i32 { 20 }

impl Default for DeviceCfg {
    fn default() -> Self {
        DeviceCfg { name: d_device_name(), model: d_device_model(), vin: None }
    }
}

impl Default for LogCfg {
    fn default() -> Self {
        LogCfg { level: d_log_level(), file: d_log_file(), max_bytes: d_log_max() }
    }
}

impl Default for UbxCfg {
    fn default() -> Self {
        UbxCfg { enabled: false, command: d_ubx_command(), args: d_ubx_args(), priority: d_ubx_priority() }
    }
}

impl Default for ExlapCfg {
    fn default() -> Self {
        ExlapCfg { enabled: true, addr: d_exlap_addr(), user: d_exlap_user(), password: None, priority: d_exlap_priority() }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        if cfg.mqtt.host.trim().is_empty() {
            return Err("mqtt.host must not be empty".into());
        }
        if Level::parse(&cfg.log.level).is_none() {
            return Err(format!("log.level '{}' is not one of error|warn|info|debug|trace", cfg.log.level));
        }
        for (id, o) in &cfg.signals {
            if catalog::lookup(id).is_none() {
                return Err(format!("unknown signal '{id}' in [signals]"));
            }
            for (name, v) in [("min_interval_s", o.min_interval_s), ("deadband", o.deadband), ("max_interval_s", o.max_interval_s)] {
                if let Some(v) = v {
                    if !(v.is_finite() && v >= 0.0) {
                        return Err(format!("signals.\"{id}\".{name} must be a non-negative number"));
                    }
                    if (name == "min_interval_s" || name == "max_interval_s") && v > 86400.0 {
                        return Err(format!("signals.\"{id}\".{name} must be <= 86400 (one day)"));
                    }
                }
            }
        }
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Config::parse(&text)
    }

    /// The catalog's default rates with `[signals]` overrides applied.
    pub fn effective_rates(&self) -> HashMap<SignalId, Rate> {
        let mut rates = catalog::default_rates();
        for (id, o) in &self.signals {
            let Some(def) = catalog::lookup(id) else { continue };
            let r = rates.get_mut(def.id).expect("default_rates covers the catalog");
            if let Some(e) = o.enabled {
                r.enabled = e;
            }
            if let Some(s) = o.min_interval_s {
                r.min_interval = Duration::from_secs_f64(s);
            }
            if let Some(d) = o.deadband {
                r.deadband = d;
            }
            if let Some(s) = o.max_interval_s {
                r.max_interval = Duration::from_secs_f64(s);
            }
        }
        rates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_gets_defaults() {
        let c = Config::parse("[mqtt]\nhost = \"broker\"\n").unwrap();
        assert_eq!(c.mqtt.port, 1883);
        assert_eq!(c.mqtt.client_id, "mib2q-carmqtt");
        assert_eq!(c.mqtt.keepalive_s, 30);
        assert_eq!(c.mqtt.topic_prefix, "mib2q");
        assert_eq!(c.mqtt.discovery_prefix, "homeassistant");
        assert_eq!(c.device.name, "Audi");
        assert_eq!(c.device.vin, None);
        assert_eq!(c.log.file, "/tmp/carmqtt.log");
        assert!(c.sources.exlap.enabled);
        assert_eq!(c.sources.exlap.addr, "127.0.0.1:25010");
        assert_eq!(c.sources.exlap.user, "RSE_3-DE1400");
        assert_eq!(c.sources.exlap.priority, 10);
    }

    #[test]
    fn signal_overrides_apply() {
        let c = Config::parse(
            "[mqtt]\nhost = \"b\"\n[signals]\n\"vehicle.speed\" = { min_interval_s = 2, deadband = 3 }\n\"accel.lateral\" = { enabled = true }\n\"odometer\" = { enabled = false }\n",
        )
        .unwrap();
        let r = c.effective_rates();
        assert_eq!(r["vehicle.speed"].min_interval, Duration::from_secs(2));
        assert_eq!(r["vehicle.speed"].deadband, 3.0);
        assert!(r["accel.lateral"].enabled);
        assert!(!r["odometer"].enabled);
        assert_eq!(r["engine.rpm"], catalog::lookup("engine.rpm").unwrap().default_rate);
    }

    #[test]
    fn rejects_bad_configs() {
        assert!(Config::parse("").unwrap_err().contains("mqtt"));
        assert!(Config::parse("[mqtt]\nhost = \"\"\n").unwrap_err().contains("host"));
        assert!(Config::parse("[mqtt]\nhost = \"b\"\n[signals]\n\"nope\" = { enabled = true }\n").unwrap_err().contains("nope"));
        assert!(Config::parse("[mqtt]\nhost = \"b\"\nbogus = 1\n").is_err());
        assert!(Config::parse("[mqtt]\nhost = \"b\"\n[log]\nlevel = \"loud\"\n").unwrap_err().contains("loud"));
        assert!(Config::parse("[mqtt]\nhost = \"b\"\n[signals]\n\"odometer\" = { deadband = -1 }\n").is_err());
    }

    #[test]
    fn rejects_intervals_over_one_day() {
        let err = Config::parse("[mqtt]\nhost = \"b\"\n[signals]\n\"odometer\" = { min_interval_s = 86401 }\n").unwrap_err();
        assert!(err.contains("min_interval_s"), "{err}");
        let err = Config::parse("[mqtt]\nhost = \"b\"\n[signals]\n\"odometer\" = { max_interval_s = 100000 }\n").unwrap_err();
        assert!(err.contains("max_interval_s"), "{err}");
        // exactly one day is fine
        assert!(Config::parse("[mqtt]\nhost = \"b\"\n[signals]\n\"odometer\" = { min_interval_s = 86400 }\n").is_ok());
    }

    #[test]
    fn example_file_parses() {
        let text = include_str!("../deploy/carmqtt.toml.example");
        let c = Config::parse(text).unwrap();
        assert_eq!(c.mqtt.port, 1883);
    }

    #[test]
    fn status_port_defaults_and_overrides() {
        assert_eq!(Config::parse("[mqtt]\nhost = \"b\"\n").unwrap().status.port, 19851);
        assert_eq!(Config::parse("[mqtt]\nhost = \"b\"\n[status]\nport = 5\n").unwrap().status.port, 5);
    }
}

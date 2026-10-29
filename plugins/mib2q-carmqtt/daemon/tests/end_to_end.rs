//! Fake ExLAP -> carmqtt pipeline -> real mosquitto -> assertions on retained topics.
//! Skips (with a notice) when mosquitto is not installed.

use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use carmqtt::config::Config;
use carmqtt::mqtt::client::{Client, Incoming, Options};
use carmqtt::testutil::{FakeExlap, FakeExlapOptions};

fn mosquitto() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MOSQUITTO") {
        return Some(PathBuf::from(p));
    }
    ["/opt/homebrew/sbin/mosquitto", "/usr/local/sbin/mosquitto", "/usr/sbin/mosquitto", "/usr/bin/mosquitto"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

struct Broker(Child);

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn subscriber(port: u16) -> Client<TcpStream> {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let opts = Options { client_id: format!("e2e-{}", std::process::id()), username: None, password: None, keepalive: Duration::from_secs(30), will: None };
    let mut c = Client::connect(s, &opts, Duration::from_secs(5)).unwrap();
    c.subscribe("#", 0).unwrap();
    c
}

fn gather(c: &mut Client<TcpStream>, seen: &mut HashMap<String, String>, done: impl Fn(&HashMap<String, String>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !done(seen) {
        for Incoming::Publish { topic, payload } in c.poll(Instant::now()).unwrap() {
            seen.insert(topic, String::from_utf8_lossy(&payload).into_owned());
        }
    }
}

#[test]
fn exlap_to_mqtt_end_to_end() {
    let Some(bin) = mosquitto() else {
        eprintln!("skipping exlap_to_mqtt_end_to_end: mosquitto not found (set MOSQUITTO=/path/to/mosquitto)");
        return;
    };
    let port = free_port();
    let _broker = Broker(
        Command::new(bin).args(["-p", &port.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("start mosquitto"),
    );
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(t0.elapsed() < Duration::from_secs(5), "mosquitto did not start");
        std::thread::sleep(Duration::from_millis(50));
    }

    let fake = FakeExlap::start(FakeExlapOptions::fixtures());
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost = \"127.0.0.1\"\nport = {port}\n[sources.exlap]\naddr = \"{}\"\n[status]\nport = 0\n[log]\nfile = \"-\"\nlevel = \"warn\"\n",
        fake.addr
    ))
    .unwrap();
    let pipeline = carmqtt::app::start(&cfg).unwrap();

    let v = "wauzzz8w0ja000001";
    let odo = format!("mib2q/{v}/state/odometer");
    let conf = format!("homeassistant/sensor/mib2q_{v}/odometer/config");
    let coolant = format!("mib2q/{v}/avail/temp/coolant");
    let status = format!("mib2q/{v}/status");
    let mut sub = subscriber(port);
    let mut seen = HashMap::new();
    gather(&mut sub, &mut seen, |s| [&odo, &conf, &coolant, &status].iter().all(|t| s.contains_key(t.as_str())));

    assert_eq!(seen.get(&status).map(String::as_str), Some("online"));
    assert_eq!(seen.get(&odo).map(String::as_str), Some("69760"));
    assert_eq!(seen.get(&format!("mib2q/{v}/state/fuel/level")).map(String::as_str), Some("66"));
    assert_eq!(seen.get(&format!("mib2q/{v}/state/service/oil/distance")).map(String::as_str), Some("8200"));
    assert_eq!(seen.get(&format!("mib2q/{v}/state/ignition/on")).map(String::as_str), Some("ON"));
    assert_eq!(seen.get(&coolant).map(String::as_str), Some("offline"));
    let j: serde_json::Value = serde_json::from_str(&seen[&conf]).unwrap();
    assert_eq!(j["unit_of_measurement"], "mi");
    assert_eq!(j["device"]["serial_number"], "TESTVIN0000000000");

    pipeline.shutdown();
    let mut after = HashMap::new();
    let mut sub2 = subscriber(port);
    gather(&mut sub2, &mut after, |s| s.get(&status).map(String::as_str) == Some("offline"));
    assert_eq!(after.get(&status).map(String::as_str), Some("offline"), "clean shutdown publishes offline");
}

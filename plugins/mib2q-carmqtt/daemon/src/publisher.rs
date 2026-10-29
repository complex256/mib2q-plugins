//! The MQTT side: connect once a VIN is known, announce discovery, publish
//! values, and replay the latest value of every signal after a reconnect.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backoff::Backoff;
use crate::catalog;
use crate::discovery::{config_payload, online_config_payload, DeviceInfo, Topics};
use crate::mqtt::client::{Client, Incoming, MqttError, Options};
use crate::mqtt::codec::Will;
use crate::status::Status;
use crate::value::{SignalId, Unit, Value};
use crate::{debug, info, warn};

pub trait Connector {
    type Io: Read + Write;
    fn connect(&mut self) -> io::Result<Self::Io>;
}

pub struct TcpConnector {
    pub host: String,
    pub port: u16,
}

impl Connector for TcpConnector {
    type Io = TcpStream;

    fn connect(&mut self) -> io::Result<TcpStream> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "host resolved to no addresses");
        for addr in (self.host.as_str(), self.port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_millis(50)))?;
                    s.set_write_timeout(Some(Duration::from_secs(10)))?;
                    s.set_nodelay(true)?;
                    return Ok(s);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

#[derive(Clone, Debug)]
pub struct PublisherConfig {
    pub topic_prefix: String,
    pub discovery_prefix: String,
    pub device: DeviceInfo,
    pub vin_override: Option<String>,
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub keepalive: Duration,
}

struct Announced {
    unit: Option<Unit>,
    available: bool,
}

/// Longest wait between broker connection attempts. At boot the broker is often
/// unreachable for minutes while Tailscale comes up; a short cap notices the route soon.
const RECONNECT_MAX: Duration = Duration::from_secs(10);

pub struct Publisher<C: Connector> {
    cfg: PublisherConfig,
    conn: C,
    client: Option<Client<C::Io>>,
    topics: Option<Topics>,
    mirror: BTreeMap<SignalId, Value>,
    announced: HashMap<SignalId, Announced>,
    /// Signals that have reported a real (available) value at least once. Unlike
    /// `announced` this survives `announced.clear()` on reconnect/HA-restart, so a
    /// signal that is now Unavailable but was once seen keeps showing its last value
    /// instead of flipping the entity to Unavailable in Home Assistant.
    seen: HashSet<SignalId>,
    backoff: Backoff,
    next_attempt: Instant,
    /// Set once "waiting for VIN" has been logged, so it isn't repeated every step.
    vin_wait_logged: bool,
    status: Arc<Status>,
}

impl<C: Connector> Publisher<C> {
    pub fn new(cfg: PublisherConfig, conn: C) -> Self {
        Publisher {
            cfg,
            conn,
            client: None,
            topics: None,
            mirror: BTreeMap::new(),
            announced: HashMap::new(),
            seen: HashSet::new(),
            backoff: Backoff::new(Duration::from_secs(1), RECONNECT_MAX),
            next_attempt: Instant::now(),
            vin_wait_logged: false,
            status: Arc::new(Status::new()),
        }
    }

    /// Report connection and publish state to the local status endpoint.
    pub fn with_status(mut self, status: Arc<Status>) -> Self {
        self.status = status;
        self
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    pub fn vin(&self) -> Option<String> {
        if let Some(v) = &self.cfg.vin_override {
            return Some(v.clone());
        }
        match self.mirror.get("vin") {
            Some(Value::Text(v)) if !v.is_empty() => Some(v.clone()),
            _ => None,
        }
    }

    pub fn handle(&mut self, signal: SignalId, value: Value) {
        self.mirror.insert(signal, value);
        self.status.set_signals(self.mirror.values().filter(|v| v.is_available()).count());
        if self.client.is_some() {
            if let Err(e) = self.emit(signal) {
                self.drop_connection(e);
            }
        }
    }

    pub fn step(&mut self, now: Instant) {
        if let Some(client) = self.client.as_mut() {
            match client.poll(now) {
                Ok(incoming) => {
                    let ha_status = self.topics.as_ref().map(|t| t.ha_status());
                    let ha_online = incoming.iter().any(|Incoming::Publish { topic, payload }| {
                        Some(topic) == ha_status.as_ref() && payload.as_slice() == b"online"
                    });
                    if ha_online {
                        info!("home assistant came online; re-announcing");
                        self.announced.clear();
                        if let Err(e) = self.emit_all() {
                            self.drop_connection(e);
                        }
                    }
                }
                Err(e) => self.drop_connection(e),
            }
            return;
        }
        if now < self.next_attempt {
            return;
        }
        let vin = self.vin();
        // vin() only gates the *first* connect (needed to build the topic
        // set); once self.topics exists, reconnect using it even if the
        // mirror's vin has since gone Unavailable.
        if self.topics.is_none() && vin.is_none() {
            if !self.vin_wait_logged {
                info!("waiting for VIN before connecting to MQTT");
                self.vin_wait_logged = true;
            }
            return;
        }
        self.vin_wait_logged = false;
        if let Err(e) = self.try_connect(vin.as_deref()) {
            self.client = None;
            self.status.set_mqtt(false);
            let d = self.backoff.next();
            warn!("mqtt connect failed: {e}; retrying in {}s", d.as_secs());
            self.next_attempt = now + d;
        }
    }

    fn try_connect(&mut self, vin: Option<&str>) -> Result<(), MqttError> {
        let topics = match &self.topics {
            Some(t) => t.clone(),
            None => {
                let vin = vin.expect("vin is required to build the topic set on the first connect");
                let t = Topics::new(&self.cfg.topic_prefix, &self.cfg.discovery_prefix, vin);
                self.topics = Some(t.clone());
                t
            }
        };
        let io = self.conn.connect()?;
        let opts = Options {
            client_id: self.cfg.client_id.clone(),
            username: self.cfg.username.clone(),
            password: self.cfg.password.clone(),
            keepalive: self.cfg.keepalive,
            will: Some(Will { topic: topics.status(), payload: b"offline".to_vec(), qos: 1, retain: true }),
        };
        let mut client = Client::connect(io, &opts, Duration::from_secs(10))?;
        client.subscribe(&topics.ha_status(), 0)?;
        client.publish(&topics.status(), b"online", 1, true)?;
        self.client = Some(client);
        self.status.set_mqtt(true);
        self.announced.clear();
        info!("mqtt connected; publishing {} signals", self.mirror.len());
        self.emit_all()?;
        self.backoff.reset();
        Ok(())
    }

    fn emit_all(&mut self) -> Result<(), MqttError> {
        if let (Some(client), Some(topics)) = (self.client.as_mut(), self.topics.as_ref()) {
            client.publish(&topics.online_config(), online_config_payload(topics, &self.cfg.device).as_bytes(), 1, true)?;
        }
        let signals: Vec<SignalId> = self.mirror.keys().copied().collect();
        for s in signals {
            self.emit(s)?;
        }
        Ok(())
    }

    fn emit(&mut self, signal: SignalId) -> Result<(), MqttError> {
        let Some(def) = catalog::lookup(signal) else {
            debug!("no catalog entry for {signal}; not publishing");
            return Ok(());
        };
        let Some(value) = self.mirror.get(signal).cloned() else { return Ok(()) };
        let fresh = value.is_available();
        if fresh {
            self.seen.insert(signal);
        }
        // Sticky availability: a signal that has ever reported a real value stays
        // `online` even when its current value is Unavailable (e.g. ExLAP marking a
        // driving signal invalid while parked). The entity then keeps its last-known
        // value in HA; only a signal never yet seen is published `offline`.
        let available = fresh || self.seen.contains(&signal);
        let (Some(client), Some(topics)) = (self.client.as_mut(), self.topics.as_ref()) else { return Ok(()) };
        let prev = self.announced.get(signal);
        let unit = if fresh {
            value.unit().or(def.default_unit)
        } else {
            prev.and_then(|a| a.unit).or(def.default_unit)
        };
        if prev.map(|a| a.unit != unit).unwrap_or(true) {
            let payload = config_payload(topics, def, unit, &self.cfg.device);
            client.publish(&topics.config(def), payload.as_bytes(), 1, true)?;
        }
        if prev.map(|a| a.available != available).unwrap_or(true) {
            let p: &[u8] = if available { b"online" } else { b"offline" };
            client.publish(&topics.avail(signal), p, 1, true)?;
        }
        if let Some(p) = value.payload() {
            client.publish(&topics.value_topic(def), p.as_bytes(), 1, true)?;
            self.status.note_publish();
        }
        self.announced.insert(signal, Announced { unit, available });
        Ok(())
    }

    fn drop_connection(&mut self, e: MqttError) {
        let d = self.backoff.next();
        warn!("mqtt connection lost: {e}; reconnecting in {}s", d.as_secs());
        self.client = None;
        self.status.set_mqtt(false);
        self.next_attempt = Instant::now() + d;
    }

    /// Publish `offline` and disconnect cleanly, if connected.
    pub fn shutdown(&mut self) {
        self.status.set_mqtt(false);
        if let (Some(mut c), Some(t)) = (self.client.take(), self.topics.as_ref()) {
            let _ = c.publish(&t.status(), b"offline", 1, true);
            let _ = c.disconnect();
        }
    }

    /// Run until `rx` disconnects (the router stopped), then shut down.
    pub fn run(mut self, rx: Receiver<(SignalId, Value)>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok((s, v)) => {
                    self.handle(s, v);
                    while let Ok((s, v)) = rx.try_recv() {
                        self.handle(s, v);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.step(Instant::now());
        }
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mqtt::codec::{self, Packet, Publish};
    use crate::testutil::{mock_pair, mqtt_packets, MockHandle, MockIo};
    use std::collections::VecDeque;

    const VIN: &str = "TESTVIN0000000000";
    const V: &str = "wauzzz8w0ja000001";

    struct MockConnector {
        queue: VecDeque<io::Result<MockIo>>,
        calls: usize,
    }

    impl Connector for MockConnector {
        type Io = MockIo;
        fn connect(&mut self) -> io::Result<MockIo> {
            self.calls += 1;
            self.queue.pop_front().unwrap_or_else(|| Err(io::ErrorKind::ConnectionRefused.into()))
        }
    }

    fn broker() -> (MockIo, MockHandle) {
        let (io, h) = mock_pair();
        h.feed(&codec::encode(&Packet::ConnAck { session_present: false, code: 0 }));
        (io, h)
    }

    fn cfg(vin_override: Option<&str>) -> PublisherConfig {
        PublisherConfig {
            topic_prefix: "mib2q".into(),
            discovery_prefix: "homeassistant".into(),
            device: DeviceInfo { name: "Audi".into(), model: "A4".into() },
            vin_override: vin_override.map(str::to_string),
            client_id: "t".into(),
            username: None,
            password: None,
            keepalive: Duration::from_secs(30),
        }
    }

    fn publisher(ios: Vec<io::Result<MockIo>>, vin_override: Option<&str>) -> Publisher<MockConnector> {
        Publisher::new(cfg(vin_override), MockConnector { queue: ios.into(), calls: 0 })
    }

    fn publishes(h: &MockHandle) -> Vec<Publish> {
        mqtt_packets(&h.take_written())
            .into_iter()
            .filter_map(|p| if let Packet::Publish(p) = p { Some(p) } else { None })
            .collect()
    }

    fn find<'a>(ps: &'a [Publish], topic: &str) -> Option<&'a Publish> {
        ps.iter().rev().find(|p| p.topic == topic)
    }

    fn text(p: Option<&Publish>) -> Option<String> {
        p.map(|p| String::from_utf8(p.payload.clone()).unwrap())
    }

    fn mi(v: f64) -> Value {
        Value::Num { v, unit: Some(Unit::Mi) }
    }

    #[test]
    fn waits_for_vin_before_connecting() {
        let (io, _h) = broker();
        let mut p = publisher(vec![Ok(io)], None);
        p.handle("odometer", mi(1.0));
        p.step(Instant::now());
        assert!(!p.is_connected());
        assert_eq!(p.conn.calls, 0);
    }

    #[test]
    fn logs_waiting_for_vin_only_once() {
        let (io, _h) = broker();
        let mut p = publisher(vec![Ok(io)], None);
        assert!(!p.vin_wait_logged);
        p.step(Instant::now());
        assert!(!p.is_connected());
        assert!(p.vin_wait_logged, "flag should flip on the first step without a VIN");
        // Further steps without a VIN must not need to log again: the flag
        // stays set (the log call itself is one-shot, gated on this field).
        p.step(Instant::now());
        assert!(p.vin_wait_logged);
        p.handle("vin", Value::Text(VIN.into()));
        p.step(Instant::now());
        assert!(p.is_connected());
        assert!(!p.vin_wait_logged, "flag resets once a connect attempt is made");
    }

    #[test]
    fn reconnects_using_existing_topics_even_if_vin_goes_unavailable() {
        let (io1, h1) = broker();
        let (io2, h2) = broker();
        let mut p = publisher(vec![Ok(io1), Ok(io2)], None);
        p.handle("vin", Value::Text(VIN.into()));
        let t0 = Instant::now();
        p.step(t0);
        assert!(p.is_connected());
        h1.close();
        p.step(t0);
        assert!(!p.is_connected());
        // The source lost the VIN before the reconnect fires; vin() only
        // gates the *first* connect, so the reconnect must still use the
        // topic set already built from it.
        p.handle("vin", Value::Unavailable);
        assert!(p.vin().is_none());
        p.step(t0 + Duration::from_secs(2));
        assert!(p.is_connected());
        assert_eq!(text(find(&publishes(&h2), &format!("mib2q/{V}/status"))).as_deref(), Some("online"));
    }

    #[test]
    fn connects_once_vin_known_and_announces() {
        let (io, h) = broker();
        let mut p = publisher(vec![Ok(io)], None);
        p.handle("vin", Value::Text(VIN.into()));
        p.handle("odometer", mi(69760.0));
        p.step(Instant::now());
        assert!(p.is_connected());
        let pk = mqtt_packets(&h.take_written());
        match &pk[0] {
            Packet::Connect(c) => {
                let w = c.will.as_ref().unwrap();
                assert_eq!(w.topic, format!("mib2q/{V}/status"));
                assert_eq!(w.payload, b"offline");
                assert!(w.retain);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&pk[1], Packet::Subscribe { topics, .. } if topics[0].0 == "homeassistant/status"));
        let ps: Vec<Publish> = pk.into_iter().filter_map(|p| if let Packet::Publish(p) = p { Some(p) } else { None }).collect();
        assert!(ps.iter().all(|p| p.retain && p.qos == 1));
        assert_eq!(text(find(&ps, &format!("mib2q/{V}/status"))).as_deref(), Some("online"));
        assert_eq!(text(find(&ps, &format!("mib2q/{V}/state/odometer"))).as_deref(), Some("69760"));
        assert_eq!(text(find(&ps, &format!("mib2q/{V}/avail/odometer"))).as_deref(), Some("online"));
        let conf: serde_json::Value = serde_json::from_str(&text(find(&ps, &format!("homeassistant/sensor/mib2q_{V}/odometer/config"))).unwrap()).unwrap();
        assert_eq!(conf["unit_of_measurement"], "mi");
        assert_eq!(text(find(&ps, &format!("mib2q/{V}/state/vin"))).as_deref(), Some(VIN));
        let online: serde_json::Value = serde_json::from_str(&text(find(&ps, &format!("homeassistant/binary_sensor/mib2q_{V}/online/config"))).unwrap()).unwrap();
        assert_eq!(online["state_topic"], format!("mib2q/{V}/status"));
    }

    #[test]
    fn config_vin_override_connects_without_exlap() {
        let (io, h) = broker();
        let mut p = publisher(vec![Ok(io)], Some(VIN));
        p.step(Instant::now());
        assert!(p.is_connected());
        assert_eq!(text(find(&publishes(&h), &format!("mib2q/{V}/status"))).as_deref(), Some("online"));
    }

    #[test]
    fn unavailable_publishes_offline_and_no_state() {
        let (io, h) = broker();
        let mut p = publisher(vec![Ok(io)], Some(VIN));
        p.step(Instant::now());
        h.take_written();
        p.handle("temp.coolant", Value::Unavailable);
        let ps = publishes(&h);
        assert_eq!(text(find(&ps, &format!("mib2q/{V}/avail/temp/coolant"))).as_deref(), Some("offline"));
        assert!(find(&ps, &format!("homeassistant/sensor/mib2q_{V}/temp_coolant/config")).is_some());
        assert!(find(&ps, &format!("mib2q/{V}/state/temp/coolant")).is_none());
    }

    #[test]
    fn available_then_unavailable_keeps_last_known() {
        // Once a signal has reported a real value, a later Unavailable reading
        // (e.g. ExLAP marking a driving signal invalid while parked) must NOT
        // flip the HA entity to Unavailable: no offline avail, no state change,
        // so the broker keeps serving the last-known value through sleep.
        let (io, h) = broker();
        let mut p = publisher(vec![Ok(io)], Some(VIN));
        p.step(Instant::now());
        p.handle("odometer", mi(100.0)); // first real value: online + state
        h.take_written();
        p.handle("odometer", Value::Unavailable);
        let ps = publishes(&h);
        assert!(
            find(&ps, &format!("mib2q/{V}/avail/odometer")).is_none(),
            "a previously-seen signal must not be republished offline",
        );
        assert!(
            find(&ps, &format!("mib2q/{V}/state/odometer")).is_none(),
            "last value retained; no new state on a transient-invalid reading",
        );
    }

    #[test]
    fn reconnect_reasserts_online_for_seen_signal_gone_unavailable() {
        // On reconnect/HA-restart `announced` is cleared and the mirror is
        // replayed. If a seen signal's mirror value is now Unavailable, the
        // replay must re-assert `online` (keeping the last value), not offline.
        let (io1, h1) = broker();
        let (io2, h2) = broker();
        let mut p = publisher(vec![Ok(io1), Ok(io2)], Some(VIN));
        let t0 = Instant::now();
        p.step(t0);
        p.handle("odometer", mi(100.0));
        p.handle("odometer", Value::Unavailable); // source now reports invalid
        h1.close();
        p.step(t0);
        assert!(!p.is_connected());
        p.step(t0 + Duration::from_secs(2));
        assert!(p.is_connected());
        let ps = publishes(&h2);
        assert_eq!(
            text(find(&ps, &format!("mib2q/{V}/avail/odometer"))).as_deref(),
            Some("online"),
            "a seen signal must come back online on reconnect, not offline",
        );
    }

    #[test]
    fn unit_change_republishes_config() {
        let (io, h) = broker();
        let mut p = publisher(vec![Ok(io)], Some(VIN));
        p.step(Instant::now());
        p.handle("odometer", mi(10.0));
        h.take_written();
        p.handle("odometer", mi(11.0));
        let ps = publishes(&h);
        assert_eq!(ps.len(), 1, "same unit: only the state is published");
        p.handle("odometer", Value::Num { v: 18.0, unit: Some(Unit::Km) });
        let ps = publishes(&h);
        let conf: serde_json::Value = serde_json::from_str(&text(find(&ps, &format!("homeassistant/sensor/mib2q_{V}/odometer/config"))).unwrap()).unwrap();
        assert_eq!(conf["unit_of_measurement"], "km");
    }

    #[test]
    fn home_assistant_restart_reannounces() {
        let (io, h) = broker();
        let mut p = publisher(vec![Ok(io)], Some(VIN));
        p.handle("odometer", mi(10.0));
        p.step(Instant::now());
        h.take_written();
        h.feed(&codec::encode(&Packet::Publish(Publish { topic: "homeassistant/status".into(), payload: b"online".to_vec(), qos: 0, retain: false, dup: false, pid: None })));
        p.step(Instant::now());
        let ps = publishes(&h);
        assert!(find(&ps, &format!("homeassistant/sensor/mib2q_{V}/odometer/config")).is_some());
        assert_eq!(text(find(&ps, &format!("mib2q/{V}/state/odometer"))).as_deref(), Some("10"));
    }

    #[test]
    fn offline_updates_publish_latest_only() {
        let (io1, h1) = broker();
        let (io2, h2) = broker();
        let mut p = publisher(vec![Ok(io1), Ok(io2)], Some(VIN));
        p.handle("odometer", mi(1.0));
        let t0 = Instant::now();
        p.step(t0);
        h1.close();
        p.step(t0);
        assert!(!p.is_connected());
        for i in 2..1000 {
            p.handle("odometer", mi(i as f64));
        }
        assert_eq!(p.mirror.len(), 1, "one slot per signal while offline");
        p.step(t0 + Duration::from_secs(2));
        assert!(p.is_connected());
        let ps = publishes(&h2);
        let states: Vec<&Publish> = ps.iter().filter(|p| p.topic == format!("mib2q/{V}/state/odometer")).collect();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].payload, b"999");
    }

    #[test]
    fn failed_connect_backs_off() {
        let (io, _h) = broker();
        let mut p = publisher(vec![Err(io::ErrorKind::ConnectionRefused.into()), Ok(io)], Some(VIN));
        let t0 = Instant::now();
        p.step(t0);
        assert!(!p.is_connected());
        p.step(t0);
        assert_eq!(p.conn.calls, 1, "no retry before the backoff elapses");
        p.step(t0 + Duration::from_secs(2));
        assert!(p.is_connected());
    }

    #[test]
    fn reconnect_wait_is_capped_at_10s() {
        // At boot the broker is often unreachable for minutes (Tailscale still coming up);
        // a long cap makes carmqtt notice late once the route appears.
        let (io, _h) = broker();
        let mut results = Vec::new();
        for _ in 0..8 {
            results.push(Err(io::ErrorKind::TimedOut.into()));
        }
        results.push(Ok(io));
        let mut p = publisher(results, Some(VIN));
        let mut t = Instant::now();
        for n in 1..=9 {
            p.step(t);
            assert_eq!(p.conn.calls, n, "attempt {n} must come within 10 s of the previous one");
            t += Duration::from_secs(10);
        }
        assert!(p.is_connected());
    }

    #[test]
    fn shutdown_publishes_offline() {
        let (io, h) = broker();
        let mut p = publisher(vec![Ok(io)], Some(VIN));
        p.step(Instant::now());
        h.take_written();
        p.shutdown();
        let pk = mqtt_packets(&h.take_written());
        assert!(matches!(&pk[0], Packet::Publish(p) if p.payload == b"offline" && p.retain));
        assert_eq!(pk[1], Packet::Disconnect);
    }

    #[test]
    fn status_signals_counts_only_available_values() {
        let st = Arc::new(Status::new());
        let mut p = publisher(vec![], Some(VIN)).with_status(st.clone());
        p.handle("odometer", mi(1.0));
        p.handle("vin", Value::Text(VIN.into()));
        assert!(st.render().contains("\nsignals=2\n"));
        p.handle("odometer", Value::Lost);
        assert!(st.render().contains("\nsignals=1\n"));
    }
}

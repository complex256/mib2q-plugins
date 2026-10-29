//! ExLAP ("VW SAI-Server HIGH") client source on 127.0.0.1:25010.

pub mod auth;
pub mod map;
pub mod session;
pub mod xml;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use self::map::Decoder;
use self::session::{ExlapError, Session};
use self::xml::{Element, Msg};
use super::{mark_lost, stopped, Source, SourceError, StopFlag};
use crate::catalog::Rate;
use crate::status::Status;
use crate::value::{SignalId, SourceId, Update};
use crate::{info, warn};

const ID: SourceId = "exlap";

#[derive(Clone, Debug)]
pub struct ExlapConfig {
    pub addr: String,
    pub user: String,
    pub password: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Timings {
    pub connect: Duration,
    pub handshake: Duration,
    /// Send a liveness `Get` after this long with nothing received.
    pub idle_probe: Duration,
    /// Give up on the session if the probe is unanswered this long.
    pub probe_timeout: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            connect: Duration::from_secs(5),
            handshake: Duration::from_secs(10),
            idle_probe: Duration::from_secs(20),
            probe_timeout: Duration::from_secs(10),
        }
    }
}

pub struct ExlapSource {
    cfg: ExlapConfig,
    timings: Timings,
    rates: HashMap<SignalId, Rate>,
    provides: Vec<SignalId>,
    status: Arc<Status>,
}

impl ExlapSource {
    pub fn new(cfg: ExlapConfig, rates: &HashMap<SignalId, Rate>, timings: Timings) -> ExlapSource {
        let provides = map::all_signals()
            .into_iter()
            .filter(|s| rates.get(s).map(|r| r.enabled).unwrap_or(false))
            .collect();
        ExlapSource { cfg, timings, rates: rates.clone(), provides, status: Arc::new(Status::new()) }
    }

    /// Report session state to the local status endpoint.
    pub fn with_status(mut self, status: Arc<Status>) -> Self {
        self.status = status;
        self
    }

    fn password(&self) -> Result<String, SourceError> {
        match (&self.cfg.password, auth::builtin_password(&self.cfg.user)) {
            (Some(p), _) => Ok(p.clone()),
            (None, Some(p)) => Ok(p.to_string()),
            (None, None) => Err(SourceError(format!("exlap: no password for user {}", self.cfg.user))),
        }
    }

    /// (url, ival_ms) for every URL with at least one enabled signal.
    pub fn subscriptions(&self) -> Vec<(&'static str, u32)> {
        map::urls()
            .into_iter()
            .filter_map(|url| {
                let min = map::signals_for(url)
                    .into_iter()
                    .filter(|s| self.provides.contains(s))
                    .filter_map(|s| self.rates.get(s))
                    .map(|r| r.min_interval.as_millis())
                    .min()?;
                Some((url, min.max(1000).min(u32::MAX as u128) as u32))
            })
            .collect()
    }

    fn forward(&self, dec: &mut Decoder, url: &str, fields: &[Element], tx: &Sender<Update>) -> Result<(), SourceError> {
        for (signal, value) in dec.decode(url, fields) {
            if self.provides.contains(&signal) {
                tx.send(Update { signal, value, ts: None, source: ID })
                    .map_err(|_| SourceError("router has stopped".into()))?;
            }
        }
        Ok(())
    }

    pub fn run_session<S: Read + Write>(&mut self, io: S, tx: &Sender<Update>, stop: &StopFlag) -> Result<(), SourceError> {
        let pass = self.password()?;
        let mut sess = Session::new(io);
        match sess.handshake(&self.cfg.user, &pass, self.timings.handshake, stop) {
            Ok(()) => {}
            Err(ExlapError::Stopped) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        info!("exlap: authenticated as {}", self.cfg.user);
        self.status.set_exlap(true);
        self.status.set_exlap_err("");
        sess.send(r#"<Heartbeat ival="10"/>"#)?;
        let mut sub_ids: HashMap<u32, &'static str> = HashMap::new();
        for (url, ival) in self.subscriptions() {
            sub_ids.insert(sess.subscribe(url, ival)?, url);
        }
        // Fetch the VIN once up front rather than waiting for its subscription
        // to push a value; the Rsp/ObjectData path in the loop below already
        // forwards whatever this returns.
        sess.get("vehicleIdenticationNumber")?;
        let mut dec = Decoder::default();
        let mut last_rx = Instant::now();
        let mut probe: Option<(u32, Instant)> = None;
        let mut reported: HashSet<&'static str> = HashSet::new();
        loop {
            if stopped(stop) {
                return Ok(());
            }
            match sess.recv(Instant::now() + Duration::from_millis(200))? {
                Some(Msg::Dat { url, children }) => {
                    last_rx = Instant::now();
                    self.forward(&mut dec, &url, &children, tx)?;
                }
                Some(Msg::Rsp { id, status, children }) => {
                    last_rx = Instant::now();
                    if probe.map(|(p, _)| Some(p) == id).unwrap_or(false) {
                        probe = None;
                    }
                    if let (Some(s), Some(url)) = (status, id.and_then(|i| sub_ids.get(&i))) {
                        if reported.insert(url) {
                            warn!("exlap: subscription to {url} refused: {s}");
                        }
                    }
                    if let Some(url) = children.iter().find(|e| e.tag == "ObjectData").and_then(|e| e.attr("url")) {
                        let url = url.to_string();
                        self.forward(&mut dec, &url, &children, tx)?;
                    }
                }
                Some(_) => last_rx = Instant::now(),
                None => {}
            }
            let now = Instant::now();
            match probe {
                None if now.duration_since(last_rx) >= self.timings.idle_probe => {
                    probe = Some((sess.get("vehicleIdenticationNumber")?, now));
                }
                Some((_, sent)) if now.duration_since(sent) >= self.timings.probe_timeout => {
                    return Err(SourceError("exlap: liveness probe unanswered".into()));
                }
                _ => {}
            }
        }
    }

    fn connect_and_run(&mut self, tx: &Sender<Update>, stop: &StopFlag) -> Result<(), SourceError> {
        let addr = self
            .cfg
            .addr
            .to_socket_addrs()
            .map_err(|e| SourceError(format!("exlap: bad address {}: {e}", self.cfg.addr)))?
            .next()
            .ok_or_else(|| SourceError(format!("exlap: {} resolved to nothing", self.cfg.addr)))?;
        let stream = TcpStream::connect_timeout(&addr, self.timings.connect)
            .map_err(|e| SourceError(format!("exlap: connect {addr}: {e}")))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| SourceError(format!("exlap: {e}")))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| SourceError(format!("exlap: {e}")))?;
        let _ = stream.set_nodelay(true);
        self.run_session(stream, tx, stop)
    }
}

impl Source for ExlapSource {
    fn id(&self) -> SourceId {
        ID
    }

    fn provides(&self) -> &[SignalId] {
        &self.provides
    }

    fn run(&mut self, tx: &Sender<Update>, stop: &StopFlag) -> Result<(), SourceError> {
        let result = self.connect_and_run(tx, stop);
        self.status.set_exlap(false);
        if let Err(e) = &result {
            self.status.set_exlap_err(&e.0);
            mark_lost(tx, ID, &self.provides);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;
    use crate::sources::{stop_flag, supervise_with};
    use crate::testutil::{FakeExlap, FakeExlapOptions};
    use crate::value::{Unit, Value};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

    fn source(addr: &str, timings: Timings) -> ExlapSource {
        ExlapSource::new(
            ExlapConfig { addr: addr.into(), user: "RSE_3-DE1400".into(), password: None },
            &catalog::default_rates(),
            timings,
        )
    }

    fn fast() -> Timings {
        Timings {
            connect: Duration::from_secs(1),
            handshake: Duration::from_secs(2),
            idle_probe: Duration::from_millis(200),
            probe_timeout: Duration::from_millis(200),
        }
    }

    /// Collect updates until `want` has been seen for every listed signal or 5 s pass.
    fn collect(rx: &mpsc::Receiver<Update>, want: &[SignalId]) -> HashMap<SignalId, Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = HashMap::new();
        while Instant::now() < deadline && !want.iter().all(|s| seen.contains_key(s)) {
            if let Ok(u) = rx.recv_timeout(Duration::from_millis(100)) {
                seen.insert(u.signal, u.value);
            }
        }
        seen
    }

    #[test]
    fn subscriptions_skip_disabled_signals_and_floor_ival() {
        let s = source("127.0.0.1:1", Timings::default());
        let subs: HashMap<&str, u32> = s.subscriptions().into_iter().collect();
        assert!(!subs.contains_key("lateralAcceleration"), "telemetry is off by default");
        assert_eq!(subs["totalDistance"], 1000);
        assert_eq!(subs["vehicleSpeed"], 5000);
        assert!(!s.provides().contains(&"accel.lateral"));
        assert!(s.provides().contains(&"odometer"));
    }

    #[test]
    fn streams_values_from_the_car_fixtures() {
        let fake = FakeExlap::start(FakeExlapOptions::fixtures());
        let (tx, rx) = mpsc::channel();
        let stop = stop_flag();
        let mut src = source(&fake.addr.to_string(), Timings::default());
        let s2 = stop.clone();
        let h = std::thread::spawn(move || src.run(&tx, &s2));
        let seen = collect(&rx, &["odometer", "vin", "temp.coolant", "service.oil.distance"]);
        assert_eq!(seen["odometer"], Value::Num { v: 69760.0, unit: Some(Unit::Mi) });
        assert_eq!(seen["vin"], Value::Text("TESTVIN0000000000".into()));
        assert_eq!(seen["temp.coolant"], Value::Unavailable);
        assert_eq!(seen["service.oil.distance"], Value::Num { v: 8200.0, unit: Some(Unit::Mi) });
        stop.store(true, Ordering::Relaxed);
        assert_eq!(h.join().unwrap(), Ok(()));
    }

    #[test]
    fn unanswered_probe_fails_and_marks_unavailable() {
        let mut opts = FakeExlapOptions::fixtures();
        opts.answer_gets = false;
        let fake = FakeExlap::start(opts);
        let (tx, rx) = mpsc::channel();
        let mut src = source(&fake.addr.to_string(), fast());
        let err = src.run(&tx, &stop_flag()).unwrap_err();
        assert!(err.0.contains("liveness"), "{err}");
        let all: Vec<Update> = rx.try_iter().collect();
        let last_odo = all.iter().rev().find(|u| u.signal == "odometer").unwrap();
        assert_eq!(last_odo.value, Value::Lost);
    }

    #[test]
    fn unknown_user_without_password_fails() {
        let fake = FakeExlap::start(FakeExlapOptions::fixtures());
        let mut src = ExlapSource::new(
            ExlapConfig { addr: fake.addr.to_string(), user: "nobody".into(), password: None },
            &catalog::default_rates(),
            fast(),
        );
        let (tx, _rx) = mpsc::channel();
        assert!(src.run(&tx, &stop_flag()).unwrap_err().0.contains("no password"));
    }

    #[test]
    fn supervisor_reconnects_after_server_drop() {
        let mut opts = FakeExlapOptions::fixtures();
        opts.drop_after = Some(Duration::from_millis(500));
        let fake = FakeExlap::start(opts);
        let (tx, rx) = mpsc::channel();
        let stop = stop_flag();
        let h = supervise_with(Box::new(source(&fake.addr.to_string(), fast())), tx, stop.clone(), Duration::from_millis(50), Duration::from_millis(200));
        // first session: a value; drop: Unavailable; second session: the value again
        let mut sequence = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && sequence.len() < 3 {
            if let Ok(u) = rx.recv_timeout(Duration::from_millis(100)) {
                if u.signal == "odometer" && sequence.last() != Some(&u.value) {
                    sequence.push(u.value);
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        let odo = Value::Num { v: 69760.0, unit: Some(Unit::Mi) };
        assert_eq!(sequence, vec![odo.clone(), Value::Lost, odo]);
        assert!(fake.connections.load(Ordering::SeqCst) >= 2);
    }
}

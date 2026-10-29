//! Blocking MQTT 3.1.1 client over any `Read + Write` with short read timeouts.

use std::fmt;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use super::codec::{self, Connect, Packet, Publish, Will};

#[derive(Clone, Debug)]
pub struct Options {
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub keepalive: Duration,
    pub will: Option<Will>,
}

#[derive(Debug)]
pub enum MqttError {
    Io(io::Error),
    Refused(u8),
    Protocol(String),
    Timeout,
}

impl From<io::Error> for MqttError {
    fn from(e: io::Error) -> Self {
        MqttError::Io(e)
    }
}

impl fmt::Display for MqttError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MqttError::Io(e) => write!(f, "i/o: {e}"),
            MqttError::Refused(c) => write!(f, "connection refused by broker (code {c})"),
            MqttError::Protocol(s) => write!(f, "protocol: {s}"),
            MqttError::Timeout => write!(f, "timed out"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Publish { topic: String, payload: Vec<u8> },
}

pub struct Client<S: Read + Write> {
    io: S,
    buf: Vec<u8>,
    next_pid: u16,
    keepalive: Duration,
    last_rx: Instant,
    ping_sent: Option<Instant>,
}

fn is_would_block(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted)
}

impl<S: Read + Write> Client<S> {
    /// Send CONNECT and wait up to `timeout` for CONNACK.
    pub fn connect(io: S, opts: &Options, timeout: Duration) -> Result<Self, MqttError> {
        let mut c = Client { io, buf: Vec::new(), next_pid: 1, keepalive: opts.keepalive, last_rx: Instant::now(), ping_sent: None };
        c.send(&Packet::Connect(Connect {
            client_id: opts.client_id.clone(),
            keepalive: opts.keepalive.as_secs().min(u16::MAX as u64) as u16,
            clean_session: true,
            username: opts.username.clone(),
            password: opts.password.clone(),
            will: opts.will.clone(),
        }))?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(p) = c.next_packet()? {
                c.last_rx = Instant::now();
                return match p {
                    Packet::ConnAck { code: 0, .. } => Ok(c),
                    Packet::ConnAck { code, .. } => Err(MqttError::Refused(code)),
                    other => Err(MqttError::Protocol(format!("expected CONNACK, got {other:?}"))),
                };
            }
            if Instant::now() >= deadline {
                return Err(MqttError::Timeout);
            }
            if !c.fill()? {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn send(&mut self, p: &Packet) -> Result<(), MqttError> {
        self.io.write_all(&codec::encode(p))?;
        self.io.flush()?;
        Ok(())
    }

    /// One read. `Ok(true)` if bytes arrived, `Ok(false)` if none were available.
    fn fill(&mut self) -> Result<bool, MqttError> {
        let mut tmp = [0u8; 4096];
        match self.io.read(&mut tmp) {
            Ok(0) => Err(MqttError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "broker closed the connection"))),
            Ok(n) => {
                self.buf.extend_from_slice(&tmp[..n]);
                Ok(true)
            }
            Err(e) if is_would_block(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn next_packet(&mut self) -> Result<Option<Packet>, MqttError> {
        match codec::decode(&self.buf) {
            Ok(Some((p, n))) => {
                self.buf.drain(..n);
                Ok(Some(p))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(MqttError::Protocol(format!("{e:?}"))),
        }
    }

    fn pid(&mut self) -> u16 {
        let p = self.next_pid;
        self.next_pid = if p == u16::MAX { 1 } else { p + 1 };
        p
    }

    pub fn publish(&mut self, topic: &str, payload: &[u8], qos: u8, retain: bool) -> Result<(), MqttError> {
        let pid = if qos > 0 { Some(self.pid()) } else { None };
        self.send(&Packet::Publish(Publish { topic: topic.to_string(), payload: payload.to_vec(), qos, retain, dup: false, pid }))
    }

    pub fn subscribe(&mut self, topic: &str, qos: u8) -> Result<(), MqttError> {
        let pid = self.pid();
        self.send(&Packet::Subscribe { pid, topics: vec![(topic.to_string(), qos)] })
    }

    /// Read whatever is available, answer/consume control packets, keep the
    /// connection alive, and return application messages.
    pub fn poll(&mut self, now: Instant) -> Result<Vec<Incoming>, MqttError> {
        while self.fill()? {}
        let mut out = Vec::new();
        while let Some(p) = self.next_packet()? {
            self.last_rx = now;
            match p {
                Packet::PingResp => self.ping_sent = None,
                Packet::Publish(p) => {
                    if let (1, Some(pid)) = (p.qos, p.pid) {
                        self.send(&Packet::PubAck(pid))?;
                    }
                    out.push(Incoming::Publish { topic: p.topic, payload: p.payload });
                }
                Packet::PubAck(_) | Packet::SubAck { .. } => {}
                other => return Err(MqttError::Protocol(format!("unexpected {other:?}"))),
            }
        }
        match self.ping_sent {
            Some(t) if now.duration_since(t) > self.keepalive => return Err(MqttError::Timeout),
            None if !self.keepalive.is_zero() && now.duration_since(self.last_rx) >= self.keepalive / 2 => {
                self.send(&Packet::PingReq)?;
                self.ping_sent = Some(now);
            }
            _ => {}
        }
        Ok(out)
    }

    pub fn disconnect(mut self) -> Result<(), MqttError> {
        self.send(&Packet::Disconnect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{mock_pair, mqtt_packets};

    fn opts() -> Options {
        Options { client_id: "t".into(), username: Some("u".into()), password: Some("p".into()), keepalive: Duration::from_secs(2), will: None }
    }

    fn connack(code: u8) -> Vec<u8> {
        codec::encode(&Packet::ConnAck { session_present: false, code })
    }

    #[test]
    fn connects_and_sends_connect() {
        let (io, h) = mock_pair();
        h.feed(&connack(0));
        let _c = Client::connect(io, &opts(), Duration::from_secs(1)).unwrap();
        match &mqtt_packets(&h.take_written())[..] {
            [Packet::Connect(c)] => {
                assert_eq!(c.client_id, "t");
                assert_eq!(c.username.as_deref(), Some("u"));
                assert_eq!(c.keepalive, 2);
                assert!(c.clean_session);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn refused_and_timeout() {
        let (io, h) = mock_pair();
        h.feed(&connack(5));
        assert!(matches!(Client::connect(io, &opts(), Duration::from_secs(1)), Err(MqttError::Refused(5))));
        let (io, _h) = mock_pair();
        assert!(matches!(Client::connect(io, &opts(), Duration::from_millis(50)), Err(MqttError::Timeout)));
    }

    #[test]
    fn publishes_qos1_retained_with_increasing_pids() {
        let (io, h) = mock_pair();
        h.feed(&connack(0));
        let mut c = Client::connect(io, &opts(), Duration::from_secs(1)).unwrap();
        h.take_written();
        c.publish("a/b", b"1", 1, true).unwrap();
        c.publish("a/b", b"2", 1, true).unwrap();
        let pk = mqtt_packets(&h.take_written());
        let pids: Vec<Option<u16>> = pk.iter().map(|p| match p { Packet::Publish(p) => { assert!(p.retain); p.pid } _ => panic!() }).collect();
        assert_eq!(pids, vec![Some(1), Some(2)]);
    }

    #[test]
    fn poll_returns_incoming_and_acks_qos1() {
        let (io, h) = mock_pair();
        h.feed(&connack(0));
        let mut c = Client::connect(io, &opts(), Duration::from_secs(1)).unwrap();
        h.take_written();
        h.feed(&codec::encode(&Packet::Publish(Publish { topic: "homeassistant/status".into(), payload: b"online".to_vec(), qos: 1, retain: false, dup: false, pid: Some(9) })));
        h.feed(&codec::encode(&Packet::SubAck { pid: 1, codes: vec![0] }));
        let got = c.poll(Instant::now()).unwrap();
        assert_eq!(got, vec![Incoming::Publish { topic: "homeassistant/status".into(), payload: b"online".to_vec() }]);
        assert_eq!(mqtt_packets(&h.take_written()), vec![Packet::PubAck(9)]);
    }

    #[test]
    fn keepalive_pings_and_times_out() {
        let (io, h) = mock_pair();
        h.feed(&connack(0));
        let mut c = Client::connect(io, &opts(), Duration::from_secs(1)).unwrap();
        h.take_written();
        let t0 = Instant::now();
        c.poll(t0 + Duration::from_secs(1)).unwrap();
        assert_eq!(mqtt_packets(&h.take_written()), vec![Packet::PingReq]);
        h.feed(&codec::encode(&Packet::PingResp));
        c.poll(t0 + Duration::from_millis(1100)).unwrap();
        // next ping, never answered
        c.poll(t0 + Duration::from_secs(3)).unwrap();
        assert_eq!(mqtt_packets(&h.take_written()), vec![Packet::PingReq]);
        assert!(matches!(c.poll(t0 + Duration::from_secs(6)), Err(MqttError::Timeout)));
    }

    #[test]
    fn publishing_without_receiving_still_pings_and_times_out() {
        // A dead link where writes still succeed (e.g. into a kernel buffer)
        // but nothing ever comes back must still be detected: the keepalive
        // schedule keys off last_rx, not off our own outbound traffic.
        let (io, h) = mock_pair();
        h.feed(&connack(0));
        let mut c = Client::connect(io, &opts(), Duration::from_secs(1)).unwrap();
        h.take_written();
        let t0 = Instant::now();
        c.publish("a/b", b"1", 1, true).unwrap();
        assert!(c.poll(t0 + Duration::from_millis(500)).unwrap().is_empty());
        c.publish("a/b", b"2", 1, true).unwrap();
        let got = c.poll(t0 + Duration::from_secs(1)).unwrap();
        assert!(got.is_empty());
        let written = mqtt_packets(&h.take_written());
        assert!(written.contains(&Packet::PingReq), "expected a ping despite ongoing publishes: {written:?}");
        // Ping goes unanswered; nothing else arrives either.
        assert!(matches!(c.poll(t0 + Duration::from_secs(4)), Err(MqttError::Timeout)));
    }

    #[test]
    fn poll_reports_closed_connection() {
        let (io, h) = mock_pair();
        h.feed(&connack(0));
        let mut c = Client::connect(io, &opts(), Duration::from_secs(1)).unwrap();
        h.close();
        assert!(matches!(c.poll(Instant::now()), Err(MqttError::Io(_))));
    }
}

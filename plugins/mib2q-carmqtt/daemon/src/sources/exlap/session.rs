//! One ExLAP TCP session: framing, request ids, handshake.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use super::auth;
use super::xml::{escape_attr, parse, Framer, Msg};
use crate::sources::{stopped, SourceError, StopFlag};

#[derive(Debug)]
pub enum ExlapError {
    Io(io::Error),
    Closed,
    Timeout(&'static str),
    AuthFailed,
    Protocol(String),
    Stopped,
}

impl From<io::Error> for ExlapError {
    fn from(e: io::Error) -> Self {
        ExlapError::Io(e)
    }
}

impl fmt::Display for ExlapError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ExlapError::Io(e) => write!(f, "i/o: {e}"),
            ExlapError::Closed => write!(f, "server closed the connection"),
            ExlapError::Timeout(what) => write!(f, "timed out waiting for {what}"),
            ExlapError::AuthFailed => write!(f, "authentication failed"),
            ExlapError::Protocol(s) => write!(f, "protocol: {s}"),
            ExlapError::Stopped => write!(f, "stopped"),
        }
    }
}

impl From<ExlapError> for SourceError {
    fn from(e: ExlapError) -> Self {
        SourceError(format!("exlap: {e}"))
    }
}

pub struct Session<S: Read + Write> {
    io: S,
    framer: Framer,
    queue: VecDeque<String>,
    next_id: u32,
}

impl<S: Read + Write> Session<S> {
    pub fn new(io: S) -> Self {
        Session { io, framer: Framer::default(), queue: VecDeque::new(), next_id: 1 }
    }

    pub fn send(&mut self, body: &str) -> Result<u32, ExlapError> {
        let id = self.next_id;
        self.next_id += 1;
        self.io.write_all(format!("<Req id=\"{id}\">{body}</Req>").as_bytes())?;
        self.io.flush()?;
        Ok(id)
    }

    /// The next message, or `None` if none arrived before `deadline`.
    pub fn recv(&mut self, deadline: Instant) -> Result<Option<Msg>, ExlapError> {
        loop {
            if let Some(m) = self.queue.pop_front() {
                return Ok(Some(parse(&m)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut buf = [0u8; 8192];
            match self.io.read(&mut buf) {
                Ok(0) => return Err(ExlapError::Closed),
                Ok(n) => {
                    let msgs = self.framer.push(&buf[..n]);
                    self.queue.extend(msgs);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Wait for a message matching `pred`, checking `stop` at least every 200 ms so a
    /// requested stop is noticed promptly instead of only at the overall `deadline`.
    fn wait_for<F>(&mut self, deadline: Instant, stop: &StopFlag, what: &'static str, mut pred: F) -> Result<Msg, ExlapError>
    where
        F: FnMut(&Msg) -> bool,
    {
        loop {
            if stopped(stop) {
                return Err(ExlapError::Stopped);
            }
            let slice = Instant::now() + Duration::from_millis(200);
            let slice_deadline = if slice < deadline { slice } else { deadline };
            match self.recv(slice_deadline)? {
                Some(m) if pred(&m) => return Ok(m),
                Some(_) => continue,
                None if Instant::now() >= deadline => return Err(ExlapError::Timeout(what)),
                None => continue,
            }
        }
    }

    pub fn handshake(&mut self, user: &str, pass: &str, timeout: Duration, stop: &StopFlag) -> Result<(), ExlapError> {
        let deadline = Instant::now() + timeout;
        self.wait_for(deadline, stop, "Init", |m| matches!(m, Msg::Init))?;
        let id = self.send(r#"<Protocol version="1" returnCapabilities="true"/>"#)?;
        self.wait_for(deadline, stop, "Protocol", |m| matches!(m, Msg::Rsp { id: Some(i), .. } if *i == id))?;
        let id = self.send(r#"<Authenticate phase="challenge" useHash="sha256"/>"#)?;
        let msg = self.wait_for(deadline, stop, "Challenge", |m| matches!(m, Msg::Rsp { id: Some(i), .. } if *i == id))?;
        let children = match msg {
            Msg::Rsp { children, .. } => children,
            _ => unreachable!("wait_for only returns matching messages"),
        };
        let nonce = children
            .iter()
            .find(|e| e.tag == "Challenge")
            .and_then(|e| e.attr("nonce"))
            .ok_or_else(|| ExlapError::Protocol("no nonce in challenge".into()))?
            .to_string();
        let cn = auth::cnonce();
        let digest = auth::digest(user, pass, &nonce, &cn);
        let id = self.send(&format!(
            r#"<Authenticate phase="response" user="{}" cnonce="{}" digest="{}"/>"#,
            escape_attr(user),
            cn,
            digest
        ))?;
        let msg = self.wait_for(deadline, stop, "Authenticate", |m| matches!(m, Msg::Rsp { id: Some(i), .. } if *i == id))?;
        let status = match msg {
            Msg::Rsp { status, .. } => status,
            _ => unreachable!("wait_for only returns matching messages"),
        };
        match status {
            None => Ok(()),
            Some(s) if s == "authenticationFailed" => Err(ExlapError::AuthFailed),
            Some(s) => Err(ExlapError::Protocol(format!("authenticate: {s}"))),
        }
    }

    pub fn subscribe(&mut self, url: &str, ival_ms: u32) -> Result<u32, ExlapError> {
        self.send(&format!(r#"<Subscribe url="{}" ival="{}" timeStamp="true"/>"#, escape_attr(url), ival_ms))
    }

    pub fn get(&mut self, url: &str) -> Result<u32, ExlapError> {
        self.send(&format!(r#"<Get url="{}"/>"#, escape_attr(url)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::stop_flag;
    use crate::testutil::{FakeExlap, FakeExlapOptions};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::Ordering;

    fn connect(addr: std::net::SocketAddr) -> TcpStream {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        s
    }

    #[test]
    fn handshake_succeeds_with_builtin_login() {
        let fake = FakeExlap::start(FakeExlapOptions::fixtures());
        let mut s = Session::new(connect(fake.addr));
        s.handshake("RSE_3-DE1400", auth::builtin_password("RSE_3-DE1400").unwrap(), Duration::from_secs(2), &stop_flag()).unwrap();
        let id = s.get("totalDistance").unwrap();
        match s.recv(Instant::now() + Duration::from_secs(2)).unwrap() {
            Some(Msg::Rsp { id: Some(i), children, .. }) => {
                assert_eq!(i, id);
                assert_eq!(children[0].attr("url"), Some("totalDistance"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn wrong_password_is_auth_failed() {
        let fake = FakeExlap::start(FakeExlapOptions::fixtures());
        let mut s = Session::new(connect(fake.addr));
        let r = s.handshake("RSE_3-DE1400", "wrong", Duration::from_secs(2), &stop_flag());
        assert!(matches!(r, Err(ExlapError::AuthFailed)), "{r:?}");
    }

    #[test]
    fn silent_server_times_out() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let _keep = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            std::thread::sleep(Duration::from_secs(2));
            drop(s);
        });
        let mut s = Session::new(connect(addr));
        assert!(matches!(s.handshake("u", "p", Duration::from_millis(200), &stop_flag()), Err(ExlapError::Timeout("Init"))));
    }

    #[test]
    fn handshake_stops_promptly() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let _keep = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            std::thread::sleep(Duration::from_secs(10));
            drop(s);
        });
        let stop = stop_flag();
        let s2 = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            s2.store(true, Ordering::Relaxed);
        });
        let mut s = Session::new(connect(addr));
        let started = Instant::now();
        // A 10 s handshake timeout: only the stop path, not the timeout, can make this pass.
        let r = s.handshake("u", "p", Duration::from_secs(10), &stop);
        assert!(matches!(r, Err(ExlapError::Stopped)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    }
}

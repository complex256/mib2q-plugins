//! Helpers shared by unit and integration tests: an in-memory duplex pipe
//! (and, from Task 13, a fake ExLAP server). Small enough to always compile.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use crate::mqtt::codec::{self, Packet};

#[derive(Default)]
struct Pipe {
    inbound: VecDeque<u8>,
    outbound: Vec<u8>,
    closed: bool,
}

/// The code-under-test side. Reads return `WouldBlock` when nothing is queued,
/// like a socket with a read timeout, and `Ok(0)` once the handle closes it.
#[derive(Clone)]
pub struct MockIo(Arc<Mutex<Pipe>>);

/// The test's side of the pipe.
#[derive(Clone)]
pub struct MockHandle(Arc<Mutex<Pipe>>);

pub fn mock_pair() -> (MockIo, MockHandle) {
    let p = Arc::new(Mutex::new(Pipe::default()));
    (MockIo(p.clone()), MockHandle(p))
}

impl Read for MockIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut p = self.0.lock().unwrap();
        if p.inbound.is_empty() {
            return if p.closed { Ok(0) } else { Err(io::ErrorKind::WouldBlock.into()) };
        }
        let n = buf.len().min(p.inbound.len());
        for (slot, b) in buf.iter_mut().zip(p.inbound.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }
}

impl Write for MockIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut p = self.0.lock().unwrap();
        if p.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        p.outbound.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl MockHandle {
    pub fn feed(&self, bytes: &[u8]) {
        self.0.lock().unwrap().inbound.extend(bytes.iter().copied());
    }

    pub fn take_written(&self) -> Vec<u8> {
        std::mem::take(&mut self.0.lock().unwrap().outbound)
    }

    pub fn close(&self) {
        self.0.lock().unwrap().closed = true;
    }
}

/// Decode every complete MQTT packet in `bytes`. Panics on malformed input.
pub fn mqtt_packets(mut bytes: &[u8]) -> Vec<Packet> {
    let mut out = Vec::new();
    while let Some((p, n)) = codec::decode(bytes).expect("valid mqtt") {
        out.push(p);
        bytes = &bytes[n..];
    }
    out
}

// ---------------------------------------------------------------------------
// Fake ExLAP server (for session/source tests and the end-to-end test).

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::sources::exlap::auth;
use crate::sources::exlap::xml::{start_tags, Framer};

const FAKE_NONCE: &str = "tDJMI/KP3mVbdlD97P2p5A==";

#[derive(Clone)]
pub struct FakeExlapOptions {
    /// URL -> the server's full `<Rsp>` to a Get (the fixture format).
    pub responses: HashMap<String, String>,
    pub user: String,
    pub password: String,
    /// When false the server ignores `Get` (to exercise the liveness probe).
    pub answer_gets: bool,
    /// Close each connection this long after it was accepted.
    pub drop_after: Option<Duration>,
}

impl FakeExlapOptions {
    /// The responses captured from the car, served to `RSE_3-DE1400`.
    pub fn fixtures() -> FakeExlapOptions {
        FakeExlapOptions {
            responses: serde_json::from_str(include_str!("../tests/fixtures/exlap/get_responses.json")).expect("fixture json"),
            user: "RSE_3-DE1400".into(),
            password: auth::builtin_password("RSE_3-DE1400").unwrap().into(),
            answer_gets: true,
            drop_after: None,
        }
    }
}

pub struct FakeExlap {
    pub addr: SocketAddr,
    pub connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for FakeExlap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl FakeExlap {
    pub fn start(opts: FakeExlapOptions) -> FakeExlap {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let opts = Arc::new(opts);
        let (s2, c2) = (stop.clone(), connections.clone());
        thread::spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        c2.fetch_add(1, Ordering::SeqCst);
                        let (o, s) = (opts.clone(), s2.clone());
                        thread::spawn(move || fake_serve(stream, &o, &s));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        FakeExlap { addr, connections, stop }
    }
}

/// `<Rsp id="NN">inner</Rsp>` -> (inner of ObjectData, whole inner after the Rsp tag).
fn split_fixture(rsp: &str) -> (String, String) {
    let after_rsp = &rsp[rsp.find('>').map(|i| i + 1).unwrap_or(0)..];
    let fields = match (after_rsp.find("<ObjectData"), after_rsp.find("</ObjectData>")) {
        (Some(a), Some(b)) => {
            let open_end = a + after_rsp[a..].find('>').unwrap() + 1;
            after_rsp[open_end..b].to_string()
        }
        _ => String::new(),
    };
    (fields, after_rsp.to_string())
}

fn fake_serve(mut s: TcpStream, o: &FakeExlapOptions, stop: &AtomicBool) {
    let accepted = Instant::now();
    let _ = s.set_read_timeout(Some(Duration::from_millis(20)));
    let _ = s.write_all(b"<Status>\n  <Init/>\n</Status>\n");
    let mut framer = Framer::default();
    let mut authed = false;
    let mut buf = [0u8; 4096];
    loop {
        if stop.load(Ordering::Relaxed) || o.drop_after.map(|d| accepted.elapsed() >= d).unwrap_or(false) {
            return;
        }
        let n = match s.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
            Err(_) => return,
        };
        for msg in framer.push(&buf[..n]) {
            let tags = start_tags(&msg);
            let (Some(req), Some(cmd)) = (tags.first(), tags.get(1)) else { continue };
            let id = req.attr("id").unwrap_or("0");
            let url = cmd.attr("url").unwrap_or("").to_string();
            let reply = match (cmd.tag.as_str(), cmd.attr("phase")) {
                ("Protocol", _) => format!(r#"<Rsp id="{id}"><Capabilities service="VW SAI-Server HIGH" version="5.0"/></Rsp>"#),
                ("Authenticate", Some("challenge")) => format!(r#"<Rsp id="{id}"><Challenge nonce="{FAKE_NONCE}"/></Rsp>"#),
                ("Authenticate", Some("response")) => {
                    let ok = cmd.attr("user") == Some(o.user.as_str())
                        && cmd.attr("digest") == Some(auth::digest(&o.user, &o.password, FAKE_NONCE, cmd.attr("cnonce").unwrap_or("")).as_str());
                    authed = ok;
                    if ok { format!(r#"<Rsp id="{id}"/>"#) } else { format!(r#"<Rsp id="{id}" status="authenticationFailed"/>"#) }
                }
                ("Heartbeat", _) => format!(r#"<Rsp id="{id}"/>"#),
                ("Subscribe", _) => match o.responses.get(&url) {
                    Some(rsp) if authed => {
                        let (fields, _) = split_fixture(rsp);
                        format!(r#"<Rsp id="{id}"/><Dat timeStamp="2026-09-25T13:21:49.679499-7:00" url="{url}">{fields}</Dat>"#)
                    }
                    _ => format!(r#"<Rsp id="{id}" status="noMatchingUrl"/>"#),
                },
                ("Get", _) if !o.answer_gets => continue,
                ("Get", _) => match o.responses.get(&url) {
                    Some(rsp) if authed || url == "vehicleIdenticationNumber" => {
                        let (_, inner) = split_fixture(rsp);
                        format!(r#"<Rsp id="{id}">{inner}"#)
                    }
                    _ => format!(r#"<Rsp id="{id}" status="noMatchingUrl"/>"#),
                },
                _ => format!(r#"<Rsp id="{id}" status="notSupported"/>"#),
            };
            if s.write_all(reply.as_bytes()).is_err() {
                return;
            }
        }
    }
}

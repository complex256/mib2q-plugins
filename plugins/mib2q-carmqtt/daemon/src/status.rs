//! Local status endpoint: on `127.0.0.1:<port>` every connection receives one
//! `key=value` snapshot of the daemon's state, then the server closes.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::sources::StopFlag;

/// Accept poll / error back-off interval.
const POLL: Duration = Duration::from_millis(100);
/// How long to drain client input after the snapshot is written.
const DRAIN: Duration = Duration::from_millis(150);

/// Longest `exlap_err` value kept; the endpoint is for a glanceable HMI line.
const ERR_MAX: usize = 60;

/// Shared daemon state. Ages are seconds from a monotonic `Instant`: the unit's
/// wall clock starts at 1970, so wall time is meaningless there.
pub struct Status {
    mqtt: AtomicBool,
    exlap: AtomicBool,
    signals: AtomicU64,
    /// Seconds since `started` at the last publish, or -1 before the first.
    last_publish_s: AtomicI64,
    exlap_err: Mutex<String>,
    started: Instant,
}

impl Default for Status {
    fn default() -> Self {
        Self::new()
    }
}

impl Status {
    pub fn new() -> Status {
        Status {
            mqtt: AtomicBool::new(false),
            exlap: AtomicBool::new(false),
            signals: AtomicU64::new(0),
            last_publish_s: AtomicI64::new(-1),
            exlap_err: Mutex::new(String::new()),
            started: Instant::now(),
        }
    }

    pub fn set_mqtt(&self, up: bool) {
        self.mqtt.store(up, Ordering::Relaxed);
    }

    pub fn set_exlap(&self, up: bool) {
        self.exlap.store(up, Ordering::Relaxed);
    }

    pub fn set_signals(&self, n: usize) {
        self.signals.store(n as u64, Ordering::Relaxed);
    }

    pub fn note_publish(&self) {
        self.last_publish_s.store(self.started.elapsed().as_secs() as i64, Ordering::Relaxed);
    }

    /// Record the latest ExLAP error (single line, truncated); "" clears it.
    pub fn set_exlap_err(&self, msg: &str) {
        let one_line: String = msg
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(ERR_MAX)
            .collect();
        if let Ok(mut g) = self.exlap_err.lock() {
            *g = one_line.trim().to_string();
        }
    }

    pub fn render(&self) -> String {
        let now = self.started.elapsed().as_secs() as i64;
        let last = self.last_publish_s.load(Ordering::Relaxed);
        let age = if last < 0 { -1 } else { (now - last).max(0) };
        let err = self.exlap_err.lock().map(|g| g.clone()).unwrap_or_default();
        format!(
            "v=1\ndaemon=carmqtt\nversion={}\nmqtt={}\nlast_publish_age_s={}\nsignals={}\nexlap={}\nexlap_err={}\nuptime_s={}\n",
            env!("CARGO_PKG_VERSION"),
            self.mqtt.load(Ordering::Relaxed) as u8,
            age,
            self.signals.load(Ordering::Relaxed),
            self.exlap.load(Ordering::Relaxed) as u8,
            err,
            now,
        )
    }
}

/// Bind `bind` and serve one snapshot per connection on a `status` thread until
/// `stop` is set. Returns the bound address (useful with port 0).
pub fn spawn(bind: &str, status: Arc<Status>, stop: StopFlag) -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
    let listener = TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let h = thread::Builder::new().name("status".into()).spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((s, _)) => serve(s, &status),
                // WouldBlock is the idle case; EMFILE/ECONNABORTED must not spin a core.
                Err(_) => thread::sleep(POLL),
            }
        }
    })?;
    Ok((addr, h))
}

/// Write the snapshot, then close cleanly: half-close, and drain anything the client
/// sent (e.g. `echo | nc`) for a bounded time so close does not RST over unread data.
fn serve(mut s: TcpStream, status: &Status) {
    let _ = s.set_nonblocking(false);
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    if s.write_all(status.render().as_bytes()).is_err() {
        return;
    }
    let _ = s.shutdown(Shutdown::Write);
    let _ = s.set_read_timeout(Some(DRAIN));
    let deadline = Instant::now() + DRAIN;
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::stop_flag;
    use std::io::Write;
    use std::io::Read;

    fn keys(s: &str) -> Vec<&str> {
        s.lines().map(|l| l.split('=').next().unwrap()).collect()
    }

    #[test]
    fn renders_all_keys() {
        let st = Status::new();
        st.set_mqtt(true);
        st.set_exlap(true);
        st.set_signals(7);
        st.set_exlap_err("boom");
        let out = st.render();
        assert_eq!(
            keys(&out),
            ["v", "daemon", "version", "mqtt", "last_publish_age_s", "signals", "exlap", "exlap_err", "uptime_s"]
        );
        assert!(out.starts_with("v=1\ndaemon=carmqtt\nversion="));
        assert!(out.contains(&format!("version={}\n", env!("CARGO_PKG_VERSION"))));
        assert!(out.contains("\nmqtt=1\n"));
        assert!(out.contains("\nsignals=7\n"));
        assert!(out.contains("\nexlap=1\n"));
        assert!(out.contains("\nexlap_err=boom\n"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn age_is_minus_one_before_first_publish() {
        let st = Status::new();
        assert!(st.render().contains("\nlast_publish_age_s=-1\n"));
        assert!(st.render().contains("\nexlap_err=\n"));
        st.note_publish();
        let out = st.render();
        assert!(out.contains("\nlast_publish_age_s=0\n"), "{out}");
    }

    #[test]
    fn serves_one_snapshot_and_closes() {
        let st = Arc::new(Status::new());
        let (addr, _h) = spawn("127.0.0.1:0", st.clone(), stop_flag()).unwrap();
        for _ in 0..2 {
            let mut c = TcpStream::connect(addr).unwrap();
            let mut buf = String::new();
            c.read_to_string(&mut buf).unwrap();
            assert!(buf.starts_with("v=1\n"));
            assert!(buf.contains("daemon=carmqtt\n"));
        }
    }

    #[test]
    fn closes_cleanly_when_client_writes_first() {
        let st = Arc::new(Status::new());
        let (addr, _h) = spawn("127.0.0.1:0", st, stop_flag()).unwrap();
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(b"\n").unwrap();
        let mut buf = String::new();
        c.read_to_string(&mut buf).unwrap(); // an RST would make this Err
        assert!(buf.starts_with("v=1\n"));
    }

    #[test]
    fn stop_flag_ends_the_thread() {
        let stop = stop_flag();
        let (_addr, h) = spawn("127.0.0.1:0", Arc::new(Status::new()), stop.clone()).unwrap();
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }
}

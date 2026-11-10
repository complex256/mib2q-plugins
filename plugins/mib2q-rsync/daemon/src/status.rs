//! Loopback status and control endpoint.
//!
//! Every connection receives one `key=value` snapshot. A client may first send
//! one command line (`status`, `sync`, `sync <id>`, `cancel`, `mode <m>` or
//! `ssid <name>`) within a short read window. Commands that affect scheduling
//! are queued for the supervisor; `cancel`, `mode` and `ssid` take effect
//! immediately.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::{self, Job};
use crate::StopFlag;

/// How long a client has to send a command before the snapshot is written.
pub const READ_TIMEOUT: Duration = Duration::from_millis(40);
/// How long to drain client input after the snapshot is written.
const DRAIN: Duration = Duration::from_millis(150);
/// Accept poll / error back-off interval.
const POLL: Duration = Duration::from_millis(100);
/// Longest free-text value kept in a snapshot line.
const TEXT_MAX: usize = 160;

/// Commands that need the supervisor (and therefore are queued).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    SyncAll,
    SyncJob(String),
}

/// Per-job view rendered by [`Status::render`].
#[derive(Clone, Debug)]
pub struct JobStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub state: String,
    pub direction: String,
    pub media_ok: bool,
    pub mount: String,
    pub current_file: String,
    pub files_changed: u64,
    pub bytes_xfer: u64,
    pub last_ok: Option<Instant>,
    pub last_changed: u64,
    pub last_hook_rc: i32,
    pub last_err: String,
    pub running: bool,
}

impl JobStatus {
    pub fn from_job(job: &Job) -> JobStatus {
        JobStatus {
            id: job.id.clone(),
            name: job.name.clone(),
            enabled: job.enabled,
            state: if job.enabled {
                "idle".into()
            } else {
                "disabled".into()
            },
            direction: job.direction.clone(),
            media_ok: false,
            mount: String::new(),
            current_file: String::new(),
            files_changed: 0,
            bytes_xfer: 0,
            last_ok: None,
            last_changed: 0,
            last_hook_rc: 0,
            last_err: String::new(),
            running: false,
        }
    }
}

/// Shared daemon state. Ages are seconds from `Instant`: the unit's wall clock
/// starts at 1970, so wall time is meaningless there.
pub struct Status {
    started: Instant,
    jobs: RwLock<Vec<JobStatus>>,
    mode: Mutex<String>,
    ssid: Mutex<String>,
    tailnet: AtomicBool,
    cancel: AtomicBool,
    tx: Sender<Command>,
    rx: Mutex<Receiver<Command>>,
}

impl Default for Status {
    fn default() -> Self {
        Self::new()
    }
}

impl Status {
    pub fn new() -> Status {
        let (tx, rx) = mpsc::channel();
        Status {
            started: Instant::now(),
            jobs: RwLock::new(Vec::new()),
            mode: Mutex::new("auto".into()),
            ssid: Mutex::new(String::new()),
            tailnet: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            tx,
            rx: Mutex::new(rx),
        }
    }

    pub fn set_jobs(&self, jobs: Vec<JobStatus>) {
        if let Ok(mut guard) = self.jobs.write() {
            *guard = jobs;
        }
    }

    pub fn update_jobs<F: FnOnce(&mut [JobStatus])>(&self, f: F) {
        if let Ok(mut guard) = self.jobs.write() {
            f(&mut guard);
        }
    }

    pub fn jobs_snapshot(&self) -> Vec<JobStatus> {
        self.jobs.read().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_mode(&self, mode: &str) -> bool {
        if !config::valid_mode(mode) {
            return false;
        }
        if let Ok(mut guard) = self.mode.lock() {
            *guard = mode.to_string();
        }
        true
    }

    pub fn mode(&self) -> String {
        self.mode
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "auto".into())
    }

    pub fn set_ssid(&self, ssid: &str) {
        if let Ok(mut guard) = self.ssid.lock() {
            *guard = ssid.to_string();
        }
    }

    pub fn ssid(&self) -> String {
        self.ssid.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_tailnet(&self, up: bool) {
        self.tailnet.store(up, Ordering::Relaxed);
    }

    pub fn tailnet(&self) -> bool {
        self.tailnet.load(Ordering::Relaxed)
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn clear_cancel(&self) {
        self.cancel.store(false, Ordering::Relaxed);
    }

    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    /// Pop one queued command, waiting at most `timeout`.
    pub fn next_command(&self, timeout: Duration) -> Option<Command> {
        let rx = self.rx.lock().ok()?;
        match rx.recv_timeout(timeout) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Non-blocking variant, useful for tests and the supervisor's drain pass.
    pub fn try_command(&self) -> Option<Command> {
        let rx = self.rx.lock().ok()?;
        rx.try_recv().ok()
    }

    /// The top-level daemon state: `syncing`, `error` or `idle`.
    pub fn state(&self) -> &'static str {
        let jobs = self.jobs_snapshot();
        if jobs.iter().any(|j| j.running) {
            "syncing"
        } else if jobs
            .iter()
            .any(|j| j.enabled && !j.last_err.trim().is_empty())
        {
            "error"
        } else {
            "idle"
        }
    }

    pub fn render(&self) -> String {
        let jobs = self.jobs_snapshot();
        let state = if jobs.iter().any(|j| j.running) {
            "syncing"
        } else if jobs
            .iter()
            .any(|j| j.enabled && !j.last_err.trim().is_empty())
        {
            "error"
        } else {
            "idle"
        };
        let active = jobs
            .iter()
            .find(|j| j.running)
            .map(|j| j.id.as_str())
            .unwrap_or("");
        let mut out = String::new();
        out.push_str(&format!(
            "v=1\ndaemon=rsyncd\nversion={}\nstate={}\nmode={}\ntailnet={}\nssid={}\nactive_job={}\njobs={}\n",
            env!("CARGO_PKG_VERSION"),
            state,
            self.mode(),
            self.tailnet() as u8,
            sanitize(&self.ssid()),
            sanitize(active),
            jobs.len(),
        ));
        for (i, job) in jobs.iter().enumerate() {
            let age = job
                .last_ok
                .map(|t| t.elapsed().as_secs() as i64)
                .unwrap_or(-1);
            out.push_str(&format!(
                "job.{i}.id={}\n\
                 job.{i}.name={}\n\
                 job.{i}.state={}\n\
                 job.{i}.direction={}\n\
                 job.{i}.media_ok={}\n\
                 job.{i}.mount={}\n\
                 job.{i}.current_file={}\n\
                 job.{i}.files_changed={}\n\
                 job.{i}.bytes_xfer={}\n\
                 job.{i}.last_ok_age_s={}\n\
                 job.{i}.last_changed={}\n\
                 job.{i}.last_hook_rc={}\n\
                 job.{i}.last_err={}\n",
                sanitize(&job.id),
                sanitize(&job.name),
                sanitize(&job.state),
                sanitize(&job.direction),
                job.media_ok as u8,
                sanitize(&job.mount),
                sanitize(&job.current_file),
                job.files_changed,
                job.bytes_xfer,
                age,
                job.last_changed,
                job.last_hook_rc,
                sanitize(&job.last_err),
            ));
        }
        out.push_str(&format!("uptime_s={}\n", self.started.elapsed().as_secs()));
        out
    }
}

/// Collapse a value to one line and cap its length.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(TEXT_MAX)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Interpret one command line. Unknown commands are ignored.
fn handle_line(line: &str, status: &Status) {
    let line = line.trim();
    if line.is_empty() || line == "status" {
        return;
    }
    if line == "sync" {
        status.send(Command::SyncAll);
        return;
    }
    if line == "cancel" {
        status.request_cancel();
        return;
    }
    if line == "mode" {
        return;
    }
    if line == "ssid" {
        status.set_ssid("");
        return;
    }
    if let Some(rest) = line.strip_prefix("sync ") {
        let id = rest.trim();
        if !id.is_empty() {
            status.send(Command::SyncJob(id.to_string()));
        }
        return;
    }
    if let Some(rest) = line.strip_prefix("mode ") {
        status.set_mode(rest.trim());
        return;
    }
    if let Some(rest) = line.strip_prefix("ssid ") {
        status.set_ssid(rest.trim_end());
    }
}

/// Read up to one line, waiting at most [`READ_TIMEOUT`].
fn read_command(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 128];
    while Instant::now() < deadline && buf.len() < 1024 {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if chunk[..n].contains(&b'\n') {
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Serve one command + snapshot, then close cleanly: half-close, and drain
/// anything else the client sent (e.g. `echo | nc`) for a bounded time so close
/// does not RST over unread data.
fn serve(mut stream: TcpStream, status: &Status) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let command = read_command(&mut stream);
    handle_line(&command, status);
    if stream.write_all(status.render().as_bytes()).is_err() {
        return;
    }
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(DRAIN));
    let deadline = Instant::now() + DRAIN;
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

/// Bind `bind` and serve snapshots/commands on a `status` thread until `stop`
/// is set. Returns the bound address (useful with port 0).
pub fn spawn(
    bind: &str,
    status: Arc<Status>,
    stop: StopFlag,
) -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
    let listener = TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let handle = thread::Builder::new()
        .name("status".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &status),
                    // WouldBlock is the idle case; EMFILE/ECONNABORTED must not spin a core.
                    Err(_) => thread::sleep(POLL),
                }
            }
        })?;
    Ok((addr, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stop_flag;

    fn job_status() -> JobStatus {
        JobStatus {
            id: "music".into(),
            name: "Music Library".into(),
            enabled: true,
            state: "idle".into(),
            direction: "pull".into(),
            media_ok: true,
            mount: "/fs/sda0".into(),
            current_file: String::new(),
            files_changed: 0,
            bytes_xfer: 0,
            last_ok: None,
            last_changed: 0,
            last_hook_rc: 0,
            last_err: String::new(),
            running: false,
        }
    }

    fn keys(s: &str) -> Vec<&str> {
        s.lines().map(|l| l.split('=').next().unwrap()).collect()
    }

    fn query(addr: SocketAddr, command: Option<&str>) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        if let Some(command) = command {
            stream.write_all(command.as_bytes()).unwrap();
        }
        let mut out = String::new();
        stream.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn renders_the_documented_snapshot() {
        let st = Status::new();
        let mut job = job_status();
        job.last_ok = Some(Instant::now());
        st.set_jobs(vec![job]);
        st.set_tailnet(true);
        let out = st.render();
        assert_eq!(
            keys(&out),
            [
                "v",
                "daemon",
                "version",
                "state",
                "mode",
                "tailnet",
                "ssid",
                "active_job",
                "jobs",
                "job.0.id",
                "job.0.name",
                "job.0.state",
                "job.0.direction",
                "job.0.media_ok",
                "job.0.mount",
                "job.0.current_file",
                "job.0.files_changed",
                "job.0.bytes_xfer",
                "job.0.last_ok_age_s",
                "job.0.last_changed",
                "job.0.last_hook_rc",
                "job.0.last_err",
                "uptime_s",
            ]
        );
        assert!(out.starts_with("v=1\ndaemon=rsyncd\nversion="));
        assert!(out.contains("\nstate=idle\nmode=auto\ntailnet=1\nssid=\nactive_job=\njobs=1\n"));
        assert!(out.contains("job.0.id=music\njob.0.name=Music Library\njob.0.state=idle\n"));
        assert!(out.contains("job.0.direction=pull\njob.0.media_ok=1\njob.0.mount=/fs/sda0\n"));
        assert!(out.contains("\njob.0.last_ok_age_s=0\n"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn age_is_minus_one_until_first_success() {
        let st = Status::new();
        st.set_jobs(vec![job_status()]);
        assert!(st.render().contains("\njob.0.last_ok_age_s=-1\n"));
    }

    #[test]
    fn top_level_state_prefers_syncing_then_error() {
        let st = Status::new();
        let mut a = job_status();
        let mut b = job_status();
        b.id = "video".into();
        a.last_err = "boom".into();
        st.set_jobs(vec![a.clone(), b.clone()]);
        assert_eq!(st.state(), "error");
        a.running = true;
        st.set_jobs(vec![a, b]);
        assert_eq!(st.state(), "syncing");
        assert!(st.render().contains("\nactive_job=music\n"));
    }

    #[test]
    fn serves_a_snapshot_without_client_input() {
        let st = Arc::new(Status::new());
        st.set_jobs(vec![job_status()]);
        let (addr, _h) = spawn("127.0.0.1:0", st, stop_flag()).unwrap();
        for _ in 0..2 {
            let out = query(addr, None);
            assert!(out.starts_with("v=1\n"));
            assert!(out.contains("daemon=rsyncd\n"));
        }
    }

    #[test]
    fn handles_sync_commands() {
        let st = Arc::new(Status::new());
        st.set_jobs(vec![job_status()]);
        let (addr, _h) = spawn("127.0.0.1:0", st.clone(), stop_flag()).unwrap();

        let out = query(addr, Some("sync\n"));
        assert!(out.starts_with("v=1\n"));
        assert_eq!(st.try_command(), Some(Command::SyncAll));

        let out = query(addr, Some("sync music\n"));
        assert!(out.starts_with("v=1\n"));
        assert_eq!(st.try_command(), Some(Command::SyncJob("music".into())));

        // Bare `sync` with no newline works too (Net.query-style writes).
        let out = query(addr, Some("sync"));
        assert!(out.starts_with("v=1\n"));
        assert_eq!(st.try_command(), Some(Command::SyncAll));
    }

    #[test]
    fn status_command_does_not_schedule_anything() {
        let st = Arc::new(Status::new());
        let (addr, _h) = spawn("127.0.0.1:0", st.clone(), stop_flag()).unwrap();
        assert!(query(addr, Some("status\n")).starts_with("v=1\n"));
        assert!(query(addr, Some("\n")).starts_with("v=1\n"));
        assert_eq!(st.try_command(), None);
    }

    #[test]
    fn handles_cancel_mode_and_ssid() {
        let st = Arc::new(Status::new());
        let (addr, _h) = spawn("127.0.0.1:0", st.clone(), stop_flag()).unwrap();

        assert!(!st.cancel_flag().load(Ordering::Relaxed));
        assert!(query(addr, Some("cancel\n")).starts_with("v=1\n"));
        assert!(st.cancel_flag().load(Ordering::Relaxed));

        assert!(query(addr, Some("mode wifi_allowlist\n")).starts_with("v=1\n"));
        assert_eq!(st.mode(), "wifi_allowlist");

        // Invalid modes are ignored.
        assert!(query(addr, Some("mode bogus\n")).starts_with("v=1\n"));
        assert_eq!(st.mode(), "wifi_allowlist");

        assert!(query(addr, Some("ssid Garage WiFi\n")).starts_with("v=1\n"));
        assert_eq!(st.ssid(), "Garage WiFi");
        let out = query(addr, Some("ssid\n"));
        assert_eq!(st.ssid(), "");
        assert!(out.contains("\nssid=\n"));

        assert_eq!(st.try_command(), None, "control commands are not queued");
    }

    #[test]
    fn closes_cleanly_when_client_writes_first() {
        let st = Arc::new(Status::new());
        let (addr, _h) = spawn("127.0.0.1:0", st, stop_flag()).unwrap();
        let out = query(addr, Some("sync\nstatus\n"));
        assert!(out.starts_with("v=1\n"));
    }

    #[test]
    fn stop_flag_ends_the_thread() {
        let stop = stop_flag();
        let (_addr, handle) = spawn("127.0.0.1:0", Arc::new(Status::new()), stop.clone()).unwrap();
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }
}

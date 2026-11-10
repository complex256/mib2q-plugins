//! Thread wiring: scheduler -> media/tailnet gating -> rsync + hooks -> status.

use std::io::Read;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::{Config, Job};
use crate::hooks::{self, Event, HookContext};
use crate::media::{self, ResolvedMedia};
use crate::status::{self, Command, JobStatus, Status};
use crate::sync::{self, RunResult, RunStats};
use crate::{debug, error, info, stop_flag, warn, StopFlag};

/// How often the tailnet endpoint is probed.
const TAILNET_PROBE: Duration = Duration::from_secs(5);
/// Connect/read budget for one tailnet probe.
const TAILNET_TIMEOUT: Duration = Duration::from_millis(300);

struct Runtime {
    job: Job,
    next_due: Instant,
    /// Set by an explicit `sync` command; bypasses mode/SSID gating once.
    force: bool,
    resolved: Option<ResolvedMedia>,
    stats: RunStats,
    last_ok: Option<Instant>,
    last_changed: u64,
    last_hook_rc: i32,
    last_err: String,
    state: String,
    running: bool,
    duration_ms: u64,
}

impl Runtime {
    fn new(job: &Job) -> Runtime {
        Runtime {
            job: job.clone(),
            next_due: Instant::now(),
            force: false,
            resolved: None,
            stats: RunStats::default(),
            last_ok: None,
            last_changed: 0,
            last_hook_rc: 0,
            last_err: String::new(),
            state: if job.enabled {
                "idle".into()
            } else {
                "disabled".into()
            },
            running: false,
            duration_ms: 0,
        }
    }
}

pub struct Daemon {
    stop: StopFlag,
    supervisor: Option<JoinHandle<()>>,
    status_thread: Option<JoinHandle<()>>,
    status_addr: Option<SocketAddr>,
}

impl Daemon {
    /// The bound loopback status address (`None` when the bind failed).
    pub fn status_addr(&self) -> Option<SocketAddr> {
        self.status_addr
    }

    /// Block for the life of the daemon.
    pub fn wait(mut self) {
        if let Some(handle) = self.supervisor.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.status_thread.take() {
            let _ = handle.join();
        }
    }

    /// Ask the supervisor and status thread to stop, then join both.
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.supervisor.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.status_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start the scheduler and the loopback status endpoint.
///
/// A status bind failure is logged and ignored: syncing still works.
pub fn start(cfg: &Config, rsync_bin: &Path) -> Result<Daemon, String> {
    let stop = stop_flag();
    let status = Arc::new(Status::new());
    status.set_mode(&cfg.daemon.mode);
    status.set_jobs(cfg.jobs.iter().map(JobStatus::from_job).collect());

    let bind = format!("127.0.0.1:{}", cfg.daemon.status_port);
    let (status_addr, status_thread) = match status::spawn(&bind, status.clone(), stop.clone()) {
        Ok((addr, handle)) => {
            info!("status endpoint on {addr}");
            (Some(addr), Some(handle))
        }
        Err(e) => {
            warn!("status endpoint disabled: cannot bind {bind}: {e}");
            (None, None)
        }
    };

    let supervisor_cfg = cfg.clone();
    let supervisor_rsync = rsync_bin.to_path_buf();
    let supervisor_status = status.clone();
    let supervisor_stop = stop.clone();
    let supervisor = match thread::Builder::new()
        .name("supervisor".into())
        .spawn(move || {
            supervise(
                supervisor_cfg,
                supervisor_rsync,
                supervisor_status,
                supervisor_stop,
            )
        }) {
        Ok(handle) => Some(handle),
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            return Err(format!("spawn supervisor: {e}"));
        }
    };

    Ok(Daemon {
        stop,
        supervisor,
        status_thread,
        status_addr,
    })
}

fn supervise(cfg: Config, rsync_bin: PathBuf, status: Arc<Status>, stop: StopFlag) {
    let mut runtimes: Vec<Runtime> = cfg.jobs.iter().map(Runtime::new).collect();
    let poll = Duration::from_secs(cfg.daemon.poll_interval_s.max(1));
    let mut last_probe: Option<Instant> = None;
    let mut tailnet = false;

    info!(
        "scheduler started: {} job(s), mode {}, poll {}s",
        runtimes.len(),
        status.mode(),
        poll.as_secs()
    );

    while !stop.load(Ordering::Relaxed) {
        let probe_due = last_probe
            .map(|at| at.elapsed() >= TAILNET_PROBE)
            .unwrap_or(true);
        if probe_due {
            tailnet = probe_tailnet(&cfg.daemon.tailscale_status_addr);
            status.set_tailnet(tailnet);
            last_probe = Some(Instant::now());
        }

        while let Some(command) = status.try_command() {
            apply_command(command, &mut runtimes);
        }

        // Run every due job, one at a time, until nothing is due. Deferred jobs
        // push their own next_due forward, so this always terminates.
        loop {
            let now = Instant::now();
            let Some(idx) = runtimes
                .iter()
                .position(|rt| rt.job.enabled && rt.next_due <= now)
            else {
                break;
            };
            attempt(
                &mut runtimes[idx],
                idx,
                &cfg,
                &rsync_bin,
                &status,
                tailnet,
                poll,
            );
        }

        let now = Instant::now();
        let next = runtimes
            .iter()
            .filter(|rt| rt.job.enabled)
            .map(|rt| rt.next_due.saturating_duration_since(now))
            .min()
            .unwrap_or(poll);
        let wait = next.min(poll).max(Duration::from_millis(50));
        if let Some(command) = status.next_command(wait) {
            apply_command(command, &mut runtimes);
        }
    }

    info!("scheduler stopped");
}

fn apply_command(command: Command, runtimes: &mut [Runtime]) {
    match command {
        Command::SyncAll => {
            let now = Instant::now();
            for rt in runtimes.iter_mut() {
                if rt.job.enabled {
                    rt.next_due = now;
                    rt.force = true;
                }
            }
        }
        Command::SyncJob(id) => {
            if let Some(rt) = runtimes.iter_mut().find(|rt| rt.job.id == id) {
                if rt.job.enabled {
                    rt.next_due = Instant::now();
                    rt.force = true;
                }
            }
        }
    }
}

fn attempt(
    rt: &mut Runtime,
    idx: usize,
    cfg: &Config,
    rsync_bin: &Path,
    status: &Status,
    tailnet: bool,
    poll: Duration,
) {
    let forced = rt.force;
    rt.force = false;
    let now = Instant::now();
    let defer = |rt: &mut Runtime, state: &str, status: &Status| {
        rt.state = state.to_string();
        rt.next_due = now + poll;
        publish(rt, idx, status);
    };

    if !forced {
        let mode = status.mode();
        if mode == "manual" {
            let state = if rt.last_err.is_empty() {
                "idle"
            } else {
                "error"
            };
            defer(rt, state, status);
            return;
        }
        if mode == "wifi_allowlist" {
            let ssid = status.ssid();
            let allowed = !ssid.is_empty()
                && cfg
                    .daemon
                    .allowed_ssids
                    .iter()
                    .any(|candidate| candidate == &ssid);
            if !allowed {
                defer(rt, "waiting_wifi", status);
                return;
            }
        }
    }

    match media::resolve(&rt.job.local, &cfg.daemon.media_roots) {
        None => {
            rt.resolved = None;
            defer(rt, "waiting_media", status);
            return;
        }
        Some(resolved) => rt.resolved = Some(resolved),
    }

    if rt.job.require_tailnet && !tailnet {
        defer(rt, "waiting_tailnet", status);
        return;
    }

    rt.running = true;
    rt.state = "syncing".into();
    rt.stats = RunStats::default();
    rt.last_err.clear();
    rt.duration_ms = 0;
    status.clear_cancel();
    publish(rt, idx, status);

    let mount = rt
        .resolved
        .as_ref()
        .map(|resolved| resolved.mount_root.display().to_string())
        .unwrap_or_default();
    info!(
        "job {}: starting {} sync (mount {})",
        rt.job.id, rt.job.direction, mount
    );

    execute(rt, idx, rsync_bin, status);

    rt.running = false;
    if rt.last_err.is_empty() {
        rt.state = "idle".into();
    } else {
        rt.state = "error".into();
    }
    rt.next_due = Instant::now() + Duration::from_secs(rt.job.interval_s.max(1));
    publish(rt, idx, status);

    info!(
        "job {}: finished (changed {}, bytes {}, state {})",
        rt.job.id,
        rt.stats.files_changed(),
        rt.stats.bytes_xfer,
        rt.state
    );
}

fn execute(rt: &mut Runtime, idx: usize, rsync_bin: &Path, status: &Status) {
    let Some(resolved) = rt.resolved.clone() else {
        fail(rt, idx, status, None, "media disappeared".into(), 0);
        return;
    };

    if let Err(e) = sync::prepare_dirs(&rt.job, &resolved) {
        fail(
            rt,
            idx,
            status,
            Some(&resolved),
            format!("prepare dirs: {e}"),
            0,
        );
        return;
    }

    if let Some(script) = rt.job.hooks.pre_sync.clone() {
        match invoke(
            &script,
            &rt.job,
            &resolved,
            Event::PreSync,
            &rt.stats,
            0,
            "",
        ) {
            Ok(()) => rt.last_hook_rc = 0,
            Err(e) => {
                rt.last_hook_rc = e.rc();
                let message = format!("pre_sync: {e}");
                fail(rt, idx, status, Some(&resolved), message, 0);
                return;
            }
        }
    }

    let args = sync::build_args(&rt.job, &resolved.local_path);
    debug!("job {}: rsync {rsync_bin:?} {args:?}", rt.job.id);
    let outcome = sync::run(rsync_bin, &args, status.cancel_flag(), |stats| {
        update_live(status, idx, stats);
    });
    rt.stats = outcome.stats.clone();
    rt.duration_ms = outcome.duration.as_millis() as u64;

    match outcome.result {
        RunResult::Cancelled => {
            info!("job {}: cancelled", rt.job.id);
            rt.last_err.clear();
            publish(rt, idx, status);
            return;
        }
        RunResult::Failed { code, message } => {
            let code = code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into());
            let message = format!("rsync failed ({code}): {message}");
            fail(rt, idx, status, Some(&resolved), message, rt.duration_ms);
            return;
        }
        RunResult::Completed => {}
    }

    if rt.stats.files_changed() > 0 {
        if let Some(script) = rt.job.hooks.on_change.clone() {
            match invoke(
                &script,
                &rt.job,
                &resolved,
                Event::OnChange,
                &rt.stats,
                rt.duration_ms,
                "",
            ) {
                Ok(()) => rt.last_hook_rc = 0,
                Err(e) => {
                    rt.last_hook_rc = e.rc();
                    let message = format!("on_change: {e}");
                    fail(rt, idx, status, Some(&resolved), message, rt.duration_ms);
                    return;
                }
            }
        }
    }

    if let Some(script) = rt.job.hooks.post_sync.clone() {
        match invoke(
            &script,
            &rt.job,
            &resolved,
            Event::PostSync,
            &rt.stats,
            rt.duration_ms,
            "",
        ) {
            Ok(()) => rt.last_hook_rc = 0,
            Err(e) => {
                rt.last_hook_rc = e.rc();
                let message = format!("post_sync: {e}");
                fail(rt, idx, status, Some(&resolved), message, rt.duration_ms);
                return;
            }
        }
    }

    rt.last_ok = Some(Instant::now());
    rt.last_changed = rt.stats.files_changed();
    rt.last_err.clear();
    if rt.stats.files_changed() > 0 {
        sync::flush_buffers();
    }
    publish(rt, idx, status);
}

fn invoke(
    script: &str,
    job: &Job,
    resolved: &ResolvedMedia,
    event: Event,
    stats: &RunStats,
    duration_ms: u64,
    error: &str,
) -> Result<(), hooks::HookError> {
    let ctx = HookContext {
        job,
        resolved,
        event,
        stats,
        duration_ms,
        error,
    };
    hooks::run(script, &ctx, job.hooks.timeout_s)
}

fn fail(
    rt: &mut Runtime,
    idx: usize,
    status: &Status,
    resolved: Option<&ResolvedMedia>,
    message: String,
    duration_ms: u64,
) {
    error!("job {}: {message}", rt.job.id);
    rt.last_err = message.clone();
    if let (Some(script), Some(resolved)) = (rt.job.hooks.on_error.clone(), resolved) {
        match invoke(
            &script,
            &rt.job,
            resolved,
            Event::OnError,
            &rt.stats,
            duration_ms,
            &message,
        ) {
            Ok(()) => rt.last_hook_rc = 0,
            Err(e) => {
                rt.last_hook_rc = e.rc();
                warn!("job {}: on_error hook failed: {e}", rt.job.id);
            }
        }
    }
    rt.state = "error".into();
    publish(rt, idx, status);
}

fn publish(rt: &Runtime, idx: usize, status: &Status) {
    status.update_jobs(|jobs| {
        if let Some(job) = jobs.get_mut(idx) {
            job.state = rt.state.clone();
            job.media_ok = rt.resolved.is_some();
            job.mount = rt
                .resolved
                .as_ref()
                .map(|resolved| resolved.mount_root.display().to_string())
                .unwrap_or_default();
            job.current_file = rt.stats.current_file.clone();
            job.files_changed = rt.stats.files_changed();
            job.bytes_xfer = rt.stats.bytes_xfer;
            job.last_ok = rt.last_ok;
            job.last_changed = rt.last_changed;
            job.last_hook_rc = rt.last_hook_rc;
            job.last_err = rt.last_err.clone();
            job.running = rt.running;
        }
    });
}

fn update_live(status: &Status, idx: usize, stats: &RunStats) {
    status.update_jobs(|jobs| {
        if let Some(job) = jobs.get_mut(idx) {
            job.current_file = stats.current_file.clone();
            job.files_changed = stats.files_changed();
            job.bytes_xfer = stats.bytes_xfer;
        }
    });
}

/// Probe the tailnet status endpoint. A connection that returns a `state=` line
/// is up only when the state is `running`; a listener that returns no status
/// text is treated as reachable.
pub fn probe_tailnet(addr: &str) -> bool {
    let addr = addr.trim();
    if addr.is_empty() {
        return false;
    }
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return false;
    };
    let Some(socket) = addrs.next() else {
        return false;
    };
    match TcpStream::connect_timeout(&socket, TAILNET_TIMEOUT) {
        Ok(mut stream) => {
            let _ = stream.set_read_timeout(Some(TAILNET_TIMEOUT));
            let mut text = String::new();
            let _ = stream.read_to_string(&mut text);
            tailnet_status_up(&text)
        }
        Err(_) => false,
    }
}

fn tailnet_status_up(text: &str) -> bool {
    if text.trim().is_empty() {
        return true;
    }
    for line in text.lines() {
        if let Some(state) = line.strip_prefix("state=") {
            return state.trim().eq_ignore_ascii_case("running");
        }
    }
    true
}

/// On-target smoke test: config parsing, media resolution, itemized parsing and
/// TCP loopback.
pub fn self_test() -> Result<(), String> {
    use crate::testutil::unique_temp_dir;
    use std::io::Write;

    Config::parse(
        r#"
[[jobs]]
id = "music"
direction = "pull"
remote = "rsync://host/music/"
local = "@media:.music-sd/Music/"
"#,
    )
    .map_err(|e| format!("config: {e}"))?;

    let dir = unique_temp_dir("selftest");
    std::fs::create_dir_all(dir.join(".music-sd")).map_err(|e| format!("mkdir: {e}"))?;
    let roots = vec![dir.to_string_lossy().into_owned()];
    let resolved =
        media::resolve("@media:.music-sd/Music/", &roots).ok_or("media resolution failed")?;
    if resolved.local_path != dir.join("Music/") {
        return Err("media local path mismatch".into());
    }

    let mut stats = RunStats::default();
    sync::parse_item_line("ITEM:>f+++++++++|3|a.txt", &mut stats);
    if stats.files_added != 1 || stats.bytes_xfer != 3 {
        return Err("itemized parser mismatch".into());
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let echo = thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf)?;
        stream.write_all(&buf)
    });
    let mut client = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    client.write_all(b"ping").map_err(|e| e.to_string())?;
    let mut buf = [0u8; 4];
    client.read_exact(&mut buf).map_err(|e| e.to_string())?;
    if &buf != b"ping" {
        return Err("loopback echo mismatch".into());
    }
    echo.join()
        .map_err(|_| "echo thread panicked".to_string())?
        .map_err(|e| e.to_string())?;

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_test_passes_on_host() {
        self_test().unwrap();
    }

    #[test]
    fn interprets_tailnet_status_text() {
        assert!(tailnet_status_up(""));
        assert!(tailnet_status_up("state=Running\nip=100.64.0.2\n"));
        assert!(tailnet_status_up("state=running\n"));
        assert!(!tailnet_status_up("state=Stopped\n"));
        assert!(!tailnet_status_up("state=NoState\n"));
        assert!(tailnet_status_up("up=1\n"));
    }

    #[test]
    fn unreachable_tailnet_probe_is_false() {
        assert!(!probe_tailnet(""));
        assert!(!probe_tailnet("127.0.0.1:1"));
    }
}

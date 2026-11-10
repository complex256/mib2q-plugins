//! Generic lifecycle hooks executed through `sh -c` with a `RSYNC_*`
//! environment and a hard timeout.

use std::fmt;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Job;
use crate::media::ResolvedMedia;
use crate::sync::RunStats;

/// Lifecycle hook events, in execution order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    PreSync,
    OnChange,
    PostSync,
    OnError,
}

impl Event {
    pub fn as_str(self) -> &'static str {
        match self {
            Event::PreSync => "pre_sync",
            Event::OnChange => "on_change",
            Event::PostSync => "post_sync",
            Event::OnError => "on_error",
        }
    }
}

/// Everything a hook needs to know about the job/run that invoked it.
pub struct HookContext<'a> {
    pub job: &'a Job,
    pub resolved: &'a ResolvedMedia,
    pub event: Event,
    pub stats: &'a RunStats,
    pub duration_ms: u64,
    /// Empty except for `on_error`.
    pub error: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookError {
    Timeout,
    Spawn(String),
    Exit { code: Option<i32>, stderr: String },
}

impl HookError {
    /// Exit status suitable for `RSYNC_*`/status output: the child's code, or
    /// -1 for a timeout/spawn failure.
    pub fn rc(&self) -> i32 {
        match self {
            HookError::Exit {
                code: Some(code), ..
            } => *code,
            HookError::Exit { code: None, .. } => -1,
            HookError::Timeout | HookError::Spawn(_) => -1,
        }
    }
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookError::Timeout => write!(f, "timed out"),
            HookError::Spawn(e) => write!(f, "spawn failed: {e}"),
            HookError::Exit { code, stderr } => {
                let code = code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into());
                if stderr.is_empty() {
                    write!(f, "exited with {code}")
                } else {
                    write!(f, "exited with {code}: {stderr}")
                }
            }
        }
    }
}

/// Build the `RSYNC_*` environment for one hook invocation.
pub fn environment(ctx: &HookContext) -> Vec<(String, String)> {
    let stats = ctx.stats;
    vec![
        ("RSYNC_JOB_ID".into(), ctx.job.id.clone()),
        ("RSYNC_JOB_NAME".into(), ctx.job.name.clone()),
        ("RSYNC_DIRECTION".into(), ctx.job.direction.clone()),
        (
            "RSYNC_LOCAL_PATH".into(),
            ctx.resolved.local_path.to_string_lossy().into_owned(),
        ),
        (
            "RSYNC_MOUNT_ROOT".into(),
            ctx.resolved.mount_root.to_string_lossy().into_owned(),
        ),
        ("RSYNC_REMOTE".into(), ctx.job.remote.clone()),
        ("RSYNC_FILES_ADDED".into(), stats.files_added.to_string()),
        (
            "RSYNC_FILES_UPDATED".into(),
            stats.files_updated.to_string(),
        ),
        (
            "RSYNC_FILES_DELETED".into(),
            stats.files_deleted.to_string(),
        ),
        (
            "RSYNC_FILES_CHANGED".into(),
            stats.files_changed().to_string(),
        ),
        ("RSYNC_BYTES_XFER".into(), stats.bytes_xfer.to_string()),
        ("RSYNC_DURATION_MS".into(), ctx.duration_ms.to_string()),
        ("RSYNC_HOOK_EVENT".into(), ctx.event.as_str().to_string()),
        ("RSYNC_ERROR".into(), ctx.error.to_string()),
    ]
}

/// Run `script` via `sh -c`, polling every 25 ms and killing it at `timeout_s`.
/// `timeout_s == 0` disables the timeout.
pub fn run(script: &str, ctx: &HookContext, timeout_s: u64) -> Result<(), HookError> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (key, value) in environment(ctx) {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .map_err(|e| HookError::Spawn(e.to_string()))?;

    let stderr = child.stderr.take().map(|mut stderr| {
        let buf = Arc::new(Mutex::new(String::new()));
        let shared = buf.clone();
        let handle = thread::spawn(move || {
            let mut chunk = [0u8; 256];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut text) = shared.lock() {
                            if text.len() < 8192 {
                                text.push_str(&String::from_utf8_lossy(&chunk[..n]));
                            }
                        }
                    }
                }
            }
        });
        (buf, handle)
    });

    let deadline = if timeout_s == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(timeout_s))
    };

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some((_, handle)) = stderr {
                    let _ = handle.join();
                }
                return Err(HookError::Spawn(e.to_string()));
            }
        }
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                // Detach the stderr reader: a grandchild may still hold the
                // pipe open, and the timeout must be enforced promptly.
                drop(stderr);
                return Err(HookError::Timeout);
            }
        }
        thread::sleep(Duration::from_millis(25));
    };

    let stderr_text = match stderr {
        Some((buf, handle)) => {
            let _ = handle.join();
            buf.lock().map(|s| s.clone()).unwrap_or_default()
        }
        None => String::new(),
    };

    match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => {
            let mut text = stderr_text.trim().to_string();
            if text.len() > 500 {
                text = text[text.len() - 500..].to_string();
            }
            Err(HookError::Exit {
                code: status.code(),
                stderr: text,
            })
        }
        None => Err(HookError::Exit {
            code: None,
            stderr: stderr_text.trim().to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;
    use std::fs;

    fn fixture(dir: &std::path::Path) -> (Job, ResolvedMedia, RunStats) {
        let job = Job {
            id: "music".into(),
            name: "Music Library".into(),
            direction: "pull".into(),
            remote: "rsync://host/music/".into(),
            local: "@media:.music-sd/Music/".into(),
            ..Job::default()
        };
        let resolved = ResolvedMedia {
            mount_root: dir.to_path_buf(),
            local_path: dir.join("Music"),
        };
        let stats = RunStats {
            files_added: 2,
            files_updated: 1,
            files_deleted: 1,
            bytes_xfer: 22,
            current_file: "x".into(),
        };
        (job, resolved, stats)
    }

    #[test]
    fn passes_the_documented_environment() {
        let dir = unique_temp_dir("hook-env");
        let (job, resolved, stats) = fixture(&dir);
        let out = dir.join("env.txt");
        let script = format!("env > '{}'", out.display());
        let ctx = HookContext {
            job: &job,
            resolved: &resolved,
            event: Event::OnChange,
            stats: &stats,
            duration_ms: 1234,
            error: "",
        };
        run(&script, &ctx, 10).unwrap();
        let text = fs::read_to_string(&out).unwrap();
        for expected in [
            "RSYNC_JOB_ID=music",
            "RSYNC_JOB_NAME=Music Library",
            "RSYNC_DIRECTION=pull",
            "RSYNC_REMOTE=rsync://host/music/",
            "RSYNC_FILES_ADDED=2",
            "RSYNC_FILES_UPDATED=1",
            "RSYNC_FILES_DELETED=1",
            "RSYNC_FILES_CHANGED=4",
            "RSYNC_BYTES_XFER=22",
            "RSYNC_DURATION_MS=1234",
            "RSYNC_HOOK_EVENT=on_change",
            "RSYNC_ERROR=",
        ] {
            assert!(
                text.lines().any(|l| l == expected),
                "missing {expected}\n{text}"
            );
        }
        assert!(text.contains(&format!(
            "RSYNC_LOCAL_PATH={}",
            resolved.local_path.display()
        )));
        assert!(text.contains(&format!("RSYNC_MOUNT_ROOT={}", dir.display())));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn environment_list_has_every_key_once() {
        let dir = unique_temp_dir("hook-list");
        let (job, resolved, stats) = fixture(&dir);
        let ctx = HookContext {
            job: &job,
            resolved: &resolved,
            event: Event::PreSync,
            stats: &stats,
            duration_ms: 0,
            error: "boom",
        };
        let env = environment(&ctx);
        assert_eq!(env.len(), 14);
        assert!(env
            .iter()
            .any(|(k, v)| k == "RSYNC_HOOK_EVENT" && v == "pre_sync"));
        assert!(env.iter().any(|(k, v)| k == "RSYNC_ERROR" && v == "boom"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_zero_exit_is_reported_with_code_and_stderr() {
        let dir = unique_temp_dir("hook-fail");
        let (job, resolved, stats) = fixture(&dir);
        let ctx = HookContext {
            job: &job,
            resolved: &resolved,
            event: Event::PostSync,
            stats: &stats,
            duration_ms: 0,
            error: "",
        };
        let err = run("echo nope >&2; exit 7", &ctx, 10).unwrap_err();
        assert_eq!(err.rc(), 7);
        assert!(err.to_string().contains("nope"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeout_kills_the_hook() {
        let dir = unique_temp_dir("hook-timeout");
        let (job, resolved, stats) = fixture(&dir);
        let ctx = HookContext {
            job: &job,
            resolved: &resolved,
            event: Event::PreSync,
            stats: &stats,
            duration_ms: 0,
            error: "",
        };
        let started = Instant::now();
        let err = run("sleep 30", &ctx, 1).unwrap_err();
        assert_eq!(err, HookError::Timeout);
        assert_eq!(err.rc(), -1);
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = fs::remove_dir_all(&dir);
    }
}

//! `rsync` argument construction, itemized-output parsing and child execution.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Job;
use crate::media::ResolvedMedia;

/// Counters accumulated from `--itemize-changes` output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunStats {
    pub files_added: u64,
    pub files_updated: u64,
    pub files_deleted: u64,
    pub bytes_xfer: u64,
    pub current_file: String,
}

impl RunStats {
    pub fn files_changed(&self) -> u64 {
        self.files_added + self.files_updated + self.files_deleted
    }
}

/// How a single `rsync` run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunResult {
    Completed,
    Cancelled,
    Failed { code: Option<i32>, message: String },
}

/// The complete result of one `rsync` invocation.
#[derive(Clone, Debug)]
pub struct SyncOutcome {
    pub stats: RunStats,
    pub result: RunResult,
    pub duration: Duration,
}

/// Build the deterministic rsync argv (without the program name).
pub fn build_args(job: &Job, local_path: &Path) -> Vec<String> {
    let mut args = vec![
        "--itemize-changes".to_string(),
        "--out-format=ITEM:%i|%l|%n".to_string(),
    ];

    if job.fat_safe {
        args.extend(
            [
                "-rt",
                "--modify-window=2",
                "--no-perms",
                "--no-owner",
                "--no-group",
                "--no-links",
                "--partial",
                "--partial-dir=.rsync-partial",
                "--temp-dir=.rsync-tmp",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
    } else {
        args.extend(["-a", "--partial"].iter().map(|s| s.to_string()));
    }

    if job.delete {
        args.push("--delete-after".to_string());
    }
    if job.bwlimit_kbps > 0 {
        args.push(format!("--bwlimit={}", job.bwlimit_kbps));
    }
    if let Some(password_file) = &job.password_file {
        args.push(format!("--password-file={password_file}"));
    }
    for pattern in &job.exclude {
        args.push(format!("--exclude={pattern}"));
    }
    args.extend(job.extra_args.iter().cloned());

    let local = local_path.to_string_lossy().into_owned();
    if job.direction == "push" {
        args.push(local);
        args.push(job.remote.clone());
    } else {
        args.push(job.remote.clone());
        args.push(local);
    }
    args
}

/// Create the destination directory (and rsync's partial/temp dirs when a
/// FAT-safe pull needs them) before the child runs.
pub fn prepare_dirs(job: &Job, resolved: &ResolvedMedia) -> std::io::Result<()> {
    fs::create_dir_all(&resolved.local_path)?;
    if job.fat_safe && job.direction == "pull" {
        fs::create_dir_all(resolved.local_path.join(".rsync-partial"))?;
        fs::create_dir_all(resolved.local_path.join(".rsync-tmp"))?;
    }
    Ok(())
}

/// Parse one `ITEM:<itemize>|<length>|<name>` line into `stats`.
///
/// Lines that are not itemized output are ignored. Lengths that are missing or
/// malformed count as zero; the name keeps any embedded `|`.
pub fn parse_item_line(line: &str, stats: &mut RunStats) {
    let Some(rest) = line.strip_prefix("ITEM:") else {
        return;
    };
    let mut parts = rest.splitn(3, '|');
    let itemize = parts.next().unwrap_or("");
    let length = parts
        .next()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let name = parts.next().unwrap_or("");

    if itemize.starts_with("*deleting") {
        stats.files_deleted += 1;
    } else if itemize.starts_with(">f+++++++++")
        || itemize.starts_with("<f+++++++++")
        || itemize.starts_with('c')
    {
        stats.files_added += 1;
        stats.bytes_xfer += length;
        stats.current_file = name.to_string();
    } else if itemize.starts_with(">f") || itemize.starts_with("<f") {
        stats.files_updated += 1;
        stats.bytes_xfer += length;
        stats.current_file = name.to_string();
    }
}

/// Flush filesystem buffers after a run that changed files. Best effort: the
/// unit can be powered off at any time, and this costs no extra crate.
pub fn flush_buffers() {
    let _ = Command::new("sync")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Run `rsync_bin` with `args`, parsing itemized output on stdout and watching
/// `cancel` every 25 ms. `progress` is called with the latest counters while
/// the child runs and once at the end.
pub fn run<F>(
    rsync_bin: &Path,
    args: &[String],
    cancel: &AtomicBool,
    mut progress: F,
) -> SyncOutcome
where
    F: FnMut(&RunStats),
{
    let started = Instant::now();
    let mut stats = RunStats::default();

    if cancel.load(Ordering::Relaxed) {
        return SyncOutcome {
            stats,
            result: RunResult::Cancelled,
            duration: started.elapsed(),
        };
    }

    let mut child = match Command::new(rsync_bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return SyncOutcome {
                stats,
                result: RunResult::Failed {
                    code: None,
                    message: format!("spawn {}: {e}", rsync_bin.display()),
                },
                duration: started.elapsed(),
            };
        }
    };

    let shared = Arc::new(Mutex::new(RunStats::default()));
    let stderr_buf = Arc::new(Mutex::new(String::new()));

    let stdout_thread = child.stdout.take().map(|stdout| {
        let shared = shared.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut stats) = shared.lock() {
                    parse_item_line(&line, &mut stats);
                }
            }
        })
    });

    let stderr_thread = child.stderr.take().map(|mut stderr| {
        let buf = stderr_buf.clone();
        thread::spawn(move || {
            let mut chunk = [0u8; 512];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut out) = buf.lock() {
                            if out.len() < 8192 {
                                out.push_str(&String::from_utf8_lossy(&chunk[..n]));
                            }
                        }
                    }
                }
            }
        })
    });

    let mut cancelled = false;
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Ok(current) = shared.lock() {
                    stats = current.clone();
                }
                // Detach the readers: the failure path must not block on a
                // grandchild that still holds a pipe open.
                drop(stdout_thread);
                drop(stderr_thread);
                return SyncOutcome {
                    stats,
                    result: RunResult::Failed {
                        code: None,
                        message: format!("wait: {e}"),
                    },
                    duration: started.elapsed(),
                };
            }
        }
        if let Ok(current) = shared.lock() {
            progress(&current);
        }
        thread::sleep(Duration::from_millis(25));
    };

    if cancelled {
        // Detached: a grandchild can still hold the pipe open after the
        // direct child was killed. Cancellation must return promptly, so do
        // not block on the reader threads here.
        drop(stdout_thread);
        drop(stderr_thread);
    } else {
        if let Some(h) = stdout_thread {
            let _ = h.join();
        }
        if let Some(h) = stderr_thread {
            let _ = h.join();
        }
    }

    if let Ok(current) = shared.lock() {
        stats = current.clone();
    }
    progress(&stats);

    let result = if cancelled {
        RunResult::Cancelled
    } else {
        match status {
            Some(status) if status.success() => RunResult::Completed,
            Some(status) => {
                let stderr = stderr_buf.lock().map(|s| s.clone()).unwrap_or_default();
                let message = stderr.trim();
                RunResult::Failed {
                    code: status.code(),
                    message: if message.is_empty() {
                        format!("rsync exited with {status}")
                    } else {
                        message.to_string()
                    },
                }
            }
            None => RunResult::Failed {
                code: None,
                message: "rsync terminated".into(),
            },
        }
    };

    SyncOutcome {
        stats,
        result,
        duration: started.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;
    use std::os::unix::fs::PermissionsExt;

    fn job() -> Job {
        Job {
            id: "music".into(),
            name: "Music".into(),
            direction: "pull".into(),
            remote: "rsync://host/music/".into(),
            local: "@media:.music-sd/Music/".into(),
            fat_safe: true,
            delete: true,
            bwlimit_kbps: 4000,
            password_file: Some("/secret".into()),
            exclude: vec![".rsync-*".into(), "System Volume Information/".into()],
            extra_args: vec!["--numeric-ids".into()],
            ..Job::default()
        }
    }

    #[test]
    fn builds_fat_safe_pull_args_in_order() {
        let args = build_args(&job(), Path::new("/fs/sda0/Music/"));
        assert_eq!(
            args,
            [
                "--itemize-changes",
                "--out-format=ITEM:%i|%l|%n",
                "-rt",
                "--modify-window=2",
                "--no-perms",
                "--no-owner",
                "--no-group",
                "--no-links",
                "--partial",
                "--partial-dir=.rsync-partial",
                "--temp-dir=.rsync-tmp",
                "--delete-after",
                "--bwlimit=4000",
                "--password-file=/secret",
                "--exclude=.rsync-*",
                "--exclude=System Volume Information/",
                "--numeric-ids",
                "rsync://host/music/",
                "/fs/sda0/Music/",
            ]
        );
    }

    #[test]
    fn builds_archive_push_args() {
        let mut j = job();
        j.direction = "push".into();
        j.fat_safe = false;
        j.delete = false;
        j.bwlimit_kbps = 0;
        j.password_file = None;
        j.exclude.clear();
        j.extra_args.clear();
        let args = build_args(&j, Path::new("/fs/sda0/Music/"));
        assert_eq!(
            args,
            [
                "--itemize-changes",
                "--out-format=ITEM:%i|%l|%n",
                "-a",
                "--partial",
                "/fs/sda0/Music/",
                "rsync://host/music/",
            ]
        );
    }

    #[test]
    fn parses_every_item_class() {
        let mut s = RunStats::default();
        parse_item_line("ITEM:>f+++++++++|10|new.txt", &mut s);
        parse_item_line("ITEM:<f+++++++++|7|in.txt", &mut s);
        parse_item_line("ITEM:cd+++++++++|0|dir/", &mut s);
        assert_eq!(s.files_added, 3);
        assert_eq!(s.bytes_xfer, 17);
        assert_eq!(s.current_file, "dir/");

        parse_item_line("ITEM:>f.st......|5|upd.txt", &mut s);
        parse_item_line("ITEM:<f.st......|3|other.txt", &mut s);
        assert_eq!(s.files_updated, 2);
        assert_eq!(s.bytes_xfer, 25);
        assert_eq!(s.current_file, "other.txt");

        parse_item_line("ITEM:*deleting  |0|gone.txt", &mut s);
        assert_eq!(s.files_deleted, 1);

        // Ignored: not itemized, or an item class we do not count.
        parse_item_line("sending incremental file list", &mut s);
        parse_item_line("ITEM:.d..t......|0|dir/", &mut s);
        parse_item_line("ITEM:", &mut s);
        assert_eq!(s.files_changed(), 6);
        assert_eq!(s.bytes_xfer, 25);
    }

    #[test]
    fn keeps_pipes_in_name_and_tolerates_bad_lengths() {
        let mut s = RunStats::default();
        parse_item_line("ITEM:>f+++++++++|nope|a|b|c", &mut s);
        assert_eq!(s.files_added, 1);
        assert_eq!(s.bytes_xfer, 0);
        assert_eq!(s.current_file, "a|b|c");
    }

    #[test]
    fn prepares_destination_and_fat_dirs() {
        let dir = unique_temp_dir("prepare");
        let local = dir.join("Music");
        let resolved = ResolvedMedia {
            mount_root: dir.clone(),
            local_path: local.clone(),
        };
        prepare_dirs(&job(), &resolved).unwrap();
        assert!(local.is_dir());
        assert!(local.join(".rsync-partial").is_dir());
        assert!(local.join(".rsync-tmp").is_dir());

        let mut push = job();
        push.direction = "push".into();
        let other = dir.join("Other");
        let resolved = ResolvedMedia {
            mount_root: dir.clone(),
            local_path: other.clone(),
        };
        prepare_dirs(&push, &resolved).unwrap();
        assert!(other.is_dir());
        assert!(
            !other.join(".rsync-tmp").exists(),
            "push must not create pull temp dirs"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runs_a_fake_rsync_and_parses_output() {
        let dir = unique_temp_dir("run");
        let script = dir.join("rsync");
        fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' 'ITEM:>f+++++++++|10|a.txt' 'ITEM:>f.st......|5|b.txt' 'ITEM:*deleting  |0|c.txt'\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let out = run(&script, &[], &cancel, |s| seen.push(s.clone()));
        assert_eq!(out.result, RunResult::Completed);
        assert_eq!(out.stats.files_added, 1);
        assert_eq!(out.stats.files_updated, 1);
        assert_eq!(out.stats.files_deleted, 1);
        assert_eq!(out.stats.bytes_xfer, 15);
        assert_eq!(out.stats.files_changed(), 3);
        assert!(!seen.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_a_failing_child_with_stderr() {
        let dir = unique_temp_dir("fail");
        let script = dir.join("rsync");
        fs::write(&script, "#!/bin/sh\necho 'boom' >&2\nexit 23\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let cancel = AtomicBool::new(false);
        let out = run(&script, &[], &cancel, |_| {});
        match out.result {
            RunResult::Failed { code, message } => {
                assert_eq!(code, Some(23));
                assert!(message.contains("boom"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellation_kills_the_child() {
        let dir = unique_temp_dir("cancel");
        let script = dir.join("rsync");
        fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let out = run(&script, &[], &cancel, |_| {});
        let _ = setter.join();
        assert_eq!(out.result, RunResult::Cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "child was not killed promptly"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binary_is_a_failure() {
        let cancel = AtomicBool::new(false);
        let out = run(Path::new("/definitely/not/rsync"), &[], &cancel, |_| {});
        assert!(matches!(out.result, RunResult::Failed { code: None, .. }));
    }
}

//! End-to-end tests: a fake `rsync` executable, real hooks through `sh -c`,
//! and the real loopback status/control endpoint.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use rsyncd::config::Config;
use rsyncd::log::{self, Level};
use rsyncd::testutil::unique_temp_dir;

fn quiet_log() {
    let _ = log::init(None, Level::Warn, 0);
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn media_root(dir: &Path) -> PathBuf {
    let mount = dir.join("mount");
    fs::create_dir_all(mount.join(".music-sd")).unwrap();
    mount
}

fn shell(path: &Path) -> String {
    format!("'{}'", path.display())
}

fn make_config(
    mode: &str,
    interval: u64,
    mount: &Path,
    local: &str,
    extra_job: &str,
    hooks: &str,
) -> String {
    format!(
        r#"
[daemon]
status_port = 0
media_roots = ["{mount}"]
tailscale_status_addr = "127.0.0.1:1"
mode = "{mode}"
poll_interval_s = 1

[log]
level = "warn"
file = "-"
max_bytes = 1048576

[[jobs]]
id = "music"
name = "Music Library"
direction = "pull"
remote = "rsync://example/music/"
local = "{local}"
interval_s = {interval}
require_tailnet = false
{extra_job}
[jobs.hooks]
{hooks}
"#,
        mount = mount.display(),
        mode = mode,
        local = local,
        interval = interval,
        extra_job = extra_job,
        hooks = hooks,
    )
}

fn start(text: &str, fake_rsync: &Path) -> rsyncd::app::Daemon {
    let cfg = match Config::parse(text) {
        Ok(cfg) => cfg,
        Err(e) => panic!("config error: {e}\n{text}"),
    };
    rsyncd::app::start(&cfg, fake_rsync).expect("start daemon")
}

fn query(addr: SocketAddr, command: Option<&str>) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect status");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    if let Some(command) = command {
        stream.write_all(command.as_bytes()).unwrap();
    }
    let mut out = String::new();
    stream.read_to_string(&mut out).expect("read status");
    out
}

fn value<'a>(snapshot: &'a str, key: &str) -> &'a str {
    snapshot
        .lines()
        .filter_map(|line| line.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .unwrap_or("")
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    done()
}

fn wait_for(path: &Path) -> bool {
    wait_until(Duration::from_secs(8), || path.exists())
}

fn wait_status(addr: SocketAddr, mut done: impl FnMut(&str) -> bool) -> String {
    let mut last = String::new();
    let matched = wait_until(Duration::from_secs(8), || {
        last = query(addr, None);
        done(&last)
    });
    assert!(matched, "status never matched; last snapshot:\n{last}");
    last
}

fn env_map(path: &Path) -> HashMap<String, String> {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn assert_env(env: &HashMap<String, String>, key: &str, expected: &str) {
    assert_eq!(
        env.get(key).map(String::as_str),
        Some(expected),
        "env {key}"
    );
}

fn hook(dir: &Path, name: &str, extra: &str) -> String {
    let base = format!("env > {}", shell(&dir.join(name)));
    if extra.is_empty() {
        base
    } else {
        format!("{base}; {extra}")
    }
}

#[test]
fn pull_reports_changes_runs_hooks_and_serves_status() {
    quiet_log();
    let dir = unique_temp_dir("e2e-pull");
    let mount = media_root(&dir);
    let args_file = dir.join("rsync.args");
    let fake = script(
        &dir,
        "rsync",
        &format!(
            "printf '%s\\n' \"$@\" > {args}\nprintf '%s\\n' \
             'ITEM:>f+++++++++|10|new.txt' \
             'ITEM:<f+++++++++|7|in.txt' \
             'ITEM:>f.st......|5|upd.txt' \
             'ITEM:*deleting  |0|old.txt'",
            args = shell(&args_file)
        ),
    );

    let pre = dir.join("pre.env");
    let on_change = dir.join("on_change.env");
    let post = dir.join("post.env");
    let hooks = format!(
        "pre_sync = \"{}\"\non_change = \"{}\"\npost_sync = \"{}\"\ntimeout_s = 10\n",
        hook(&dir, "pre.env", ""),
        hook(&dir, "on_change.env", ""),
        hook(&dir, "post.env", ""),
    );
    let extra = "fat_safe = true\ndelete = true\nbwlimit_kbps = 4000\nexclude = [\".rsync-*\"]\nextra_args = [\"--numeric-ids\"]\npassword_file = \"/secret\"\n";
    let cfg = make_config(
        "auto",
        3600,
        &mount,
        "@media:.music-sd/Music/",
        extra,
        &hooks,
    );
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    assert!(wait_for(&post), "post_sync did not run");
    let snapshot = wait_status(addr, |s| {
        value(s, "state") == "idle"
            && value(s, "job.0.state") == "idle"
            && value(s, "job.0.files_changed") == "4"
    });

    // Directory layout prepared for a FAT-safe pull.
    let local = mount.join("Music");
    assert!(local.is_dir());
    assert!(local.join(".rsync-partial").is_dir());
    assert!(local.join(".rsync-tmp").is_dir());

    // Deterministic argv.
    let args = fs::read_to_string(&args_file).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    for expected in [
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
        "--numeric-ids",
    ] {
        assert!(lines.contains(&expected), "missing {expected}: {args}");
    }
    let source = lines
        .iter()
        .position(|l| *l == "rsync://example/music/")
        .unwrap();
    let destination = lines
        .iter()
        .position(|l| l.trim_end_matches('/') == local.to_str().unwrap().trim_end_matches('/'))
        .unwrap();
    assert!(
        source < destination,
        "pull must put remote before local: {args}"
    );

    // Hook environment, including the itemized counters.
    let pre_env = env_map(&pre);
    assert_env(&pre_env, "RSYNC_HOOK_EVENT", "pre_sync");
    assert_env(&pre_env, "RSYNC_FILES_CHANGED", "0");
    assert_env(&pre_env, "RSYNC_FILES_ADDED", "0");
    assert_env(&pre_env, "RSYNC_JOB_ID", "music");
    assert_env(&pre_env, "RSYNC_DIRECTION", "pull");
    assert_env(&pre_env, "RSYNC_REMOTE", "rsync://example/music/");
    assert_env(&pre_env, "RSYNC_MOUNT_ROOT", mount.to_str().unwrap());
    assert_eq!(
        pre_env["RSYNC_LOCAL_PATH"].trim_end_matches('/'),
        local.to_str().unwrap().trim_end_matches('/')
    );

    let change_env = env_map(&on_change);
    assert_env(&change_env, "RSYNC_HOOK_EVENT", "on_change");
    assert_env(&change_env, "RSYNC_FILES_ADDED", "2");
    assert_env(&change_env, "RSYNC_FILES_UPDATED", "1");
    assert_env(&change_env, "RSYNC_FILES_DELETED", "1");
    assert_env(&change_env, "RSYNC_FILES_CHANGED", "4");
    assert_env(&change_env, "RSYNC_BYTES_XFER", "22");
    assert_env(&change_env, "RSYNC_ERROR", "");

    let post_env = env_map(&post);
    assert_env(&post_env, "RSYNC_HOOK_EVENT", "post_sync");
    assert_env(&post_env, "RSYNC_FILES_CHANGED", "4");

    // Snapshot fields.
    assert_eq!(value(&snapshot, "daemon"), "rsyncd");
    assert_eq!(value(&snapshot, "mode"), "auto");
    assert_eq!(value(&snapshot, "jobs"), "1");
    assert_eq!(value(&snapshot, "job.0.id"), "music");
    assert_eq!(value(&snapshot, "job.0.name"), "Music Library");
    assert_eq!(value(&snapshot, "job.0.media_ok"), "1");
    assert_eq!(value(&snapshot, "job.0.mount"), mount.to_str().unwrap());
    assert_eq!(value(&snapshot, "job.0.last_changed"), "4");
    assert_eq!(value(&snapshot, "job.0.bytes_xfer"), "22");
    assert_eq!(value(&snapshot, "job.0.current_file"), "upd.txt");
    assert_eq!(value(&snapshot, "job.0.last_err"), "");
    assert_ne!(value(&snapshot, "job.0.last_ok_age_s"), "-1");

    daemon.shutdown();
}

#[test]
fn zero_changes_skip_on_change_but_run_post_sync() {
    quiet_log();
    let dir = unique_temp_dir("e2e-zero");
    let mount = media_root(&dir);
    let fake = script(
        &dir,
        "rsync",
        "printf '%s\\n' 'sending incremental file list'",
    );
    let on_change = dir.join("on_change.env");
    let post = dir.join("post.env");
    let hooks = format!(
        "on_change = \"{}\"\npost_sync = \"{}\"\ntimeout_s = 10\n",
        hook(&dir, "on_change.env", ""),
        hook(&dir, "post.env", ""),
    );
    let cfg = make_config("auto", 3600, &mount, "@media:.music-sd/Music/", "", &hooks);
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    assert!(wait_for(&post), "post_sync did not run");
    let snapshot = wait_status(addr, |s| value(s, "job.0.state") == "idle");
    assert_eq!(value(&snapshot, "job.0.files_changed"), "0");
    assert_eq!(value(&snapshot, "job.0.last_changed"), "0");
    assert!(
        !on_change.exists(),
        "on_change must not run when nothing changed"
    );

    let post_env = env_map(&post);
    assert_env(&post_env, "RSYNC_HOOK_EVENT", "post_sync");
    assert_env(&post_env, "RSYNC_FILES_CHANGED", "0");

    daemon.shutdown();
}

#[test]
fn waits_for_removable_media_then_syncs_when_it_appears() {
    quiet_log();
    let dir = unique_temp_dir("e2e-media");
    let mount = dir.join("mount");
    fs::create_dir_all(&mount).unwrap(); // marker intentionally absent
    let ran = dir.join("ran.marker");
    let fake = script(
        &dir,
        "rsync",
        &format!(
            "printf '%s\\n' 'ITEM:>f+++++++++|1|a'\necho ran > {ran}",
            ran = shell(&ran)
        ),
    );
    let cfg = make_config("auto", 3600, &mount, "@media:.music-sd/Music/", "", "");
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    let snapshot = wait_status(addr, |s| value(s, "job.0.state") == "waiting_media");
    assert_eq!(value(&snapshot, "job.0.media_ok"), "0");
    assert_eq!(value(&snapshot, "job.0.mount"), "");
    assert!(!ran.exists());

    fs::create_dir_all(mount.join(".music-sd")).unwrap();
    assert!(wait_for(&ran), "job did not retry after the media appeared");
    let snapshot = wait_status(addr, |s| {
        value(s, "job.0.state") == "idle" && value(s, "job.0.files_changed") == "1"
    });
    assert_eq!(value(&snapshot, "job.0.media_ok"), "1");
    assert_eq!(value(&snapshot, "job.0.mount"), mount.to_str().unwrap());

    daemon.shutdown();
}

#[test]
fn manual_mode_syncs_only_on_command() {
    quiet_log();
    let dir = unique_temp_dir("e2e-manual");
    let mount = media_root(&dir);
    let ran = dir.join("ran.marker");
    let fake = script(
        &dir,
        "rsync",
        &format!(
            "printf '%s\\n' 'ITEM:>f+++++++++|1|a'\necho ran > {ran}",
            ran = shell(&ran)
        ),
    );
    let cfg = make_config("manual", 3600, &mount, "@media:.music-sd/Music/", "", "");
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    // Give the scheduler several passes: manual mode must not sync by itself.
    thread::sleep(Duration::from_millis(400));
    assert!(!ran.exists(), "manual mode ran without a command");
    let snapshot = query(addr, None);
    assert_eq!(value(&snapshot, "mode"), "manual");

    let out = query(addr, Some("sync music\n"));
    assert!(out.starts_with("v=1\n"));
    assert!(wait_for(&ran), "sync command did not start the job");
    wait_status(addr, |s| value(s, "job.0.state") == "idle");

    daemon.shutdown();
}

#[test]
fn cancel_stops_a_running_sync_without_an_error() {
    quiet_log();
    let dir = unique_temp_dir("e2e-cancel");
    let mount = media_root(&dir);
    let started = dir.join("started.marker");
    let fake = script(
        &dir,
        "rsync",
        &format!("echo started > {}\nexec sleep 30", shell(&started)),
    );
    let cfg = make_config("auto", 3600, &mount, "@media:.music-sd/Music/", "", "");
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    assert!(wait_for(&started), "rsync never started");
    let snapshot = wait_status(addr, |s| value(s, "state") == "syncing");
    assert_eq!(value(&snapshot, "active_job"), "music");

    let out = query(addr, Some("cancel\n"));
    assert!(out.starts_with("v=1\n"));
    let snapshot = wait_status(addr, |s| {
        value(s, "state") == "idle"
            && value(s, "job.0.state") == "idle"
            && value(s, "active_job").is_empty()
    });
    assert_eq!(value(&snapshot, "job.0.last_err"), "");
    assert_eq!(value(&snapshot, "job.0.files_changed"), "0");

    daemon.shutdown();
}

#[test]
fn pre_sync_failure_aborts_and_runs_on_error() {
    quiet_log();
    let dir = unique_temp_dir("e2e-pre-fail");
    let mount = media_root(&dir);
    let ran = dir.join("ran.marker");
    let fake = script(&dir, "rsync", &format!("echo ran > {}", shell(&ran)));
    let on_error = dir.join("on_error.env");
    let hooks = format!(
        "pre_sync = \"{}\"\non_error = \"{}\"\ntimeout_s = 10\n",
        hook(&dir, "pre.env", "exit 3"),
        hook(&dir, "on_error.env", ""),
    );
    let cfg = make_config("auto", 3600, &mount, "@media:.music-sd/Music/", "", &hooks);
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    assert!(wait_for(&on_error), "on_error did not run");
    assert!(!ran.exists(), "rsync ran despite a failing pre_sync");
    let snapshot = wait_status(addr, |s| value(s, "state") == "error");
    assert_eq!(value(&snapshot, "job.0.state"), "error");
    assert!(
        value(&snapshot, "job.0.last_err").contains("pre_sync"),
        "{snapshot}"
    );

    let env = env_map(&on_error);
    assert_env(&env, "RSYNC_HOOK_EVENT", "on_error");
    assert_env(&env, "RSYNC_JOB_ID", "music");
    assert!(
        env["RSYNC_ERROR"].contains("pre_sync"),
        "{:?}",
        env["RSYNC_ERROR"]
    );

    daemon.shutdown();
}

#[test]
fn rsync_failure_runs_on_error_with_stderr() {
    quiet_log();
    let dir = unique_temp_dir("e2e-rsync-fail");
    let mount = media_root(&dir);
    let fake = script(&dir, "rsync", "echo 'disk full' >&2\nexit 5");
    let on_error = dir.join("on_error.env");
    let hooks = format!(
        "on_error = \"{}\"\ntimeout_s = 10\n",
        hook(&dir, "on_error.env", "")
    );
    let cfg = make_config("auto", 3600, &mount, "@media:.music-sd/Music/", "", &hooks);
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    assert!(wait_for(&on_error), "on_error did not run");
    let snapshot = wait_status(addr, |s| value(s, "state") == "error");
    assert!(
        value(&snapshot, "job.0.last_err").contains("rsync failed"),
        "{snapshot}"
    );
    assert!(
        value(&snapshot, "job.0.last_err").contains("disk full"),
        "{snapshot}"
    );

    let env = env_map(&on_error);
    assert_env(&env, "RSYNC_HOOK_EVENT", "on_error");
    assert!(
        env["RSYNC_ERROR"].contains("disk full"),
        "{:?}",
        env["RSYNC_ERROR"]
    );

    daemon.shutdown();
}

#[test]
fn on_change_failure_runs_on_error_and_skips_post_sync() {
    quiet_log();
    let dir = unique_temp_dir("e2e-change-fail");
    let mount = media_root(&dir);
    let fake = script(&dir, "rsync", "printf '%s\\n' 'ITEM:>f+++++++++|3|a.txt'");
    let on_error = dir.join("on_error.env");
    let post = dir.join("post.env");
    let hooks = format!(
        "on_change = \"{}\"\npost_sync = \"{}\"\non_error = \"{}\"\ntimeout_s = 10\n",
        hook(&dir, "on_change.env", "exit 2"),
        hook(&dir, "post.env", ""),
        hook(&dir, "on_error.env", ""),
    );
    let cfg = make_config("auto", 3600, &mount, "@media:.music-sd/Music/", "", &hooks);
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    assert!(wait_for(&on_error), "on_error did not run");
    assert!(
        !post.exists(),
        "post_sync must not run after on_change failed"
    );
    let snapshot = wait_status(addr, |s| value(s, "state") == "error");
    assert!(
        value(&snapshot, "job.0.last_err").contains("on_change"),
        "{snapshot}"
    );
    let env = env_map(&on_error);
    assert!(
        env["RSYNC_ERROR"].contains("on_change"),
        "{:?}",
        env["RSYNC_ERROR"]
    );
    assert_env(&env, "RSYNC_FILES_CHANGED", "1");

    daemon.shutdown();
}

#[test]
fn post_sync_failure_runs_on_error() {
    quiet_log();
    let dir = unique_temp_dir("e2e-post-fail");
    let mount = media_root(&dir);
    let fake = script(
        &dir,
        "rsync",
        "printf '%s\\n' 'sending incremental file list'",
    );
    let on_error = dir.join("on_error.env");
    let on_change = dir.join("on_change.env");
    let hooks = format!(
        "on_change = \"{}\"\npost_sync = \"{}\"\non_error = \"{}\"\ntimeout_s = 10\n",
        hook(&dir, "on_change.env", ""),
        hook(&dir, "post.env", "exit 4"),
        hook(&dir, "on_error.env", ""),
    );
    let cfg = make_config("auto", 3600, &mount, "@media:.music-sd/Music/", "", &hooks);
    let daemon = start(&cfg, &fake);
    let addr = daemon.status_addr().unwrap();

    assert!(wait_for(&on_error), "on_error did not run");
    assert!(
        !on_change.exists(),
        "on_change must not run with zero changes"
    );
    let snapshot = wait_status(addr, |s| value(s, "state") == "error");
    assert!(
        value(&snapshot, "job.0.last_err").contains("post_sync"),
        "{snapshot}"
    );
    let env = env_map(&on_error);
    assert!(
        env["RSYNC_ERROR"].contains("post_sync"),
        "{:?}",
        env["RSYNC_ERROR"]
    );

    daemon.shutdown();
}

//! `rsyncd.toml`: daemon settings, media roots and a list of sync jobs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::log::Level;

/// Runtime sync gating modes.
pub const MODES: [&str; 3] = ["auto", "wifi_allowlist", "manual"];

/// Built-in `rsync` path used when neither `--rsync` nor `[daemon].rsync_bin`
/// is given.
pub const DEFAULT_RSYNC_BIN: &str = "/mnt/app/root/rsyncd/bin/rsync";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonCfg,
    #[serde(default)]
    pub log: LogCfg,
    #[serde(default)]
    pub jobs: Vec<Job>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonCfg {
    /// Loopback status/control endpoint port.
    #[serde(default = "d_status_port")]
    pub status_port: u16,
    /// Path to the `rsync` binary. The CLI `--rsync` overrides this; when this
    /// is blank, [`DEFAULT_RSYNC_BIN`] is used.
    #[serde(default = "d_rsync_bin")]
    pub rsync_bin: String,
    /// Mount points searched for `@media:<marker>` jobs.
    #[serde(default = "d_media_roots")]
    pub media_roots: Vec<String>,
    /// `tsstatus` (or compatible) endpoint used by `require_tailnet` jobs.
    #[serde(default = "d_tailscale_status_addr")]
    pub tailscale_status_addr: String,
    /// `auto` | `wifi_allowlist` | `manual`.
    #[serde(default = "d_mode")]
    pub mode: String,
    /// SSIDs that permit syncing in `wifi_allowlist` mode.
    #[serde(default)]
    pub allowed_ssids: Vec<String>,
    /// How often deferred jobs are retried, in seconds.
    #[serde(default = "d_poll_interval")]
    pub poll_interval_s: u64,
}

impl Default for DaemonCfg {
    fn default() -> Self {
        DaemonCfg {
            status_port: d_status_port(),
            rsync_bin: d_rsync_bin(),
            media_roots: d_media_roots(),
            tailscale_status_addr: d_tailscale_status_addr(),
            mode: d_mode(),
            allowed_ssids: Vec::new(),
            poll_interval_s: d_poll_interval(),
        }
    }
}

impl DaemonCfg {
    /// `[daemon].rsync_bin`, or [`DEFAULT_RSYNC_BIN`] when it is blank.
    /// `main` applies a CLI `--rsync` before this.
    pub fn effective_rsync_bin(&self) -> PathBuf {
        let value = self.rsync_bin.trim();
        if value.is_empty() {
            PathBuf::from(DEFAULT_RSYNC_BIN)
        } else {
            PathBuf::from(value)
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogCfg {
    #[serde(default = "d_log_level")]
    pub level: String,
    /// `-` means stderr.
    #[serde(default = "d_log_file")]
    pub file: String,
    #[serde(default = "d_log_max")]
    pub max_bytes: u64,
}

impl Default for LogCfg {
    fn default() -> Self {
        LogCfg {
            level: d_log_level(),
            file: d_log_file(),
            max_bytes: d_log_max(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    #[serde(default)]
    pub id: String,
    /// Display name; defaults to `id` when blank.
    #[serde(default)]
    pub name: String,
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// `pull` (remote -> local) or `push` (local -> remote).
    #[serde(default = "d_direction")]
    pub direction: String,
    #[serde(default)]
    pub remote: String,
    /// An ordinary path or `@media:<marker>/<relpath>`.
    #[serde(default)]
    pub local: String,
    #[serde(default = "d_interval")]
    pub interval_s: u64,
    #[serde(default = "d_true")]
    pub require_tailnet: bool,
    /// Use FAT/exFAT-safe rsync flags (no permissions/owners/symlinks, 2s window).
    #[serde(default = "d_true")]
    pub fat_safe: bool,
    #[serde(default)]
    pub delete: bool,
    #[serde(default)]
    pub bwlimit_kbps: u64,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub password_file: Option<String>,
    #[serde(default)]
    pub hooks: HooksCfg,
}

impl Default for Job {
    fn default() -> Self {
        Job {
            id: String::new(),
            name: String::new(),
            enabled: true,
            direction: d_direction(),
            remote: String::new(),
            local: String::new(),
            interval_s: d_interval(),
            require_tailnet: true,
            fat_safe: true,
            delete: false,
            bwlimit_kbps: 0,
            exclude: Vec::new(),
            extra_args: Vec::new(),
            password_file: None,
            hooks: HooksCfg::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksCfg {
    /// Run before `rsync`; a failure aborts the run.
    #[serde(default)]
    pub pre_sync: Option<String>,
    /// Run after a successful `rsync` only when files changed.
    #[serde(default)]
    pub on_change: Option<String>,
    /// Run after every successful `rsync` (after `on_change`).
    #[serde(default)]
    pub post_sync: Option<String>,
    /// Run when any phase fails.
    #[serde(default)]
    pub on_error: Option<String>,
    #[serde(default = "d_hook_timeout")]
    pub timeout_s: u64,
}

impl Default for HooksCfg {
    fn default() -> Self {
        HooksCfg {
            pre_sync: None,
            on_change: None,
            post_sync: None,
            on_error: None,
            timeout_s: d_hook_timeout(),
        }
    }
}

fn d_status_port() -> u16 {
    19852
}
fn d_rsync_bin() -> String {
    DEFAULT_RSYNC_BIN.into()
}
fn d_media_roots() -> Vec<String> {
    ["/fs/sda0", "/fs/sdb0", "/fs/sd0", "/fs/sd1", "/fs/usb0"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
fn d_tailscale_status_addr() -> String {
    "127.0.0.1:19850".into()
}
fn d_mode() -> String {
    "auto".into()
}
fn d_poll_interval() -> u64 {
    10
}
fn d_log_level() -> String {
    "info".into()
}
fn d_log_file() -> String {
    "/tmp/rsyncd.log".into()
}
fn d_log_max() -> u64 {
    1_048_576
}
fn d_true() -> bool {
    true
}
fn d_direction() -> String {
    "pull".into()
}
fn d_interval() -> u64 {
    1800
}
fn d_hook_timeout() -> u64 {
    30
}

/// True when `mode` is one of [`MODES`].
pub fn valid_mode(mode: &str) -> bool {
    MODES.contains(&mode)
}

/// A job id is one or more of `[A-Za-z0-9_-]`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;

        if !valid_mode(&cfg.daemon.mode) {
            return Err(format!(
                "daemon.mode '{}' is not one of {}",
                cfg.daemon.mode,
                MODES.join("|")
            ));
        }
        if Level::parse(&cfg.log.level).is_none() {
            return Err(format!(
                "log.level '{}' is not one of error|warn|info|debug|trace",
                cfg.log.level
            ));
        }

        let mut seen = HashSet::new();
        for (i, job) in cfg.jobs.iter_mut().enumerate() {
            if !valid_id(&job.id) {
                return Err(format!(
                    "jobs[{i}].id '{}' must be non-empty and match [A-Za-z0-9_-]+",
                    job.id
                ));
            }
            if !seen.insert(job.id.clone()) {
                return Err(format!("duplicate job id '{}'", job.id));
            }
            if job.direction != "pull" && job.direction != "push" {
                return Err(format!(
                    "job '{}': direction '{}' must be pull or push",
                    job.id, job.direction
                ));
            }
            if job.remote.trim().is_empty() {
                return Err(format!("job '{}': remote must not be empty", job.id));
            }
            if job.local.trim().is_empty() {
                return Err(format!("job '{}': local must not be empty", job.id));
            }
            if job.name.trim().is_empty() {
                job.name = job.id.clone();
            }
        }

        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Config::parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = r#"
[[jobs]]
id = "music"
direction = "pull"
remote = "rsync://host/music/"
local = "@media:.music-sd/Music/"
"#;

    #[test]
    fn defaults_apply() {
        let c = Config::parse(MIN).unwrap();
        assert_eq!(c.daemon.status_port, 19852);
        assert_eq!(c.daemon.rsync_bin, DEFAULT_RSYNC_BIN);
        assert_eq!(
            c.daemon.effective_rsync_bin(),
            PathBuf::from(DEFAULT_RSYNC_BIN)
        );
        assert_eq!(
            c.daemon.media_roots,
            ["/fs/sda0", "/fs/sdb0", "/fs/sd0", "/fs/sd1", "/fs/usb0"]
        );
        assert_eq!(c.daemon.tailscale_status_addr, "127.0.0.1:19850");
        assert_eq!(c.daemon.mode, "auto");
        assert_eq!(c.daemon.poll_interval_s, 10);
        assert_eq!(c.log.level, "info");
        assert_eq!(c.log.file, "/tmp/rsyncd.log");
        assert_eq!(c.log.max_bytes, 1_048_576);

        let j = &c.jobs[0];
        assert_eq!(j.name, "music", "blank name falls back to the id");
        assert!(j.enabled);
        assert_eq!(j.interval_s, 1800);
        assert!(j.require_tailnet);
        assert!(j.fat_safe);
        assert!(!j.delete);
        assert_eq!(j.bwlimit_kbps, 0);
        assert!(j.exclude.is_empty());
        assert!(j.extra_args.is_empty());
        assert_eq!(j.password_file, None);
        assert_eq!(j.hooks.timeout_s, 30);
    }

    #[test]
    fn parses_all_fields() {
        let text = r#"
[daemon]
status_port = 1234
rsync_bin = "/pkg/bin/rsync"
media_roots = ["/m"]
tailscale_status_addr = "127.0.0.1:1"
mode = "wifi_allowlist"
allowed_ssids = ["garage"]
poll_interval_s = 3

[log]
level = "debug"
file = "-"
max_bytes = 5

[[jobs]]
id = "a-b_1"
name = "A"
enabled = false
direction = "push"
remote = "rsync://h/x/"
local = "/fs/sda0/Music/"
interval_s = 60
require_tailnet = false
fat_safe = false
delete = true
bwlimit_kbps = 4000
exclude = ["x/"]
extra_args = ["--numeric-ids"]
password_file = "/secret"

[jobs.hooks]
pre_sync = "true"
on_change = "true"
post_sync = "true"
on_error = "true"
timeout_s = 5
"#;
        let c = Config::parse(text).unwrap();
        assert_eq!(c.daemon.status_port, 1234);
        assert_eq!(c.daemon.rsync_bin, "/pkg/bin/rsync");
        assert_eq!(
            c.daemon.effective_rsync_bin(),
            PathBuf::from("/pkg/bin/rsync")
        );
        assert_eq!(c.daemon.mode, "wifi_allowlist");
        assert_eq!(c.daemon.allowed_ssids, ["garage"]);
        assert_eq!(c.log.level, "debug");
        let j = &c.jobs[0];
        assert_eq!(j.id, "a-b_1");
        assert_eq!(j.direction, "push");
        assert!(!j.enabled);
        assert!(!j.fat_safe);
        assert!(j.delete);
        assert_eq!(j.password_file.as_deref(), Some("/secret"));
        assert_eq!(j.hooks.timeout_s, 5);
        assert_eq!(j.hooks.pre_sync.as_deref(), Some("true"));
    }

    #[test]
    fn rejects_invalid_jobs_and_modes() {
        let bad_id = MIN.replace("id = \"music\"", "id = \"bad id\"");
        assert!(Config::parse(&bad_id).unwrap_err().contains("id"));
        let empty_id = MIN.replace("id = \"music\"", "id = \"\"");
        assert!(Config::parse(&empty_id).unwrap_err().contains("id"));
        let dup = format!("{MIN}\n[[jobs]]\nid=\"music\"\nremote=\"r\"\nlocal=\"l\"\n");
        assert!(Config::parse(&dup).unwrap_err().contains("duplicate"));
        let bad_dir = MIN.replace("direction = \"pull\"", "direction = \"sideways\"");
        assert!(Config::parse(&bad_dir).unwrap_err().contains("direction"));
        let no_remote = MIN.replace("remote = \"rsync://host/music/\"", "remote = \"\"");
        assert!(Config::parse(&no_remote).unwrap_err().contains("remote"));
        let no_local = MIN.replace("local = \"@media:.music-sd/Music/\"", "local = \" \"");
        assert!(Config::parse(&no_local).unwrap_err().contains("local"));
        let bad_mode = format!("[daemon]\nmode = \"sometimes\"\n{MIN}");
        assert!(Config::parse(&bad_mode).unwrap_err().contains("mode"));
        let bad_level = format!("[log]\nlevel = \"loud\"\n{MIN}");
        assert!(Config::parse(&bad_level).unwrap_err().contains("log.level"));
        assert!(Config::parse(&format!("{MIN}\nbogus = 1\n")).is_err());
    }

    #[test]
    fn example_file_parses() {
        let text = include_str!("../deploy/rsyncd.toml.example");
        let c = Config::parse(text).unwrap();
        assert_eq!(c.jobs.len(), 1);
        assert_eq!(c.jobs[0].id, "music");
        assert_eq!(c.daemon.status_port, 19852);
    }
}

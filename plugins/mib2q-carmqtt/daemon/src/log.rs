//! Minimal leveled logger writing to a size-capped file, or stderr.
//!
//! The unit's /tmp is RAM, so the file is capped and rotated once to `<path>.1`.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s.to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            "trace" => Some(Level::Trace),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static SINK: Mutex<Option<Sink>> = Mutex::new(None);

struct Sink {
    path: PathBuf,
    file: File,
    written: u64,
    cap: u64,
}

/// Send log lines to `path` (None = stderr), keeping messages at or above
/// `level`, and rotate once to `<path>.1` when the file would exceed `cap`.
pub fn init(path: Option<PathBuf>, level: Level, cap: u64) -> std::io::Result<()> {
    LEVEL.store(level as u8, Ordering::Relaxed);
    let sink = match path {
        None => None,
        Some(path) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            let written = file.metadata().map(|m| m.len()).unwrap_or(0);
            Some(Sink { path, file, written, cap })
        }
    };
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = sink;
    Ok(())
}

pub fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

pub fn write(level: Level, module: &str, args: fmt::Arguments) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{secs} {} {module}: {args}\n", level.tag());
    let mut guard = SINK.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_mut() {
        None => {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
        Some(sink) => {
            if sink.written + line.len() as u64 > sink.cap {
                let rotated = PathBuf::from(format!("{}.1", sink.path.display()));
                let _ = fs::rename(&sink.path, &rotated);
                if let Ok(f) = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&sink.path)
                {
                    sink.file = f;
                    sink.written = 0;
                }
            }
            if sink.file.write_all(line.as_bytes()).is_ok() {
                sink.written += line.len() as u64;
            }
        }
    }
}

#[macro_export]
macro_rules! error {
    ($($t:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Error) {
            $crate::log::write($crate::log::Level::Error, module_path!(), format_args!($($t)*))
        }
    };
}

#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Warn) {
            $crate::log::write($crate::log::Level::Warn, module_path!(), format_args!($($t)*))
        }
    };
}

#[macro_export]
macro_rules! info {
    ($($t:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Info) {
            $crate::log::write($crate::log::Level::Info, module_path!(), format_args!($($t)*))
        }
    };
}

#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Debug) {
            $crate::log::write($crate::log::Level::Debug, module_path!(), format_args!($($t)*))
        }
    };
}

#[macro_export]
macro_rules! trace {
    ($($t:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Trace) {
            $crate::log::write($crate::log::Level::Trace, module_path!(), format_args!($($t)*))
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_levels() {
        assert_eq!(Level::parse("DEBUG"), Some(Level::Debug));
        assert_eq!(Level::parse("warn"), Some(Level::Warn));
        assert_eq!(Level::parse("loud"), None);
    }

    #[test]
    fn rotates_once_past_cap() {
        let dir = std::env::temp_dir().join(format!("carmqtt-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("t.log");
        init(Some(path.clone()), Level::Info, 200).unwrap();
        for i in 0..20 {
            write(Level::Info, "t", format_args!("line {i} padding padding"));
        }
        init(None, Level::Info, 0).unwrap();
        let main = fs::metadata(&path).unwrap().len();
        assert!(main <= 200, "main log is {main} bytes");
        assert!(dir.join("t.log.1").exists(), "rotated file missing");
        let _ = fs::remove_dir_all(&dir);
    }
}

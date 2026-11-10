//! Small helpers shared by tests and the on-target self test.
//!
//! Kept in the library (rather than a `#[cfg(test)]` module) so `app::self_test`
//! can use the same code path on the unit.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a fresh, empty directory under the system temp dir.
///
/// The name includes the process id, a counter and the current nanoseconds so
/// parallel test threads never collide.
pub fn unique_temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rsyncd-{tag}-{}-{n}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

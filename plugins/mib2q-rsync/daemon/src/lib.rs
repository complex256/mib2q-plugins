//! rsyncd: periodic rsync synchronization for MIB2 High (MHI2Q) head units.
//!
//! The daemon owns the schedule, media/tailnet gating, the `rsync` child
//! process, lifecycle hooks and a loopback status/control endpoint. It is
//! deliberately dependency-light (serde + toml) so it cross-compiles with the
//! QNX Rust toolchain using `build-std`.

pub mod app;
pub mod config;
pub mod hooks;
pub mod log;
pub mod media;
pub mod status;
pub mod sync;
pub mod testutil;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Cooperative stop/cancel flag shared by the supervisor and helper threads.
pub type StopFlag = Arc<AtomicBool>;

/// Create a cleared stop flag.
pub fn stop_flag() -> StopFlag {
    Arc::new(AtomicBool::new(false))
}

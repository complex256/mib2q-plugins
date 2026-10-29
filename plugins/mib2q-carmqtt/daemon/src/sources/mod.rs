//! Data sources. Each maps its own protocol onto catalog signals and runs on
//! its own supervised thread.

pub mod exlap;
pub mod ubx;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::backoff::Backoff;
use crate::value::{SignalId, SourceId, Update, Value};
use crate::warn;

pub type StopFlag = Arc<AtomicBool>;

pub fn stop_flag() -> StopFlag {
    Arc::new(AtomicBool::new(false))
}

pub fn stopped(stop: &StopFlag) -> bool {
    stop.load(Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceError(pub String);

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait Source: Send {
    fn id(&self) -> SourceId;
    /// Every catalog signal this source can emit (after config filtering).
    fn provides(&self) -> &[SignalId];
    /// Block until `stop` is set or the source fails. Owns its own
    /// connect/auth loop; returning `Err` makes the supervisor retry.
    fn run(&mut self, tx: &Sender<Update>, stop: &StopFlag) -> Result<(), SourceError>;
}

/// Tell the router this source no longer has values for `signals`.
pub fn mark_lost(tx: &Sender<Update>, source: SourceId, signals: &[SignalId]) {
    for &signal in signals {
        let _ = tx.send(Update { signal, value: Value::Lost, ts: None, source });
    }
}

pub fn supervise(src: Box<dyn Source>, tx: Sender<Update>, stop: StopFlag) -> JoinHandle<()> {
    supervise_with(src, tx, stop, Duration::from_secs(1), Duration::from_secs(60))
}

pub fn supervise_with(mut src: Box<dyn Source>, tx: Sender<Update>, stop: StopFlag, min: Duration, max: Duration) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("src-{}", src.id()))
        .spawn(move || {
            let mut backoff = Backoff::new(min, max);
            while !stopped(&stop) {
                let started = Instant::now();
                match src.run(&tx, &stop) {
                    Ok(()) if stopped(&stop) => break,
                    Ok(()) => warn!("source {} returned; restarting", src.id()),
                    Err(e) => warn!("source {} failed: {e}", src.id()),
                }
                if started.elapsed() >= max {
                    backoff.reset();
                }
                let until = Instant::now() + backoff.next();
                while !stopped(&stop) {
                    let remaining = until.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    thread::sleep(remaining.min(Duration::from_millis(200)));
                }
            }
        })
        .expect("spawn source thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    struct Flaky {
        runs: Arc<AtomicUsize>,
    }

    impl Source for Flaky {
        fn id(&self) -> SourceId {
            "flaky"
        }
        fn provides(&self) -> &[SignalId] {
            &["odometer"]
        }
        fn run(&mut self, tx: &Sender<Update>, stop: &StopFlag) -> Result<(), SourceError> {
            let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 3 {
                mark_lost(tx, self.id(), self.provides());
                return Err(SourceError(format!("attempt {n} failed")));
            }
            tx.send(Update { signal: "odometer", value: Value::Num { v: 1.0, unit: None }, ts: None, source: "flaky" }).unwrap();
            while !stopped(stop) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        }
    }

    #[test]
    fn retries_until_success_then_stops() {
        let runs = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let stop = stop_flag();
        let h = supervise_with(Box::new(Flaky { runs: runs.clone() }), tx, stop.clone(), Duration::from_millis(10), Duration::from_millis(40));
        let mut got = Vec::new();
        while got.len() < 3 {
            got.push(rx.recv_timeout(Duration::from_secs(2)).expect("update"));
        }
        assert_eq!(got[0].value, Value::Lost);
        assert_eq!(got[1].value, Value::Lost);
        assert_eq!(got[2].value, Value::Num { v: 1.0, unit: None });
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err(), "sender dropped after stop");
    }
}

//! UBX GPS source: reads the RCC's ublox stream over QNX Qnet and emits `location`.
//!
//! The GNSS receiver lives on the RCC node; its position is reachable from the MMX
//! only by running a reader ON the RCC (`on -f rcc cat /dev/nav/gps`) and streaming
//! the UBX bytes back over Qnet — a plain Qnet read of the device returns nothing.
//! We spawn that command, parse the UBX NAV messages (see `parse`), and publish the
//! combined lat/lon/heading/altitude as the catalog `location` signal.

mod parse;

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use parse::{Nav, UbxParser};

use crate::sources::{mark_lost, stopped, Source, SourceError, StopFlag};
use crate::value::{SignalId, SourceId, Update, Value};
use crate::{debug, info};

const PROVIDES: &[SignalId] = &["location"];

pub struct UbxConfig {
    /// Command + args that stream raw UBX to stdout. Default: `on -f rcc cat /dev/nav/gps`.
    pub command: String,
    pub args: Vec<String>,
}

impl Default for UbxConfig {
    fn default() -> Self {
        Self {
            command: "on".into(),
            args: ["-f", "rcc", "cat", "/dev/nav/gps"].iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Metres/second to miles/hour.
const MPS_TO_MPH: f64 = 2.236_936_3;

pub struct UbxSource {
    cfg: UbxConfig,
    heading: Option<f64>, // latest heading from NAV-VELNED, merged into position fixes
    speed_mph: Option<f64>, // latest ground speed from NAV-VELNED, merged into fixes
    logged_fix: bool,     // log the first fix at info, the rest at debug
}

impl UbxSource {
    pub fn new(cfg: UbxConfig) -> Self {
        Self { cfg, heading: None, speed_mph: None, logged_fix: false }
    }

    /// Fold one decoded NAV message into state and, for a position, emit `location`.
    fn emit(&mut self, nav: Nav, tx: &Sender<Update>) {
        match nav {
            Nav::Vel { heading_deg, ground_speed_mps } => {
                self.heading = Some(heading_deg);
                self.speed_mph = Some(ground_speed_mps * MPS_TO_MPH);
            }
            Nav::Pos { lat, lon, alt_m } => {
                if self.logged_fix {
                    debug!("ubx: lat={lat:.6} lon={lon:.6} alt={alt_m:.1}m hdg={:?} mph={:?}", self.heading, self.speed_mph);
                } else {
                    info!("ubx: first fix lat={lat:.6} lon={lon:.6} alt={alt_m:.1}m");
                    self.logged_fix = true;
                }
                let value = Value::Geo { lat, lon, heading: self.heading, alt: Some(alt_m), speed: self.speed_mph };
                let _ = tx.send(Update { signal: "location", value, ts: None, source: self.id() });
            }
        }
    }
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl Source for UbxSource {
    fn id(&self) -> SourceId {
        "ubx"
    }
    fn provides(&self) -> &[SignalId] {
        PROVIDES
    }

    fn run(&mut self, tx: &Sender<Update>, stop: &StopFlag) -> Result<(), SourceError> {
        let mut child = Command::new(&self.cfg.command)
            .args(&self.cfg.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| SourceError(format!("spawn {}: {e}", self.cfg.command)))?;

        let mut out = child.stdout.take().ok_or_else(|| SourceError("no child stdout".into()))?;

        // Read stdout on a helper thread so the run loop can honour `stop` promptly;
        // a blocking read on the pipe would otherwise pin us until the child emits.
        let (btx, brx) = mpsc::channel::<Vec<u8>>();
        let reader = std::thread::spawn(move || {
            let mut b = [0u8; 2048];
            loop {
                match out.read(&mut b) {
                    Ok(0) | Err(_) => break, // EOF or error ends the stream
                    Ok(n) => {
                        if btx.send(b[..n].to_vec()).is_err() {
                            break; // run loop is gone
                        }
                    }
                }
            }
        });

        let mut parser = UbxParser::new();
        let result = loop {
            if stopped(stop) {
                break Ok(());
            }
            match brx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => {
                    for nav in parser.feed(&chunk) {
                        self.emit(nav, tx);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break Err(SourceError("gps stream ended".into())),
            }
        };

        kill(&mut child);
        let _ = reader.join();
        if result.is_err() {
            mark_lost(tx, self.id(), PROVIDES);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::stop_flag;
    use std::sync::atomic::Ordering;

    // Drive a real position fix end-to-end through the source using a fake "reader"
    // command (`printf` of a NAV-VELNED then NAV-POSLLH, hex-encoded) so we exercise
    // spawn -> parse -> emit without the car. Skipped if the shell helpers are absent.
    #[test]
    fn emits_location_from_streamed_ubx() {
        // Build VELNED(heading 123.0) + POSLLH(lat 12.34, lon -56.78, 10 m) as raw bytes,
        // write them to a temp file, and use `cat` as the streaming command.
        use std::io::Write;
        fn frame(cls: u8, id: u8, payload: &[u8]) -> Vec<u8> {
            let mut f = vec![0xB5, 0x62, cls, id, (payload.len() & 0xff) as u8, (payload.len() >> 8) as u8];
            f.extend_from_slice(payload);
            let (mut a, mut b) = (0u8, 0u8);
            for &x in &f[2..] {
                a = a.wrapping_add(x);
                b = b.wrapping_add(a);
            }
            f.push(a);
            f.push(b);
            f
        }
        let mut vel = vec![0u8; 36];
        vel[20..24].copy_from_slice(&2000u32.to_le_bytes()); // gSpeed 2000 cm/s = 20 m/s
        vel[24..28].copy_from_slice(&12_300_000i32.to_le_bytes());
        let mut pos = vec![0u8; 28];
        pos[4..8].copy_from_slice(&(-567_800_000i32).to_le_bytes()); // lon -56.78°
        pos[8..12].copy_from_slice(&123_400_000i32.to_le_bytes()); // lat 12.34°
        pos[16..20].copy_from_slice(&10_000i32.to_le_bytes()); // hMSL 10.000 m
        let mut bytes = frame(0x01, 0x12, &vel);
        bytes.extend_from_slice(&frame(0x01, 0x02, &pos));

        let path = std::env::temp_dir().join(format!("carmqtt_ubx_test_{}.bin", std::process::id()));
        std::fs::File::create(&path).unwrap().write_all(&bytes).unwrap();

        let mut src = UbxSource::new(UbxConfig { command: "cat".into(), args: vec![path.to_string_lossy().into()] });
        let (tx, rx) = mpsc::channel();
        let stop = stop_flag();
        // `cat` exits at EOF -> run returns Err(stream ended) after emitting; that's fine.
        let stop2 = stop.clone();
        let h = std::thread::spawn(move || src.run(&tx, &stop2));

        let upd = rx.recv_timeout(Duration::from_secs(5)).expect("a location update");
        assert_eq!(upd.signal, "location");
        match upd.value {
            Value::Geo { lat, lon, heading, alt, speed } => {
                assert!((lat - 12.34).abs() < 1e-6, "lat {lat}");
                assert!((lon + 56.78).abs() < 1e-6, "lon {lon}");
                assert!(heading.map_or(false, |h| (h - 123.0).abs() < 1e-3), "heading {heading:?}");
                assert!(alt.map_or(false, |a| (a - 10.0).abs() < 1e-6), "alt {alt:?}");
                // 20 m/s -> ~44.7 mph
                assert!(speed.map_or(false, |s| (s - 44.739).abs() < 0.01), "speed {speed:?}");
            }
            v => panic!("expected Geo, got {v:?}"),
        }
        stop.store(true, Ordering::Relaxed);
        let _ = h.join();
        let _ = std::fs::remove_file(&path);
    }
}

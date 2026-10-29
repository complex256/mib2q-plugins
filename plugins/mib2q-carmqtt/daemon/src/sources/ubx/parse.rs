//! Minimal ublox UBX parser.
//!
//! The RCC's `devc-navsens-mib` driver streams ublox UBX binary (sync `0xB5 0x62`)
//! on `/dev/nav/gps`, wrapped in a small driver header. We ignore the wrapper by
//! resyncing on the UBX sync bytes, validate the Fletcher checksum, and decode the
//! two NAV messages carmqtt's `location` needs:
//!   - NAV-POSLLH (0x01 0x02): longitude, latitude, height above MSL
//!   - NAV-VELNED (0x01 0x12): heading of motion
//!
//! `UbxParser` is a streaming parser: feed it arbitrary byte chunks and it returns
//! the complete NAV messages decoded so far, buffering any partial trailing frame.

/// A decoded NAV message we care about.
#[derive(Debug, Clone, PartialEq)]
pub enum Nav {
    /// NAV-POSLLH: WGS84 position. `alt_m` is height above mean sea level (metres).
    Pos { lat: f64, lon: f64, alt_m: f64 },
    /// NAV-VELNED: heading of motion (0..360 deg) and ground speed (metres/sec).
    Vel { heading_deg: f64, ground_speed_mps: f64 },
}

const SYNC1: u8 = 0xB5;
const SYNC2: u8 = 0x62;
const MAX_PAYLOAD: usize = 4096; // UBX payloads are small; guard against garbage lengths.

pub struct UbxParser {
    buf: Vec<u8>,
}

impl UbxParser {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(1024) }
    }

    /// Feed raw bytes; returns every complete, checksum-valid NAV message found.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Nav> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut i = 0;
        while i + 8 <= self.buf.len() {
            // Resync: advance to the next UBX sync pair.
            if !(self.buf[i] == SYNC1 && self.buf[i + 1] == SYNC2) {
                i += 1;
                continue;
            }
            let len = self.buf[i + 4] as usize | ((self.buf[i + 5] as usize) << 8);
            if len > MAX_PAYLOAD {
                i += 1; // implausible length — treat the sync as coincidence, resync.
                continue;
            }
            let total = 6 + len + 2;
            if i + total > self.buf.len() {
                break; // frame not fully buffered yet; wait for more bytes.
            }
            let frame = &self.buf[i..i + total];
            if checksum_ok(frame) {
                if let Some(n) = decode_nav(frame[2], frame[3], &frame[6..6 + len]) {
                    out.push(n);
                }
                i += total;
            } else {
                i += 1; // bad checksum — the sync bytes were data; resync.
            }
        }
        self.buf.drain(..i);
        // Bound the buffer so a stream of garbage (no valid frames) can't grow it forever.
        if self.buf.len() > 2 * MAX_PAYLOAD {
            let cut = self.buf.len() - MAX_PAYLOAD;
            self.buf.drain(..cut);
        }
        out
    }
}

/// UBX Fletcher-8 checksum over class, id, length and payload (everything between
/// the sync bytes and the two trailing checksum bytes).
fn checksum_ok(frame: &[u8]) -> bool {
    let body = &frame[2..frame.len() - 2];
    let (mut a, mut b) = (0u8, 0u8);
    for &x in body {
        a = a.wrapping_add(x);
        b = b.wrapping_add(a);
    }
    a == frame[frame.len() - 2] && b == frame[frame.len() - 1]
}

fn i32le(p: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]])
}

fn u32le(p: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]])
}

fn decode_nav(cls: u8, id: u8, p: &[u8]) -> Option<Nav> {
    match (cls, id) {
        // NAV-POSLLH: iTOW u32@0, lon i32@4, lat i32@8, height i32@12, hMSL i32@16 (mm).
        (0x01, 0x02) if p.len() >= 20 => {
            let lon = i32le(p, 4) as f64 * 1e-7;
            let lat = i32le(p, 8) as f64 * 1e-7;
            let alt_m = i32le(p, 16) as f64 / 1000.0;
            Some(Nav::Pos { lat, lon, alt_m })
        }
        // NAV-VELNED: gSpeed u32@20 (cm/s, ground speed), heading i32@24 (1e-5 deg).
        (0x01, 0x12) if p.len() >= 28 => Some(Nav::Vel {
            heading_deg: i32le(p, 24) as f64 * 1e-5,
            ground_speed_mps: u32le(p, 20) as f64 / 100.0,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid UBX frame with a correct checksum.
    fn frame(cls: u8, id: u8, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![SYNC1, SYNC2, cls, id, (payload.len() & 0xff) as u8, (payload.len() >> 8) as u8];
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

    fn posllh(lon_e7: i32, lat_e7: i32, hmsl_mm: i32) -> Vec<u8> {
        let mut p = vec![0u8; 28];
        p[0..4].copy_from_slice(&1234u32.to_le_bytes()); // iTOW
        p[4..8].copy_from_slice(&lon_e7.to_le_bytes());
        p[8..12].copy_from_slice(&lat_e7.to_le_bytes());
        p[12..16].copy_from_slice(&50000i32.to_le_bytes()); // height
        p[16..20].copy_from_slice(&hmsl_mm.to_le_bytes());
        frame(0x01, 0x02, &p)
    }

    fn velned(heading_e5: i32) -> Vec<u8> {
        let mut p = vec![0u8; 36];
        p[20..24].copy_from_slice(&1000u32.to_le_bytes()); // gSpeed 1000 cm/s = 10 m/s
        p[24..28].copy_from_slice(&heading_e5.to_le_bytes());
        frame(0x01, 0x12, &p)
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn decodes_posllh_position() {
        let mut p = UbxParser::new();
        // 37.4219° N, -122.0841° W, 30.5 m MSL
        let out = p.feed(&posllh(-1_220_841_000, 374_219_000, 30_500));
        assert_eq!(out.len(), 1);
        match out[0] {
            Nav::Pos { lat, lon, alt_m } => {
                assert!(approx(lat, 37.4219), "lat {lat}");
                assert!(approx(lon, -122.0841), "lon {lon}");
                assert!(approx(alt_m, 30.5), "alt {alt_m}");
            }
            _ => panic!("expected Pos"),
        }
    }

    #[test]
    fn decodes_velned_heading() {
        let mut p = UbxParser::new();
        let out = p.feed(&velned(9_000_000)); // 90.00000°
        assert_eq!(out.len(), 1);
        match out[0] {
            Nav::Vel { heading_deg, ground_speed_mps } => {
                assert!(approx(heading_deg, 90.0), "heading {heading_deg}");
                assert!(approx(ground_speed_mps, 10.0), "speed {ground_speed_mps}");
            }
            _ => panic!("expected Vel"),
        }
    }

    #[test]
    fn resyncs_past_driver_wrapper() {
        // The device wraps each UBX frame in a small header (no B5 62 sync); we must
        // resync across it and decode both frames.
        let mut p = UbxParser::new();
        let mut stream = vec![0xCE, 0xC1, 0x0C, 0x00, 0x00, 0x01, 0x01, 0x00, 0x1C, 0x00];
        stream.extend_from_slice(&posllh(100_000_000, 400_000_000, 1000));
        stream.extend_from_slice(&[0xD3, 0xC1, 0x0C, 0x00]); // next frame's wrapper
        stream.extend_from_slice(&velned(4_500_000));
        let out = p.feed(&stream);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Nav::Pos { .. }));
        match out[1] {
            Nav::Vel { heading_deg, .. } => assert!(approx(heading_deg, 45.0), "heading {heading_deg}"),
            _ => panic!("expected Vel"),
        }
    }

    #[test]
    fn resyncs_after_bad_frame() {
        // A stray B5 62 with a small bogus length and wrong checksum must not swallow
        // the following valid frame.
        let mut p = UbxParser::new();
        let mut s = vec![0xB5, 0x62, 0x05, 0x05, 0x02, 0x00, 0xAA, 0xBB, 0x00, 0x00];
        s.extend_from_slice(&velned(1_000_000));
        let out = p.feed(&s);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Nav::Vel { .. }));
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut p = UbxParser::new();
        let mut bad = posllh(1, 2, 3);
        let n = bad.len();
        bad[n - 1] ^= 0xff; // corrupt checksum
        assert!(p.feed(&bad).is_empty());
    }

    #[test]
    fn reassembles_frame_split_across_feeds() {
        let mut p = UbxParser::new();
        let f = posllh(-1_220_000_000, 370_000_000, 0);
        let (head, tail) = f.split_at(7);
        assert!(p.feed(head).is_empty(), "partial frame yields nothing");
        let out = p.feed(tail);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Nav::Pos { .. }));
    }

    #[test]
    fn skips_unknown_messages() {
        let mut p = UbxParser::new();
        let out = p.feed(&frame(0x01, 0x21, &[0u8; 20])); // NAV-TIMEUTC, ignored
        assert!(out.is_empty());
    }
}

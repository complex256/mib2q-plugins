//! ExLAP authentication: SHA-256 digest challenge/response.
//!
//! No crypto crates: SHA-256 is implemented here (FIPS 180-4) and checked
//! against the standard vectors, which keeps the QNX 6.5 build dependency-free.

use std::fs::File;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The public ExLAP logins (from the aa-proxy-rs / `exlap` crate projects).
/// On this car `RSE_3-DE1400` sees the most URLs.
pub const CREDENTIALS: [(&str, &str); 4] = [
    ("Test_TB-105000", "s4T2K6BAv0a7LQvrv3vdaUl17xEl2WJOpTmAThpRZe0=="),
    ("RSE_L-CA2000", "T53Facvq51jO8vQJrBNx3MqLWmPcHf/hkow7yLu7SuA=="),
    ("RSE_3-DE1400", "KozPo8iE0j72pkbWXKcP0QihpxgML3Opp8fNJZ0wN24=="),
    ("ML_74-125000", "Fo7arEpPhAgMMznzxRlV8B7eeZgNDIYQcy0Gr7Ad1Fg=="),
];

pub fn builtin_password(user: &str) -> Option<&'static str> {
    CREDENTIALS.iter().find(|(u, _)| *u == user).map(|(_, p)| *p)
}

/// The server only hashes the first 44 characters of each field (the
/// passwords are 45 characters long).
fn t44(s: &str) -> &str {
    match s.char_indices().nth(44) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn digest(user: &str, pass: &str, nonce: &str, cnonce: &str) -> String {
    let input = format!("{}:{}:{}:{}", t44(user), t44(pass), t44(nonce), t44(cnonce));
    b64(&sha256(input.as_bytes()))
}

pub fn cnonce() -> String {
    let mut buf = [0u8; 16];
    if File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        buf = t.to_le_bytes();
    }
    b64(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        assert_eq!(b64(&sha256(b"abc")), "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=");
        assert_eq!(b64(&sha256(b"")), "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=");
        assert_eq!(b64(&sha256(&[b'a'; 1000])), "Qe3s5C1j6Nm/UVqbppMuHCDLyfWl0TRkWttdsblzfqM=");
    }

    #[test]
    fn base64_padding() {
        assert_eq!(b64(b"a"), "YQ==");
        assert_eq!(b64(b"ab"), "YWI=");
        assert_eq!(b64(&[0u8; 16]), "AAAAAAAAAAAAAAAAAAAAAA==");
    }

    #[test]
    fn digest_matches_reference_and_truncates() {
        let pass = builtin_password("RSE_3-DE1400").unwrap();
        assert_eq!(pass.len(), 45);
        let d = digest("RSE_3-DE1400", pass, "tDJMI/KP3mVbdlD97P2p5A==", "AAAAAAAAAAAAAAAAAAAAAA==");
        assert_eq!(d, "eKlKZ3i3vCHPXp2QCWO5ehrFuSupc7CvLgWvnJB1+pw=");
        assert_eq!(d, digest("RSE_3-DE1400", &pass[..44], "tDJMI/KP3mVbdlD97P2p5A==", "AAAAAAAAAAAAAAAAAAAAAA=="));
    }

    #[test]
    fn credentials_and_cnonce() {
        assert!(builtin_password("nobody").is_none());
        let a = cnonce();
        assert_eq!(a.len(), 24);
        assert_ne!(a, cnonce());
    }
}

//! MQTT 3.1.1 packet codec (sans-IO). Covers what carmqtt sends and receives,
//! plus decoding of CONNECT/SUBSCRIBE so tests can play the broker.

#[derive(Clone, Debug, PartialEq)]
pub struct Will {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Connect {
    pub client_id: String,
    pub keepalive: u16,
    pub clean_session: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    pub will: Option<Will>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Publish {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
    pub dup: bool,
    pub pid: Option<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Connect(Connect),
    ConnAck { session_present: bool, code: u8 },
    Publish(Publish),
    PubAck(u16),
    Subscribe { pid: u16, topics: Vec<(String, u8)> },
    SubAck { pid: u16, codes: Vec<u8> },
    PingReq,
    PingResp,
    Disconnect,
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
    Malformed(&'static str),
    Unsupported(u8),
}

pub fn encode_remaining_length(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

/// `Ok(None)` = need more bytes; otherwise (value, bytes used).
fn decode_remaining_length(buf: &[u8]) -> Result<Option<(usize, usize)>, DecodeError> {
    let mut mult = 1usize;
    let mut value = 0usize;
    for i in 0..4 {
        let Some(&b) = buf.get(i) else { return Ok(None) };
        value += (b & 0x7F) as usize * mult;
        if b & 0x80 == 0 {
            return Ok(Some((value, i + 1)));
        }
        mult *= 128;
    }
    Err(DecodeError::Malformed("remaining length longer than 4 bytes"))
}

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

pub fn encode(p: &Packet) -> Vec<u8> {
    let (first, body): (u8, Vec<u8>) = match p {
        Packet::Connect(c) => {
            let mut b = Vec::new();
            put_str(&mut b, b"MQTT");
            b.push(4);
            let mut flags = 0u8;
            if c.clean_session {
                flags |= 0x02;
            }
            if let Some(w) = &c.will {
                flags |= 0x04 | (w.qos << 3);
                if w.retain {
                    flags |= 0x20;
                }
            }
            if c.password.is_some() {
                flags |= 0x40;
            }
            if c.username.is_some() {
                flags |= 0x80;
            }
            b.push(flags);
            b.extend_from_slice(&c.keepalive.to_be_bytes());
            put_str(&mut b, c.client_id.as_bytes());
            if let Some(w) = &c.will {
                put_str(&mut b, w.topic.as_bytes());
                put_str(&mut b, &w.payload);
            }
            if let Some(u) = &c.username {
                put_str(&mut b, u.as_bytes());
            }
            if let Some(pw) = &c.password {
                put_str(&mut b, pw.as_bytes());
            }
            (0x10, b)
        }
        Packet::ConnAck { session_present, code } => (0x20, vec![*session_present as u8, *code]),
        Packet::Publish(p) => {
            let mut b = Vec::new();
            put_str(&mut b, p.topic.as_bytes());
            if p.qos > 0 {
                b.extend_from_slice(&p.pid.unwrap_or(0).to_be_bytes());
            }
            b.extend_from_slice(&p.payload);
            (0x30 | ((p.dup as u8) << 3) | (p.qos << 1) | p.retain as u8, b)
        }
        Packet::PubAck(pid) => (0x40, pid.to_be_bytes().to_vec()),
        Packet::Subscribe { pid, topics } => {
            let mut b = pid.to_be_bytes().to_vec();
            for (t, q) in topics {
                put_str(&mut b, t.as_bytes());
                b.push(*q);
            }
            (0x82, b)
        }
        Packet::SubAck { pid, codes } => {
            let mut b = pid.to_be_bytes().to_vec();
            b.extend_from_slice(codes);
            (0x90, b)
        }
        Packet::PingReq => (0xC0, Vec::new()),
        Packet::PingResp => (0xD0, Vec::new()),
        Packet::Disconnect => (0xE0, Vec::new()),
    };
    let mut out = vec![first];
    encode_remaining_length(body.len(), &mut out);
    out.extend_from_slice(&body);
    out
}

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Rd<'a> {
    fn u8(&mut self) -> Result<u8, DecodeError> {
        let v = *self.b.get(self.i).ok_or(DecodeError::Malformed("short packet"))?;
        self.i += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let n = self.u16()? as usize;
        let s = self.b.get(self.i..self.i + n).ok_or(DecodeError::Malformed("short string"))?;
        self.i += n;
        Ok(s.to_vec())
    }
    fn string(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.bytes()?).map_err(|_| DecodeError::Malformed("string is not utf-8"))
    }
    fn rest(&mut self) -> Vec<u8> {
        let r = self.b[self.i..].to_vec();
        self.i = self.b.len();
        r
    }
    fn done(&self) -> bool {
        self.i >= self.b.len()
    }
}

pub fn decode(buf: &[u8]) -> Result<Option<(Packet, usize)>, DecodeError> {
    let Some(&first) = buf.first() else { return Ok(None) };
    let Some((len, hl)) = decode_remaining_length(&buf[1..])? else { return Ok(None) };
    let total = 1 + hl + len;
    if buf.len() < total {
        return Ok(None);
    }
    let mut r = Rd { b: &buf[1 + hl..total], i: 0 };
    let pkt = match first >> 4 {
        1 => {
            if r.bytes()? != b"MQTT" {
                return Err(DecodeError::Malformed("protocol name"));
            }
            let _level = r.u8()?;
            let flags = r.u8()?;
            let keepalive = r.u16()?;
            let client_id = r.string()?;
            let will = if flags & 0x04 != 0 {
                let topic = r.string()?;
                let payload = r.bytes()?;
                Some(Will { topic, payload, qos: (flags >> 3) & 3, retain: flags & 0x20 != 0 })
            } else {
                None
            };
            let username = if flags & 0x80 != 0 { Some(r.string()?) } else { None };
            let password = if flags & 0x40 != 0 { Some(r.string()?) } else { None };
            Packet::Connect(Connect { client_id, keepalive, clean_session: flags & 0x02 != 0, username, password, will })
        }
        2 => {
            let session_present = r.u8()? & 1 == 1;
            let code = r.u8()?;
            Packet::ConnAck { session_present, code }
        }
        3 => {
            let qos = (first >> 1) & 3;
            let topic = r.string()?;
            let pid = if qos > 0 { Some(r.u16()?) } else { None };
            Packet::Publish(Publish { topic, payload: r.rest(), qos, retain: first & 1 == 1, dup: first & 8 != 0, pid })
        }
        4 => Packet::PubAck(r.u16()?),
        8 => {
            let pid = r.u16()?;
            let mut topics = Vec::new();
            while !r.done() {
                let t = r.string()?;
                let q = r.u8()?;
                topics.push((t, q));
            }
            Packet::Subscribe { pid, topics }
        }
        9 => {
            let pid = r.u16()?;
            Packet::SubAck { pid, codes: r.rest() }
        }
        12 => Packet::PingReq,
        13 => Packet::PingResp,
        14 => Packet::Disconnect,
        t => return Err(DecodeError::Unsupported(t)),
    };
    Ok(Some((pkt, total)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(p: Packet) {
        let bytes = encode(&p);
        assert_eq!(decode(&bytes).unwrap(), Some((p, bytes.len())));
    }

    #[test]
    fn remaining_length_boundaries() {
        for (n, want) in [
            (0usize, vec![0x00]),
            (127, vec![0x7F]),
            (128, vec![0x80, 0x01]),
            (16383, vec![0xFF, 0x7F]),
            (16384, vec![0x80, 0x80, 0x01]),
        ] {
            let mut out = Vec::new();
            encode_remaining_length(n, &mut out);
            assert_eq!(out, want, "n={n}");
            assert_eq!(decode_remaining_length(&out).unwrap(), Some((n, want.len())));
        }
    }

    #[test]
    fn connect_bytes() {
        let c = Packet::Connect(Connect { client_id: "c".into(), keepalive: 30, clean_session: true, username: None, password: None, will: None });
        assert_eq!(encode(&c), vec![0x10, 13, 0, 4, b'M', b'Q', b'T', b'T', 4, 0x02, 0, 30, 0, 1, b'c']);
        roundtrip(c);
    }

    #[test]
    fn connect_with_everything_roundtrips() {
        let c = Connect {
            client_id: "mib2q-carmqtt".into(),
            keepalive: 30,
            clean_session: true,
            username: Some("u".into()),
            password: Some("p".into()),
            will: Some(Will { topic: "mib2q/v/status".into(), payload: b"offline".to_vec(), qos: 1, retain: true }),
        };
        let bytes = encode(&Packet::Connect(c.clone()));
        assert_eq!(bytes[9], 0x80 | 0x40 | 0x20 | 0x08 | 0x04 | 0x02);
        roundtrip(Packet::Connect(c));
    }

    #[test]
    fn publish_bytes() {
        let p0 = Packet::Publish(Publish { topic: "a/b".into(), payload: b"hi".to_vec(), qos: 0, retain: true, dup: false, pid: None });
        assert_eq!(encode(&p0), vec![0x31, 7, 0, 3, b'a', b'/', b'b', b'h', b'i']);
        let p1 = Packet::Publish(Publish { topic: "a/b".into(), payload: b"hi".to_vec(), qos: 1, retain: true, dup: false, pid: Some(10) });
        assert_eq!(encode(&p1), vec![0x33, 9, 0, 3, b'a', b'/', b'b', 0, 10, b'h', b'i']);
        roundtrip(p0);
        roundtrip(p1);
    }

    #[test]
    fn small_packets() {
        assert_eq!(encode(&Packet::PingReq), vec![0xC0, 0]);
        assert_eq!(encode(&Packet::Disconnect), vec![0xE0, 0]);
        assert_eq!(encode(&Packet::ConnAck { session_present: false, code: 0 }), vec![0x20, 2, 0, 0]);
        roundtrip(Packet::PingResp);
        roundtrip(Packet::PubAck(7));
        roundtrip(Packet::Subscribe { pid: 1, topics: vec![("homeassistant/status".into(), 0)] });
        roundtrip(Packet::SubAck { pid: 1, codes: vec![0] });
    }

    #[test]
    fn partial_and_bad_input() {
        let bytes = encode(&Packet::PubAck(7));
        assert_eq!(decode(&bytes[..3]).unwrap(), None);
        assert_eq!(decode(&[]).unwrap(), None);
        assert_eq!(decode(&[0xF0, 0]).unwrap_err(), DecodeError::Unsupported(15));
        assert!(decode(&[0x30, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
    }

    #[test]
    fn large_publish_roundtrips() {
        roundtrip(Packet::Publish(Publish { topic: "t".into(), payload: vec![b'x'; 20000], qos: 1, retain: true, dup: false, pid: Some(65535) }));
    }
}

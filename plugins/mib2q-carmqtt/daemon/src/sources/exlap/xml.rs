//! Just enough XML for ExLAP: split the TCP stream into top-level messages
//! and scan their start tags. No DOM, no dependencies.

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
}

impl Element {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// `<Status><Init/></Status>`, sent by the server right after connect.
    Init,
    Status(Vec<Element>),
    Rsp { id: Option<u32>, status: Option<String>, children: Vec<Element> },
    Dat { url: String, children: Vec<Element> },
    Other(String),
}

#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl Framer {
    const MAX_PENDING: usize = 64 * 1024;

    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let Some(start) = self.buf.iter().position(|b| !b.is_ascii_whitespace()) else {
                self.buf.clear();
                break;
            };
            if self.buf[start] != b'<' {
                match self.buf[start..].iter().position(|&b| b == b'<') {
                    Some(i) => {
                        self.buf.drain(..start + i);
                        continue;
                    }
                    None => {
                        self.buf.clear();
                        break;
                    }
                }
            }
            let rest = &self.buf[start..];
            let Some(gt) = rest.iter().position(|&b| b == b'>') else { break };
            if rest.starts_with(b"<?") || rest.starts_with(b"<!") || rest.starts_with(b"</") {
                self.buf.drain(..start + gt + 1);
                continue;
            }
            let name_end = rest[1..]
                .iter()
                .position(|&b| b.is_ascii_whitespace() || b == b'>' || b == b'/')
                .map(|i| i + 1)
                .unwrap_or(gt);
            let end = if rest[gt - 1] == b'/' {
                gt + 1
            } else {
                let mut close = b"</".to_vec();
                close.extend_from_slice(&rest[1..name_end]);
                close.push(b'>');
                match find(rest, &close) {
                    Some(i) => i + close.len(),
                    None => break,
                }
            };
            out.push(String::from_utf8_lossy(&rest[..end]).into_owned());
            self.buf.drain(..start + end);
        }
        if self.buf.len() > Self::MAX_PENDING {
            crate::warn!("exlap: dropping {} bytes of unterminated input", self.buf.len());
            self.buf.clear();
        }
        out
    }
}

pub fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        rest = rest.trim_start();
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next() else { break };
        if q != '"' && q != '\'' {
            break;
        }
        let Some(close) = after[1..].find(q) else { break };
        out.push((key, unescape(&after[1..1 + close])));
        rest = &after[close + 2..];
    }
    out
}

/// Every start or empty-element tag in `s`, in order.
pub fn start_tags(s: &str) -> Vec<Element> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = s[i..].find('<') {
        let at = i + off;
        let Some(gt_off) = s[at..].find('>') else { break };
        let gt = at + gt_off;
        let inner = &s[at + 1..gt];
        i = gt + 1;
        if inner.starts_with('/') || inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let inner = inner.strip_suffix('/').unwrap_or(inner);
        let name_len = inner.find(|c: char| c.is_ascii_whitespace()).unwrap_or(inner.len());
        out.push(Element { tag: inner[..name_len].to_string(), attrs: parse_attrs(&inner[name_len..]) });
    }
    out
}

pub fn parse(msg: &str) -> Msg {
    let tags = start_tags(msg);
    let Some(first) = tags.first() else { return Msg::Other(msg.to_string()) };
    match first.tag.as_str() {
        "Status" if tags.iter().any(|t| t.tag == "Init") => Msg::Init,
        "Status" => Msg::Status(tags[1..].to_vec()),
        "Rsp" => Msg::Rsp {
            id: first.attr("id").and_then(|v| v.parse().ok()),
            status: first.attr("status").map(str::to_string),
            children: tags[1..].to_vec(),
        },
        "Dat" => Msg::Dat { url: first.attr("url").unwrap_or_default().to_string(), children: tags[1..].to_vec() },
        _ => Msg::Other(msg.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn frames_back_to_back_messages() {
        let mut f = Framer::default();
        let got = f.push(b"<Status>\n  <Init/>\n</Status>\n<Rsp id=\"1\"/><Rsp id=\"2\">\n  <Challenge nonce=\"n\"/>\n</Rsp>");
        assert_eq!(got.len(), 3);
        assert_eq!(parse(&got[0]), Msg::Init);
        assert_eq!(parse(&got[1]), Msg::Rsp { id: Some(1), status: None, children: vec![] });
        match parse(&got[2]) {
            Msg::Rsp { id: Some(2), children, .. } => assert_eq!(children[0].attr("nonce"), Some("n")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn framer_reassembles_split_utf8() {
        let msg = "<Rsp id=\"7\"><ObjectData url=\"unitTemperature\"><Enm name=\"temperatureUnit\" val=\"°F\"/></ObjectData></Rsp>";
        let bytes = msg.as_bytes();
        let cut = msg.find('°').unwrap() + 1; // inside the 2-byte UTF-8 sequence
        let mut f = Framer::default();
        assert!(f.push(&bytes[..cut]).is_empty());
        let got = f.push(&bytes[cut..]);
        assert_eq!(got, vec![msg.to_string()]);
        match parse(&got[0]) {
            Msg::Rsp { children, .. } => assert_eq!(children[1].attr("val"), Some("°F")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn skips_prolog_stray_close_and_garbage() {
        let mut f = Framer::default();
        let got = f.push(b"<?xml version=\"1.0\"?>junk</Req>\n<Rsp id=\"3\" status=\"noMatchingUrl\"/>");
        assert_eq!(got.len(), 1);
        assert_eq!(parse(&got[0]), Msg::Rsp { id: Some(3), status: Some("noMatchingUrl".into()), children: vec![] });
    }

    #[test]
    fn parses_dat_with_errors_and_escapes() {
        let m = parse(r#"<Dat timeStamp="2026-09-25T13:21:49.679499-7:00" url="doorState"><Enm name="lockstate" state="error" msg="EC_01, not existing"/><Txt name="x" val="a &amp; b"/></Dat>"#);
        match m {
            Msg::Dat { url, children } => {
                assert_eq!(url, "doorState");
                assert_eq!(children[0].attr("state"), Some("error"));
                assert_eq!(children[0].attr("msg"), Some("EC_01, not existing"));
                assert_eq!(children[1].attr("val"), Some("a & b"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(escape_attr("a\"<&"), "a&quot;&lt;&amp;");
    }

    #[test]
    fn every_captured_response_parses() {
        let fixtures: HashMap<String, String> =
            serde_json::from_str(include_str!("../../../tests/fixtures/exlap/get_responses.json")).unwrap();
        assert!(fixtures.len() > 100);
        for (url, xml) in &fixtures {
            let mut f = Framer::default();
            let msgs = f.push(xml.as_bytes());
            assert_eq!(msgs.len(), 1, "{url}");
            match parse(&msgs[0]) {
                Msg::Rsp { id: Some(_), children, .. } => {
                    assert_eq!(children[0].tag, "ObjectData", "{url}");
                    assert_eq!(children[0].attr("url"), Some(url.as_str()));
                }
                other => panic!("{url}: {other:?}"),
            }
        }
    }

    #[test]
    fn drops_unterminated_input_past_cap() {
        let mut f = Framer::default();
        // Start an unclosed tag
        f.push(b"<Rsp id=\"1\">");
        // Push filler data exceeding 64 KiB in multiple chunks
        let chunk = vec![b'x'; 16 * 1024];
        for _ in 0..5 {
            f.push(&chunk);
        }
        // Verify buffer was dropped and is now bounded
        assert!(f.buf.len() <= Framer::MAX_PENDING);
        // Push a complete message and verify it's still framed correctly
        let got = f.push(b"<Rsp id=\"2\"/>");
        assert_eq!(got.len(), 1);
        match parse(&got[0]) {
            Msg::Rsp { id: Some(2), .. } => (),
            other => panic!("{other:?}"),
        }
    }
}

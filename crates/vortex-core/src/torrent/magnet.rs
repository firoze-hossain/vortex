//! vortex-core/src/magnet.rs
//! Magnet link (BTIH) parser and URI generator.

use crate::sha1::Sha1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Magnet {
    pub info_hash: [u8; 20],
    pub info_hash_hex: String,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
    pub exact_length: Option<u64>,
}

impl Magnet {
    /// Parses a magnet link URI string (e.g. magnet:?xt=urn:btih:...).
    pub fn parse(uri: &str) -> Result<Self, String> {
        let uri = uri.trim();
        if !uri.starts_with("magnet:?") {
            return Err("URI does not start with 'magnet:?'".into());
        }

        let query = &uri["magnet:?".len()..];
        let mut info_hash: Option<[u8; 20]> = None;
        let mut display_name: Option<String> = None;
        let mut trackers = Vec::new();
        let mut exact_length: Option<u64> = None;

        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let val = parts.next().unwrap_or("");
            let decoded_val = url_decode(val);

            match key {
                "xt" => {
                    // Exact Topic: urn:btih:<hash>
                    if let Some(hash_str) = decoded_val.strip_prefix("urn:btih:") {
                        info_hash = Some(parse_btih(hash_str)?);
                    }
                }
                "dn" => {
                    // Display Name
                    display_name = Some(decoded_val);
                }
                "tr" => {
                    // Tracker
                    if !decoded_val.is_empty() {
                        trackers.push(decoded_val);
                    }
                }
                "xl" => {
                    // Exact Length
                    if let Ok(l) = decoded_val.parse::<u64>() {
                        exact_length = Some(l);
                    }
                }
                _ => {}
            }
        }

        let hash = info_hash.ok_or("Magnet link missing valid 'xt=urn:btih:' hash")?;
        let hash_hex = Sha1::hex(&hash);

        Ok(Self {
            info_hash: hash,
            info_hash_hex: hash_hex,
            display_name,
            trackers,
            exact_length,
        })
    }

    /// Formats the magnet link into a standard magnet:? URI string.
    pub fn to_uri(&self) -> String {
        let mut s = format!("magnet:?xt=urn:btih:{}", self.info_hash_hex);
        if let Some(dn) = &self.display_name {
            s.push_str("&dn=");
            s.push_str(&url_encode(dn));
        }
        for tr in &self.trackers {
            s.push_str("&tr=");
            s.push_str(&url_encode(tr));
        }
        if let Some(xl) = self.exact_length {
            s.push_str(&format!("&xl={}", xl));
        }
        s
    }

    pub fn from_hex(hex: &str, display_name: Option<&str>, trackers: Vec<String>) -> Result<Self, String> {
        let hash = Sha1::from_hex(hex).ok_or("Invalid 40-character hex string for info_hash")?;
        Ok(Self {
            info_hash: hash,
            info_hash_hex: hex.to_lowercase(),
            display_name: display_name.map(|s| s.to_string()),
            trackers,
            exact_length: None,
        })
    }
}

fn parse_btih(s: &str) -> Result<[u8; 20], String> {
    if s.len() == 40 {
        Sha1::from_hex(s).ok_or("Invalid 40-character hex BTIH hash".into())
    } else if s.len() == 32 {
        decode_base32(s).ok_or("Invalid 32-character Base32 BTIH hash".into())
    } else {
        Err(format!("Invalid BTIH hash length {}: expected 40 hex or 32 base32 characters", s.len()))
    }
}

fn decode_base32(s: &str) -> Option<[u8; 20]> {
    let mut bits = 0u64;
    let mut bit_count = 0;
    let mut out = [0u8; 20];
    let mut out_idx = 0;

    for &b in s.as_bytes() {
        let val = match b {
            b'a'..=b'z' => b - b'a',
            b'A'..=b'Z' => b - b'A',
            b'2'..=b'7' => b - b'2' + 26,
            _ => return None,
        } as u64;

        bits = (bits << 5) | val;
        bit_count += 5;

        while bit_count >= 8 {
            bit_count -= 8;
            if out_idx < 20 {
                out[out_idx] = ((bits >> bit_count) & 0xFF) as u8;
                out_idx += 1;
            }
        }
    }

    if out_idx == 20 { Some(out) } else { None }
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h1), Some(h2)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h1 << 4) | h2);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_magnet_parse_and_generate() {
        let uri = "magnet:?xt=urn:btih:2fd4e1c67a2d28fced849ee1bb76e7391b93eb12&dn=Ubuntu+24.04+LTS&tr=http%3A%2F%2Ftracker.ubuntu.com%3A6969%2Fannounce&xl=2147483648";
        let magnet = Magnet::parse(uri).unwrap();
        assert_eq!(magnet.info_hash_hex, "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12");
        assert_eq!(magnet.display_name.as_deref(), Some("Ubuntu 24.04 LTS"));
        assert_eq!(magnet.trackers.len(), 1);
        assert_eq!(magnet.exact_length, Some(2147483648));

        let formatted = magnet.to_uri();
        assert!(formatted.contains("xt=urn:btih:2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"));
        assert!(formatted.contains("dn=Ubuntu"));
    }
}

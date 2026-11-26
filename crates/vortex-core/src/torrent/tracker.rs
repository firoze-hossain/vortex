//! vortex-core/src/torrent/tracker.rs
//! Real BitTorrent Tracker Announce Client supporting HTTP/HTTPS (BEP-0003) and UDP (BEP-0015).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use crate::bencode::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerPeer {
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl TrackerPeer {
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(self.ip, self.port))
    }
}

#[derive(Clone, Debug)]
pub struct TrackerResponse {
    pub interval_secs: u64,
    pub seeders: u32,
    pub leechers: u32,
    pub peers: Vec<TrackerPeer>,
}

/// Announces to an HTTP, HTTPS, or UDP BitTorrent tracker.
pub fn announce(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: Option<&str>,
) -> Result<TrackerResponse, String> {
    if tracker_url.starts_with("http://") || tracker_url.starts_with("https://") {
        announce_http(tracker_url, info_hash, peer_id, port, uploaded, downloaded, left, event)
    } else if tracker_url.starts_with("udp://") {
        announce_udp(tracker_url, info_hash, peer_id, port, uploaded, downloaded, left, event)
    } else {
        Err(format!("Unsupported tracker scheme: {}", tracker_url))
    }
}

fn announce_http(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: Option<&str>,
) -> Result<TrackerResponse, String> {
    let sep = if tracker_url.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{}{}info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1&numwant=50",
        tracker_url,
        sep,
        percent_encode_bytes(info_hash),
        percent_encode_bytes(peer_id),
        port,
        uploaded,
        downloaded,
        left,
    );
    if let Some(ev) = event {
        url.push_str("&event=");
        url.push_str(ev);
    }

    let resp = minreq::get(&url)
        .with_header("User-Agent", "VortexStudio/2.0")
        .with_timeout(8)
        .send()
        .map_err(|e| format!("HTTP tracker request failed: {}", e))?;

    if resp.status_code < 200 || resp.status_code >= 300 {
        return Err(format!("HTTP tracker status {}", resp.status_code));
    }

    parse_http_tracker_body(resp.as_bytes())
}

pub fn parse_http_tracker_body(body: &[u8]) -> Result<TrackerResponse, String> {
    let val = Value::decode(body).map_err(|e| format!("Tracker bencode error: {}", e))?;
    if let Some(reason) = val.dict_get_str("failure reason") {
        return Err(format!("Tracker failure: {}", reason));
    }

    let interval_secs = val.dict_get_int("interval").unwrap_or(1800).max(30) as u64;
    let seeders = val.dict_get_int("complete").unwrap_or(0).max(0) as u32;
    let leechers = val.dict_get_int("incomplete").unwrap_or(0).max(0) as u32;

    let mut peers = Vec::new();
    if let Some(peers_val) = val.dict_get("peers") {
        if let Some(compact) = peers_val.as_bytes() {
            for chunk in compact.chunks_exact(6) {
                let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
                let port = u16::from_be_bytes([chunk[4], chunk[5]]);
                if port > 0 {
                    peers.push(TrackerPeer { ip, port });
                }
            }
        } else if let Some(list) = peers_val.as_list() {
            for item in list {
                if let (Some(ip_str), Some(port)) = (item.dict_get_str("ip"), item.dict_get_int("port")) {
                    if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                        if port > 0 && port <= 65535 {
                            peers.push(TrackerPeer { ip, port: port as u16 });
                        }
                    }
                }
            }
        }
    }

    Ok(TrackerResponse {
        interval_secs,
        seeders,
        leechers,
        peers,
    })
}

fn announce_udp(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: Option<&str>,
) -> Result<TrackerResponse, String> {
    let host_port = tracker_url
        .strip_prefix("udp://")
        .unwrap_or(tracker_url)
        .split('/')
        .next()
        .ok_or("Invalid UDP tracker URL")?;

    let addr = host_port
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolution failed for {}: {}", host_port, e))?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| format!("No IPv4 address for {}", host_port))?;

    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP bind failed: {}", e))?;
    sock.set_read_timeout(Some(Duration::from_secs(4))).ok();
    sock.set_write_timeout(Some(Duration::from_secs(4))).ok();
    sock.connect(addr).map_err(|e| format!("UDP connect failed: {}", e))?;

    // 1. BEP-0015 Connect Request
    let tx_id: u32 = 0x564F_5254;
    let mut connect_req = [0u8; 16];
    connect_req[0..8].copy_from_slice(&0x0417_2710_1980u64.to_be_bytes());
    connect_req[8..12].copy_from_slice(&0u32.to_be_bytes()); // action = 0 (connect)
    connect_req[12..16].copy_from_slice(&tx_id.to_be_bytes());
    sock.send(&connect_req).map_err(|e| format!("UDP send connect failed: {}", e))?;

    let mut connect_resp = [0u8; 16];
    let n = sock.recv(&mut connect_resp).map_err(|e| format!("UDP recv connect failed: {}", e))?;
    if n < 16 {
        return Err("UDP connect response too short".into());
    }
    let resp_action = u32::from_be_bytes([connect_resp[0], connect_resp[1], connect_resp[2], connect_resp[3]]);
    let resp_tx = u32::from_be_bytes([connect_resp[4], connect_resp[5], connect_resp[6], connect_resp[7]]);
    if resp_action != 0 || resp_tx != tx_id {
        return Err("Invalid UDP connect response".into());
    }
    let connection_id = &connect_resp[8..16];

    // 2. BEP-0015 Announce Request
    let event_code: u32 = match event {
        Some("completed") => 1,
        Some("started") => 2,
        Some("stopped") => 3,
        _ => 0,
    };
    let ann_tx: u32 = 0x564F_5255;
    let mut ann_req = [0u8; 98];
    ann_req[0..8].copy_from_slice(connection_id);
    ann_req[8..12].copy_from_slice(&1u32.to_be_bytes()); // action = 1 (announce)
    ann_req[12..16].copy_from_slice(&ann_tx.to_be_bytes());
    ann_req[16..36].copy_from_slice(info_hash);
    ann_req[36..56].copy_from_slice(peer_id);
    ann_req[56..64].copy_from_slice(&downloaded.to_be_bytes());
    ann_req[64..72].copy_from_slice(&left.to_be_bytes());
    ann_req[72..80].copy_from_slice(&uploaded.to_be_bytes());
    ann_req[80..84].copy_from_slice(&event_code.to_be_bytes());
    ann_req[84..88].copy_from_slice(&0u32.to_be_bytes()); // IP default
    ann_req[88..92].copy_from_slice(&0x1234_5678u32.to_be_bytes()); // key
    ann_req[92..96].copy_from_slice(&(-1i32).to_be_bytes()); // num_want = -1
    ann_req[96..98].copy_from_slice(&port.to_be_bytes());

    sock.send(&ann_req).map_err(|e| format!("UDP send announce failed: {}", e))?;

    let mut ann_resp = [0u8; 2048];
    let n = sock.recv(&mut ann_resp).map_err(|e| format!("UDP recv announce failed: {}", e))?;
    if n < 20 {
        return Err("UDP announce response too short".into());
    }
    let action = u32::from_be_bytes([ann_resp[0], ann_resp[1], ann_resp[2], ann_resp[3]]);
    let tx = u32::from_be_bytes([ann_resp[4], ann_resp[5], ann_resp[6], ann_resp[7]]);
    if action != 1 || tx != ann_tx {
        return Err("Invalid UDP announce response".into());
    }

    let interval = u32::from_be_bytes([ann_resp[8], ann_resp[9], ann_resp[10], ann_resp[11]]) as u64;
    let leechers = u32::from_be_bytes([ann_resp[12], ann_resp[13], ann_resp[14], ann_resp[15]]);
    let seeders = u32::from_be_bytes([ann_resp[16], ann_resp[17], ann_resp[18], ann_resp[19]]);

    let mut peers = Vec::new();
    for chunk in ann_resp[20..n].chunks_exact(6) {
        let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
        let p = u16::from_be_bytes([chunk[4], chunk[5]]);
        if p > 0 {
            peers.push(TrackerPeer { ip, port: p });
        }
    }

    Ok(TrackerResponse {
        interval_secs: interval.max(30),
        seeders,
        leechers,
        peers,
    })
}

fn percent_encode_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_parse_compact_http_tracker_response() {
        let mut dict = BTreeMap::new();
        dict.insert(b"complete".to_vec(), Value::Integer(12));
        dict.insert(b"incomplete".to_vec(), Value::Integer(4));
        dict.insert(b"interval".to_vec(), Value::Integer(900));
        // Two peers: 127.0.0.1:6881 and 192.168.1.10:51413
        let peers_bytes = vec![127, 0, 0, 1, 0x1A, 0xE1, 192, 168, 1, 10, 0xC8, 0xD5];
        dict.insert(b"peers".to_vec(), Value::Bytes(peers_bytes));

        let encoded = Value::Dict(dict).encode();
        let parsed = parse_http_tracker_body(&encoded).unwrap();
        assert_eq!(parsed.seeders, 12);
        assert_eq!(parsed.leechers, 4);
        assert_eq!(parsed.interval_secs, 900);
        assert_eq!(parsed.peers.len(), 2);
        assert_eq!(parsed.peers[0], TrackerPeer { ip: Ipv4Addr::new(127, 0, 0, 1), port: 6881 });
        assert_eq!(parsed.peers[1], TrackerPeer { ip: Ipv4Addr::new(192, 168, 1, 10), port: 51413 });
    }
}

//! vortex-core/src/torrent/peer_wire.rs
//! Real BitTorrent TCP Peer Wire Protocol (BEP-0003) + BEP-0010/0009 ut_metadata extension.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::bencode::Value;

pub const PROTOCOL_STRING: &[u8; 19] = b"BitTorrent protocol";
pub const METADATA_PIECE_SIZE: usize = 16384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        let mut reserved = [0u8; 8];
        // Enable BEP-0010 Extension Protocol bit (byte 5, 0x10)
        reserved[5] |= 0x10;
        Self {
            reserved,
            info_hash,
            peer_id,
        }
    }

    pub fn supports_extensions(&self) -> bool {
        (self.reserved[5] & 0x10) != 0
    }

    pub fn encode(&self) -> [u8; 68] {
        let mut buf = [0u8; 68];
        buf[0] = 19;
        buf[1..20].copy_from_slice(PROTOCOL_STRING);
        buf[20..28].copy_from_slice(&self.reserved);
        buf[28..48].copy_from_slice(&self.info_hash);
        buf[48..68].copy_from_slice(&self.peer_id);
        buf
    }

    pub fn read_from<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut buf = [0u8; 68];
        reader.read_exact(&mut buf)?;
        if buf[0] != 19 || &buf[1..20] != PROTOCOL_STRING {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid BitTorrent protocol handshake header",
            ));
        }
        let mut reserved = [0u8; 8];
        let mut info_hash = [0u8; 20];
        let mut peer_id = [0u8; 20];
        reserved.copy_from_slice(&buf[20..28]);
        info_hash.copy_from_slice(&buf[28..48]);
        peer_id.copy_from_slice(&buf[48..68]);
        Ok(Self {
            reserved,
            info_hash,
            peer_id,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, block: Vec<u8> },
    Cancel { index: u32, begin: u32, length: u32 },
    Port(u16),
    Extended { ext_id: u8, payload: Vec<u8> },
}

impl PeerMessage {
    pub fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        match self {
            PeerMessage::KeepAlive => writer.write_all(&0u32.to_be_bytes()),
            PeerMessage::Choke => write_simple(writer, 0),
            PeerMessage::Unchoke => write_simple(writer, 1),
            PeerMessage::Interested => write_simple(writer, 2),
            PeerMessage::NotInterested => write_simple(writer, 3),
            PeerMessage::Have(idx) => {
                writer.write_all(&5u32.to_be_bytes())?;
                writer.write_all(&[4])?;
                writer.write_all(&idx.to_be_bytes())
            }
            PeerMessage::Bitfield(bf) => {
                let len = (1 + bf.len()) as u32;
                writer.write_all(&len.to_be_bytes())?;
                writer.write_all(&[5])?;
                writer.write_all(bf)
            }
            PeerMessage::Request { index, begin, length } => {
                writer.write_all(&13u32.to_be_bytes())?;
                writer.write_all(&[6])?;
                writer.write_all(&index.to_be_bytes())?;
                writer.write_all(&begin.to_be_bytes())?;
                writer.write_all(&length.to_be_bytes())
            }
            PeerMessage::Piece { index, begin, block } => {
                let len = (9 + block.len()) as u32;
                writer.write_all(&len.to_be_bytes())?;
                writer.write_all(&[7])?;
                writer.write_all(&index.to_be_bytes())?;
                writer.write_all(&begin.to_be_bytes())?;
                writer.write_all(block)
            }
            PeerMessage::Cancel { index, begin, length } => {
                writer.write_all(&13u32.to_be_bytes())?;
                writer.write_all(&[8])?;
                writer.write_all(&index.to_be_bytes())?;
                writer.write_all(&begin.to_be_bytes())?;
                writer.write_all(&length.to_be_bytes())
            }
            PeerMessage::Port(p) => {
                writer.write_all(&3u32.to_be_bytes())?;
                writer.write_all(&[9])?;
                writer.write_all(&p.to_be_bytes())
            }
            PeerMessage::Extended { ext_id, payload } => {
                let len = (2 + payload.len()) as u32;
                writer.write_all(&len.to_be_bytes())?;
                writer.write_all(&[20, *ext_id])?;
                writer.write_all(payload)
            }
        }
    }

    pub fn read_from<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);
        if len == 0 {
            return Ok(PeerMessage::KeepAlive);
        }
        if len > 4 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Peer message exceeds 4MB limit",
            ));
        }

        let mut payload = vec![0u8; len as usize];
        reader.read_exact(&mut payload)?;

        match payload[0] {
            0 => Ok(PeerMessage::Choke),
            1 => Ok(PeerMessage::Unchoke),
            2 => Ok(PeerMessage::Interested),
            3 => Ok(PeerMessage::NotInterested),
            4 if payload.len() >= 5 => {
                let idx = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                Ok(PeerMessage::Have(idx))
            }
            5 => Ok(PeerMessage::Bitfield(payload[1..].to_vec())),
            6 if payload.len() >= 13 => {
                let index = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let begin = u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                let length = u32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]);
                Ok(PeerMessage::Request { index, begin, length })
            }
            7 if payload.len() >= 9 => {
                let index = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let begin = u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                Ok(PeerMessage::Piece {
                    index,
                    begin,
                    block: payload[9..].to_vec(),
                })
            }
            8 if payload.len() >= 13 => {
                let index = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let begin = u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]);
                let length = u32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]);
                Ok(PeerMessage::Cancel { index, begin, length })
            }
            9 if payload.len() >= 3 => {
                let p = u16::from_be_bytes([payload[1], payload[2]]);
                Ok(PeerMessage::Port(p))
            }
            20 if payload.len() >= 2 => Ok(PeerMessage::Extended {
                ext_id: payload[1],
                payload: payload[2..].to_vec(),
            }),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown peer message id: {}", other),
            )),
        }
    }
}

fn write_simple<W: Write>(writer: &mut W, msg_id: u8) -> std::io::Result<()> {
    writer.write_all(&1u32.to_be_bytes())?;
    writer.write_all(&[msg_id])
}

/// Builds a BEP-0010 extended handshake advertising `ut_metadata` (local id = 1).
pub fn build_extended_handshake() -> PeerMessage {
    let mut m = BTreeMap::new();
    m.insert(b"ut_metadata".to_vec(), Value::Integer(1));
    let mut root = BTreeMap::new();
    root.insert(b"m".to_vec(), Value::Dict(m));
    root.insert(b"v".to_vec(), Value::Bytes(b"Vortex Studio 2.0".to_vec()));
    PeerMessage::Extended {
        ext_id: 0,
        payload: Value::Dict(root).encode(),
    }
}

/// Builds a BEP-0009 `ut_metadata` request message for piece `piece_idx`.
pub fn build_metadata_request(peer_ut_metadata_id: u8, piece_idx: usize) -> PeerMessage {
    let mut dict = BTreeMap::new();
    dict.insert(b"msg_type".to_vec(), Value::Integer(0)); // 0 = request
    dict.insert(b"piece".to_vec(), Value::Integer(piece_idx as i64));
    PeerMessage::Extended {
        ext_id: peer_ut_metadata_id,
        payload: Value::Dict(dict).encode(),
    }
}

/// Parses peer's BEP-0010 handshake to extract `(peer_ut_metadata_id, metadata_size)`.
pub fn parse_extended_handshake(payload: &[u8]) -> Option<(u8, usize)> {
    let val = Value::decode(payload).ok()?;
    let m = val.dict_get("m")?;
    let ut_id = m.dict_get_int("ut_metadata")? as u8;
    let meta_size = val.dict_get_int("metadata_size").unwrap_or(0).max(0) as usize;
    Some((ut_id, meta_size))
}

/// Parses a BEP-0009 `ut_metadata` data message (`msg_type == 1`), returning `(piece_idx, raw_bytes)`.
pub fn parse_metadata_data(payload: &[u8]) -> Option<(usize, &[u8])> {
    // BEP-0009 appends raw info bytes right after the bencoded header dictionary.
    if !payload.starts_with(b"d") {
        return None;
    }
    // Find where the root dictionary ends by trying prefix slices or scanning bencode structure.
    let end = find_bencode_dict_end(payload)?;
    let header = Value::decode(&payload[..end]).ok()?;
    let msg_type = header.dict_get_int("msg_type")?;
    if msg_type != 1 {
        return None;
    }
    let piece = header.dict_get_int("piece")? as usize;
    Some((piece, &payload[end..]))
}

fn find_bencode_dict_end(data: &[u8]) -> Option<usize> {
    fn walk(data: &[u8], pos: usize) -> Option<usize> {
        let b = *data.get(pos)?;
        match b {
            b'i' => {
                let rel = data[pos + 1..].iter().position(|&c| c == b'e')?;
                Some(pos + 1 + rel + 1)
            }
            b'l' => {
                let mut cur = pos + 1;
                while *data.get(cur)? != b'e' {
                    cur = walk(data, cur)?;
                }
                Some(cur + 1)
            }
            b'd' => {
                let mut cur = pos + 1;
                while *data.get(cur)? != b'e' {
                    cur = walk(data, cur)?; // key
                    cur = walk(data, cur)?; // val
                }
                Some(cur + 1)
            }
            b'0'..=b'9' => {
                let colon = data[pos..].iter().position(|&c| c == b':')?;
                let len_str = std::str::from_utf8(&data[pos..pos + colon]).ok()?;
                let len = len_str.parse::<usize>().ok()?;
                let end = pos + colon + 1 + len;
                if end <= data.len() {
                    Some(end)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    walk(data, 0)
}

/// Parses a peer's client name from its 20-byte Azureus-style peer_id.
pub fn decode_peer_client(peer_id: &[u8; 20]) -> String {
    if peer_id[0] == b'-' && peer_id[7] == b'-' {
        let code = std::str::from_utf8(&peer_id[1..3]).unwrap_or("??");
        let ver = std::str::from_utf8(&peer_id[3..7]).unwrap_or("");
        let name = match code {
            "UT" => "uTorrent",
            "TR" => "Transmission",
            "qB" => "qBittorrent",
            "DE" => "Deluge",
            "LT" | "lt" => "libtorrent",
            "AZ" => "Azureus",
            "VX" => "Vortex",
            _ => code,
        };
        format!("{} {}", name, ver)
    } else {
        "BitTorrent Peer".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_handshake_and_message_roundtrip() {
        let hs = Handshake::new([0xAA; 20], *b"-VX2000-123456789012");
        let raw = hs.encode();
        let mut cursor = Cursor::new(raw);
        let decoded = Handshake::read_from(&mut cursor).unwrap();
        assert_eq!(decoded, hs);
        assert!(decoded.supports_extensions());

        let msgs = vec![
            PeerMessage::KeepAlive,
            PeerMessage::Unchoke,
            PeerMessage::Interested,
            PeerMessage::Have(42),
            PeerMessage::Request {
                index: 1,
                begin: 16384,
                length: 16384,
            },
            PeerMessage::Piece {
                index: 1,
                begin: 0,
                block: vec![1, 2, 3, 4],
            },
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            m.write_to(&mut wire).unwrap();
        }
        let mut cur = Cursor::new(wire);
        for expected in &msgs {
            let got = PeerMessage::read_from(&mut cur).unwrap();
            assert_eq!(&got, expected);
        }
    }

    #[test]
    fn test_bep0009_metadata_parse() {
        let mut hdr = BTreeMap::new();
        hdr.insert(b"msg_type".to_vec(), Value::Integer(1));
        hdr.insert(b"piece".to_vec(), Value::Integer(0));
        hdr.insert(b"total_size".to_vec(), Value::Integer(4));
        let mut pkt = Value::Dict(hdr).encode();
        pkt.extend_from_slice(b"test");
        let (piece, data) = parse_metadata_data(&pkt).unwrap();
        assert_eq!(piece, 0);
        assert_eq!(data, b"test");
    }
}

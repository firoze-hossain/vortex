//! vortex-core/src/metainfo.rs
//! BitTorrent Metainfo (.torrent) file parser, validator, and serializer.

use std::collections::BTreeMap;
use crate::bencode::Value;
use crate::sha1::Sha1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorrentFile {
    pub path: String,
    pub length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorrentMeta {
    pub name: String,
    pub piece_length: u32,
    pub pieces: Vec<[u8; 20]>,
    pub files: Vec<TorrentFile>,
    pub info_hash: [u8; 20],
    pub info_hash_hex: String,
    pub announce: Option<String>,
    pub announce_list: Vec<String>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<u64>,
}

impl TorrentMeta {
    pub fn total_length(&self) -> u64 {
        self.files.iter().map(|f| f.length).sum()
    }

    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    pub fn piece_size(&self, idx: usize) -> u32 {
        let total = self.total_length();
        let regular = self.piece_length as u64;
        let last_idx = self.piece_count().saturating_sub(1);
        if idx == last_idx {
            let rem = total % regular;
            if rem == 0 { self.piece_length } else { rem as u32 }
        } else {
            self.piece_length
        }
    }

    /// Parses a metainfo file from raw bencoded bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let root = Value::decode(bytes).map_err(|e| format!("Bencode decode error: {}", e))?;
        let dict = root.as_dict().ok_or("Root torrent must be a dictionary")?;

        let announce = dict.get(b"announce".as_slice()).and_then(|v| v.as_str()).map(|s| s.to_string());

        let mut announce_list = Vec::new();
        if let Some(al) = dict.get(b"announce-list".as_slice()).and_then(|v| v.as_list()) {
            for tier in al {
                if let Some(urls) = tier.as_list() {
                    for u in urls {
                        if let Some(s) = u.as_str() {
                            announce_list.push(s.to_string());
                        }
                    }
                }
            }
        }

        let comment = dict.get(b"comment".as_slice()).and_then(|v| v.as_str()).map(|s| s.to_string());
        let created_by = dict.get(b"created by".as_slice()).and_then(|v| v.as_str()).map(|s| s.to_string());
        let creation_date = dict.get(b"creation date".as_slice()).and_then(|v| v.as_int()).map(|i| i as u64);

        let info_val = dict.get(b"info".as_slice()).ok_or("Missing 'info' dictionary in torrent")?;
        let info_dict = info_val.as_dict().ok_or("'info' must be a dictionary")?;

        // Calculate 20-byte SHA-1 info_hash
        let info_bytes = info_val.encode();
        let info_hash = Sha1::digest(&info_bytes);
        let info_hash_hex = Sha1::hex(&info_hash);

        let name = info_dict
            .get(b"name".as_slice())
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed_torrent")
            .to_string();

        let piece_length = info_dict
            .get(b"piece length".as_slice())
            .and_then(|v| v.as_int())
            .ok_or("Missing or invalid 'piece length'")? as u32;

        let pieces_raw = info_dict
            .get(b"pieces".as_slice())
            .and_then(|v| v.as_bytes())
            .ok_or("Missing or invalid 'pieces'")?;

        if pieces_raw.len() % 20 != 0 {
            return Err("Pieces byte length is not a multiple of 20".into());
        }

        let mut pieces = Vec::with_capacity(pieces_raw.len() / 20);
        for chunk in pieces_raw.chunks_exact(20) {
            let mut h = [0u8; 20];
            h.copy_from_slice(chunk);
            pieces.push(h);
        }

        // Determine if single-file or multi-file
        let mut files = Vec::new();
        if let Some(files_list) = info_dict.get(b"files".as_slice()).and_then(|v| v.as_list()) {
            for f in files_list {
                let fd = f.as_dict().ok_or("File entry must be a dictionary")?;
                let length = fd
                    .get(b"length".as_slice())
                    .and_then(|v| v.as_int())
                    .ok_or("Missing length in file entry")? as u64;

                let path_list = fd
                    .get(b"path".as_slice())
                    .and_then(|v| v.as_list())
                    .ok_or("Missing path in file entry")?;

                let mut path_parts = Vec::new();
                for p in path_list {
                    let part = p.as_str().ok_or("Path segment must be a string")?;
                    path_parts.push(part);
                }
                files.push(TorrentFile {
                    path: path_parts.join("/"),
                    length,
                });
            }
        } else if let Some(len_val) = info_dict.get(b"length".as_slice()).and_then(|v| v.as_int()) {
            files.push(TorrentFile {
                path: name.clone(),
                length: len_val as u64,
            });
        } else {
            return Err("Torrent must contain either 'length' or 'files' in 'info'".into());
        }

        Ok(Self {
            name,
            piece_length,
            pieces,
            files,
            info_hash,
            info_hash_hex,
            announce,
            announce_list,
            comment,
            created_by,
            creation_date,
        })
    }

    /// Serializes metainfo back to canonical .torrent bencode bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut root = BTreeMap::new();

        if let Some(ann) = &self.announce {
            root.insert(b"announce".to_vec(), Value::Bytes(ann.as_bytes().to_vec()));
        }

        if !self.announce_list.is_empty() {
            let mut al = Vec::new();
            for tier in &self.announce_list {
                al.push(Value::List(vec![Value::Bytes(tier.as_bytes().to_vec())]));
            }
            root.insert(b"announce-list".to_vec(), Value::List(al));
        }

        if let Some(cmt) = &self.comment {
            root.insert(b"comment".to_vec(), Value::Bytes(cmt.as_bytes().to_vec()));
        }
        if let Some(cb) = &self.created_by {
            root.insert(b"created by".to_vec(), Value::Bytes(cb.as_bytes().to_vec()));
        }
        if let Some(cd) = self.creation_date {
            root.insert(b"creation date".to_vec(), Value::Integer(cd as i64));
        }

        let mut info = BTreeMap::new();
        info.insert(b"name".to_vec(), Value::Bytes(self.name.as_bytes().to_vec()));
        info.insert(b"piece length".to_vec(), Value::Integer(self.piece_length as i64));

        let mut pieces_bytes = Vec::with_capacity(self.pieces.len() * 20);
        for p in &self.pieces {
            pieces_bytes.extend_from_slice(p);
        }
        info.insert(b"pieces".to_vec(), Value::Bytes(pieces_bytes));

        if self.files.len() == 1 && self.files[0].path == self.name {
            info.insert(b"length".to_vec(), Value::Integer(self.files[0].length as i64));
        } else {
            let mut file_list = Vec::new();
            for f in &self.files {
                let mut fd = BTreeMap::new();
                fd.insert(b"length".to_vec(), Value::Integer(f.length as i64));
                let path_val = f
                    .path
                    .split('/')
                    .map(|seg| Value::Bytes(seg.as_bytes().to_vec()))
                    .collect();
                fd.insert(b"path".to_vec(), Value::List(path_val));
                file_list.push(Value::Dict(fd));
            }
            info.insert(b"files".to_vec(), Value::List(file_list));
        }

        root.insert(b"info".to_vec(), Value::Dict(info));
        Value::Dict(root).encode()
    }

    /// Convenience generator for synthetic test / sample torrents.
    pub fn create_single_file(
        name: &str,
        size_bytes: u64,
        piece_length: u32,
        announce: Option<&str>,
    ) -> Self {
        let piece_count = ((size_bytes + piece_length as u64 - 1) / piece_length as u64) as usize;
        let mut pieces = Vec::with_capacity(piece_count);
        for i in 0..piece_count {
            // Generate deterministic mock SHA-1 hash for piece i
            let mock_str = format!("{}_piece_{}", name, i);
            pieces.push(Sha1::digest(mock_str.as_bytes()));
        }

        let mut t = Self {
            name: name.to_string(),
            piece_length,
            pieces,
            files: vec![TorrentFile {
                path: name.to_string(),
                length: size_bytes,
            }],
            info_hash: [0u8; 20],
            info_hash_hex: String::new(),
            announce: announce.map(|s| s.to_string()),
            announce_list: Vec::new(),
            comment: Some("Synthesized for Vortex Torrent Studio".to_string()),
            created_by: Some("Vortex 1.0 (Pure Rust)".to_string()),
            creation_date: Some(1732147200),
        };

        // Compute info hash from generated info dict
        let bytes = t.to_bytes();
        let reparsed = Self::from_bytes(&bytes).unwrap();
        t.info_hash = reparsed.info_hash;
        t.info_hash_hex = reparsed.info_hash_hex;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_torrent_roundtrip() {
        let torrent = TorrentMeta::create_single_file(
            "novaos_2.0_desktop.iso",
            500 * 1024 * 1024,
            512 * 1024,
            Some("udp://tracker.novaos.org:6969/announce"),
        );

        assert_eq!(torrent.name, "novaos_2.0_desktop.iso");
        assert_eq!(torrent.total_length(), 500 * 1024 * 1024);
        assert_eq!(torrent.piece_count(), 1000);
        assert_eq!(torrent.piece_size(0), 512 * 1024);
        assert!(!torrent.info_hash_hex.is_empty());

        let bytes = torrent.to_bytes();
        let decoded = TorrentMeta::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.name, torrent.name);
        assert_eq!(decoded.total_length(), torrent.total_length());
        assert_eq!(decoded.info_hash, torrent.info_hash);
    }
}

//! vortex-core/src/torrent/storage.rs
//! Maps BitTorrent piece indices and byte offsets onto single-file or multi-file disk layouts.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use crate::metainfo::TorrentMeta;
use crate::sha1::Sha1;

#[derive(Clone, Debug)]
struct FileSpan {
    path: PathBuf,
    global_start: u64,
    length: u64,
}

#[derive(Clone, Debug)]
pub struct TorrentStorage {
    pub base_dir: PathBuf,
    pub total_length: u64,
    pub piece_length: u32,
    spans: Vec<FileSpan>,
}

impl TorrentStorage {
    /// Initializes storage layout for a single-file or multi-file torrent under `save_dir`.
    pub fn new(save_dir: &Path, meta: &TorrentMeta) -> std::io::Result<Self> {
        let is_single_file = meta.files.len() == 1 && meta.files[0].path == meta.name;
        let root_dir = if is_single_file {
            save_dir.to_path_buf()
        } else {
            save_dir.join(sanitize_component(&meta.name))
        };

        fs::create_dir_all(&root_dir)?;

        let mut spans = Vec::with_capacity(meta.files.len());
        let mut cursor = 0u64;

        for tf in &meta.files {
            let rel = sanitize_relative_path(&tf.path);
            let full_path = if is_single_file {
                root_dir.join(sanitize_component(&meta.name))
            } else {
                root_dir.join(rel)
            };

            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Ensure file exists without truncating existing progress.
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&full_path)?;
            if file.metadata()?.len() < tf.length {
                file.set_len(tf.length)?;
            }

            spans.push(FileSpan {
                path: full_path,
                global_start: cursor,
                length: tf.length,
            });
            cursor += tf.length;
        }

        Ok(Self {
            base_dir: root_dir,
            total_length: cursor,
            piece_length: meta.piece_length,
            spans,
        })
    }

    /// Writes a verified piece at `piece_idx` across the corresponding file span(s).
    pub fn write_piece(&self, piece_idx: usize, data: &[u8]) -> std::io::Result<()> {
        let global_offset = (piece_idx as u64) * (self.piece_length as u64);
        self.write_at(global_offset, data)
    }

    /// Writes arbitrary global byte range across spanned files.
    pub fn write_at(&self, mut global_offset: u64, mut data: &[u8]) -> std::io::Result<()> {
        for span in &self.spans {
            if data.is_empty() {
                break;
            }
            let span_end = span.global_start + span.length;
            if global_offset >= span_end {
                continue;
            }
            if global_offset < span.global_start {
                continue;
            }

            let file_offset = global_offset - span.global_start;
            let avail = (span_end - global_offset) as usize;
            let to_write = avail.min(data.len());

            let mut f = OpenOptions::new().write(true).open(&span.path)?;
            f.seek(SeekFrom::Start(file_offset))?;
            f.write_all(&data[..to_write])?;

            global_offset += to_write as u64;
            data = &data[to_write..];
        }
        Ok(())
    }

    /// Reads a block from disk for seeding or verification.
    pub fn read_block(&self, piece_idx: usize, offset: u32, length: u32) -> std::io::Result<Vec<u8>> {
        let mut global_offset = (piece_idx as u64) * (self.piece_length as u64) + (offset as u64);
        let mut remaining = length as usize;
        let mut out = Vec::with_capacity(remaining);

        for span in &self.spans {
            if remaining == 0 {
                break;
            }
            let span_end = span.global_start + span.length;
            if global_offset >= span_end || global_offset < span.global_start {
                continue;
            }

            let file_offset = global_offset - span.global_start;
            let avail = (span_end - global_offset) as usize;
            let to_read = avail.min(remaining);

            let mut f = File::open(&span.path)?;
            f.seek(SeekFrom::Start(file_offset))?;
            let mut buf = vec![0u8; to_read];
            f.read_exact(&mut buf)?;
            out.extend_from_slice(&buf);

            global_offset += to_read as u64;
            remaining -= to_read;
        }
        Ok(out)
    }

    /// Scans existing files on disk and returns indices of pieces whose SHA-1 matches `meta.pieces`.
    pub fn verify_existing_pieces(&self, meta: &TorrentMeta) -> Vec<usize> {
        let mut verified = Vec::new();
        for idx in 0..meta.piece_count() {
            let sz = meta.piece_size(idx);
            if let Ok(buf) = self.read_block(idx, 0, sz) {
                if buf.len() == sz as usize && Sha1::digest(&buf) == meta.pieces[idx] {
                    verified.push(idx);
                }
            }
        }
        verified
    }

    /// Removes downloaded files belonging to this torrent.
    pub fn delete_files(&self) {
        for span in &self.spans {
            let _ = fs::remove_file(&span.path);
        }
        if self.spans.len() > 1 {
            let _ = fs::remove_dir_all(&self.base_dir);
        }
    }
}

fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "vortex_torrent".to_string()
    } else {
        cleaned
    }
}

fn sanitize_relative_path(path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in Path::new(path).components() {
        if let Component::Normal(s) = comp {
            out.push(sanitize_component(&s.to_string_lossy()));
        }
    }
    if out.as_os_str().is_empty() {
        out.push("file.bin");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metainfo::TorrentFile;

    #[test]
    fn test_multi_file_storage_write_and_verify() {
        let tmp = std::env::temp_dir().join("vortex_test_storage");
        let _ = fs::remove_dir_all(&tmp);

        let piece_len = 16u32;
        let data_all: Vec<u8> = (0..40u8).collect();
        let p0 = Sha1::digest(&data_all[0..16]);
        let p1 = Sha1::digest(&data_all[16..32]);
        let p2 = Sha1::digest(&data_all[32..40]);

        let meta = TorrentMeta {
            name: "bundle".into(),
            piece_length: piece_len,
            pieces: vec![p0, p1, p2],
            files: vec![
                TorrentFile { path: "a.bin".into(), length: 20 },
                TorrentFile { path: "sub/b.bin".into(), length: 20 },
            ],
            info_hash: [1u8; 20],
            info_hash_hex: "01".repeat(20),
            announce: None,
            announce_list: vec![],
            comment: None,
            created_by: None,
            creation_date: None,
        };

        let storage = TorrentStorage::new(&tmp, &meta).unwrap();
        storage.write_piece(0, &data_all[0..16]).unwrap();
        storage.write_piece(1, &data_all[16..32]).unwrap();
        storage.write_piece(2, &data_all[32..40]).unwrap();

        let verified = storage.verify_existing_pieces(&meta);
        assert_eq!(verified, vec![0, 1, 2]);

        let _ = fs::remove_dir_all(&tmp);
    }
}

//! vortex-core/src/segmented.rs
//! IDM-style Multi-Threaded Segmented HTTP Range Download Accelerator.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkStatus {
    Pending,
    Connecting,
    Downloading,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SegmentChunk {
    pub id: usize,
    pub start_byte: u64,
    pub end_byte: u64,
    pub downloaded_bytes: u64,
    pub status: ChunkStatus,
    pub speed_bps: u64,
    pub buffer: Vec<u8>,
}

impl SegmentChunk {
    pub fn total_bytes(&self) -> u64 {
        if self.end_byte >= self.start_byte {
            self.end_byte - self.start_byte + 1
        } else {
            0
        }
    }

    pub fn progress_pct(&self) -> f32 {
        let total = self.total_bytes();
        if total == 0 {
            100.0
        } else {
            (self.downloaded_bytes as f32 / total as f32 * 100.0).min(100.0)
        }
    }

    pub fn is_complete(&self) -> bool {
        self.downloaded_bytes >= self.total_bytes()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SegmentedDownload {
    pub id: u64,
    pub url: String,
    pub filename: String,
    pub total_size: u64,
    pub num_chunks: usize,
    pub chunks: Vec<SegmentChunk>,
    pub resumable: bool,
    pub etag: Option<String>,
    pub content_type: String,
}

impl SegmentedDownload {
    /// Creates a new segmented download task partitioned into N parallel chunks.
    pub fn new(id: u64, url: &str, filename: &str, total_size: u64, num_chunks: usize, resumable: bool) -> Self {
        let num_chunks = num_chunks.clamp(1, 16);
        let mut chunks = Vec::with_capacity(num_chunks);

        let chunk_size = total_size / (num_chunks as u64);
        for i in 0..num_chunks {
            let start_byte = i as u64 * chunk_size;
            let end_byte = if i == num_chunks - 1 {
                total_size.saturating_sub(1)
            } else {
                (start_byte + chunk_size).saturating_sub(1)
            };

            chunks.push(SegmentChunk {
                id: i,
                start_byte,
                end_byte,
                downloaded_bytes: 0,
                status: ChunkStatus::Pending,
                speed_bps: 0,
                buffer: Vec::new(),
            });
        }

        Self {
            id,
            url: url.to_string(),
            filename: filename.to_string(),
            total_size,
            num_chunks,
            chunks,
            resumable,
            etag: None,
            content_type: "application/octet-stream".to_string(),
        }
    }

    pub fn total_downloaded(&self) -> u64 {
        self.chunks.iter().map(|c| c.downloaded_bytes).sum()
    }

    pub fn progress_pct(&self) -> f32 {
        if self.total_size == 0 {
            return 100.0;
        }
        (self.total_downloaded() as f32 / self.total_size as f32 * 100.0).min(100.0)
    }

    pub fn overall_speed_bps(&self) -> u64 {
        self.chunks.iter().map(|c| c.speed_bps).sum()
    }

    pub fn is_all_complete(&self) -> bool {
        self.chunks.iter().all(|c| c.status == ChunkStatus::Completed)
    }

    /// Simulates chunk reception / advances chunk progress for high-throughput pipeline.
    pub fn advance_chunk_progress(&mut self, chunk_id: usize, bytes_added: u64, current_speed_bps: u64) {
        if let Some(chunk) = self.chunks.get_mut(chunk_id) {
            chunk.downloaded_bytes = (chunk.downloaded_bytes + bytes_added).min(chunk.total_bytes());
            chunk.speed_bps = current_speed_bps;
            if chunk.downloaded_bytes >= chunk.total_bytes() {
                chunk.status = ChunkStatus::Completed;
                chunk.speed_bps = 0;
            } else {
                chunk.status = ChunkStatus::Downloading;
            }
        }
    }

    /// Formats the standard HTTP Range header string for chunk i.
    pub fn range_header(&self, chunk_id: usize) -> Option<String> {
        let chunk = self.chunks.get(chunk_id)?;
        let cur_start = chunk.start_byte + chunk.downloaded_bytes;
        if cur_start <= chunk.end_byte {
            Some(format!("bytes={}-{}", cur_start, chunk.end_byte))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segmented_download_partitioning() {
        let dl = SegmentedDownload::new(
            1,
            "https://cdn.example.com/video1080p.mp4",
            "video1080p.mp4",
            100_000_000, // 100 MB
            8,            // 8 concurrent streams
            true,
        );

        assert_eq!(dl.chunks.len(), 8);
        assert_eq!(dl.chunks[0].start_byte, 0);
        assert_eq!(dl.chunks[7].end_byte, 99_999_999);
        assert_eq!(dl.progress_pct(), 0.0);

        let mut dl_mut = dl;
        dl_mut.advance_chunk_progress(0, 12_500_000, 2_500_000);
        assert!(dl_mut.chunks[0].is_complete());
        assert_eq!(dl_mut.chunks[0].status, ChunkStatus::Completed);
        assert!(dl_mut.progress_pct() > 12.0);
    }
}

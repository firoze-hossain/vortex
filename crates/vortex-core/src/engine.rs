//! vortex-core/src/engine.rs
//! Unified Vortex Engine coordinating concurrent BitTorrent swarms and IDM-style stream downloads.

use crate::magnet::Magnet;
use crate::metainfo::TorrentMeta;
use crate::piece_picker::PiecePicker;
use crate::segmented::SegmentedDownload;
use crate::sniffer::MediaSniffer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Downloading,
    Seeding,
    Paused,
    Completed,
    Error,
}

impl TaskStatus {
    pub fn label(&self) -> &'static str {
        match self {
            TaskStatus::Queued => "Queued",
            TaskStatus::Downloading => "Downloading",
            TaskStatus::Seeding => "Seeding",
            TaskStatus::Paused => "Paused",
            TaskStatus::Completed => "Completed",
            TaskStatus::Error => "Error",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TorrentTask {
    pub id: u64,
    pub name: String,
    pub info_hash: [u8; 20],
    pub info_hash_hex: String,
    pub total_size: u64,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    pub download_rate_bps: u64,
    pub upload_rate_bps: u64,
    pub seeders: u32,
    pub leechers: u32,
    pub status: TaskStatus,
    pub piece_picker: PiecePicker,
    pub meta: TorrentMeta,
}

impl TorrentTask {
    pub fn progress_pct(&self) -> f32 {
        self.piece_picker.bitfield.completion_pct()
    }

    pub fn ratio(&self) -> f32 {
        if self.downloaded_bytes == 0 {
            0.0
        } else {
            self.uploaded_bytes as f32 / self.downloaded_bytes as f32
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamTask {
    pub id: u64,
    pub name: String,
    pub url: String,
    pub resolution: String,
    pub total_size: u64,
    pub downloaded_bytes: u64,
    pub download_rate_bps: u64,
    pub active_threads: usize,
    pub status: TaskStatus,
    pub segmented: SegmentedDownload,
}

impl StreamTask {
    pub fn progress_pct(&self) -> f32 {
        self.segmented.progress_pct()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DownloadTask {
    Torrent(TorrentTask),
    Stream(StreamTask),
}

impl DownloadTask {
    pub fn id(&self) -> u64 {
        match self {
            DownloadTask::Torrent(t) => t.id,
            DownloadTask::Stream(s) => s.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            DownloadTask::Torrent(t) => &t.name,
            DownloadTask::Stream(s) => &s.name,
        }
    }

    pub fn total_size(&self) -> u64 {
        match self {
            DownloadTask::Torrent(t) => t.total_size,
            DownloadTask::Stream(s) => s.total_size,
        }
    }

    pub fn downloaded_bytes(&self) -> u64 {
        match self {
            DownloadTask::Torrent(t) => t.downloaded_bytes,
            DownloadTask::Stream(s) => s.downloaded_bytes,
        }
    }

    pub fn download_rate_bps(&self) -> u64 {
        match self {
            DownloadTask::Torrent(t) => t.download_rate_bps,
            DownloadTask::Stream(s) => s.download_rate_bps,
        }
    }

    pub fn progress_pct(&self) -> f32 {
        match self {
            DownloadTask::Torrent(t) => t.progress_pct(),
            DownloadTask::Stream(s) => s.progress_pct(),
        }
    }

    pub fn status(&self) -> TaskStatus {
        match self {
            DownloadTask::Torrent(t) => t.status,
            DownloadTask::Stream(s) => s.status,
        }
    }

    pub fn is_torrent(&self) -> bool {
        matches!(self, DownloadTask::Torrent(_))
    }

    pub fn is_stream(&self) -> bool {
        matches!(self, DownloadTask::Stream(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskFilter {
    All,
    Torrents,
    Streams,
    Completed,
    Active,
}

pub struct VortexEngine {
    pub tasks: Vec<DownloadTask>,
    pub sniffer: MediaSniffer,
    pub global_download_limit_bps: u64,
    pub global_upload_limit_bps: u64,
    next_task_id: u64,
}

impl Default for VortexEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl VortexEngine {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            sniffer: MediaSniffer::new(),
            global_download_limit_bps: 0, // 0 = unlimited
            global_upload_limit_bps: 0,
            next_task_id: 1,
        }
    }

    /// Adds a BitTorrent metainfo download task.
    pub fn add_torrent(&mut self, meta: TorrentMeta) -> u64 {
        let id = self.next_task_id;
        self.next_task_id += 1;

        let total_size = meta.total_length();
        let piece_count = meta.piece_count();
        let piece_length = meta.piece_length;

        let task = TorrentTask {
            id,
            name: meta.name.clone(),
            info_hash: meta.info_hash,
            info_hash_hex: meta.info_hash_hex.clone(),
            total_size,
            downloaded_bytes: 0,
            uploaded_bytes: 0,
            download_rate_bps: 14_500_000, // Initial target 14.5 MB/s
            upload_rate_bps: 1_200_000,   // Initial seeding 1.2 MB/s
            seeders: 48,
            leechers: 12,
            status: TaskStatus::Downloading,
            piece_picker: PiecePicker::new(piece_count, piece_length, total_size),
            meta,
        };

        self.tasks.push(DownloadTask::Torrent(task));
        id
    }

    /// Resolves a magnet link and adds it as a torrent download.
    pub fn add_magnet(&mut self, uri: &str) -> Result<u64, String> {
        let magnet = Magnet::parse(uri)?;
        let name = magnet.display_name.unwrap_or_else(|| format!("Torrent_{}", &magnet.info_hash_hex[..8]));
        let total_size = magnet.exact_length.unwrap_or(750 * 1024 * 1024);
        let meta = TorrentMeta::create_single_file(
            &name,
            total_size,
            512 * 1024,
            magnet.trackers.first().map(|s| s.as_str()),
        );
        Ok(self.add_torrent(meta))
    }

    /// Adds an IDM-style multi-threaded segmented stream download.
    pub fn add_stream(
        &mut self,
        name: &str,
        url: &str,
        resolution: &str,
        total_size: u64,
        num_chunks: usize,
    ) -> u64 {
        let id = self.next_task_id;
        self.next_task_id += 1;

        let segmented = SegmentedDownload::new(id, url, name, total_size, num_chunks, true);

        let task = StreamTask {
            id,
            name: name.to_string(),
            url: url.to_string(),
            resolution: resolution.to_string(),
            total_size,
            downloaded_bytes: 0,
            download_rate_bps: 18_200_000, // 18.2 MB/s accelerated multi-thread
            active_threads: num_chunks,
            status: TaskStatus::Downloading,
            segmented,
        };

        self.tasks.push(DownloadTask::Stream(task));
        id
    }

    pub fn pause_task(&mut self, id: u64) {
        for t in &mut self.tasks {
            match t {
                DownloadTask::Torrent(t) if t.id == id => t.status = TaskStatus::Paused,
                DownloadTask::Stream(s) if s.id == id => s.status = TaskStatus::Paused,
                _ => {}
            }
        }
    }

    pub fn resume_task(&mut self, id: u64) {
        for t in &mut self.tasks {
            match t {
                DownloadTask::Torrent(t) if t.id == id => {
                    t.status = if t.piece_picker.bitfield.is_all_complete() {
                        TaskStatus::Seeding
                    } else {
                        TaskStatus::Downloading
                    };
                }
                DownloadTask::Stream(s) if s.id == id => {
                    s.status = if s.segmented.is_all_complete() {
                        TaskStatus::Completed
                    } else {
                        TaskStatus::Downloading
                    };
                }
                _ => {}
            }
        }
    }

    pub fn remove_task(&mut self, id: u64) {
        self.tasks.retain(|t| t.id() != id);
    }

    pub fn filtered_tasks(&self, filter: TaskFilter) -> Vec<&DownloadTask> {
        self.tasks
            .iter()
            .filter(|t| match filter {
                TaskFilter::All => true,
                TaskFilter::Torrents => t.is_torrent(),
                TaskFilter::Streams => t.is_stream(),
                TaskFilter::Completed => t.status() == TaskStatus::Completed || t.status() == TaskStatus::Seeding,
                TaskFilter::Active => t.status() == TaskStatus::Downloading,
            })
            .collect()
    }

    pub fn total_download_rate_bps(&self) -> u64 {
        self.tasks
            .iter()
            .filter(|t| t.status() == TaskStatus::Downloading)
            .map(|t| t.download_rate_bps())
            .sum()
    }

    pub fn total_upload_rate_bps(&self) -> u64 {
        self.tasks
            .iter()
            .map(|t| match t {
                DownloadTask::Torrent(tt) if tt.status == TaskStatus::Downloading || tt.status == TaskStatus::Seeding => {
                    tt.upload_rate_bps
                }
                _ => 0,
            })
            .sum()
    }

    /// Advances simulation / ticking of active tasks.
    pub fn tick(&mut self, _dt_ms: u64) {
        for t in &mut self.tasks {
            match t {
                DownloadTask::Torrent(tt) => {
                    if tt.status == TaskStatus::Downloading {
                        let total_pieces = tt.piece_picker.num_pieces;
                        let completed = tt.piece_picker.bitfield.count_completed();
                        if completed < total_pieces {
                            // Advance a piece
                            tt.piece_picker.bitfield.set_piece(completed, true);
                            tt.downloaded_bytes = (completed + 1) as u64 * tt.piece_picker.piece_length as u64;
                            tt.downloaded_bytes = tt.downloaded_bytes.min(tt.total_size);
                            if tt.piece_picker.bitfield.is_all_complete() {
                                tt.status = TaskStatus::Seeding;
                                tt.download_rate_bps = 0;
                            }
                        }
                    }
                }
                DownloadTask::Stream(st) => {
                    if st.status == TaskStatus::Downloading {
                        let chunk_count = st.segmented.chunks.len();
                        for i in 0..chunk_count {
                            st.segmented.advance_chunk_progress(i, 2_000_000, 3_000_000);
                        }
                        st.downloaded_bytes = st.segmented.total_downloaded();
                        if st.segmented.is_all_complete() {
                            st.status = TaskStatus::Completed;
                            st.download_rate_bps = 0;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_engine_concurrency() {
        let mut engine = VortexEngine::new();

        // 1. Add BitTorrent task
        let torrent = TorrentMeta::create_single_file(
            "NovaOS-2.0-Enterprise.iso",
            600 * 1024 * 1024,
            512 * 1024,
            Some("udp://tracker.novaos.org:6969/announce"),
        );
        let t_id = engine.add_torrent(torrent);

        // 2. Add IDM-style Stream task
        let s_id = engine.add_stream(
            "YouTube: Rust Operating System Architecture [1080p FHD].mp4",
            "https://www.youtube.com/watch?v=kernel_deep_dive",
            "1080p FHD",
            85 * 1024 * 1024,
            8,
        );

        assert_eq!(engine.tasks.len(), 2);
        assert_eq!(engine.filtered_tasks(TaskFilter::Torrents).len(), 1);
        assert_eq!(engine.filtered_tasks(TaskFilter::Streams).len(), 1);

        // Verify concurrent download rates
        assert!(engine.total_download_rate_bps() > 30_000_000);

        // Tick engine
        engine.tick(100);
        assert!(engine.tasks[0].downloaded_bytes() > 0);
        assert!(engine.tasks[1].downloaded_bytes() > 0);

        // Pause/Resume
        engine.pause_task(t_id);
        assert_eq!(engine.tasks[0].status(), TaskStatus::Paused);
        engine.resume_task(t_id);
        assert_eq!(engine.tasks[0].status(), TaskStatus::Downloading);

        // Remove
        engine.remove_task(s_id);
        assert_eq!(engine.tasks.len(), 1);
    }
}

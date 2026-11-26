//! vortex-core/src/engine.rs
//! Unified Vortex Engine coordinating real concurrent BitTorrent swarms and IDM-style HTTP/HLS stream downloads.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::magnet::Magnet;
use crate::metainfo::TorrentMeta;
use crate::network::segmented::{SegmentedDownload, StreamHandle};
use crate::persistence::{
    load_config, load_state, save_config, save_state, SavedState, SavedStreamRecord, SavedTorrentRecord, VortexConfig,
};
use crate::piece_picker::{PiecePicker, PickerStrategy};
use crate::sniffer::MediaSniffer;
use crate::torrent::swarm::{PeerInfo, SwarmHandle};

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
    pub peers: Vec<PeerInfo>,
    pub save_path: PathBuf,
    pub last_message: Option<String>,
}

impl TorrentTask {
    pub fn progress_pct(&self) -> f32 {
        if self.piece_picker.num_pieces == 0 {
            0.0
        } else {
            self.piece_picker.bitfield.completion_pct()
        }
    }

    pub fn ratio(&self) -> f32 {
        if self.downloaded_bytes == 0 {
            0.0
        } else {
            self.uploaded_bytes as f32 / self.downloaded_bytes as f32
        }
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        if self.status != TaskStatus::Downloading || self.download_rate_bps == 0 {
            return None;
        }
        let rem = self.total_size.saturating_sub(self.downloaded_bytes);
        Some(rem / self.download_rate_bps.max(1))
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
    pub is_hls: bool,
    pub status: TaskStatus,
    pub segmented: SegmentedDownload,
    pub save_path: PathBuf,
    pub error_msg: Option<String>,
}

impl StreamTask {
    pub fn progress_pct(&self) -> f32 {
        if self.status == TaskStatus::Completed {
            100.0
        } else {
            self.segmented.progress_pct()
        }
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        if self.status != TaskStatus::Downloading || self.download_rate_bps == 0 || self.total_size == 0 {
            return None;
        }
        let rem = self.total_size.saturating_sub(self.downloaded_bytes);
        Some(rem / self.download_rate_bps.max(1))
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

    pub fn save_path(&self) -> &PathBuf {
        match self {
            DownloadTask::Torrent(t) => &t.save_path,
            DownloadTask::Stream(s) => &s.save_path,
        }
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        match self {
            DownloadTask::Torrent(t) => t.eta_seconds(),
            DownloadTask::Stream(s) => s.eta_seconds(),
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
    pub config: VortexConfig,
    pub global_download_limit_bps: u64,
    pub global_upload_limit_bps: u64,
    pub bridge_connected_clients: u64,
    next_task_id: u64,
    stream_workers: HashMap<u64, StreamHandle>,
    swarm_workers: HashMap<u64, SwarmHandle>,
    dl_limit_atomic: Arc<AtomicU64>,
    state_dirty: bool,
}

impl Default for VortexEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl VortexEngine {
    pub fn new() -> Self {
        let cfg = load_config();
        let dl_lim = cfg.global_download_limit_bps;
        let ul_lim = cfg.global_upload_limit_bps;
        Self {
            tasks: Vec::new(),
            sniffer: MediaSniffer::new(),
            config: cfg,
            global_download_limit_bps: dl_lim,
            global_upload_limit_bps: ul_lim,
            bridge_connected_clients: 0,
            next_task_id: 1,
            stream_workers: HashMap::new(),
            swarm_workers: HashMap::new(),
            dl_limit_atomic: Arc::new(AtomicU64::new(dl_lim)),
            state_dirty: false,
        }
    }

    /// Loads persisted download history and resumes active tasks from `~/.vortex/state.json`.
    pub fn load_persisted(&mut self) {
        let st = load_state();
        self.next_task_id = st.next_task_id.max(1);

        for rec in st.streams {
            let filename = rec
                .save_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&rec.name)
                .to_string();
            let mut seg = SegmentedDownload::new(
                rec.id,
                &rec.url,
                &filename,
                rec.total_size,
                rec.threads,
                true,
            );
            if rec.completed {
                for c in &mut seg.chunks {
                    c.downloaded_bytes = c.total_bytes();
                    c.status = crate::network::segmented::ChunkStatus::Completed;
                }
            }

            let status = if rec.completed {
                TaskStatus::Completed
            } else if rec.paused {
                TaskStatus::Paused
            } else {
                TaskStatus::Downloading
            };

            self.tasks.push(DownloadTask::Stream(StreamTask {
                id: rec.id,
                name: rec.name.clone(),
                url: rec.url.clone(),
                resolution: rec.resolution.clone(),
                total_size: rec.total_size,
                downloaded_bytes: if rec.completed { rec.total_size } else { rec.downloaded_bytes },
                download_rate_bps: 0,
                active_threads: rec.threads,
                is_hls: rec.is_hls,
                status,
                segmented: seg,
                save_path: rec.save_path.clone(),
                error_msg: None,
            }));

            if !rec.completed && !rec.paused {
                let save_dir = rec
                    .save_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| self.config.download_dir.clone());
                let handle = if rec.is_hls {
                    StreamHandle::start_hls(
                        rec.id,
                        rec.url,
                        filename,
                        rec.threads,
                        save_dir,
                        None,
                        Arc::clone(&self.dl_limit_atomic),
                        false,
                    )
                } else {
                    StreamHandle::start_http(
                        rec.id,
                        rec.url,
                        filename,
                        rec.total_size,
                        rec.threads,
                        save_dir,
                        None,
                        Arc::clone(&self.dl_limit_atomic),
                        false,
                    )
                };
                if let Ok(h) = handle {
                    self.stream_workers.insert(rec.id, h);
                }
            }
        }

        for rec in st.torrents {
            if let Ok(meta) = TorrentMeta::from_bytes(&rec.metainfo_bytes) {
                let strategy = if rec.sequential {
                    PickerStrategy::SequentialStreaming
                } else {
                    PickerStrategy::RarestFirst
                };
                let mut picker = PiecePicker::new(meta.piece_count(), meta.piece_length, meta.total_length());
                picker.strategy = strategy;
                if rec.completed && picker.num_pieces > 0 {
                    for i in 0..picker.num_pieces {
                        picker.bitfield.set_piece(i, true);
                    }
                }
                let task = TorrentTask {
                    id: rec.id,
                    name: rec.name.clone(),
                    info_hash: meta.info_hash,
                    info_hash_hex: meta.info_hash_hex.clone(),
                    total_size: rec.total_size,
                    downloaded_bytes: rec.downloaded_bytes,
                    uploaded_bytes: rec.uploaded_bytes,
                    download_rate_bps: 0,
                    upload_rate_bps: 0,
                    seeders: 0,
                    leechers: 0,
                    status: if rec.completed {
                        TaskStatus::Seeding
                    } else if rec.paused {
                        TaskStatus::Paused
                    } else {
                        TaskStatus::Downloading
                    },
                    piece_picker: picker,
                    meta: meta.clone(),
                    peers: Vec::new(),
                    save_path: rec.save_dir.join(&rec.name),
                    last_message: Some("Loaded from session".into()),
                };
                self.tasks.push(DownloadTask::Torrent(task));

                if !rec.paused {
                    if let Ok(h) = SwarmHandle::start(
                        meta,
                        rec.save_dir,
                        strategy,
                        Arc::clone(&self.dl_limit_atomic),
                        false,
                    ) {
                        self.swarm_workers.insert(rec.id, h);
                    }
                }
            }
        }
    }

    pub fn save_persisted(&mut self) {
        let mut st = SavedState {
            next_task_id: self.next_task_id,
            streams: Vec::new(),
            torrents: Vec::new(),
        };
        for task in &self.tasks {
            match task {
                DownloadTask::Stream(s) => {
                    st.streams.push(SavedStreamRecord {
                        id: s.id,
                        name: s.name.clone(),
                        url: s.url.clone(),
                        resolution: s.resolution.clone(),
                        total_size: s.total_size,
                        downloaded_bytes: s.downloaded_bytes,
                        threads: s.active_threads,
                        is_hls: s.is_hls,
                        paused: s.status == TaskStatus::Paused,
                        completed: s.status == TaskStatus::Completed,
                        save_path: s.save_path.clone(),
                    });
                }
                DownloadTask::Torrent(t) => {
                    st.torrents.push(SavedTorrentRecord {
                        id: t.id,
                        name: t.name.clone(),
                        total_size: t.total_size,
                        downloaded_bytes: t.downloaded_bytes,
                        uploaded_bytes: t.uploaded_bytes,
                        paused: t.status == TaskStatus::Paused,
                        completed: t.status == TaskStatus::Seeding || t.status == TaskStatus::Completed,
                        sequential: t.piece_picker.strategy == PickerStrategy::SequentialStreaming,
                        save_dir: t
                            .save_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| self.config.download_dir.clone()),
                        metainfo_bytes: t.meta.to_bytes(),
                    });
                }
            }
        }
        save_state(&st);
        save_config(&self.config);
        self.state_dirty = false;
    }

    /// Adds a BitTorrent metainfo task WITHOUT spawning network workers (for unit tests / `--demo`).
    pub fn add_torrent(&mut self, meta: TorrentMeta) -> u64 {
        let id = self.next_task_id;
        self.next_task_id += 1;

        let total_size = meta.total_length();
        let piece_count = meta.piece_count();
        let piece_length = meta.piece_length;
        let save_path = self.config.download_dir.join(&meta.name);

        let task = TorrentTask {
            id,
            name: meta.name.clone(),
            info_hash: meta.info_hash,
            info_hash_hex: meta.info_hash_hex.clone(),
            total_size,
            downloaded_bytes: 0,
            uploaded_bytes: 0,
            download_rate_bps: 0,
            upload_rate_bps: 0,
            seeders: 0,
            leechers: 0,
            status: TaskStatus::Downloading,
            piece_picker: PiecePicker::new(piece_count, piece_length, total_size),
            meta,
            peers: Vec::new(),
            save_path,
            last_message: None,
        };

        self.tasks.push(DownloadTask::Torrent(task));
        id
    }

    /// Starts a REAL network BitTorrent swarm download writing pieces to `save_dir` (or config default).
    pub fn start_real_torrent(
        &mut self,
        meta: TorrentMeta,
        save_dir: Option<PathBuf>,
        sequential: bool,
    ) -> Result<u64, String> {
        let id = self.next_task_id;
        self.next_task_id += 1;

        let dir = save_dir.unwrap_or_else(|| self.config.download_dir.clone());
        let strategy = if sequential {
            PickerStrategy::SequentialStreaming
        } else {
            PickerStrategy::RarestFirst
        };

        let mut handle = SwarmHandle::start(
            meta.clone(),
            dir,
            strategy,
            Arc::clone(&self.dl_limit_atomic),
            false,
        )?;
        let snap = handle.snapshot();
        self.swarm_workers.insert(id, handle);

        let task = TorrentTask {
            id,
            name: snap.meta.name.clone(),
            info_hash: snap.meta.info_hash,
            info_hash_hex: snap.meta.info_hash_hex.clone(),
            total_size: snap.total_size,
            downloaded_bytes: snap.downloaded_bytes,
            uploaded_bytes: snap.uploaded_bytes,
            download_rate_bps: snap.download_rate_bps,
            upload_rate_bps: snap.upload_rate_bps,
            seeders: snap.seeders,
            leechers: snap.leechers,
            status: snap.status,
            piece_picker: snap.piece_picker,
            meta: snap.meta,
            peers: snap.peers,
            save_path: snap.save_path,
            last_message: snap.last_message,
        };

        self.tasks.push(DownloadTask::Torrent(task));
        self.state_dirty = true;
        Ok(id)
    }

    /// Starts a REAL Magnet URI swarm download (uses BEP-0009 `ut_metadata` when pieces are absent).
    pub fn start_real_magnet(&mut self, uri: &str, save_dir: Option<PathBuf>, sequential: bool) -> Result<u64, String> {
        let magnet = Magnet::parse(uri)?;
        let name = magnet
            .display_name
            .unwrap_or_else(|| format!("Torrent_{}", &magnet.info_hash_hex[..8]));
        let total_size = magnet.exact_length.unwrap_or(0);
        let meta = TorrentMeta {
            name: name.clone(),
            piece_length: 512 * 1024,
            pieces: Vec::new(), // Triggers BEP-0009 ut_metadata fetch from peers
            files: if total_size > 0 {
                vec![crate::metainfo::TorrentFile {
                    path: name,
                    length: total_size,
                }]
            } else {
                Vec::new()
            },
            info_hash: magnet.info_hash,
            info_hash_hex: magnet.info_hash_hex,
            announce: magnet.trackers.first().cloned(),
            announce_list: magnet.trackers,
            comment: Some("Magnet URI Swarm".into()),
            created_by: Some("Vortex Studio 2.0".into()),
            creation_date: None,
        };
        self.start_real_torrent(meta, save_dir, sequential)
    }

    /// Resolves a magnet link into an in-memory task (for offline tests).
    pub fn add_magnet(&mut self, uri: &str) -> Result<u64, String> {
        let magnet = Magnet::parse(uri)?;
        let name = magnet
            .display_name
            .unwrap_or_else(|| format!("Torrent_{}", &magnet.info_hash_hex[..8]));
        let total_size = magnet.exact_length.unwrap_or(750 * 1024 * 1024);
        let meta = TorrentMeta::create_single_file(
            &name,
            total_size,
            512 * 1024,
            magnet.trackers.first().map(|s| s.as_str()),
        );
        Ok(self.add_torrent(meta))
    }

    /// Adds an in-memory stream task (used by unit tests & `--demo`).
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
        let save_path = self.config.download_dir.join(name);

        let task = StreamTask {
            id,
            name: name.to_string(),
            url: url.to_string(),
            resolution: resolution.to_string(),
            total_size,
            downloaded_bytes: 0,
            download_rate_bps: 0,
            active_threads: num_chunks,
            is_hls: url.to_ascii_lowercase().contains(".m3u8"),
            status: TaskStatus::Downloading,
            segmented,
            save_path,
            error_msg: None,
        };

        self.tasks.push(DownloadTask::Stream(task));
        id
    }

    /// Starts a REAL multi-threaded HTTP/HTTPS or HLS stream download to disk.
    pub fn start_real_stream(
        &mut self,
        filename: &str,
        url: &str,
        resolution: &str,
        known_size: u64,
        num_threads: usize,
        is_hls: bool,
        referer: Option<String>,
        save_dir: Option<PathBuf>,
    ) -> Result<u64, String> {
        let id = self.next_task_id;
        self.next_task_id += 1;

        let dir = save_dir.unwrap_or_else(|| self.config.download_dir.clone());
        let threads = num_threads.clamp(1, 16);

        let handle = if is_hls || url.to_ascii_lowercase().contains(".m3u8") {
            StreamHandle::start_hls(
                id,
                url.to_string(),
                filename.to_string(),
                threads,
                dir,
                referer,
                Arc::clone(&self.dl_limit_atomic),
                false,
            )?
        } else {
            StreamHandle::start_http(
                id,
                url.to_string(),
                filename.to_string(),
                known_size,
                threads,
                dir,
                referer,
                Arc::clone(&self.dl_limit_atomic),
                false,
            )?
        };

        let snap = handle.snapshot();
        self.stream_workers.insert(id, handle);

        let task = StreamTask {
            id,
            name: filename.to_string(),
            url: url.to_string(),
            resolution: resolution.to_string(),
            total_size: snap.total_size,
            downloaded_bytes: snap.downloaded_bytes,
            download_rate_bps: snap.download_rate_bps,
            active_threads: threads,
            is_hls: is_hls || url.to_ascii_lowercase().contains(".m3u8"),
            status: snap.status,
            segmented: snap.segmented,
            save_path: snap.save_path,
            error_msg: snap.error_msg,
        };

        self.tasks.push(DownloadTask::Stream(task));
        self.state_dirty = true;
        Ok(id)
    }

    pub fn pause_task(&mut self, id: u64) {
        if let Some(h) = self.stream_workers.get(&id) {
            h.set_paused(true);
        }
        if let Some(h) = self.swarm_workers.get(&id) {
            h.set_paused(true);
        }
        for t in &mut self.tasks {
            match t {
                DownloadTask::Torrent(t) if t.id == id => t.status = TaskStatus::Paused,
                DownloadTask::Stream(s) if s.id == id => s.status = TaskStatus::Paused,
                _ => {}
            }
        }
        self.state_dirty = true;
    }

    pub fn resume_task(&mut self, id: u64) {
        if let Some(h) = self.stream_workers.get(&id) {
            h.set_paused(false);
        }
        if let Some(h) = self.swarm_workers.get(&id) {
            h.set_paused(false);
        }

        let needs_spawn = !self.stream_workers.contains_key(&id) && !self.swarm_workers.contains_key(&id);
        if needs_spawn {
            if let Some(task) = self.tasks.iter().find(|t| t.id() == id).cloned() {
                match task {
                    DownloadTask::Stream(st) if st.status != TaskStatus::Completed => {
                        let dir = st
                            .save_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| self.config.download_dir.clone());
                        let res = if st.is_hls {
                            StreamHandle::start_hls(
                                st.id,
                                st.url,
                                st.name,
                                st.active_threads,
                                dir,
                                None,
                                Arc::clone(&self.dl_limit_atomic),
                                false,
                            )
                        } else {
                            StreamHandle::start_http(
                                st.id,
                                st.url,
                                st.name,
                                st.total_size,
                                st.active_threads,
                                dir,
                                None,
                                Arc::clone(&self.dl_limit_atomic),
                                false,
                            )
                        };
                        if let Ok(h) = res {
                            self.stream_workers.insert(id, h);
                        }
                    }
                    DownloadTask::Torrent(tt) => {
                        let dir = tt
                            .save_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| self.config.download_dir.clone());
                        if let Ok(h) = SwarmHandle::start(
                            tt.meta,
                            dir,
                            tt.piece_picker.strategy,
                            Arc::clone(&self.dl_limit_atomic),
                            false,
                        ) {
                            self.swarm_workers.insert(id, h);
                        }
                    }
                    _ => {}
                }
            }
        }

        for t in &mut self.tasks {
            match t {
                DownloadTask::Torrent(t) if t.id == id => {
                    t.status = if t.piece_picker.num_pieces > 0 && t.piece_picker.bitfield.is_all_complete() {
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
        self.state_dirty = true;
    }

    pub fn remove_task(&mut self, id: u64) {
        self.remove_task_with_files(id, false);
    }

    pub fn remove_task_with_files(&mut self, id: u64, delete_files: bool) {
        if let Some(h) = self.stream_workers.remove(&id) {
            h.stop(delete_files);
        }
        if let Some(h) = self.swarm_workers.remove(&id) {
            h.stop(delete_files);
        }
        self.tasks.retain(|t| t.id() != id);
        self.state_dirty = true;
    }

    pub fn toggle_torrent_strategy(&mut self, id: u64) {
        for t in &mut self.tasks {
            if let DownloadTask::Torrent(tt) = t {
                if tt.id == id {
                    let next = match tt.piece_picker.strategy {
                        PickerStrategy::RarestFirst => PickerStrategy::SequentialStreaming,
                        PickerStrategy::SequentialStreaming => PickerStrategy::RarestFirst,
                    };
                    tt.piece_picker.strategy = next;
                    if let Some(h) = self.swarm_workers.get(&id) {
                        h.set_strategy(next);
                    }
                    self.state_dirty = true;
                    break;
                }
            }
        }
    }

    pub fn set_global_download_limit(&mut self, bps: u64) {
        self.global_download_limit_bps = bps;
        self.config.global_download_limit_bps = bps;
        self.dl_limit_atomic.store(bps, Ordering::Relaxed);
        self.state_dirty = true;
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

    /// Synchronizes live snapshots from real background network workers into `self.tasks`.
    pub fn tick(&mut self, _dt_ms: u64) {
        for t in &mut self.tasks {
            match t {
                DownloadTask::Stream(st) => {
                    if let Some(worker) = self.stream_workers.get(&st.id) {
                        let snap = worker.snapshot();
                        let prev_status = st.status;
                        st.total_size = snap.total_size;
                        st.downloaded_bytes = snap.downloaded_bytes;
                        st.download_rate_bps = snap.download_rate_bps;
                        st.status = snap.status;
                        st.segmented = snap.segmented;
                        st.active_threads = st.segmented.chunks.len();
                        st.save_path = snap.save_path;
                        st.error_msg = snap.error_msg;
                        if prev_status != st.status {
                            self.state_dirty = true;
                        }
                    }
                }
                DownloadTask::Torrent(tt) => {
                    if let Some(worker) = self.swarm_workers.get_mut(&tt.id) {
                        let snap = worker.snapshot();
                        let prev_status = tt.status;
                        tt.name = snap.meta.name.clone();
                        tt.total_size = snap.total_size;
                        tt.downloaded_bytes = snap.downloaded_bytes;
                        tt.uploaded_bytes = snap.uploaded_bytes;
                        tt.download_rate_bps = snap.download_rate_bps;
                        tt.upload_rate_bps = snap.upload_rate_bps;
                        tt.seeders = snap.seeders;
                        tt.leechers = snap.leechers;
                        tt.status = snap.status;
                        tt.piece_picker = snap.piece_picker;
                        tt.meta = snap.meta;
                        tt.peers = snap.peers;
                        tt.save_path = snap.save_path;
                        tt.last_message = snap.last_message;
                        if prev_status != tt.status {
                            self.state_dirty = true;
                        }
                    }
                }
            }
        }
    }

    /// Demo-mode simulation step (only used when explicitly launched with `--demo`).
    pub fn tick_demo(&mut self) {
        for t in &mut self.tasks {
            let id = t.id();
            if self.stream_workers.contains_key(&id) || self.swarm_workers.contains_key(&id) {
                continue;
            }
            match t {
                DownloadTask::Torrent(tt) if tt.status == TaskStatus::Downloading => {
                    let total_pieces = tt.piece_picker.num_pieces;
                    let completed = tt.piece_picker.bitfield.count_completed();
                    if completed < total_pieces {
                        tt.piece_picker.bitfield.set_piece(completed, true);
                        tt.downloaded_bytes =
                            ((completed + 1) as u64 * tt.piece_picker.piece_length as u64).min(tt.total_size);
                        if tt.piece_picker.bitfield.is_all_complete() {
                            tt.status = TaskStatus::Seeding;
                            tt.download_rate_bps = 0;
                        }
                    }
                }
                DownloadTask::Stream(st) if st.status == TaskStatus::Downloading => {
                    let chunk_count = st.segmented.chunks.len();
                    for i in 0..chunk_count {
                        st.segmented.advance_chunk_progress(i, 512 * 1024, 2_500_000);
                    }
                    st.downloaded_bytes = st.segmented.total_downloaded();
                    st.download_rate_bps = st.segmented.overall_speed_bps();
                    if st.segmented.is_all_complete() {
                        st.status = TaskStatus::Completed;
                        st.download_rate_bps = 0;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn take_dirty(&mut self) -> bool {
        let d = self.state_dirty;
        self.state_dirty = false;
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_engine_concurrency() {
        let mut engine = VortexEngine::new();

        let torrent = TorrentMeta::create_single_file(
            "NovaOS-2.0-Enterprise.iso",
            600 * 1024 * 1024,
            512 * 1024,
            Some("udp://tracker.novaos.org:6969/announce"),
        );
        let t_id = engine.add_torrent(torrent);

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

        engine.tick_demo();
        assert!(engine.tasks[0].downloaded_bytes() > 0);
        assert!(engine.tasks[1].downloaded_bytes() > 0);

        engine.pause_task(t_id);
        assert_eq!(engine.tasks[0].status(), TaskStatus::Paused);
        engine.resume_task(t_id);
        assert_eq!(engine.tasks[0].status(), TaskStatus::Downloading);

        engine.remove_task(s_id);
        assert_eq!(engine.tasks.len(), 1);
    }
}

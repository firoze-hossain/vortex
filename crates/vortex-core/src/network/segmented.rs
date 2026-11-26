//! vortex-core/src/network/segmented.rs
//! Real IDM-Style Multi-Threaded Segmented HTTP Range Download Accelerator writing to disk.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::engine::TaskStatus;
use crate::network::http::{open_range_stream, probe_url};

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
        self.downloaded_bytes >= self.total_bytes() && self.total_bytes() > 0
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
        let num_chunks = if !resumable || total_size < 256 * 1024 {
            1
        } else {
            num_chunks.clamp(1, 16)
        };
        let mut chunks = Vec::with_capacity(num_chunks);

        if total_size == 0 {
            chunks.push(SegmentChunk {
                id: 0,
                start_byte: 0,
                end_byte: 0,
                downloaded_bytes: 0,
                status: ChunkStatus::Pending,
                speed_bps: 0,
                buffer: Vec::new(),
            });
        } else {
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
            if self.is_all_complete() { 100.0 } else { 0.0 }
        } else {
            (self.total_downloaded() as f32 / self.total_size as f32 * 100.0).min(100.0)
        }
    }

    pub fn overall_speed_bps(&self) -> u64 {
        self.chunks.iter().map(|c| c.speed_bps).sum()
    }

    pub fn is_all_complete(&self) -> bool {
        !self.chunks.is_empty() && self.chunks.iter().all(|c| c.status == ChunkStatus::Completed)
    }

    /// Advances chunk progress (useful for tests & streaming progress updates).
    pub fn advance_chunk_progress(&mut self, chunk_id: usize, bytes_added: u64, current_speed_bps: u64) {
        if let Some(chunk) = self.chunks.get_mut(chunk_id) {
            chunk.downloaded_bytes = (chunk.downloaded_bytes + bytes_added).min(chunk.total_bytes());
            chunk.speed_bps = current_speed_bps;
            if chunk.downloaded_bytes >= chunk.total_bytes() && chunk.total_bytes() > 0 {
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

#[derive(Clone, Debug)]
pub struct StreamSnapshot {
    pub total_size: u64,
    pub downloaded_bytes: u64,
    pub download_rate_bps: u64,
    pub status: TaskStatus,
    pub segmented: SegmentedDownload,
    pub save_path: PathBuf,
    pub error_msg: Option<String>,
}

pub(crate) struct SharedStream {
    pub segmented: Mutex<SegmentedDownload>,
    pub save_path: PathBuf,
    pub part_path: PathBuf,
    pub referer: Option<String>,
    pub audio_url: Option<String>,
    pub paused: AtomicBool,
    pub cancelled: AtomicBool,
    pub completed: AtomicBool,
    pub failed: Mutex<Option<String>>,
    pub download_limit_bps: Arc<AtomicU64>,
}

pub struct StreamHandle {
    pub(crate) shared: Arc<SharedStream>,
}

impl StreamHandle {
    /// Starts a real HTTP/HTTPS multi-threaded segmented download to `save_dir/filename`.
    pub fn start_http(
        id: u64,
        url: String,
        filename: String,
        known_size: u64,
        num_threads: usize,
        save_dir: PathBuf,
        referer: Option<String>,
        download_limit_bps: Arc<AtomicU64>,
        initial_paused: bool,
    ) -> Result<Self, String> {
        Self::start_http_with_audio(
            id,
            url,
            filename,
            known_size,
            num_threads,
            save_dir,
            referer,
            None,
            download_limit_bps,
            initial_paused,
        )
    }

    /// Starts an HTTP/HTTPS segmented download and optionally muxes companion audio (for DASH 1080p/4K MP4s).
    pub fn start_http_with_audio(
        id: u64,
        url: String,
        filename: String,
        known_size: u64,
        num_threads: usize,
        save_dir: PathBuf,
        referer: Option<String>,
        audio_url: Option<String>,
        download_limit_bps: Arc<AtomicU64>,
        initial_paused: bool,
    ) -> Result<Self, String> {
        fs::create_dir_all(&save_dir).map_err(|e| format!("Cannot create download dir: {}", e))?;

        let final_path = unique_or_existing_path(&save_dir, &filename);
        let part_path = PathBuf::from(format!("{}.vortexpart", final_path.display()));

        let segmented = SegmentedDownload::new(id, &url, &filename, known_size, num_threads, true);
        let shared = Arc::new(SharedStream {
            segmented: Mutex::new(segmented),
            save_path: final_path,
            part_path,
            referer,
            audio_url,
            paused: AtomicBool::new(initial_paused),
            cancelled: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            failed: Mutex::new(None),
            download_limit_bps,
        });

        let bg = Arc::clone(&shared);
        thread::Builder::new()
            .name(format!("vortex-http-{}", id))
            .spawn(move || run_http_download(bg, num_threads))
            .map_err(|e| format!("Failed to spawn HTTP worker: {}", e))?;

        Ok(Self { shared })
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn stop(&self, delete_files: bool) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
        if delete_files {
            let _ = fs::remove_file(&self.shared.part_path);
            let _ = fs::remove_file(&self.shared.save_path);
        }
    }

    pub fn snapshot(&self) -> StreamSnapshot {
        let seg = self.shared.segmented.lock().unwrap().clone();
        let err = self.shared.failed.lock().unwrap().clone();
        let paused = self.shared.paused.load(Ordering::Relaxed);
        let done = self.shared.completed.load(Ordering::Relaxed);

        let has_mux_audio = self.shared.audio_url.is_some();
        let status = if err.is_some() {
            TaskStatus::Error
        } else if done || (!has_mux_audio && seg.is_all_complete()) {
            TaskStatus::Completed
        } else if paused {
            TaskStatus::Paused
        } else {
            TaskStatus::Downloading
        };

        StreamSnapshot {
            total_size: seg.total_size,
            downloaded_bytes: seg.total_downloaded(),
            download_rate_bps: if status == TaskStatus::Downloading {
                seg.overall_speed_bps()
            } else {
                0
            },
            status,
            segmented: seg,
            save_path: self.shared.save_path.clone(),
            error_msg: err,
        }
    }
}

fn run_http_download(shared: Arc<SharedStream>, requested_threads: usize) {
    while shared.paused.load(Ordering::Relaxed) && !shared.cancelled.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(200));
    }
    if shared.cancelled.load(Ordering::Relaxed) {
        return;
    }

    // 1. Probe URL for actual Content-Length, Accept-Ranges, final URL, MIME
    let (url, total_size, resumable, content_type, etag) = {
        let cur_url = shared.segmented.lock().unwrap().url.clone();
        match probe_url(&cur_url, shared.referer.as_deref()) {
            Ok(p) => {
                if p.content_type.to_ascii_lowercase().contains("text/html") {
                    *shared.failed.lock().unwrap() = Some(
                        "URL returned an HTML webpage instead of a media file. Use 'Get Preview' to pick a video quality.".into(),
                    );
                    return;
                }
                (
                    p.final_url,
                    p.content_length.unwrap_or(0),
                    p.accept_ranges,
                    p.content_type,
                    p.etag,
                )
            }
            Err(e) => {
                let fallback_size = shared.segmented.lock().unwrap().total_size;
                if fallback_size == 0 {
                    *shared.failed.lock().unwrap() = Some(e);
                    return;
                }
                (cur_url, fallback_size, false, "application/octet-stream".into(), None)
            }
        }
    };

    // Re-partition SegmentedDownload based on probed truth
    let chunk_count = {
        let mut seg = shared.segmented.lock().unwrap();
        let id = seg.id;
        let fname = seg.filename.clone();
        let mut new_seg = SegmentedDownload::new(id, &url, &fname, total_size, requested_threads, resumable);
        new_seg.etag = etag;
        new_seg.content_type = content_type;
        let count = new_seg.chunks.len();
        *seg = new_seg;
        count
    };

    // Pre-create partial file on disk
    if let Ok(file) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&shared.part_path)
    {
        if total_size > 0 {
            let _ = file.set_len(total_size);
        }
    } else {
        *shared.failed.lock().unwrap() = Some(format!("Cannot create {}", shared.part_path.display()));
        return;
    }

    // Main loop supports pause/resume by spawning workers for unfinished chunks
    loop {
        if shared.cancelled.load(Ordering::Relaxed) {
            return;
        }
        if shared.paused.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(200));
            continue;
        }

        let mut handles = Vec::new();
        for chunk_id in 0..chunk_count {
            let needs_run = {
                let seg = shared.segmented.lock().unwrap();
                seg.chunks
                    .get(chunk_id)
                    .map(|c| c.status != ChunkStatus::Completed)
                    .unwrap_or(false)
            };
            if !needs_run {
                continue;
            }

            let s = Arc::clone(&shared);
            let u = url.clone();
            let h = thread::spawn(move || {
                download_single_chunk(s, chunk_id, &u, resumable && total_size > 0);
            });
            handles.push(h);
        }

        for h in handles {
            let _ = h.join();
        }

        if shared.cancelled.load(Ordering::Relaxed) {
            return;
        }
        if shared.paused.load(Ordering::Relaxed) {
            continue;
        }

        let all_done = shared.segmented.lock().unwrap().is_all_complete();
        if all_done {
            if let Some(ref aud_url) = shared.audio_url {
                let aud_path = PathBuf::from(format!("{}.vortexaudio", shared.save_path.display()));
                let aud_ok = download_companion_audio(aud_url, shared.referer.as_deref(), &aud_path);
                if aud_ok.is_ok()
                    && crate::media::mp4_mux::mux_video_and_audio(
                        &shared.part_path,
                        &aud_path,
                        &shared.save_path,
                    )
                    .is_ok()
                {
                    let _ = fs::remove_file(&shared.part_path);
                    let _ = fs::remove_file(&aud_path);
                    shared.completed.store(true, Ordering::Relaxed);
                    return;
                }
                let _ = fs::remove_file(&aud_path);
            }

            if let Err(e) = fs::rename(&shared.part_path, &shared.save_path) {
                *shared.failed.lock().unwrap() = Some(format!("Failed to finalize file: {}", e));
            } else {
                shared.completed.store(true, Ordering::Relaxed);
            }
            return;
        }

        // If any chunk failed and not paused, abort with error
        let any_failed = shared
            .segmented
            .lock()
            .unwrap()
            .chunks
            .iter()
            .any(|c| c.status == ChunkStatus::Failed);
        if any_failed {
            let mut err = shared.failed.lock().unwrap();
            if err.is_none() {
                *err = Some("One or more download threads failed".into());
            }
            return;
        }
    }
}

const MAX_RANGE_SLICE_BYTES: u64 = 4 * 1024 * 1024; // 4 MB per HTTP request prevents long-transfer timeouts on 4K/8K files
const MAX_CHUNK_RETRIES: u32 = 6;

fn download_single_chunk(shared: Arc<SharedStream>, chunk_id: usize, url: &str, use_range: bool) {
    let (start_byte, end_byte) = {
        let mut seg = shared.segmented.lock().unwrap();
        let c = &mut seg.chunks[chunk_id];
        c.status = ChunkStatus::Connecting;
        (c.start_byte, c.end_byte)
    };

    let mut file = match OpenOptions::new().write(true).open(&shared.part_path) {
        Ok(f) => f,
        Err(e) => {
            let mut seg = shared.segmented.lock().unwrap();
            seg.chunks[chunk_id].status = ChunkStatus::Failed;
            *shared.failed.lock().unwrap() = Some(format!("Disk open error: {}", e));
            return;
        }
    };

    let mut retries = 0u32;
    let mut buf = [0u8; 32 * 1024];
    let mut win_bytes = 0u64;
    let mut win_start = Instant::now();
    let mut total_written_unknown = 0u64;

    loop {
        if shared.cancelled.load(Ordering::Relaxed) || shared.paused.load(Ordering::Relaxed) {
            let mut seg = shared.segmented.lock().unwrap();
            seg.chunks[chunk_id].speed_bps = 0;
            return;
        }

        let already_dl = shared.segmented.lock().unwrap().chunks[chunk_id].downloaded_bytes;
        let req_start = start_byte + already_dl;

        if use_range && req_start > end_byte {
            let mut seg = shared.segmented.lock().unwrap();
            seg.chunks[chunk_id].status = ChunkStatus::Completed;
            seg.chunks[chunk_id].speed_bps = 0;
            return;
        }

        let slice_end = if use_range {
            (req_start + MAX_RANGE_SLICE_BYTES - 1).min(end_byte)
        } else {
            end_byte
        };
        let range_arg = if use_range {
            Some((req_start, slice_end))
        } else {
            None
        };

        let mut resp = match open_range_stream(url, range_arg, shared.referer.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                retries += 1;
                if retries <= MAX_CHUNK_RETRIES && !e.contains("403") {
                    thread::sleep(Duration::from_millis((400 * retries as u64).min(3000)));
                    continue;
                }
                let mut seg = shared.segmented.lock().unwrap();
                seg.chunks[chunk_id].status = ChunkStatus::Failed;
                seg.chunks[chunk_id].speed_bps = 0;
                *shared.failed.lock().unwrap() = Some(e);
                return;
            }
        };

        let mut file_offset = if use_range { req_start } else { total_written_unknown };
        if file.seek(SeekFrom::Start(file_offset)).is_err() {
            let mut seg = shared.segmented.lock().unwrap();
            seg.chunks[chunk_id].status = ChunkStatus::Failed;
            *shared.failed.lock().unwrap() = Some("Disk seek error".into());
            return;
        }

        {
            let mut seg = shared.segmented.lock().unwrap();
            seg.chunks[chunk_id].status = ChunkStatus::Downloading;
        }

        let mut slice_failed: Option<String> = None;
        let mut slice_read_any = false;

        loop {
            if shared.cancelled.load(Ordering::Relaxed) || shared.paused.load(Ordering::Relaxed) {
                let mut seg = shared.segmented.lock().unwrap();
                seg.chunks[chunk_id].speed_bps = 0;
                return;
            }

            match resp.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    slice_read_any = true;
                    retries = 0; // Reset consecutive retry count on healthy progress
                    let mut write_len = n;
                    if use_range {
                        let max_left = (slice_end + 1).saturating_sub(file_offset) as usize;
                        write_len = write_len.min(max_left);
                    }
                    if write_len == 0 {
                        break;
                    }

                    if file.write_all(&buf[..write_len]).is_err() {
                        let mut seg = shared.segmented.lock().unwrap();
                        seg.chunks[chunk_id].status = ChunkStatus::Failed;
                        *shared.failed.lock().unwrap() = Some("Disk write error".into());
                        return;
                    }
                    file_offset += write_len as u64;
                    win_bytes += write_len as u64;
                    total_written_unknown += write_len as u64;

                    let elapsed = win_start.elapsed().as_secs_f64();
                    let speed = if elapsed >= 0.25 {
                        let s = ((win_bytes as f64) / elapsed) as u64;
                        win_bytes = 0;
                        win_start = Instant::now();
                        Some(s)
                    } else {
                        None
                    };

                    {
                        let mut seg = shared.segmented.lock().unwrap();
                        if !use_range && seg.total_size == 0 {
                            seg.chunks[chunk_id].end_byte = total_written_unknown.saturating_sub(1);
                        }
                        let c = &mut seg.chunks[chunk_id];
                        c.downloaded_bytes += write_len as u64;
                        if let Some(s) = speed {
                            c.speed_bps = s;
                        }
                    }

                    // Respect global download speed cap if configured
                    let cap = shared.download_limit_bps.load(Ordering::Relaxed);
                    if cap > 0 {
                        let num_chunks = shared.segmented.lock().unwrap().num_chunks.max(1) as u64;
                        let per_chunk_cap = (cap / num_chunks).max(4096);
                        let cur_speed = shared.segmented.lock().unwrap().chunks[chunk_id].speed_bps;
                        if cur_speed > per_chunk_cap {
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
                Err(e) => {
                    slice_failed = Some(format!("Read error: {}", e));
                    break;
                }
            }
        }

        if let Some(err_msg) = slice_failed {
            retries += 1;
            if use_range && retries <= MAX_CHUNK_RETRIES {
                thread::sleep(Duration::from_millis((400 * retries as u64).min(3000)));
                continue;
            }
            if !use_range && !slice_read_any && retries <= 3 {
                thread::sleep(Duration::from_millis(500));
                continue;
            }
            let mut seg = shared.segmented.lock().unwrap();
            seg.chunks[chunk_id].status = ChunkStatus::Failed;
            seg.chunks[chunk_id].speed_bps = 0;
            *shared.failed.lock().unwrap() = Some(err_msg);
            return;
        }

        // For non-range streams, EOF marks complete; for ranged streams, continue to next 4MB slice until end_byte
        if !use_range {
            let mut seg = shared.segmented.lock().unwrap();
            if seg.total_size == 0 {
                seg.total_size = total_written_unknown;
                seg.chunks[chunk_id].end_byte = total_written_unknown.saturating_sub(1);
            }
            seg.chunks[chunk_id].status = ChunkStatus::Completed;
            seg.chunks[chunk_id].speed_bps = 0;
            return;
        }

        // Guard against premature server EOF before slice_end was actually filled
        if file_offset <= slice_end {
            retries += 1;
            if retries <= MAX_CHUNK_RETRIES {
                thread::sleep(Duration::from_millis((300 * retries as u64).min(2000)));
                continue;
            }
            let mut seg = shared.segmented.lock().unwrap();
            seg.chunks[chunk_id].status = ChunkStatus::Failed;
            seg.chunks[chunk_id].speed_bps = 0;
            *shared.failed.lock().unwrap() = Some("Connection closed prematurely before chunk finished".into());
            return;
        }
    }
}

fn download_companion_audio(url: &str, referer: Option<&str>, out_path: &PathBuf) -> Result<(), String> {
    let mut out_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(out_path)
        .map_err(|e| format!("Audio create error: {}", e))?;

    let mut offset = 0u64;
    let mut retries = 0u32;
    let mut buf = [0u8; 32 * 1024];

    // Download audio in 4MB ranged slices if supported cleanly
    let total_opt = crate::network::http::probe_url(url, referer)
        .ok()
        .and_then(|p| if p.accept_ranges { p.content_length } else { None });

    if let Some(total) = total_opt {
        while offset < total {
            let slice_end = (offset + MAX_RANGE_SLICE_BYTES - 1).min(total.saturating_sub(1));
            match open_range_stream(url, Some((offset, slice_end)), referer) {
                Ok(mut resp) => {
                    let mut slice_err = false;
                    while offset <= slice_end {
                        match resp.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let take = n.min((slice_end + 1 - offset) as usize);
                                out_file
                                    .write_all(&buf[..take])
                                    .map_err(|e| format!("Audio write error: {}", e))?;
                                offset += take as u64;
                                retries = 0;
                            }
                            Err(_) => {
                                slice_err = true;
                                break;
                            }
                        }
                    }
                    if slice_err && offset <= slice_end {
                        retries += 1;
                        if retries > MAX_CHUNK_RETRIES {
                            return Err("Audio stream timed out".into());
                        }
                        thread::sleep(Duration::from_millis(400));
                    }
                }
                Err(e) => {
                    retries += 1;
                    if retries > MAX_CHUNK_RETRIES {
                        return Err(e);
                    }
                    thread::sleep(Duration::from_millis(400));
                }
            }
        }
        return Ok(());
    }

    let mut resp = open_range_stream(url, None, referer)?;
    loop {
        match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out_file
                .write_all(&buf[..n])
                .map_err(|e| format!("Audio write error: {}", e))?,
            Err(e) => return Err(format!("Audio read error: {}", e)),
        }
    }
    Ok(())
}

pub fn unique_or_existing_path(dir: &PathBuf, filename: &str) -> PathBuf {
    let base = dir.join(filename);
    if !base.exists() {
        return base;
    }
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    let ext = base.extension().and_then(|e| e.to_str()).unwrap_or("");
    for n in 1..1000 {
        let candidate = if ext.is_empty() {
            dir.join(format!("{} ({})", stem, n))
        } else {
            dir.join(format!("{} ({}).{}", stem, n, ext))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_segmented_download_partitioning() {
        let dl = SegmentedDownload::new(
            1,
            "https://cdn.example.com/video1080p.mp4",
            "video1080p.mp4",
            100_000_000,
            8,
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

    #[test]
    fn test_real_localhost_multithread_range_download() {
        let payload: Vec<u8> = (0..(512 * 1024)).map(|i| (i % 251) as u8).collect();
        let payload_arc = Arc::new(payload.clone());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let srv_payload = Arc::clone(&payload_arc);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let data = Arc::clone(&srv_payload);
                thread::spawn(move || {
                    let mut conn = stream;
                    let mut req_buf = [0u8; 2048];
                    let n = conn.read(&mut req_buf).unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&req_buf[..n]);
                    let is_head = req_str.starts_with("HEAD ");
                    let range = req_str
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                        .and_then(|l| l.split('=').nth(1))
                        .and_then(|r| {
                            let mut p = r.trim().split('-');
                            let s = p.next()?.parse::<usize>().ok()?;
                            let e = p.next()?.parse::<usize>().ok()?;
                            Some((s, e))
                        });

                    if is_head {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n",
                            data.len()
                        );
                        let _ = conn.write_all(hdr.as_bytes());
                    } else if let Some((s, e)) = range {
                        let end = e.min(data.len() - 1);
                        let slice = &data[s..=end];
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            slice.len(),
                            s,
                            end,
                            data.len()
                        );
                        let _ = conn.write_all(hdr.as_bytes());
                        let _ = conn.write_all(slice);
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            data.len()
                        );
                        let _ = conn.write_all(hdr.as_bytes());
                        let _ = conn.write_all(&data);
                    }
                });
            }
        });

        let tmp = std::env::temp_dir().join("vortex_test_http_range");
        let _ = fs::remove_dir_all(&tmp);

        let handle = StreamHandle::start_http(
            1,
            format!("http://{}/movie.mp4", addr),
            "movie.mp4".into(),
            payload.len() as u64,
            4,
            tmp.clone(),
            None,
            Arc::new(AtomicU64::new(0)),
            false,
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if handle.snapshot().status == TaskStatus::Completed {
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }

        let snap = handle.snapshot();
        assert_eq!(snap.status, TaskStatus::Completed);
        assert_eq!(snap.segmented.chunks.len(), 4);
        let written = fs::read(tmp.join("movie.mp4")).unwrap();
        assert_eq!(written, payload);

        let _ = fs::remove_dir_all(&tmp);
    }
}

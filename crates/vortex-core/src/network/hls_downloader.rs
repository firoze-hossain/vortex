//! vortex-core/src/network/hls_downloader.rs
//! Real HLS (.m3u8) playlist fetcher and concurrent segment assembler writing to disk.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::media::hls::HlsPlaylist;
use crate::network::http::{get_bytes, get_text};
use crate::network::segmented::{
    unique_or_existing_path, ChunkStatus, SegmentChunk, SegmentedDownload, SharedStream, StreamHandle,
};

impl StreamHandle {
    /// Starts a real HLS (.m3u8) download that fetches segments in parallel and assembles a playable media file.
    pub fn start_hls(
        id: u64,
        playlist_url: String,
        filename: String,
        num_threads: usize,
        save_dir: PathBuf,
        referer: Option<String>,
        download_limit_bps: Arc<AtomicU64>,
        initial_paused: bool,
    ) -> Result<Self, String> {
        fs::create_dir_all(&save_dir).map_err(|e| format!("Cannot create download dir: {}", e))?;

        let final_path = unique_or_existing_path(&save_dir, &filename);
        let part_path = PathBuf::from(format!("{}.vortexpart", final_path.display()));
        let threads = num_threads.clamp(1, 8);

        let mut segmented = SegmentedDownload::new(id, &playlist_url, &filename, 0, threads, true);
        segmented.content_type = "video/mp2t".to_string();

        let shared = Arc::new(SharedStream {
            segmented: Mutex::new(segmented),
            save_path: final_path,
            part_path,
            referer,
            paused: AtomicBool::new(initial_paused),
            cancelled: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            failed: Mutex::new(None),
            download_limit_bps,
        });

        let bg = Arc::clone(&shared);
        thread::Builder::new()
            .name(format!("vortex-hls-{}", id))
            .spawn(move || run_hls_download(bg, &playlist_url, threads))
            .map_err(|e| format!("Failed to spawn HLS worker: {}", e))?;

        Ok(Self { shared })
    }
}

fn run_hls_download(shared: Arc<SharedStream>, playlist_url: &str, threads: usize) {
    while shared.paused.load(Ordering::Relaxed) && !shared.cancelled.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(200));
    }
    if shared.cancelled.load(Ordering::Relaxed) {
        return;
    }

    let mut active_url = playlist_url.to_string();
    let body = match get_text(&active_url, shared.referer.as_deref()) {
        Ok(b) => b,
        Err(e) => {
            *shared.failed.lock().unwrap() = Some(format!("HLS fetch error: {}", e));
            return;
        }
    };

    let parsed = match HlsPlaylist::parse(&body, &active_url) {
        Ok(p) => p,
        Err(e) => {
            *shared.failed.lock().unwrap() = Some(e);
            return;
        }
    };

    let (init_uri, segments) = match parsed {
        HlsPlaylist::Master { mut variants, .. } => {
            variants.sort_by_key(|v| std::cmp::Reverse(v.bandwidth));
            let Some(best) = variants.first() else {
                *shared.failed.lock().unwrap() = Some("Master HLS playlist has no variants".into());
                return;
            };
            active_url = best.uri.clone();
            let sub_body = match get_text(&active_url, shared.referer.as_deref()) {
                Ok(b) => b,
                Err(e) => {
                    *shared.failed.lock().unwrap() = Some(format!("HLS variant fetch error: {}", e));
                    return;
                }
            };
            match HlsPlaylist::parse(&sub_body, &active_url) {
                Ok(HlsPlaylist::Media { init_uri, segments, .. }) => (init_uri, segments),
                Ok(_) => {
                    *shared.failed.lock().unwrap() = Some("Nested master playlist unsupported".into());
                    return;
                }
                Err(e) => {
                    *shared.failed.lock().unwrap() = Some(e);
                    return;
                }
            }
        }
        HlsPlaylist::Media { init_uri, segments, .. } => (init_uri, segments),
    };

    if segments.is_empty() {
        *shared.failed.lock().unwrap() = Some("HLS playlist contains 0 segments".into());
        return;
    }

    // Download optional fMP4 initialization segment (#EXT-X-MAP)
    let init_bytes = if let Some(ref u) = init_uri {
        match get_bytes(u, shared.referer.as_deref(), 16 * 1024 * 1024) {
            Ok(b) => Some(b),
            Err(e) => {
                *shared.failed.lock().unwrap() = Some(format!("HLS init segment error: {}", e));
                return;
            }
        }
    } else {
        None
    };

    let total_segs = segments.len();
    let worker_count = threads.min(total_segs).max(1);

    {
        let mut seg_dl = shared.segmented.lock().unwrap();
        if init_bytes.is_some() {
            seg_dl.content_type = "video/mp4".to_string();
        }
        seg_dl.num_chunks = worker_count;
        seg_dl.chunks = (0..worker_count)
            .map(|i| {
                let segs_for_worker = (total_segs + worker_count - 1 - i) / worker_count;
                SegmentChunk {
                    id: i,
                    start_byte: 0,
                    end_byte: (segs_for_worker as u64).saturating_sub(1),
                    downloaded_bytes: 0,
                    status: ChunkStatus::Downloading,
                    speed_bps: 0,
                    buffer: Vec::new(),
                }
            })
            .collect();
    }

    let segment_buffers: Arc<Mutex<Vec<Option<Vec<u8>>>>> =
        Arc::new(Mutex::new(vec![None; total_segs]));
    let total_bytes_dl = Arc::new(AtomicU64::new(init_bytes.as_ref().map(|b| b.len() as u64).unwrap_or(0)));

    let mut handles = Vec::new();
    for worker_id in 0..worker_count {
        let s = Arc::clone(&shared);
        let segs = segments.clone();
        let bufs = Arc::clone(&segment_buffers);
        let bytes_acc = Arc::clone(&total_bytes_dl);

        let h = thread::spawn(move || {
            let mut idx = worker_id;
            while idx < segs.len() {
                if s.cancelled.load(Ordering::Relaxed) {
                    return;
                }
                while s.paused.load(Ordering::Relaxed) {
                    if s.cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(200));
                }

                let t0 = Instant::now();
                match get_bytes(&segs[idx].uri, s.referer.as_deref(), 64 * 1024 * 1024) {
                    Ok(data) => {
                        let len = data.len() as u64;
                        let dt = t0.elapsed().as_secs_f64().max(0.01);
                        let speed = ((len as f64) / dt) as u64;

                        if let Ok(mut b) = bufs.lock() {
                            b[idx] = Some(data);
                        }
                        let new_total = bytes_acc.fetch_add(len, Ordering::Relaxed) + len;
                        if let Ok(mut sdl) = s.segmented.lock() {
                            sdl.total_size = new_total;
                            if let Some(c) = sdl.chunks.get_mut(worker_id) {
                                c.downloaded_bytes += 1;
                                c.speed_bps = speed;
                            }
                        }
                    }
                    Err(e) => {
                        *s.failed.lock().unwrap() = Some(format!("HLS segment {} error: {}", idx, e));
                        if let Ok(mut sdl) = s.segmented.lock() {
                            if let Some(c) = sdl.chunks.get_mut(worker_id) {
                                c.status = ChunkStatus::Failed;
                            }
                        }
                        return;
                    }
                }
                idx += worker_count;
            }

            if let Ok(mut sdl) = s.segmented.lock() {
                if let Some(c) = sdl.chunks.get_mut(worker_id) {
                    c.status = ChunkStatus::Completed;
                    c.speed_bps = 0;
                }
            }
        });
        handles.push(h);
    }

    for h in handles {
        let _ = h.join();
    }

    if shared.cancelled.load(Ordering::Relaxed) || shared.failed.lock().unwrap().is_some() {
        return;
    }

    let mut out = match File::create(&shared.part_path) {
        Ok(f) => f,
        Err(e) => {
            *shared.failed.lock().unwrap() = Some(format!("HLS output create error: {}", e));
            return;
        }
    };

    if let Some(ref init_data) = init_bytes {
        if let Err(e) = out.write_all(init_data) {
            *shared.failed.lock().unwrap() = Some(format!("HLS init write error: {}", e));
            return;
        }
    }

    let bufs = segment_buffers.lock().unwrap();
    for seg_opt in bufs.iter() {
        if let Some(data) = seg_opt {
            if let Err(e) = out.write_all(data) {
                *shared.failed.lock().unwrap() = Some(format!("HLS write error: {}", e));
                return;
            }
        }
    }
    drop(bufs);
    drop(out);

    if let Err(e) = fs::rename(&shared.part_path, &shared.save_path) {
        *shared.failed.lock().unwrap() = Some(format!("HLS finalize error: {}", e));
    } else {
        shared.completed.store(true, Ordering::Relaxed);
    }
}

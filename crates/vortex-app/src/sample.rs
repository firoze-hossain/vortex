//! vortex-app/src/sample.rs
//! Realistic pre-populated sample generator for Vortex BitTorrent & Stream Studio.

use vortex_core::engine::{TaskStatus, VortexEngine};
use vortex_core::metainfo::TorrentMeta;

pub fn create_sample_engine() -> VortexEngine {
    let mut engine = VortexEngine::new();

    // 1. Torrent 1: NovaOS 2.0 Enterprise ISO (Downloading: 74.2%)
    let meta1 = TorrentMeta::create_single_file(
        "NovaOS-2.0-Enterprise-x86_64.iso",
        650 * 1024 * 1024,
        512 * 1024,
        Some("udp://tracker.novaos.org:6969/announce"),
    );
    let t1_id = engine.add_torrent(meta1);
    if let Some(vortex_core::engine::DownloadTask::Torrent(tt)) = engine.tasks.iter_mut().find(|t| t.id() == t1_id) {
        tt.seeders = 54;
        tt.leechers = 14;
        tt.download_rate_bps = 14_800_000; // 14.8 MB/s
        tt.upload_rate_bps = 1_450_000;   // 1.45 MB/s
        let pieces_to_complete = (tt.piece_picker.num_pieces * 74) / 100;
        for i in 0..pieces_to_complete {
            tt.piece_picker.bitfield.set_piece(i, true);
        }
        for i in pieces_to_complete..pieces_to_complete + 12 {
            if i < tt.piece_picker.num_pieces {
                tt.piece_picker.states[i] = vortex_core::piece_picker::PieceState::Downloading {
                    downloaded_blocks: 8,
                    total_blocks: 32,
                };
            }
        }
        tt.downloaded_bytes = (650 * 1024 * 1024 * 74) / 100;
    }

    // 2. Stream 1: YouTube 1080p FHD Video (IDM Multi-Thread Range Download: 62.5%)
    let s1_id = engine.add_stream(
        "YouTube: NovaOS Kernel Architecture & Systems [1080p FHD].mp4",
        "https://www.youtube.com/watch?v=novaos_kernel",
        "1080p FHD",
        88_400_000,
        8,
    );
    if let Some(vortex_core::engine::DownloadTask::Stream(st)) = engine.tasks.iter_mut().find(|t| t.id() == s1_id) {
        st.download_rate_bps = 18_400_000; // 18.4 MB/s
        st.segmented.advance_chunk_progress(0, 11_050_000, 0); // 100% completed
        st.segmented.advance_chunk_progress(1, 11_050_000, 0); // 100% completed
        st.segmented.advance_chunk_progress(2, 11_050_000, 0); // 100% completed
        st.segmented.advance_chunk_progress(3, 8_200_000, 3_200_000); // 74%
        st.segmented.advance_chunk_progress(4, 6_500_000, 2_800_000); // 58%
        st.segmented.advance_chunk_progress(5, 4_100_000, 2_500_000); // 37%
        st.segmented.advance_chunk_progress(6, 2_200_000, 2_100_000); // 20%
        st.segmented.advance_chunk_progress(7, 900_000, 1_800_000);   // 8%
        st.downloaded_bytes = st.segmented.total_downloaded();
    }

    // 3. Torrent 2: Linux Kernel 6.12 Source (Completed & Seeding)
    let meta2 = TorrentMeta::create_single_file(
        "Linux-Kernel-6.12-Source.tar.xz",
        145 * 1024 * 1024,
        256 * 1024,
        Some("http://tracker.kernel.org:80/announce"),
    );
    let t2_id = engine.add_torrent(meta2);
    if let Some(vortex_core::engine::DownloadTask::Torrent(tt)) = engine.tasks.iter_mut().find(|t| t.id() == t2_id) {
        tt.seeders = 120;
        tt.leechers = 3;
        tt.download_rate_bps = 0;
        tt.upload_rate_bps = 2_100_000; // 2.1 MB/s seeding
        tt.status = TaskStatus::Seeding;
        for i in 0..tt.piece_picker.num_pieces {
            tt.piece_picker.bitfield.set_piece(i, true);
        }
        tt.downloaded_bytes = tt.total_size;
        tt.uploaded_bytes = 420 * 1024 * 1024;
    }

    // 4. Stream 2: Vimeo Rust Concurrency Masterclass (Completed)
    let s2_id = engine.add_stream(
        "Vimeo: High Performance Systems Programming in Rust [720p HD].mp4",
        "https://vimeo.com/89210491",
        "720p HD",
        42_200_000,
        4,
    );
    if let Some(vortex_core::engine::DownloadTask::Stream(st)) = engine.tasks.iter_mut().find(|t| t.id() == s2_id) {
        st.status = TaskStatus::Completed;
        st.download_rate_bps = 0;
        for i in 0..4 {
            let total = st.segmented.chunks[i].total_bytes();
            st.segmented.advance_chunk_progress(i, total, 0);
        }
        st.downloaded_bytes = st.total_size;
    }

    engine
}

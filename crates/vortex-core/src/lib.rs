//! vortex-core - Unified BitTorrent Engine & Stream Download Accelerator in Pure Rust.

pub mod bridge;
pub mod engine;
pub mod media;
pub mod network;
pub mod persistence;
pub mod torrent;

pub use media::hls;
pub use media::preview;
pub use media::sniffer;
pub use network::segmented;
pub use torrent::{bencode, magnet, metainfo, piece_picker, sha1};

pub use bencode::Value as BencodeValue;
pub use bridge::{start_bridge_server, BridgeServerHandle};
pub use engine::{DownloadTask, StreamTask, TaskFilter, TaskStatus, TorrentTask, VortexEngine};
pub use hls::{HlsPlaylist, HlsSegment, HlsStreamVariant};
pub use magnet::Magnet;
pub use metainfo::{TorrentFile, TorrentMeta};
pub use persistence::{load_config, save_config, VortexConfig};
pub use piece_picker::{Bitfield, BlockRequest, PiecePicker, PieceState, PickerStrategy};
pub use preview::{fetch_url_preview, MediaPreview, PreviewKind};
pub use segmented::{ChunkStatus, SegmentChunk, SegmentedDownload};
pub use sha1::Sha1;
pub use sniffer::{MediaSniffer, MediaStreamQuality, SniffedMedia};
pub use torrent::swarm::PeerInfo;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_full_workflow() {
        let mut engine = VortexEngine::new();

        let meta = TorrentMeta::create_single_file(
            "linux_kernel_source.tar.gz",
            200 * 1024 * 1024,
            256 * 1024,
            None,
        );
        let tid = engine.add_torrent(meta);
        assert_eq!(tid, 1);

        let (title, url, label, size) = {
            let sniffed = engine
                .sniffer
                .inspect_url(
                    "https://www.youtube.com/watch?v=rust_async",
                    "https://www.youtube.com/watch?v=rust_async",
                    "Rust Async Concurrency In Depth",
                )
                .unwrap();
            let best_q = sniffed.best_video_quality().unwrap();
            (
                sniffed.title.clone(),
                best_q.url.clone(),
                best_q.label.clone(),
                best_q.estimated_size_bytes,
            )
        };

        let sid = engine.add_stream(&title, &url, &label, size, 8);
        assert_eq!(sid, 2);
        assert_eq!(engine.tasks.len(), 2);
    }
}

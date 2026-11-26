//! vortex-core/src/media/mod.rs
//! Real media detection, HLS playlist parsing, and live video preview metadata extraction.

pub mod hls;
pub mod mp4_mux;
pub mod preview;
pub mod sniffer;

pub use hls::{HlsPlaylist, HlsSegment, HlsStreamVariant};
pub use mp4_mux::{mux_fmp4_files, mux_video_and_audio};
pub use preview::{fetch_url_preview, MediaPreview, PreviewKind};
pub use sniffer::{MediaSniffer, MediaStreamQuality, SniffedMedia};

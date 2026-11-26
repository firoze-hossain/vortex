//! vortex-core/src/network/mod.rs
//! Real HTTP/HTTPS networking, multi-threaded IDM range segmentation, and HLS stream downloading.

pub mod hls_downloader;
pub mod http;
pub mod segmented;

pub use http::{probe_url, ProbeResult};
pub use segmented::{ChunkStatus, SegmentChunk, SegmentedDownload, StreamHandle, StreamSnapshot};

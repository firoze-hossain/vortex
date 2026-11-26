//! vortex-core/src/torrent/mod.rs
//! Complete BitTorrent protocol suite: Bencode, SHA-1, Metainfo, Magnet,
//! Piece Picker, Disk Storage, HTTP/UDP Trackers, Peer Wire, and Swarm Coordinator.

pub mod bencode;
pub mod magnet;
pub mod metainfo;
pub mod peer_wire;
pub mod piece_picker;
pub mod sha1;
pub mod storage;
pub mod swarm;
pub mod tracker;

pub use bencode::Value as BencodeValue;
pub use magnet::Magnet;
pub use metainfo::{TorrentFile, TorrentMeta};
pub use piece_picker::{Bitfield, BlockRequest, PiecePicker, PieceState, PickerStrategy};
pub use sha1::Sha1;
pub use storage::TorrentStorage;
pub use swarm::{PeerInfo, SwarmController, SwarmHandle, SwarmSnapshot};
pub use tracker::{TrackerPeer, TrackerResponse};

//! vortex-core/src/torrent/swarm.rs
//! Real multi-peer BitTorrent Swarm Coordinator with TCP Peer Wire, Tracker Announce,
//! BEP-0009/0010 ut_metadata Magnet resolution, SHA-1 piece verification, and disk storage.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::bencode::Value;
use crate::engine::TaskStatus;
use crate::metainfo::TorrentMeta;
use crate::piece_picker::{Bitfield, PiecePicker, PieceState, PickerStrategy, STANDARD_BLOCK_SIZE};
use crate::sha1::Sha1;
use crate::torrent::peer_wire::{
    build_extended_handshake, build_metadata_request, decode_peer_client, parse_extended_handshake,
    parse_metadata_data, Handshake, PeerMessage, METADATA_PIECE_SIZE,
};
use crate::torrent::storage::TorrentStorage;
use crate::torrent::tracker;

const MAX_PEERS: usize = 16;
const PIPELINE_DEPTH: usize = 5;

#[derive(Clone, Debug, PartialEq)]
pub struct PeerInfo {
    pub addr: String,
    pub client: String,
    pub down_bps: u64,
    pub up_bps: u64,
    pub progress_pct: f32,
    pub peer_choking: bool,
    pub am_interested: bool,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct SwarmSnapshot {
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

struct PieceAssembly {
    data: Vec<u8>,
    received_blocks: HashSet<u32>,
    total_blocks: u32,
}

#[derive(Default)]
struct MetadataAssembly {
    total_size: usize,
    pieces: HashMap<usize, Vec<u8>>,
    resolved: bool,
}

struct SharedSwarm {
    meta: Mutex<TorrentMeta>,
    info_hash: [u8; 20],
    save_dir: PathBuf,
    storage: Mutex<Option<TorrentStorage>>,
    picker: Mutex<PiecePicker>,
    assemblies: Mutex<HashMap<usize, PieceAssembly>>,
    metadata_asm: Mutex<MetadataAssembly>,
    peers: Mutex<HashMap<String, PeerInfo>>,
    known_addrs: Mutex<HashSet<SocketAddr>>,
    downloaded_bytes: AtomicU64,
    uploaded_bytes: AtomicU64,
    seeders: AtomicU64,
    leechers: AtomicU64,
    paused: AtomicBool,
    stopped: AtomicBool,
    completed: AtomicBool,
    status_msg: Mutex<Option<String>>,
    peer_id: [u8; 20],
    download_limit_bps: Arc<AtomicU64>,
}

pub struct SwarmHandle {
    shared: Arc<SharedSwarm>,
    last_dl: u64,
    last_ul: u64,
    last_sample: Instant,
    dl_rate_bps: u64,
    ul_rate_bps: u64,
}

impl SwarmHandle {
    pub fn start(
        meta: TorrentMeta,
        save_dir: PathBuf,
        strategy: PickerStrategy,
        download_limit_bps: Arc<AtomicU64>,
        initial_paused: bool,
    ) -> Result<Self, String> {
        let has_metadata = !meta.pieces.is_empty();
        let (storage_opt, initial_dl, picker, all_done, initial_path) = if has_metadata {
            let storage = TorrentStorage::new(&save_dir, &meta)
                .map_err(|e| format!("Failed to initialize torrent storage: {}", e))?;
            let total_size = meta.total_length();
            let mut picker = PiecePicker::new(meta.piece_count(), meta.piece_length, total_size);
            picker.strategy = strategy;

            let existing = storage.verify_existing_pieces(&meta);
            let mut dl = 0u64;
            for idx in existing {
                picker.bitfield.set_piece(idx, true);
                picker.states[idx] = PieceState::Verified;
                dl += meta.piece_size(idx) as u64;
            }
            let done = picker.bitfield.is_all_complete();
            let path = storage.base_dir.clone();
            (Some(storage), dl, picker, done, path)
        } else {
            let mut picker = PiecePicker::new(0, meta.piece_length.max(512 * 1024), 0);
            picker.strategy = strategy;
            (None, 0, picker, false, save_dir.clone())
        };
        let _ = initial_path;

        let info_hash = meta.info_hash;
        let info_hash_prefix = meta.info_hash_hex[..8.min(meta.info_hash_hex.len())].to_string();
        let peer_id = generate_peer_id();
        let shared = Arc::new(SharedSwarm {
            meta: Mutex::new(meta),
            info_hash,
            save_dir,
            storage: Mutex::new(storage_opt),
            picker: Mutex::new(picker),
            assemblies: Mutex::new(HashMap::new()),
            metadata_asm: Mutex::new(MetadataAssembly {
                total_size: 0,
                pieces: HashMap::new(),
                resolved: has_metadata,
            }),
            peers: Mutex::new(HashMap::new()),
            known_addrs: Mutex::new(HashSet::new()),
            downloaded_bytes: AtomicU64::new(initial_dl),
            uploaded_bytes: AtomicU64::new(0),
            seeders: AtomicU64::new(0),
            leechers: AtomicU64::new(0),
            paused: AtomicBool::new(initial_paused),
            stopped: AtomicBool::new(false),
            completed: AtomicBool::new(all_done),
            status_msg: Mutex::new(Some(if all_done {
                "Verified on disk (Seeding)".to_string()
            } else if !has_metadata {
                "Magnet: Discovering peers & metadata (BEP-0009)...".to_string()
            } else {
                "Announcing to trackers...".to_string()
            })),
            peer_id,
            download_limit_bps,
        });

        let bg = Arc::clone(&shared);
        thread::Builder::new()
            .name(format!("vortex-swarm-{}", info_hash_prefix))
            .spawn(move || run_swarm_coordinator(bg))
            .map_err(|e| format!("Failed to spawn swarm thread: {}", e))?;

        Ok(Self {
            shared,
            last_dl: initial_dl,
            last_ul: 0,
            last_sample: Instant::now(),
            dl_rate_bps: 0,
            ul_rate_bps: 0,
        })
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
        if let Ok(mut msg) = self.shared.status_msg.lock() {
            *msg = Some(if paused {
                "Paused by user".into()
            } else if self.shared.completed.load(Ordering::Relaxed) {
                "Seeding".into()
            } else {
                "Downloading from swarm".into()
            });
        }
    }

    pub fn set_strategy(&self, strategy: PickerStrategy) {
        if let Ok(mut p) = self.shared.picker.lock() {
            p.strategy = strategy;
        }
    }

    pub fn stop(&self, delete_files: bool) {
        self.shared.stopped.store(true, Ordering::Relaxed);
        if delete_files {
            if let Ok(guard) = self.shared.storage.lock() {
                if let Some(st) = guard.as_ref() {
                    st.delete_files();
                }
            }
        }
    }

    pub fn snapshot(&mut self) -> SwarmSnapshot {
        let now = Instant::now();
        let dt = now.duration_since(self.last_sample).as_secs_f64();
        let cur_dl = self.shared.downloaded_bytes.load(Ordering::Relaxed);
        let cur_ul = self.shared.uploaded_bytes.load(Ordering::Relaxed);

        if dt >= 0.4 {
            let d_dl = cur_dl.saturating_sub(self.last_dl);
            let d_ul = cur_ul.saturating_sub(self.last_ul);
            self.dl_rate_bps = ((d_dl as f64) / dt) as u64;
            self.ul_rate_bps = ((d_ul as f64) / dt) as u64;
            self.last_dl = cur_dl;
            self.last_ul = cur_ul;
            self.last_sample = now;
        }

        let is_paused = self.shared.paused.load(Ordering::Relaxed);
        let is_done = self.shared.completed.load(Ordering::Relaxed);

        let status = if is_paused {
            TaskStatus::Paused
        } else if is_done {
            TaskStatus::Seeding
        } else {
            TaskStatus::Downloading
        };

        let meta = self.shared.meta.lock().unwrap().clone();
        let picker = self.shared.picker.lock().map(|p| p.clone()).unwrap_or_else(|_| {
            PiecePicker::new(meta.piece_count(), meta.piece_length, meta.total_length())
        });
        let peers = self
            .shared
            .peers
            .lock()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        let save_path = self
            .shared
            .storage
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.base_dir.clone()))
            .unwrap_or_else(|| self.shared.save_dir.clone());
        let last_message = self.shared.status_msg.lock().ok().and_then(|g| g.clone());

        SwarmSnapshot {
            total_size: meta.total_length(),
            downloaded_bytes: cur_dl,
            uploaded_bytes: cur_ul,
            download_rate_bps: if is_paused || is_done { 0 } else { self.dl_rate_bps },
            upload_rate_bps: if is_paused { 0 } else { self.ul_rate_bps },
            seeders: self.shared.seeders.load(Ordering::Relaxed) as u32,
            leechers: self.shared.leechers.load(Ordering::Relaxed) as u32,
            status,
            piece_picker: picker,
            meta,
            peers,
            save_path,
            last_message,
        }
    }
}

pub struct SwarmController;

fn run_swarm_coordinator(shared: Arc<SharedSwarm>) {
    let tracker_urls = {
        let meta = shared.meta.lock().unwrap();
        let mut urls = Vec::new();
        if let Some(a) = &meta.announce {
            urls.push(a.clone());
        }
        for a in &meta.announce_list {
            if !urls.contains(a) {
                urls.push(a.clone());
            }
        }
        urls
    };

    let mut active_workers: HashSet<SocketAddr> = HashSet::new();
    let mut last_announce = Instant::now() - Duration::from_secs(600);

    while !shared.stopped.load(Ordering::Relaxed) {
        if shared.paused.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(250));
            continue;
        }

        if last_announce.elapsed() >= Duration::from_secs(60) && !tracker_urls.is_empty() {
            last_announce = Instant::now();
            let dl = shared.downloaded_bytes.load(Ordering::Relaxed);
            let ul = shared.uploaded_bytes.load(Ordering::Relaxed);
            let total_len = shared.meta.lock().unwrap().total_length();
            let left = total_len.saturating_sub(dl);
            let ev = if dl == 0 {
                Some("started")
            } else if left == 0 && total_len > 0 {
                Some("completed")
            } else {
                None
            };

            let mut total_seeders = 0u32;
            let mut total_leechers = 0u32;
            let mut any_ok = false;

            for tr_url in &tracker_urls {
                if shared.stopped.load(Ordering::Relaxed) {
                    return;
                }
                match tracker::announce(
                    tr_url,
                    &shared.info_hash,
                    &shared.peer_id,
                    6881,
                    ul,
                    dl,
                    left,
                    ev,
                ) {
                    Ok(resp) => {
                        any_ok = true;
                        total_seeders = total_seeders.max(resp.seeders);
                        total_leechers = total_leechers.max(resp.leechers);
                        if let Ok(mut known) = shared.known_addrs.lock() {
                            for p in resp.peers {
                                known.insert(p.socket_addr());
                            }
                        }
                    }
                    Err(e) => {
                        if let Ok(mut msg) = shared.status_msg.lock() {
                            *msg = Some(format!("Tracker {}: {}", tr_url, e));
                        }
                    }
                }
            }

            if any_ok {
                shared.seeders.store(total_seeders as u64, Ordering::Relaxed);
                shared.leechers.store(total_leechers as u64, Ordering::Relaxed);
                let peer_total = shared.known_addrs.lock().map(|k| k.len()).unwrap_or(0);
                if let Ok(mut msg) = shared.status_msg.lock() {
                    *msg = Some(format!("Connected: {} peers discovered", peer_total));
                }
            }
        }

        if !shared.completed.load(Ordering::Relaxed) {
            let candidates: Vec<SocketAddr> = shared
                .known_addrs
                .lock()
                .map(|k| k.iter().copied().collect())
                .unwrap_or_default();

            let currently_connected = shared.peers.lock().map(|p| p.len()).unwrap_or(0);
            if currently_connected < MAX_PEERS {
                for addr in candidates {
                    if active_workers.contains(&addr) {
                        continue;
                    }
                    if active_workers.len() >= MAX_PEERS * 2 {
                        break;
                    }
                    active_workers.insert(addr);
                    let peer_shared = Arc::clone(&shared);
                    thread::Builder::new()
                        .name("vortex-peer".into())
                        .spawn(move || run_peer_session(peer_shared, addr))
                        .ok();
                }
            }
        }

        thread::sleep(Duration::from_millis(400));
    }
}

fn run_peer_session(shared: Arc<SharedSwarm>, addr: SocketAddr) {
    let addr_str = addr.to_string();
    let stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(4)) {
        Ok(s) => s,
        Err(_) => return,
    };
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let mut reader = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut writer = stream;

    let hs = Handshake::new(shared.info_hash, shared.peer_id);
    if writer.write_all(&hs.encode()).is_err() {
        return;
    }
    let peer_hs = match Handshake::read_from(&mut reader) {
        Ok(h) if h.info_hash == shared.info_hash => h,
        _ => return,
    };

    if peer_hs.supports_extensions() {
        let _ = build_extended_handshake().write_to(&mut writer);
    }

    let client_name = decode_peer_client(&peer_hs.peer_id);
    let mut num_pieces = shared.picker.lock().map(|p| p.num_pieces).unwrap_or(0);
    let mut peer_bf = Bitfield::new(num_pieces);
    let mut peer_choking = true;
    let mut in_flight: HashSet<(usize, u32)> = HashSet::new();

    if let Ok(mut peers) = shared.peers.lock() {
        peers.insert(
            addr_str.clone(),
            PeerInfo {
                addr: addr_str.clone(),
                client: client_name.clone(),
                down_bps: 0,
                up_bps: 0,
                progress_pct: 0.0,
                peer_choking: true,
                am_interested: false,
                state: "Handshaked".into(),
            },
        );
    }

    if let Ok(picker) = shared.picker.lock() {
        if picker.bitfield.count_completed() > 0 {
            let _ = PeerMessage::Bitfield(picker.bitfield.as_bytes().to_vec()).write_to(&mut writer);
        }
    }
    let _ = PeerMessage::Interested.write_to(&mut writer);

    let mut bytes_this_window = 0u64;
    let mut window_start = Instant::now();

    while !shared.stopped.load(Ordering::Relaxed) {
        if shared.paused.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(200));
            continue;
        }

        let metadata_ready = shared.metadata_asm.lock().map(|m| m.resolved).unwrap_or(true);
        if metadata_ready && !peer_choking && !shared.completed.load(Ordering::Relaxed) && in_flight.len() < PIPELINE_DEPTH {
            while in_flight.len() < PIPELINE_DEPTH {
                let next = select_block_for_peer(&shared, &peer_bf, &in_flight);
                let Some((piece_idx, offset, length)) = next else {
                    break;
                };
                if (PeerMessage::Request {
                    index: piece_idx as u32,
                    begin: offset,
                    length,
                })
                .write_to(&mut writer)
                .is_err()
                {
                    cleanup_peer(&shared, &addr_str);
                    return;
                }
                in_flight.insert((piece_idx, offset));
            }
        }

        match PeerMessage::read_from(&mut reader) {
            Ok(PeerMessage::KeepAlive) => {}
            Ok(PeerMessage::Choke) => {
                peer_choking = true;
                in_flight.clear();
                update_peer_state(&shared, &addr_str, |p| {
                    p.peer_choking = true;
                    p.state = "Choked".into();
                });
            }
            Ok(PeerMessage::Unchoke) => {
                peer_choking = false;
                update_peer_state(&shared, &addr_str, |p| {
                    p.peer_choking = false;
                    p.am_interested = true;
                    p.state = "Downloading".into();
                });
            }
            Ok(PeerMessage::Have(idx)) => {
                if num_pieces > 0 {
                    peer_bf.set_piece(idx as usize, true);
                    if let Ok(mut picker) = shared.picker.lock() {
                        picker.register_peer_have(idx as usize);
                    }
                    let pct = peer_bf.completion_pct();
                    update_peer_state(&shared, &addr_str, |p| p.progress_pct = pct);
                }
            }
            Ok(PeerMessage::Bitfield(bytes)) => {
                num_pieces = shared.picker.lock().map(|p| p.num_pieces).unwrap_or(bytes.len() * 8);
                peer_bf = Bitfield::from_bytes(bytes, num_pieces);
                if let Ok(mut picker) = shared.picker.lock() {
                    if picker.num_pieces > 0 {
                        picker.register_peer_bitfield(&peer_bf);
                    }
                }
                let pct = peer_bf.completion_pct();
                update_peer_state(&shared, &addr_str, |p| p.progress_pct = pct);
            }
            Ok(PeerMessage::Extended { ext_id: 0, payload }) => {
                let need_meta = shared.metadata_asm.lock().map(|m| !m.resolved).unwrap_or(false);
                if need_meta {
                    if let Some((peer_ut_id, meta_size)) = parse_extended_handshake(&payload) {
                        if meta_size > 0 && meta_size <= 16 * 1024 * 1024 {
                            if let Ok(mut asm) = shared.metadata_asm.lock() {
                                asm.total_size = meta_size;
                            }
                            let count = (meta_size + METADATA_PIECE_SIZE - 1) / METADATA_PIECE_SIZE;
                            for idx in 0..count {
                                let _ = build_metadata_request(peer_ut_id, idx).write_to(&mut writer);
                            }
                        }
                    }
                }
            }
            Ok(PeerMessage::Extended { ext_id: 1, payload }) => {
                if let Some((piece_idx, raw)) = parse_metadata_data(&payload) {
                    try_apply_metadata_piece(&shared, piece_idx, raw);
                    num_pieces = shared.picker.lock().map(|p| p.num_pieces).unwrap_or(num_pieces);
                }
            }
            Ok(PeerMessage::Piece { index, begin, block }) => {
                let piece_idx = index as usize;
                in_flight.remove(&(piece_idx, begin));

                let block_len = block.len() as u64;
                bytes_this_window += block_len;

                let limit = shared.download_limit_bps.load(Ordering::Relaxed);
                if limit > 0 {
                    let elapsed = window_start.elapsed().as_secs_f64().max(0.001);
                    let cur_bps = (bytes_this_window as f64) / elapsed;
                    if cur_bps > (limit as f64) {
                        thread::sleep(Duration::from_millis(25));
                    }
                }

                if window_start.elapsed() >= Duration::from_millis(750) {
                    let rate = ((bytes_this_window as f64) / window_start.elapsed().as_secs_f64()) as u64;
                    bytes_this_window = 0;
                    window_start = Instant::now();
                    update_peer_state(&shared, &addr_str, |p| p.down_bps = rate);
                }

                handle_received_block(&shared, piece_idx, begin, &block);
            }
            Ok(PeerMessage::Request { index, begin, length }) => {
                if length <= 32768 {
                    let has = shared
                        .picker
                        .lock()
                        .map(|p| p.bitfield.has_piece(index as usize))
                        .unwrap_or(false);
                    if has {
                        let block_opt = shared
                            .storage
                            .lock()
                            .ok()
                            .and_then(|g| g.as_ref().and_then(|s| s.read_block(index as usize, begin, length).ok()));
                        if let Some(data) = block_opt {
                            let len = data.len() as u64;
                            if (PeerMessage::Piece {
                                index,
                                begin,
                                block: data,
                            })
                            .write_to(&mut writer)
                            .is_ok()
                            {
                                shared.uploaded_bytes.fetch_add(len, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                let _ = PeerMessage::KeepAlive.write_to(&mut writer);
            }
            Err(_) => break,
        }
    }

    cleanup_peer(&shared, &addr_str);
}

fn try_apply_metadata_piece(shared: &Arc<SharedSwarm>, piece_idx: usize, raw: &[u8]) {
    let assembled_opt = {
        let Ok(mut asm) = shared.metadata_asm.lock() else { return };
        if asm.resolved || asm.total_size == 0 {
            return;
        }
        asm.pieces.insert(piece_idx, raw.to_vec());
        let expected_pieces = (asm.total_size + METADATA_PIECE_SIZE - 1) / METADATA_PIECE_SIZE;
        if asm.pieces.len() < expected_pieces {
            return;
        }
        let mut full = Vec::with_capacity(asm.total_size);
        for i in 0..expected_pieces {
            if let Some(chunk) = asm.pieces.get(&i) {
                full.extend_from_slice(chunk);
            } else {
                return;
            }
        }
        if full.len() != asm.total_size || Sha1::digest(&full) != shared.info_hash {
            asm.pieces.clear();
            return;
        }
        asm.resolved = true;
        Some(full)
    };

    let Some(info_bytes) = assembled_opt else { return };
    let Ok(info_val) = Value::decode(&info_bytes) else { return };

    let mut root = BTreeMap::new();
    {
        let meta_old = shared.meta.lock().unwrap();
        if let Some(ann) = &meta_old.announce {
            root.insert(b"announce".to_vec(), Value::Bytes(ann.as_bytes().to_vec()));
        }
        if !meta_old.announce_list.is_empty() {
            let al = meta_old
                .announce_list
                .iter()
                .map(|u| Value::List(vec![Value::Bytes(u.as_bytes().to_vec())]))
                .collect();
            root.insert(b"announce-list".to_vec(), Value::List(al));
        }
    }
    root.insert(b"info".to_vec(), info_val);
    let torrent_bytes = Value::Dict(root).encode();

    if let Ok(new_meta) = TorrentMeta::from_bytes(&torrent_bytes) {
        let storage_res = TorrentStorage::new(&shared.save_dir, &new_meta);
        if let Ok(storage) = storage_res {
            let strategy = shared
                .picker
                .lock()
                .map(|p| p.strategy)
                .unwrap_or(PickerStrategy::RarestFirst);
            let mut new_picker = PiecePicker::new(new_meta.piece_count(), new_meta.piece_length, new_meta.total_length());
            new_picker.strategy = strategy;
            let existing = storage.verify_existing_pieces(&new_meta);
            let mut dl = 0u64;
            for idx in existing {
                new_picker.bitfield.set_piece(idx, true);
                new_picker.states[idx] = PieceState::Verified;
                dl += new_meta.piece_size(idx) as u64;
            }
            shared.downloaded_bytes.store(dl, Ordering::Relaxed);
            if new_picker.bitfield.is_all_complete() {
                shared.completed.store(true, Ordering::Relaxed);
            }
            *shared.picker.lock().unwrap() = new_picker;
            *shared.storage.lock().unwrap() = Some(storage);
            *shared.meta.lock().unwrap() = new_meta;
            if let Ok(mut msg) = shared.status_msg.lock() {
                *msg = Some("Magnet metadata resolved via BEP-0009".into());
            }
        }
    }
}

fn select_block_for_peer(
    shared: &SharedSwarm,
    peer_bf: &Bitfield,
    in_flight: &HashSet<(usize, u32)>,
) -> Option<(usize, u32, u32)> {
    let picker = shared.picker.lock().ok()?;
    let assemblies = shared.assemblies.lock().ok()?;

    let mut candidates: Vec<usize> = (0..picker.num_pieces)
        .filter(|&i| !picker.bitfield.has_piece(i) && peer_bf.has_piece(i))
        .collect();

    if picker.strategy == PickerStrategy::RarestFirst {
        candidates.sort_by_key(|&i| picker.peer_availability[i]);
    }

    for piece_idx in candidates {
        let piece_sz = picker.piece_size(piece_idx);
        let total_blocks = picker.blocks_per_piece(piece_idx);
        let received = assemblies.get(&piece_idx).map(|a| &a.received_blocks);

        for block_idx in 0..total_blocks {
            let offset = block_idx * STANDARD_BLOCK_SIZE;
            if in_flight.contains(&(piece_idx, offset)) {
                continue;
            }
            if let Some(rec) = received {
                if rec.contains(&offset) {
                    continue;
                }
            }
            let len = (piece_sz - offset).min(STANDARD_BLOCK_SIZE);
            return Some((piece_idx, offset, len));
        }
    }
    None
}

fn handle_received_block(shared: &SharedSwarm, piece_idx: usize, offset: u32, block: &[u8]) {
    let (piece_count, piece_sz, expected_hash) = {
        let meta = shared.meta.lock().unwrap();
        if piece_idx >= meta.piece_count() {
            return;
        }
        (meta.piece_count(), meta.piece_size(piece_idx) as usize, meta.pieces[piece_idx])
    };
    if piece_idx >= piece_count || piece_sz == 0 {
        return;
    }
    let total_blocks = ((piece_sz as u32) + STANDARD_BLOCK_SIZE - 1) / STANDARD_BLOCK_SIZE;

    let completed_piece_data: Option<Vec<u8>> = {
        let Ok(mut assemblies) = shared.assemblies.lock() else { return };
        let entry = assemblies.entry(piece_idx).or_insert_with(|| PieceAssembly {
            data: vec![0u8; piece_sz],
            received_blocks: HashSet::new(),
            total_blocks,
        });

        let end = ((offset as usize) + block.len()).min(piece_sz);
        if (offset as usize) < piece_sz {
            entry.data[offset as usize..end].copy_from_slice(&block[..(end - offset as usize)]);
            entry.received_blocks.insert(offset);
        }

        if let Ok(mut picker) = shared.picker.lock() {
            if !picker.bitfield.has_piece(piece_idx) {
                picker.states[piece_idx] = PieceState::Downloading {
                    downloaded_blocks: entry.received_blocks.len() as u32,
                    total_blocks,
                };
            }
        }

        if entry.received_blocks.len() as u32 >= entry.total_blocks {
            assemblies.remove(&piece_idx).map(|a| a.data)
        } else {
            None
        }
    };

    if let Some(piece_data) = completed_piece_data {
        let valid = {
            let Ok(mut picker) = shared.picker.lock() else { return };
            picker.mark_piece_completed(piece_idx, &piece_data, &expected_hash)
        };
        if valid {
            if let Ok(guard) = shared.storage.lock() {
                if let Some(st) = guard.as_ref() {
                    let _ = st.write_piece(piece_idx, &piece_data);
                }
            }
            shared
                .downloaded_bytes
                .fetch_add(piece_data.len() as u64, Ordering::Relaxed);

            if let Ok(picker) = shared.picker.lock() {
                if picker.bitfield.is_all_complete() {
                    shared.completed.store(true, Ordering::Relaxed);
                    if let Ok(mut msg) = shared.status_msg.lock() {
                        *msg = Some("Completed & Seeding".into());
                    }
                }
            }
        }
    }
}

fn update_peer_state<F: FnOnce(&mut PeerInfo)>(shared: &SharedSwarm, addr: &str, f: F) {
    if let Ok(mut peers) = shared.peers.lock() {
        if let Some(p) = peers.get_mut(addr) {
            f(p);
        }
    }
}

fn cleanup_peer(shared: &SharedSwarm, addr: &str) {
    if let Ok(mut peers) = shared.peers.lock() {
        peers.remove(addr);
    }
}

fn generate_peer_id() -> [u8; 20] {
    let mut id = *b"-VX2000-000000000000";
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(123456789);
    let s = format!("{:012}", (nanos % 1_000_000_000_000) as u64);
    id[8..20].copy_from_slice(&s.as_bytes()[..12]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn test_real_localhost_swarm_download_and_verify() {
        let payload: Vec<u8> = (0..49152).map(|i| (i % 251) as u8).collect();
        let piece_len = 24576u32;
        let p0 = Sha1::digest(&payload[0..24576]);
        let p1 = Sha1::digest(&payload[24576..49152]);

        let peer_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer_addr = peer_listener.local_addr().unwrap();

        let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tracker_addr = tracker_listener.local_addr().unwrap();

        let mut meta = TorrentMeta::create_single_file(
            "real_test_payload.bin",
            payload.len() as u64,
            piece_len,
            Some(&format!("http://{}/announce", tracker_addr)),
        );
        meta.pieces = vec![p0, p1];
        let info_bytes = {
            let decoded = Value::decode(&meta.to_bytes()).unwrap();
            decoded.dict_get("info").unwrap().encode()
        };
        meta.info_hash = Sha1::digest(&info_bytes);
        meta.info_hash_hex = Sha1::hex(&meta.info_hash);

        let info_hash_copy = meta.info_hash;
        let payload_copy = payload.clone();

        thread::spawn(move || {
            if let Ok((mut conn, _)) = tracker_listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = conn.read(&mut buf);

                let ip = match peer_addr.ip() {
                    std::net::IpAddr::V4(v4) => v4.octets(),
                    _ => [127, 0, 0, 1],
                };
                let port = peer_addr.port().to_be_bytes();
                let compact = vec![ip[0], ip[1], ip[2], ip[3], port[0], port[1]];

                let mut dict = BTreeMap::new();
                dict.insert(b"complete".to_vec(), Value::Integer(1));
                dict.insert(b"incomplete".to_vec(), Value::Integer(0));
                dict.insert(b"interval".to_vec(), Value::Integer(60));
                dict.insert(b"peers".to_vec(), Value::Bytes(compact));
                let body = Value::Dict(dict).encode();

                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = conn.write_all(hdr.as_bytes());
                let _ = conn.write_all(&body);
            }
        });

        thread::spawn(move || {
            if let Ok((mut conn, _)) = peer_listener.accept() {
                let _hs = Handshake::read_from(&mut conn).unwrap();
                let reply = Handshake::new(info_hash_copy, *b"-TR4000-112233445566");
                conn.write_all(&reply.encode()).unwrap();
                PeerMessage::Bitfield(vec![0b1100_0000]).write_to(&mut conn).unwrap();
                PeerMessage::Unchoke.write_to(&mut conn).unwrap();

                while let Ok(msg) = PeerMessage::read_from(&mut conn) {
                    if let PeerMessage::Request { index, begin, length } = msg {
                        let start = (index as usize) * (piece_len as usize) + (begin as usize);
                        let end = (start + length as usize).min(payload_copy.len());
                        PeerMessage::Piece {
                            index,
                            begin,
                            block: payload_copy[start..end].to_vec(),
                        }
                        .write_to(&mut conn)
                        .unwrap();
                    }
                }
            }
        });

        let save_dir = std::env::temp_dir().join("vortex_test_swarm_dl");
        let _ = std::fs::remove_dir_all(&save_dir);

        let mut handle = SwarmHandle::start(
            meta,
            save_dir.clone(),
            PickerStrategy::SequentialStreaming,
            Arc::new(AtomicU64::new(0)),
            false,
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut done = false;
        while Instant::now() < deadline {
            let snap = handle.snapshot();
            if snap.status == TaskStatus::Seeding {
                done = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        assert!(done, "Localhost swarm should complete downloading and verifying");
        let on_disk = std::fs::read(save_dir.join("real_test_payload.bin")).unwrap();
        assert_eq!(on_disk, payload);

        handle.stop(true);
        let _ = std::fs::remove_dir_all(&save_dir);
    }
}

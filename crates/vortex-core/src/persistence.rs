//! vortex-core/src/persistence.rs
//! Cross-platform configuration and download queue state persistence (~/.vortex/).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VortexConfig {
    pub download_dir: PathBuf,
    pub default_threads: usize,
    pub global_download_limit_bps: u64,
    pub global_upload_limit_bps: u64,
    pub bridge_port: u16,
    pub sequential_torrents_default: bool,
    #[serde(default = "default_true")]
    pub clipboard_monitor: bool,
}

impl Default for VortexConfig {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            default_threads: 8,
            global_download_limit_bps: 0,
            global_upload_limit_bps: 0,
            bridge_port: 47391,
            sequential_torrents_default: false,
            clipboard_monitor: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedStreamRecord {
    pub id: u64,
    pub name: String,
    pub url: String,
    pub resolution: String,
    pub total_size: u64,
    pub downloaded_bytes: u64,
    pub threads: usize,
    pub is_hls: bool,
    pub paused: bool,
    pub completed: bool,
    pub save_path: PathBuf,
    #[serde(default)]
    pub audio_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedTorrentRecord {
    pub id: u64,
    pub name: String,
    pub total_size: u64,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    pub paused: bool,
    pub completed: bool,
    pub sequential: bool,
    pub save_dir: PathBuf,
    pub metainfo_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedState {
    pub next_task_id: u64,
    pub streams: Vec<SavedStreamRecord>,
    pub torrents: Vec<SavedTorrentRecord>,
}

pub fn vortex_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".vortex")
}

pub fn default_download_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("Downloads").join("Vortex")
}

pub fn load_config() -> VortexConfig {
    let path = vortex_data_dir().join("config.json");
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(cfg) = serde_json::from_slice(&bytes) {
            return cfg;
        }
    }
    VortexConfig::default()
}

pub fn save_config(cfg: &VortexConfig) {
    let dir = vortex_data_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_vec_pretty(cfg) {
        let _ = fs::write(dir.join("config.json"), json);
    }
}

pub fn load_state() -> SavedState {
    let path = vortex_data_dir().join("state.json");
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(st) = serde_json::from_slice(&bytes) {
            return st;
        }
    }
    SavedState {
        next_task_id: 1,
        streams: Vec::new(),
        torrents: Vec::new(),
    }
}

pub fn save_state(state: &SavedState) {
    let dir = vortex_data_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_vec(state) {
        let _ = fs::write(dir.join("state.json"), json);
    }
}

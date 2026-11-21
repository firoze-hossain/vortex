# Vortex Studio 🌀⚡

> **Unified BitTorrent Client & Intelligent Stream Accelerator in Pure Rust (1.99.1)**  
> Seamlessly combining the swarming power of modern BitTorrent clients with the intelligent video stream sniffing and multi-connection range acceleration of advanced download managers.

[![Rust](https://img.shields.io/badge/rust-1.99.1-blue.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-green.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-NovaOS%20%7C%20Linux%20%7C%20macOS%20%7C%20Windows-orange.svg)]()
[![Tests](https://img.shields.io/badge/tests-15%2F15%20passing-brightgreen.svg)]()

---

## 🌟 Overview

**Vortex Studio** is a sovereign, high-performance desktop application and engine written purely in **Rust 1.99.1** with zero external C/C++ dependencies. It unifies two traditionally disparate download paradigms into a single coordinated pipeline:

1. **BitTorrent Swarm Client**:
   - Conforming **BEP-0003** bencoding parser and serializer.
   - **BEP-0009** Magnet URI parser and resolver with SHA-1 hash validation.
   - Comprehensive `.torrent` metadata inspector supporting single-file and multi-file directory structures.
   - Piece picker supporting both **Rarest-First** (for swarm health) and **Sequential Streaming Mode** (for previewing media files while downloading).
   - Real-time **Piece Map Grid Visualizer**: 2D colored matrix displaying hundreds of pieces (Verified, In-Flight, and Missing).

2. **Stream Accelerator & Intelligent Video Sniffer**:
   - **IDM-Style Multi-Threaded Range Acceleration**: Dynamically partitions target media files into 4 to 16 parallel byte ranges (`bytes=start-end`), multiplying transfer speeds and bypassing single-connection bandwidth caps.
   - **Browser Video Sniffer**: Injected bridge and DOM monitor that detects active HTML5 `<video>`, YouTube streams, and HLS playlists.
   - **Floating Action Badge**: Surfaces an interactive pop-up overlay (`[ ⬇ Download This Video | 1080p FHD ▾ ]`) directly beside video players in any web browser.
   - **Parallel Thread Visualizer**: Real-time progress bars for each chunk connection showing transfer rates and completed ranges.
   - **HLS (.m3u8) Stream Grabber**: Parses master and media playlists, downloading and assembling transport stream segments into contiguous files.

3. **Concurrency & Unified Engine**:
   - Capable of downloading multiple torrent swarms AND multiple accelerated video streams concurrently in parallel.
   - Global bandwidth throttler with configurable download and upload caps.
   - Task management: Pause, Resume, Prioritize, and Remove with reverse dependency safety.

---

## 🏗️ Architecture & Modular Packaging

The project is structured into clean, decoupled Rust crates:

```
vortex/
├── crates/
│   ├── vortex-core/          # Pure Rust protocol engine
│   │   ├── src/
│   │   │   ├── bencode.rs     # BEP-0003 bencode parser & encoder
│   │   │   ├── sha1.rs        # Pure Rust RFC 3174 SHA-1 digest
│   │   │   ├── metainfo.rs    # .torrent parser & info_hash generator
│   │   │   ├── magnet.rs      # Magnet URI parser & BTIH extractor
│   │   │   ├── piece_picker.rs# Bitfield, block scheduler & verification
│   │   │   ├── segmented.rs   # Multi-thread HTTP range chunk accelerator
│   │   │   ├── sniffer.rs     # Media sniffer & video format detector
│   │   │   ├── hls.rs         # HTTP Live Streaming (m3u8) parser
│   │   │   ├── engine.rs      # Unified concurrent VortexEngine
│   │   │   └── lib.rs
│   │   └── Cargo.toml
│   │
│   ├── vortex-ui/            # Composited UI component library
│   │   ├── src/
│   │   │   ├── geom.rs        # Point, Rect, layout math
│   │   │   ├── draw.rs        # 2D software drawing on 32-bit ARGB surfaces
│   │   │   ├── font.rs        # Embedded typography bitmap renderer
│   │   │   ├── piece_grid.rs  # 2D BitTorrent piece map visualizer
│   │   │   ├── chunk_bars.rs  # IDM multi-thread segmented progress bars
│   │   │   ├── floating_sniffer.rs # Floating video download badge & menu
│   │   │   ├── view.rs        # Desktop window view & click dispatch
│   │   │   └── lib.rs
│   │   └── Cargo.toml
│   │
│   └── vortex-app/           # Application binary & CLI driver
│       ├── src/
│       │   ├── png.rs         # Pure Rust PNG frame exporter
│       │   ├── sample.rs      # Sample torrents & video stream synthesizer
│       │   └── main.rs        # CLI argument parser & runner
│       └── Cargo.toml
│
├── novaos/                    # NovaOS Bare-Metal Ring-3 Integration
│   ├── ffi.rs                 # Ring-3 system call bindings
│   ├── vortex.rs              # Bare-metal standalone userland executable
│   └── build.sh               # ELF cross-compilation & disk image fixture installer
│
├── assets/                    # Sample torrent metadata assets
├── Cargo.toml                 # Workspace manifest
└── README.md
```

---

## 🚀 Quickstart & CLI Usage

### Build Workspace
```bash
cargo build --release
```

### Run Autonomous Test Suite
```bash
cargo test --workspace
```

### List Active Swarms and Stream Downloads
```bash
./target/release/vortex --list
```

### Sniff Web Video Stream Options
```bash
./target/release/vortex --sniff "https://www.youtube.com/watch?v=kernel_deep_dive"
```

### Inspect Torrent File or Magnet Link
```bash
./target/release/vortex --info "magnet:?xt=urn:btih:2fd4e1c67a2d28fced849ee1bb76e7391b93eb12&dn=NovaOS"
```

### Export Visual UI Snapshots to PNG
```bash
./target/release/vortex --export-view-png /tmp/vortex_desktop.png
./target/release/vortex --export-piece-grid-png /tmp/vortex_piece_grid.png
./target/release/vortex --export-chunk-bars-png /tmp/vortex_chunk_bars.png
./target/release/vortex --export-sniffer-badge-png /tmp/vortex_sniffer_badge.png
```

---

## 🏪 NovaOS App Store Integration

Vortex is registered in the **NovaOS App Store (`nova-store`)** under the **Internet** category:
- **Discoverable**: App Store catalog card with icon `[VTX]`, 4.9 rating, screenshot previews, and permission declarations.
- **On-Demand Lifecycle**: Users can 1-click **Install**, **Update**, **Open**, or **Remove** Vortex cleanly without cluttering default system apps.
- **Terminal Commands**: Executable directly via `vortex`, `torrent`, and `idm`.
- **Aero Browser Bridge**: Shows the floating download badge beside video elements when navigating video sites in NovaOS.

---

## 📄 License

Dual-licensed under MIT or Apache-2.0.

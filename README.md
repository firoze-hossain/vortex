# Vortex Studio

> **Cross-Platform Unified BitTorrent Client, IDM-Style Multi-Thread Stream Accelerator, Live Video Previewer & Pure-Rust Bridge**

Vortex Studio combines a real **BitTorrent swarm engine** (like uTorrent / Transmission) with an **IDM-style multi-threaded HTTP/HLS video & file downloader**, **real URL video previewing** (live title, duration, formats, and decoded PNG/JPEG thumbnail), **extensionless clipboard auto-capture**, and a **pure-Rust local IPC bridge** — written **100% in Rust with zero JavaScript or TypeScript**.

---

## Architecture & Modular Packaging

```
vortex/
├── crates/
│   ├── vortex-core/                   # Pure Rust download & protocol engine (no UI deps)
│   │   └── src/
│   │       ├── engine.rs              # Unified VortexEngine coordinating real workers & state
│   │       ├── persistence.rs         # ~/.vortex/config.json & state.json persistence
│   │       ├── torrent/               # BitTorrent Protocol Suite
│   │       │   ├── bencode.rs         # BEP-0003 bencode parser & encoder
│   │       │   ├── sha1.rs            # Pure Rust RFC 3174 SHA-1
│   │       │   ├── metainfo.rs        # .torrent parser & serializer
│   │       │   ├── magnet.rs          # Magnet URI (BTIH) parser & generator
│   │       │   ├── piece_picker.rs    # Bitfield, Rarest-First / Sequential picker, SHA-1 verify
│   │       │   ├── storage.rs         # Single & multi-file disk piece I/O
│   │       │   ├── tracker.rs         # HTTP/HTTPS (BEP-0003) & UDP (BEP-0015) tracker client
│   │       │   ├── peer_wire.rs       # TCP Peer Wire Protocol + BEP-0010/0009 ut_metadata
│   │       │   └── swarm.rs           # Concurrent multi-peer SwarmHandle & PeerInfo telemetry
│   │       ├── network/               # HTTP/HTTPS & HLS Acceleration
│   │       │   ├── http.rs            # HEAD/Range probe, headers, streaming reader
│   │       │   ├── segmented.rs       # 1-16 thread IDM range downloader writing to disk
│   │       │   └── hls_downloader.rs  # Concurrent .m3u8 segment fetcher & assembler
│   │       ├── media/                 # Video Sniffing & Live Preview
│   │       │   ├── hls.rs             # M3U8 master/media playlist parser
│   │       │   ├── sniffer.rs         # Media URL classifier & stream registry
│   │       │   └── preview.rs         # Real title, formats, sizes & thumbnail fetcher
│   │       └── bridge/                # Local HTTP/JSON IPC Bridge
│   │           └── server.rs          # Local API & status page on 127.0.0.1:47391
│   │
│   ├── vortex-ui/                     # Cross-platform UI library
│   │   └── src/
│   │       ├── theme.rs               # Dark UI color palette
│   │       ├── font.rs                # Complete 95-char printable ASCII bitmap font
│   │       ├── image_decode.rs        # Pure-Rust PNG (miniz_oxide) & Baseline JPEG decoder
│   │       ├── draw.rs                # 2D primitives, icons, and scaled thumbnail blitter
│   │       ├── modals.rs              # Add URL + Video Preview, Add Torrent/Magnet, Settings
│   │       ├── piece_grid.rs          # 2D BitTorrent piece map visualizer
│   │       ├── chunk_bars.rs          # IDM multi-thread chunk progress bars
│   │       ├── floating_sniffer.rs    # Floating badge preview widget (for Aero Browser)
│   │       └── view.rs                # Desktop Studio view & 3-tab inspector
│   │
│   └── vortex-app/                    # Cross-platform Desktop Binary & CLI
│       └── src/
│           ├── main.rs                # GUI loop, clipboard monitor, async preview, CLI
│           ├── clipboard.rs           # macOS / Linux / Windows clipboard, file picker, folder open
│           ├── png.rs                 # PNG snapshot exporter
│           └── sample.rs              # Demo mode (--demo) sample generator
│
├── browser-extension/                 # IDM-style floating video button for Chrome, Brave & Edge (MV3)
├── browser-extension-firefox/         # IDM-style floating video button for Firefox (WebExtension)
└── novaos/                            # NovaOS bare-metal ring-3 integration (VORTEX.ELF / VORTEX.PKG)
```

---

## Quickstart

### 1. Build & Run Tests
```bash
cargo test --workspace
cargo build --release
```

### 2. Launch Vortex Studio Desktop App
```bash
./target/release/vortex
```
- **Copy any URL in any browser** (with Clipboard Monitor ON) or press **`Cmd+V` / `Ctrl+V`**, or click **`+ Add URL`**: Fetches real video metadata, decoded thumbnail preview, duration, available stream qualities/sizes, and downloads with 1–16 parallel IDM threads to `~/Downloads/Vortex/`.
- **Click `+ Torrent`**: Browse a `.torrent` file or paste a `magnet:?` URI (supports BEP-0009 `ut_metadata` + Sequential Streaming Mode), and download via real HTTP/UDP trackers & TCP peers with live piece map & peer telemetry.
- **Demo preview mode**: `./target/release/vortex --demo`

### 3. CLI Utilities
```bash
./target/release/vortex --sniff "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
./target/release/vortex --info "magnet:?xt=urn:btih:2fd4e1c67a2d28fced849ee1bb76e7391b93eb12&dn=Example"
./target/release/vortex --list
```

### 4. Browser Integration (IDM-Style Floating Download Button)
- **Brave / Google Chrome / Microsoft Edge:** Load unpacked `/home/firoze/projects/others/vortex/browser-extension` in `brave://extensions`, `chrome://extensions`, or `edge://extensions` (see [browser-extension/README.md](browser-extension/README.md)).
- **Mozilla Firefox:** Load temporary add-on `/home/firoze/projects/others/vortex/browser-extension-firefox/manifest.json` in `about:debugging#/runtime/this-firefox`.
- **NovaOS Aero Browser:** Integrated natively on watch pages — no extension required.

### 5. Build NovaOS App Store Package (`VORTEX.ELF`, `TORRENT.ELF`, `VORTEX.PKG`)
```bash
./novaos/build.sh
```

## License
Dual-licensed under MIT or Apache-2.0.

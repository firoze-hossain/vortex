//! vortex-app/src/main.rs
//! Vortex: Unified BitTorrent Client & Intelligent Stream Accelerator Application.

pub mod png;
pub mod sample;

use std::env;
use std::fs;
use vortex_core::metainfo::TorrentMeta;
use vortex_core::sniffer::MediaSniffer;
use vortex_ui::chunk_bars::ChunkBarsView;
use vortex_ui::floating_sniffer::FloatingSnifferBadge;
use vortex_ui::geom::{Point, Rect};
use vortex_ui::piece_grid::PieceGridView;
use vortex_ui::view::VortexView;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    if args.iter().any(|a| a == "--about" || a == "-v" || a == "--version") {
        println!("================================================================================");
        println!("  VORTEX STUDIO: Unified BitTorrent & Intelligent Stream Accelerator");
        println!("  Version: 1.0.0 (Pure Rust 1.99.1, Sovereign ISO & Stream Architecture)");
        println!("================================================================================");
        println!("  * BitTorrent Swarm Protocol: BEP-0003, BEP-0009 Magnet, Rarest-First & Sequential");
        println!("  * IDM Stream Acceleration: Multi-threaded HTTP Range Segmentation (4-16 Chunks)");
        println!("  * Browser Video Sniffer: Floating Action Badge over YouTube & HTML5 Video Elements");
        println!("  * Visualizers: 2D BitTorrent Piece Grid (uTorrent) & Segmented Stream Bars (IDM)");
        println!("  * Packaging: App Store Registered (nova-store / Internet Category)");
        println!("  * Cross-Platform Desktop: macOS • Linux • Windows • NovaOS");
        return;
    }

    if args.iter().any(|a| a == "--list" || a == "-l") {
        let engine = sample::create_sample_engine();
        println!("Vortex Studio -- Active Download Queue (Total Tasks: {})", engine.tasks.len());
        println!("--------------------------------------------------------------------------------");
        for task in &engine.tasks {
            let (type_str, extra) = match task {
                vortex_core::engine::DownloadTask::Torrent(tt) => (
                    "[TORRENT]",
                    format!("Peers: {}S / {}L | Pieces: {}/{}", tt.seeders, tt.leechers, tt.piece_picker.bitfield.count_completed(), tt.piece_picker.num_pieces),
                ),
                vortex_core::engine::DownloadTask::Stream(st) => (
                    "[STREAM] ",
                    format!("Parallel Streams: {} active | Resumable: Yes", st.segmented.chunks.len()),
                ),
            };
            let speed_mb = task.download_rate_bps() as f32 / (1024.0 * 1024.0);
            let size_mb = task.total_size() as f32 / (1024.0 * 1024.0);
            println!(
                "  * {:<9} {:<42} {:>5.1}% | {:>5.1} MB/s | {:>7.1} MB | {}",
                type_str,
                task.name(),
                task.progress_pct(),
                speed_mb,
                size_mb,
                extra
            );
        }
        let total_down = engine.total_download_rate_bps() as f32 / (1024.0 * 1024.0);
        let total_up = engine.total_upload_rate_bps() as f32 / (1024.0 * 1024.0);
        println!("--------------------------------------------------------------------------------");
        println!("Aggregated Swarm Throughput: Download: {:.2} MB/s | Seeding: {:.2} MB/s", total_down, total_up);
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--sniff" || a == "-s") {
        let url = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("https://www.youtube.com/watch?v=kernel_deep_dive");
        let mut sniffer = MediaSniffer::new();
        println!("[*] Sniffing web page for video streams: {}", url);
        if let Some(media) = sniffer.inspect_url(url, url, "NovaOS Kernel Architecture Video") {
            println!("[+] Detected Video Stream: {}", media.title);
            println!("    Source: {} | Available Formats: {}", media.source_domain, media.qualities.len());
            for q in &media.qualities {
                let size_mb = q.estimated_size_bytes as f32 / (1024.0 * 1024.0);
                let kind = if q.is_audio_only { "Audio" } else { "Video" };
                println!("    - {:<10} {:<10} {:>5.1} MB ({} kbps) -> {}", kind, q.label, size_mb, q.bitrate_kbps, q.url);
            }
            println!("[+] Floating Sniffer Badge: Pop-up overlay ready beside browser video element.");
        } else {
            println!("[-] No video streams detected on page.");
        }
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--info" || a == "-i") {
        let path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("assets/sample.torrent");
        if path.starts_with("magnet:") {
            match vortex_core::magnet::Magnet::parse(path) {
                Ok(mag) => {
                    println!("Magnet Link Telemetry:");
                    println!("  * Info Hash (Hex): {}", mag.info_hash_hex);
                    println!("  * Display Name: {}", mag.display_name.as_deref().unwrap_or("None"));
                    println!("  * Trackers: {}", mag.trackers.len());
                    for tr in &mag.trackers {
                        println!("    - {}", tr);
                    }
                }
                Err(e) => eprintln!("[-] Failed to parse magnet link: {}", e),
            }
        } else if let Ok(bytes) = fs::read(path) {
            match TorrentMeta::from_bytes(&bytes) {
                Ok(t) => {
                    println!("Torrent Metainfo Inspector:");
                    println!("  * Name: {}", t.name);
                    println!("  * Info Hash (Hex): {}", t.info_hash_hex);
                    println!("  * Total Size: {:.2} MB", t.total_length() as f32 / (1024.0 * 1024.0));
                    println!("  * Pieces: {} ({} KB each)", t.piece_count(), t.piece_length / 1024);
                    println!("  * Files: {}", t.files.len());
                }
                Err(e) => eprintln!("[-] Failed to parse torrent metainfo: {}", e),
            }
        } else {
            eprintln!("[-] File not found or unreadable: {}", path);
        }
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--export-view-png") {
        let out_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("/tmp/vortex_desktop_view.png");
        let engine = sample::create_sample_engine();
        let view = VortexView::new(1024, 768);
        let mut surface = vec![0u32; 1024 * 768];
        view.render(&mut surface, &engine);

        let png_bytes = png::encode_png_argb(1024, 768, &surface);
        fs::write(out_path, png_bytes).expect("Failed to write view PNG");
        println!("[+] Successfully exported Vortex desktop view PNG to {}", out_path);
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--export-piece-grid-png") {
        let out_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("/tmp/vortex_piece_grid.png");
        let engine = sample::create_sample_engine();
        let mut surface = vec![0xFF0F172A; 800 * 400];
        if let Some(vortex_core::engine::DownloadTask::Torrent(tt)) = engine.tasks.first() {
            PieceGridView::render(
                &mut surface,
                800,
                400,
                Rect::new(20, 20, 760, 360),
                &tt.piece_picker.bitfield,
                &tt.piece_picker.states,
                tt.piece_picker.piece_length,
            );
        }
        let png_bytes = png::encode_png_argb(800, 400, &surface);
        fs::write(out_path, png_bytes).expect("Failed to write piece grid PNG");
        println!("[+] Successfully exported Piece Grid visualizer PNG to {}", out_path);
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--export-chunk-bars-png") {
        let out_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("/tmp/vortex_chunk_bars.png");
        let engine = sample::create_sample_engine();
        let mut surface = vec![0xFF0F172A; 800 * 300];
        if let Some(vortex_core::engine::DownloadTask::Stream(st)) = engine.tasks.iter().find(|t| t.is_stream()) {
            ChunkBarsView::render(
                &mut surface,
                800,
                300,
                Rect::new(20, 20, 760, 260),
                &st.segmented.chunks,
                st.total_size,
            );
        }
        let png_bytes = png::encode_png_argb(800, 300, &surface);
        fs::write(out_path, png_bytes).expect("Failed to write chunk bars PNG");
        println!("[+] Successfully exported IDM Chunk Bars visualizer PNG to {}", out_path);
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--export-sniffer-badge-png") {
        let out_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("/tmp/vortex_sniffer_badge.png");
        let mut sniffer = MediaSniffer::new();
        let detected = sniffer.inspect_url(
            "https://www.youtube.com/watch?v=kernel_deep_dive",
            "https://www.youtube.com/watch?v=kernel_deep_dive",
            "NovaOS Kernel Architecture & Systems",
        ).unwrap();

        let mut surface = vec![0xFF0B1120; 600 * 250];
        FloatingSnifferBadge::render(&mut surface, 600, 250, Point::new(40, 30), detected, true);

        let png_bytes = png::encode_png_argb(600, 250, &surface);
        fs::write(out_path, png_bytes).expect("Failed to write sniffer badge PNG");
        println!("[+] Successfully exported Floating Sniffer Badge PNG to {}", out_path);
        return;
    }

    // Launch Interactive Cross-Platform Desktop GUI
    let mut engine = sample::create_sample_engine();

    // Check if user passed a torrent file or magnet link to open
    for arg in args.iter().skip(1) {
        if !arg.starts_with('-') {
            if arg.starts_with("magnet:") {
                let _ = engine.add_magnet(arg);
                println!("[Vortex] Added Magnet URI download task: {}", arg);
            } else if let Ok(bytes) = fs::read(arg) {
                if let Ok(meta) = TorrentMeta::from_bytes(&bytes) {
                    let tid = engine.add_torrent(meta);
                    println!("[Vortex] Added Torrent file download task: {} (ID: {})", arg, tid);
                }
            }
        }
    }

    run_desktop_gui(engine);
}

fn run_desktop_gui(mut engine: vortex_core::engine::VortexEngine) {
    let mut sw = 1024;
    let mut sh = 768;

    let mut view = VortexView::new(sw, sh);

    let mut window = match minifb::Window::new(
        "Vortex Studio - BitTorrent Swarm & Intelligent Stream Accelerator",
        sw,
        sh,
        minifb::WindowOptions {
            resize: true,
            ..minifb::WindowOptions::default()
        },
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[Vortex] Note: Native window could not be opened ({}). Rendering offscreen...", e);
            let mut surface = vec![0u32; sw * sh];
            view.render(&mut surface, &engine);
            let non_zero = surface.iter().filter(|&&p| p != 0).count();
            println!("[Vortex] Headless offscreen render complete: {} non-zero pixels.", non_zero);
            return;
        }
    };

    window.set_target_fps(60);
    println!("[Vortex] Native cross-platform desktop window created ({}x{}).", sw, sh);

    let mut surface = vec![0u32; sw * sh];
    let mut mouse_down = false;
    let mut last_tick = std::time::Instant::now();

    while window.is_open() && !window.is_key_down(minifb::Key::Escape) && !window.is_key_down(minifb::Key::Q) {
        // 1. Dynamic Window Resizing
        let (cur_w, cur_h) = window.get_size();
        if (cur_w != sw || cur_h != sh) && cur_w >= 320 && cur_h >= 240 {
            sw = cur_w;
            sh = cur_h;
            view = VortexView::new(sw, sh);
            surface = vec![0u32; sw * sh];
        }

        // 2. Mouse Input Handling
        if let Some((mx, my)) = window.get_mouse_pos(minifb::MouseMode::Discard) {
            let px = mx as i32;
            let py = my as i32;
            let is_down = window.get_mouse_down(minifb::MouseButton::Left);

            if is_down && !mouse_down {
                mouse_down = true;
                view.handle_click(px, py, &mut engine);
            } else if !is_down && mouse_down {
                mouse_down = false;
            }
        }

        // 3. Engine simulation tick (~60fps)
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(last_tick);
        let delta_ms = elapsed.as_millis().max(1) as u64;
        last_tick = now;
        engine.tick(delta_ms);

        // 4. Render & Present
        view.render(&mut surface, &engine);
        if let Err(e) = window.update_with_buffer(&surface, sw, sh) {
            eprintln!("[Vortex] Window update error: {}", e);
            break;
        }
    }

    println!("[Vortex] Session ended cleanly.");
}

fn print_help() {
    println!("Vortex Studio -- Unified BitTorrent Client & Intelligent Stream Accelerator");
    println!("Usage: vortex [OPTIONS] [FILE.torrent | magnet:...]");
    println!();
    println!("Options:");
    println!("  --list, -l                       List active BitTorrent swarms and IDM video streams");
    println!("  --sniff, -s <url>                Sniff web video page and display floating download options");
    println!("  --info, -i <torrent | magnet>    Inspect torrent metadata or magnet URI parameters");
    println!("  --export-view-png <path>         Render full desktop dashboard to 1024x768 PNG");
    println!("  --export-piece-grid-png <path>   Render uTorrent/Transmission piece map visualizer");
    println!("  --export-chunk-bars-png <path>   Render IDM multi-thread segmented progress bars");
    println!("  --export-sniffer-badge-png <path>Render browser floating video download badge");
    println!("  --about, -v                      Display architecture details and telemetry");
    println!("  --help, -h                       Display this help reference");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_engine_and_png_export() {
        let engine = sample::create_sample_engine();
        assert_eq!(engine.tasks.len(), 4);

        let view = VortexView::new(800, 600);
        let mut surface = vec![0u32; 800 * 600];
        view.render(&mut surface, &engine);

        let png_bytes = png::encode_png_argb(800, 600, &surface);
        assert!(!png_bytes.is_empty());
        assert_eq!(&png_bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }
}

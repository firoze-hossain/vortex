//! vortex-app/src/main.rs
//! Vortex Studio: Cross-Platform BitTorrent Client, IDM Multi-Thread Video/File Accelerator,
//! Real URL Video Previewer, and Browser Extension Bridge.

pub mod clipboard;
pub mod png;
pub mod sample;

use std::env;
use std::fs;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use softbuffer::{Context as SbContext, Surface as SbSurface};
use vortex_core::bridge::start_bridge_server;
use vortex_core::engine::VortexEngine;
use vortex_core::media::preview::{fetch_url_preview, MediaPreview};
use vortex_core::metainfo::TorrentMeta;
use vortex_core::sniffer::MediaSniffer;
use vortex_ui::chunk_bars::ChunkBarsView;
use vortex_ui::floating_sniffer::FloatingSnifferBadge;
use vortex_ui::geom::{Point, Rect};
use vortex_ui::modals::{ActiveModal, AddTorrentModalState, ModalAction, PreviewFetchState};
use vortex_ui::piece_grid::PieceGridView;
use vortex_ui::view::{ViewCommand, VortexView};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    if args.iter().any(|a| a == "--about" || a == "-v" || a == "--version") {
        println!("================================================================================");
        println!("  VORTEX STUDIO v2.0.0: BitTorrent Swarm & Intelligent Stream Accelerator");
        println!("================================================================================");
        println!("  * BitTorrent Engine: HTTP/UDP Trackers, Peer Wire Protocol, SHA-1, Multi-File");
        println!("  * IDM Stream Acceleration: Multi-threaded HTTP Range (1-16 threads) & HLS .m3u8");
        println!("  * Live URL Preview: Real metadata, thumbnail (PNG/JPEG), formats & sizes");
        println!("  * Browser Bridge: Local HTTP/CORS Pure-Rust IPC on 127.0.0.1:47391");
        println!("  * Clipboard Auto-Capture: Watches copied URLs & Magnets across any browser");
        println!("  * Cross-Platform Desktop: macOS | Linux | Windows | NovaOS");
        return;
    }

    if args.iter().any(|a| a == "--list" || a == "-l") {
        let mut engine = VortexEngine::new();
        engine.load_persisted();
        if engine.tasks.is_empty() && args.iter().any(|a| a == "--demo") {
            engine = sample::create_sample_engine();
        }
        println!("Vortex Studio -- Queue (Total Tasks: {})", engine.tasks.len());
        println!("--------------------------------------------------------------------------------");
        for task in &engine.tasks {
            let speed_mb = task.download_rate_bps() as f32 / (1024.0 * 1024.0);
            let size_mb = task.total_size() as f32 / (1024.0 * 1024.0);
            println!(
                "  * #{:<3} {:<9} {:<40} {:>5.1}% | {:>5.1} MB/s | {:>7.1} MB | {}",
                task.id(),
                if task.is_torrent() { "[TORRENT]" } else { "[STREAM]" },
                task.name(),
                task.progress_pct(),
                speed_mb,
                size_mb,
                task.status().label(),
            );
        }
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--sniff" || a == "-s") {
        let Some(url) = args.get(pos + 1) else {
            eprintln!("Usage: vortex --sniff <url>");
            return;
        };
        println!("[*] Resolving live preview for: {}", url);
        match fetch_url_preview(url) {
            Ok(preview) => {
                println!("[+] Title: {}", preview.title);
                println!("[+] Domain: {} | Kind: {:?}", preview.source_domain, preview.kind);
                if let Some(a) = &preview.author {
                    println!("[+] Author: {}", a);
                }
                if let Some(d) = preview.duration_secs {
                    println!("[+] Duration: {}:{:02}", d / 60, d % 60);
                }
                println!("[+] Save As: {} | Range Resumable: {}", preview.suggested_filename, preview.resumable);
                println!("[+] Formats ({}):", preview.qualities.len());
                for q in &preview.qualities {
                    let size_mb = q.estimated_size_bytes as f32 / (1024.0 * 1024.0);
                    println!("    - {:<24} {:<18} {:>6.1} MB -> {}", q.label, q.mime_type, size_mb, q.url);
                }
                if let Some(note) = &preview.note {
                    println!("[i] {}", note);
                }
            }
            Err(e) => eprintln!("[-] Preview failed: {}", e),
        }
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--info" || a == "-i") {
        let Some(path) = args.get(pos + 1) else {
            eprintln!("Usage: vortex --info <file.torrent | magnet:...>");
            return;
        };
        if path.starts_with("magnet:") {
            match vortex_core::magnet::Magnet::parse(path) {
                Ok(mag) => {
                    println!("Magnet Link Telemetry:");
                    println!("  * Info Hash: {}", mag.info_hash_hex);
                    println!("  * Display Name: {}", mag.display_name.as_deref().unwrap_or("None"));
                    for tr in &mag.trackers {
                        println!("  * Tracker: {}", tr);
                    }
                }
                Err(e) => eprintln!("[-] Failed to parse magnet link: {}", e),
            }
        } else if let Ok(bytes) = fs::read(path) {
            match TorrentMeta::from_bytes(&bytes) {
                Ok(t) => {
                    println!("Torrent Metainfo Inspector:");
                    println!("  * Name: {}", t.name);
                    println!("  * Info Hash: {}", t.info_hash_hex);
                    println!("  * Total Size: {:.2} MB", t.total_length() as f32 / (1024.0 * 1024.0));
                    println!("  * Pieces: {} ({} KB each)", t.piece_count(), t.piece_length / 1024);
                    println!("  * Files: {}", t.files.len());
                }
                Err(e) => eprintln!("[-] Failed to parse torrent metainfo: {}", e),
            }
        } else {
            eprintln!("[-] File not found: {}", path);
        }
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--export-view-png") {
        let out_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("/tmp/vortex_desktop_view.png");
        let engine = sample::create_sample_engine();
        let view = VortexView::new(1080, 760);
        let mut surface = vec![0u32; 1080 * 760];
        view.render(&mut surface, &engine);
        let png_bytes = png::encode_png_argb(1080, 760, &surface);
        fs::write(out_path, png_bytes).expect("Failed to write view PNG");
        println!("[+] Exported Vortex desktop view PNG to {}", out_path);
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
        fs::write(out_path, png::encode_png_argb(800, 400, &surface)).expect("Failed to write PNG");
        println!("[+] Exported Piece Grid PNG to {}", out_path);
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--export-chunk-bars-png") {
        let out_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("/tmp/vortex_chunk_bars.png");
        let engine = sample::create_sample_engine();
        let mut surface = vec![0xFF0F172A; 800 * 300];
        if let Some(vortex_core::engine::DownloadTask::Stream(st)) = engine.tasks.iter().find(|t| t.is_stream()) {
            ChunkBarsView::render(&mut surface, 800, 300, Rect::new(20, 20, 760, 260), &st.segmented.chunks, st.total_size);
        }
        fs::write(out_path, png::encode_png_argb(800, 300, &surface)).expect("Failed to write PNG");
        println!("[+] Exported Chunk Bars PNG to {}", out_path);
        return;
    }

    if let Some(pos) = args.iter().position(|a| a == "--export-sniffer-badge-png") {
        let out_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("/tmp/vortex_sniffer_badge.png");
        let mut sniffer = MediaSniffer::new();
        let detected = sniffer
            .inspect_url(
                "https://www.youtube.com/watch?v=kernel_deep_dive",
                "https://www.youtube.com/watch?v=kernel_deep_dive",
                "NovaOS Kernel Architecture & Systems",
            )
            .unwrap();
        let mut surface = vec![0xFF0B1120; 600 * 250];
        FloatingSnifferBadge::render(&mut surface, 600, 250, Point::new(40, 30), detected, true);
        fs::write(out_path, png::encode_png_argb(600, 250, &surface)).expect("Failed to write PNG");
        println!("[+] Exported Floating Sniffer Badge PNG to {}", out_path);
        return;
    }

    // Launch real interactive engine (loads persisted session unless --demo)
    let is_demo = args.iter().any(|a| a == "--demo");
    let mut engine = if is_demo {
        sample::create_sample_engine()
    } else {
        let mut e = VortexEngine::new();
        e.load_persisted();
        e
    };

    // Process CLI positional URLs, magnets, or .torrent files
    for arg in args.iter().skip(1) {
        if !arg.starts_with('-') {
            if arg.starts_with("magnet:") {
                if let Ok(id) = engine.start_real_magnet(arg, None, false) {
                    println!("[Vortex] Started Magnet task #{}", id);
                }
            } else if arg.starts_with("http://") || arg.starts_with("https://") {
                let fname = vortex_core::network::http::filename_from_url(arg);
                let threads = engine.config.default_threads;
                if let Ok(id) = engine.start_real_stream(&fname, arg, "Direct", 0, threads, arg.contains(".m3u8"), None, None) {
                    println!("[Vortex] Started HTTP/Stream task #{} ({})", id, fname);
                }
            } else if let Ok(bytes) = fs::read(arg) {
                if let Ok(meta) = TorrentMeta::from_bytes(&bytes) {
                    if let Ok(id) = engine.start_real_torrent(meta, None, false) {
                        println!("[Vortex] Started Torrent task #{} from {}", id, arg);
                    }
                }
            }
        }
    }

    run_desktop_gui(engine, is_demo);
}

struct VortexDesktopApp {
    engine_arc: Arc<Mutex<VortexEngine>>,
    is_demo: bool,
    view: VortexView,
    pixel_buf: Vec<u32>,
    window: Option<Arc<Window>>,
    sb_context: Option<SbContext<Arc<Window>>>,
    sb_surface: Option<SbSurface<Arc<Window>, Arc<Window>>>,
    cursor_pos: (i32, i32),
    modifiers: ModifiersState,
    preview_tx: mpsc::Sender<Result<MediaPreview, String>>,
    preview_rx: mpsc::Receiver<Result<MediaPreview, String>>,
    clip_rx: mpsc::Receiver<String>,
    clip_enabled: Arc<AtomicBool>,
    auto_preview_pending: Option<(String, Instant)>,
    last_auto_preview_url: String,
    last_tick: Instant,
    last_save: Instant,
}

impl VortexDesktopApp {
    fn trigger_url_preview(&mut self, url: String) {
        self.last_auto_preview_url = url.clone();
        self.auto_preview_pending = None;
        if let ActiveModal::AddUrl(st) = &mut self.view.modal {
            st.url_input = url.clone();
            st.preview_state = PreviewFetchState::Loading(url.clone());
        } else {
            self.view.open_add_url_with_text(url.clone());
            if let ActiveModal::AddUrl(st) = &mut self.view.modal {
                st.preview_state = PreviewFetchState::Loading(url.clone());
            }
        }
        spawn_preview_fetch(url, self.preview_tx.clone());
    }

    fn apply_clipboard_paste(&mut self) {
        let Some(clip) = clipboard::read_clipboard_text() else {
            if let ActiveModal::AddUrl(st) = &mut self.view.modal {
                st.preview_state = PreviewFetchState::Error(
                    "Clipboard is empty or restricted. Copy URL again or type/Enter above.".into(),
                );
            } else if let ActiveModal::AddTorrent(st) = &mut self.view.modal {
                st.error = Some("Clipboard is empty. Copy a magnet URI or browse .torrent file.".into());
            }
            return;
        };

        match &mut self.view.modal {
            ActiveModal::AddUrl(_) => {
                self.trigger_url_preview(clip);
            }
            ActiveModal::AddTorrent(st) => {
                st.input = clip;
                st.error = None;
            }
            ActiveModal::None => {
                if clip.starts_with("http://") || clip.starts_with("https://") {
                    self.trigger_url_preview(clip);
                } else if clip.starts_with("magnet:?") {
                    let mut st = AddTorrentModalState::default();
                    st.input = clip;
                    self.view.modal = ActiveModal::AddTorrent(st);
                }
            }
            _ => {}
        }
    }

    fn schedule_debounced_preview(&mut self) {
        if let ActiveModal::AddUrl(st) = &self.view.modal {
            let trimmed = st.url_input.trim().to_string();
            if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
                && trimmed.len() >= 12
                && trimmed.contains('.')
            {
                self.auto_preview_pending = Some((trimmed, Instant::now()));
            } else {
                self.auto_preview_pending = None;
            }
        }
    }

    fn tick_background(&mut self) {
        self.view.frame_counter = self.view.frame_counter.wrapping_add(1);

        while let Ok(res) = self.preview_rx.try_recv() {
            if let ActiveModal::AddUrl(st) = &mut self.view.modal {
                st.set_preview_result(res);
            }
        }

        while let Ok(clip_url) = self.clip_rx.try_recv() {
            if matches!(self.view.modal, ActiveModal::None) {
                if clip_url.starts_with("magnet:?") {
                    let mut st = AddTorrentModalState::default();
                    st.input = clip_url;
                    self.view.modal = ActiveModal::AddTorrent(st);
                } else {
                    self.trigger_url_preview(clip_url);
                }
            }
        }

        if let Some((pending_url, edited_at)) = &self.auto_preview_pending {
            if edited_at.elapsed() >= Duration::from_millis(420)
                && pending_url != &self.last_auto_preview_url
            {
                let u = pending_url.clone();
                if matches!(self.view.modal, ActiveModal::AddUrl(_)) {
                    self.trigger_url_preview(u);
                } else {
                    self.auto_preview_pending = None;
                }
            }
        }

        let now = Instant::now();
        let delta_ms = now.duration_since(self.last_tick).as_millis().max(1) as u64;
        self.last_tick = now;

        let mut eng = self.engine_arc.lock().unwrap();
        self.clip_enabled
            .store(eng.config.clipboard_monitor, Ordering::Relaxed);
        eng.tick(delta_ms);
        if self.is_demo {
            eng.tick_demo();
        }
        if !self.is_demo && eng.take_dirty() && self.last_save.elapsed() >= Duration::from_secs(2) {
            eng.save_persisted();
            self.last_save = Instant::now();
        }
    }

    fn toggle_fullscreen(&self) {
        if let Some(win) = &self.window {
            if win.fullscreen().is_some() {
                win.set_fullscreen(None);
            } else {
                win.set_fullscreen(Some(Fullscreen::Borderless(None)));
            }
        }
    }

    fn render_frame(&mut self) {
        let Some(win) = self.window.as_ref().map(Arc::clone) else {
            return;
        };
        let Some(sb_surface) = self.sb_surface.as_mut() else {
            return;
        };

        let size = win.inner_size();
        let sw = size.width as usize;
        let sh = size.height as usize;
        if sw == 0 || sh == 0 {
            return;
        }
        let (Some(w_nz), Some(h_nz)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };

        if sb_surface.resize(w_nz, h_nz).is_err() {
            return;
        }
        self.view.width = sw;
        self.view.height = sh;
        self.view.is_maximized = win.is_maximized();
        self.view.is_fullscreen = win.fullscreen().is_some();

        if self.pixel_buf.len() != sw * sh {
            self.pixel_buf.resize(sw * sh, 0);
        }

        {
            let eng = self.engine_arc.lock().unwrap();
            self.view.render(&mut self.pixel_buf, &eng);
        }

        if let Ok(mut buf) = sb_surface.buffer_mut() {
            if buf.len() == self.pixel_buf.len() {
                buf.copy_from_slice(&self.pixel_buf);
                let _ = buf.present();
            }
        }
    }
}

impl ApplicationHandler for VortexDesktopApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Vortex Studio - BitTorrent Swarm & Intelligent Stream Accelerator")
            .with_inner_size(LogicalSize::new(1080.0, 760.0))
            .with_min_inner_size(LogicalSize::new(640.0, 460.0));

        match event_loop.create_window(attrs) {
            Ok(win) => {
                let win_arc = Arc::new(win);
                if let Ok(ctx) = SbContext::new(Arc::clone(&win_arc)) {
                    if let Ok(surf) = SbSurface::new(&ctx, Arc::clone(&win_arc)) {
                        self.sb_context = Some(ctx);
                        self.sb_surface = Some(surf);
                    }
                }
                win_arc.request_redraw();
                self.window = Some(win_arc);
            }
            Err(e) => {
                eprintln!("[Vortex] Failed to create window ({}). Exiting cleanly.", e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x.round() as i32, position.y.round() as i32);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let sy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => (pos.y / 24.0) as f32,
                };
                let eng = self.engine_arc.lock().unwrap();
                let total = eng.filtered_tasks(self.view.active_filter).len();
                if sy < -0.1 && self.view.scroll_offset + 1 < total {
                    self.view.scroll_offset += 1;
                } else if sy > 0.1 && self.view.scroll_offset > 0 {
                    self.view.scroll_offset -= 1;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let (px, py) = self.cursor_pos;
                let was_modal = !matches!(self.view.modal, ActiveModal::None);
                let cmd = {
                    let mut eng = self.engine_arc.lock().unwrap();
                    self.view.handle_click_with_actions(px, py, &mut eng)
                };
                match cmd {
                    ViewCommand::Quit => event_loop.exit(),
                    ViewCommand::Minimize => {
                        if let Some(win) = &self.window {
                            win.set_minimized(true);
                        }
                    }
                    ViewCommand::ToggleMaximize => {
                        if let Some(win) = &self.window {
                            win.set_maximized(!win.is_maximized());
                        }
                    }
                    ViewCommand::ToggleFullscreen => self.toggle_fullscreen(),
                    ViewCommand::OpenFolder(path) => clipboard::open_in_file_manager(&path),
                    ViewCommand::Modal(ModalAction::RequestPasteUrl)
                    | ViewCommand::Modal(ModalAction::RequestPasteTorrent) => {
                        self.apply_clipboard_paste();
                    }
                    ViewCommand::Modal(act) => {
                        process_modal_action(act, &mut self.view, &self.engine_arc, &self.preview_tx);
                    }
                    ViewCommand::None => {
                        // Drag window by empty top toolbar strip (outside toolbar buttons)
                        if !was_modal && py >= 0 && py <= 48 && px > 680 && px < (self.view.width as i32) - 168 {
                            if let Some(win) = &self.window {
                                let _ = win.drag_window();
                            }
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let cmd_or_ctrl = self.modifiers.control_key() || self.modifiers.super_key();

                if matches!(event.logical_key, Key::Named(NamedKey::F11)) {
                    self.toggle_fullscreen();
                    return;
                }
                if cmd_or_ctrl && matches!(event.logical_key.as_ref(), Key::Character(c) if c.eq_ignore_ascii_case("q")) {
                    event_loop.exit();
                    return;
                }
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    if !matches!(self.view.modal, ActiveModal::None) {
                        self.view.modal = ActiveModal::None;
                        self.view.show_about = false;
                        self.auto_preview_pending = None;
                    } else if self.window.as_ref().is_some_and(|w| w.fullscreen().is_some()) {
                        self.toggle_fullscreen();
                    }
                    return;
                }

                let paste_hotkey = (cmd_or_ctrl
                    && matches!(event.logical_key.as_ref(), Key::Character(c) if c.eq_ignore_ascii_case("v")))
                    || event.text.as_deref() == Some("\x16");
                if paste_hotkey {
                    self.apply_clipboard_paste();
                    return;
                }

                match &mut self.view.modal {
                    ActiveModal::AddUrl(st) => {
                        let mut edited = false;
                        if matches!(event.logical_key, Key::Named(NamedKey::Backspace)) {
                            if cmd_or_ctrl {
                                st.url_input.clear();
                            } else {
                                st.url_input.pop();
                            }
                            edited = true;
                        } else if matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
                            let u = st.url_input.trim().to_string();
                            if !u.is_empty() {
                                self.trigger_url_preview(u);
                            }
                            return;
                        } else if !cmd_or_ctrl {
                            if let Some(txt) = &event.text {
                                for c in txt.chars() {
                                    if !c.is_control() {
                                        st.url_input.push(c);
                                        edited = true;
                                    }
                                }
                            }
                        }
                        if edited {
                            self.schedule_debounced_preview();
                        }
                    }
                    ActiveModal::AddTorrent(st) => {
                        if matches!(event.logical_key, Key::Named(NamedKey::Backspace)) {
                            if cmd_or_ctrl {
                                st.input.clear();
                            } else {
                                st.input.pop();
                            }
                            st.error = None;
                        } else if matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
                            let inp = st.input.trim().to_string();
                            if !inp.is_empty() {
                                let seq = st.sequential;
                                let mut eng = self.engine_arc.lock().unwrap();
                                let dest_dir = st.custom_save_dir.clone();
                                match start_torrent_from_input(&mut eng, &inp, seq, dest_dir) {
                                    Ok(id) => {
                                        self.view.selected_task_id = Some(id);
                                        self.view.modal = ActiveModal::None;
                                    }
                                    Err(e) => st.error = Some(e),
                                }
                            }
                        } else if !cmd_or_ctrl {
                            if let Some(txt) = &event.text {
                                for c in txt.chars() {
                                    if !c.is_control() {
                                        st.input.push(c);
                                        st.error = None;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.render_frame();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.tick_background();
        if let Some(win) = &self.window {
            win.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
    }
}

fn run_desktop_gui(engine_init: VortexEngine, is_demo: bool) {
    let bridge_port = engine_init.config.bridge_port;
    let engine_arc = Arc::new(Mutex::new(engine_init));

    let bridge_handle = match start_bridge_server(Arc::clone(&engine_arc), bridge_port) {
        Ok(h) => {
            println!("[Vortex] Browser Extension Bridge listening on http://127.0.0.1:{}", bridge_port);
            Some(h)
        }
        Err(e) => {
            eprintln!("[Vortex] Warning: Browser Bridge unavailable ({})", e);
            None
        }
    };

    let (preview_tx, preview_rx) = mpsc::channel::<Result<MediaPreview, String>>();
    let (clip_tx, clip_rx) = mpsc::channel::<String>();
    let clip_enabled = Arc::new(AtomicBool::new(
        engine_arc.lock().unwrap().config.clipboard_monitor,
    ));
    let clip_stop = Arc::new(AtomicBool::new(false));
    {
        let en = Arc::clone(&clip_enabled);
        let stp = Arc::clone(&clip_stop);
        let initial_clip = clipboard::read_clipboard_text().unwrap_or_default();
        thread::Builder::new()
            .name("vortex-clipboard-watch".into())
            .spawn(move || {
                let mut last_seen = initial_clip;
                while !stp.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(900));
                    if !en.load(Ordering::Relaxed) {
                        continue;
                    }
                    if let Some(txt) = clipboard::read_clipboard_text() {
                        if txt != last_seen {
                            last_seen = txt.clone();
                            let trimmed = txt.trim();
                            if trimmed.starts_with("http://")
                                || trimmed.starts_with("https://")
                                || trimmed.starts_with("magnet:?")
                            {
                                let _ = clip_tx.send(trimmed.to_string());
                            }
                        }
                    }
                }
            })
            .ok();
    }

    match EventLoop::new() {
        Ok(event_loop) => {
            let mut app = VortexDesktopApp {
                engine_arc: Arc::clone(&engine_arc),
                is_demo,
                view: VortexView::new(1080, 760),
                pixel_buf: vec![0u32; 1080 * 760],
                window: None,
                sb_context: None,
                sb_surface: None,
                cursor_pos: (0, 0),
                modifiers: ModifiersState::default(),
                preview_tx,
                preview_rx,
                clip_rx,
                clip_enabled,
                auto_preview_pending: None,
                last_auto_preview_url: String::new(),
                last_tick: Instant::now(),
                last_save: Instant::now(),
            };
            if let Err(e) = event_loop.run_app(&mut app) {
                eprintln!("[Vortex] Event loop error: {}", e);
            }
        }
        Err(e) => {
            eprintln!("[Vortex] Headless mode ({}). Rendering offscreen...", e);
            let view = VortexView::new(1080, 760);
            let mut surface = vec![0u32; 1080 * 760];
            let eng = engine_arc.lock().unwrap();
            view.render(&mut surface, &eng);
        }
    }

    clip_stop.store(true, Ordering::Relaxed);
    if !is_demo {
        let mut eng = engine_arc.lock().unwrap();
        eng.save_persisted();
    }
    if let Some(h) = bridge_handle {
        h.stop();
    }
    println!("[Vortex] Session ended cleanly.");
}

fn process_modal_action(
    act: ModalAction,
    view: &mut VortexView,
    engine_arc: &Arc<Mutex<VortexEngine>>,
    preview_tx: &mpsc::Sender<Result<MediaPreview, String>>,
) {
    match act {
        ModalAction::RequestBrowseTorrent => {
            if let Some(path) = clipboard::pick_torrent_file() {
                if let ActiveModal::AddTorrent(st) = &mut view.modal {
                    st.input = path.to_string_lossy().to_string();
                    st.error = None;
                }
            }
        }
        ModalAction::ChooseDefaultDownloadFolder => {
            let current_dir = engine_arc.lock().unwrap().config.download_dir.clone();
            if let Some(dir) = clipboard::pick_download_folder(Some(&current_dir)) {
                let target = normalize_vortex_download_dir(dir);
                let mut eng = engine_arc.lock().unwrap();
                eng.config.download_dir = target;
                eng.save_persisted();
            }
        }
        ModalAction::ChooseModalSaveFolder => {
            let current_dir = match &view.modal {
                ActiveModal::AddUrl(st) => st.custom_save_dir.clone(),
                ActiveModal::AddTorrent(st) => st.custom_save_dir.clone(),
                _ => None,
            }
            .unwrap_or_else(|| engine_arc.lock().unwrap().config.download_dir.clone());
            if let Some(dir) = clipboard::pick_download_folder(Some(&current_dir)) {
                let target = normalize_vortex_download_dir(dir);
                match &mut view.modal {
                    ActiveModal::AddUrl(st) => st.custom_save_dir = Some(target),
                    ActiveModal::AddTorrent(st) => st.custom_save_dir = Some(target),
                    _ => {}
                }
            }
        }
        ModalAction::PlayMediaFile(path) => {
            clipboard::open_media_file(&path);
        }
        ModalAction::FetchPreview(url) => {
            spawn_preview_fetch(url, preview_tx.clone());
        }
        ModalAction::StartStream {
            url,
            filename,
            resolution,
            size,
            threads,
            is_hls,
            referer,
            dest_dir,
            audio_url,
        } => {
            let cached_thumb = match &view.modal {
                ActiveModal::AddUrl(st) => match &st.preview_state {
                    PreviewFetchState::Ready { thumbnail, .. } => thumbnail.clone(),
                    _ => None,
                },
                _ => None,
            };
            let mut eng = engine_arc.lock().unwrap();
            match eng.start_real_stream_with_audio(
                &filename,
                &url,
                &resolution,
                size,
                threads,
                is_hls,
                referer,
                dest_dir,
                audio_url,
            ) {
                Ok(id) => {
                    if let Some(img) = cached_thumb {
                        view.thumb_cache.insert(id, img);
                    }
                    view.selected_task_id = Some(id);
                    view.modal = ActiveModal::None;
                }
                Err(e) => {
                    if let ActiveModal::AddUrl(st) = &mut view.modal {
                        st.preview_state = PreviewFetchState::Error(e);
                    }
                }
            }
        }
        ModalAction::StartTorrentOrMagnet {
            input,
            sequential,
            dest_dir,
        } => {
            let mut eng = engine_arc.lock().unwrap();
            match start_torrent_from_input(&mut eng, &input, sequential, dest_dir) {
                Ok(id) => {
                    view.selected_task_id = Some(id);
                    view.modal = ActiveModal::None;
                }
                Err(e) => {
                    if let ActiveModal::AddTorrent(st) = &mut view.modal {
                        st.error = Some(e);
                    } else if let ActiveModal::AddUrl(st) = &mut view.modal {
                        st.preview_state = PreviewFetchState::Error(e);
                    }
                }
            }
        }
        _ => {}
    }
}

fn normalize_vortex_download_dir(selected: PathBuf) -> PathBuf {
    let already_vortex = selected
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("vortex"));
    let target = if already_vortex {
        selected
    } else {
        selected.join("Vortex")
    };
    let _ = fs::create_dir_all(&target);
    target
}

fn spawn_preview_fetch(url: String, tx: mpsc::Sender<Result<MediaPreview, String>>) {
    thread::Builder::new()
        .name("vortex-preview".into())
        .spawn(move || {
            let res = fetch_url_preview(&url);
            let _ = tx.send(res);
        })
        .ok();
}

fn start_torrent_from_input(
    engine: &mut VortexEngine,
    input: &str,
    sequential: bool,
    dest_dir: Option<PathBuf>,
) -> Result<u64, String> {
    let trimmed = input.trim();
    if trimmed.starts_with("magnet:?") {
        engine.start_real_magnet(trimmed, dest_dir, sequential)
    } else {
        let bytes = fs::read(trimmed).map_err(|e| format!("Cannot read '{}': {}", trimmed, e))?;
        let meta = TorrentMeta::from_bytes(&bytes)?;
        engine.start_real_torrent(meta, dest_dir, sequential)
    }
}

fn print_help() {
    println!("Vortex Studio v2.0 -- Unified BitTorrent Client & Intelligent Stream Accelerator");
    println!("Usage: vortex [OPTIONS] [URL | FILE.torrent | magnet:...]");
    println!();
    println!("Options:");
    println!("  --demo                           Launch GUI with sample demo items");
    println!("  --list, -l                       List active BitTorrent swarms and stream downloads");
    println!("  --sniff, -s <url>                Fetch real video/file preview metadata for a URL");
    println!("  --info, -i <torrent | magnet>    Inspect torrent metadata or magnet URI");
    println!("  --export-view-png <path>         Render desktop dashboard snapshot to PNG");
    println!("  --export-piece-grid-png <path>   Render piece map visualizer snapshot to PNG");
    println!("  --export-chunk-bars-png <path>   Render IDM multi-thread bars snapshot to PNG");
    println!("  --export-sniffer-badge-png <path>Render floating browser sniffer badge PNG");
    println!("  --about, -v                      Display architecture details");
    println!("  --help, -h                       Display this help reference");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_engine_and_png_export() {
        let engine = sample::create_sample_engine();
        assert_eq!(engine.tasks.len(), 4);

        let view = VortexView::new(1080, 760);
        let mut surface = vec![0u32; 1080 * 760];
        view.render(&mut surface, &engine);

        let png_bytes = png::encode_png_argb(1080, 760, &surface);
        assert!(!png_bytes.is_empty());
        assert_eq!(&png_bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    #[test]
    fn test_normalize_vortex_download_dir_appends_subfolder() {
        let base = std::env::temp_dir().join("vortex_dir_test_root");
        let docs = base.join("Documents");
        let resolved = normalize_vortex_download_dir(docs.clone());
        assert_eq!(resolved, docs.join("Vortex"));
        assert!(resolved.exists());

        // Selecting an existing Vortex folder does not nest Vortex/Vortex
        let resolved_again = normalize_vortex_download_dir(resolved.clone());
        assert_eq!(resolved_again, resolved);
        let _ = fs::remove_dir_all(&base);
    }
}

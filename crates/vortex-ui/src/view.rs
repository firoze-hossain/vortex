//! vortex-ui/src/view.rs
//! Main Desktop Window View for Vortex Studio.

use crate::chunk_bars::ChunkBarsView;
use crate::draw::{draw_progress_bar, draw_rect_outline, fill_rect, fill_rounded_rect};
use crate::font::Font;
use crate::geom::Rect;
use crate::piece_grid::PieceGridView;
use vortex_core::engine::{DownloadTask, TaskFilter, TaskStatus, VortexEngine};

pub struct VortexView {
    pub selected_task_id: Option<u64>,
    pub active_filter: TaskFilter,
    pub inspector_tab: usize, // 0: Visualizer (Piece Grid or Chunk Bars), 1: Peers & Streams, 2: Info
    pub show_about: bool,
    pub width: usize,
    pub height: usize,
}

impl Default for VortexView {
    fn default() -> Self {
        Self::new(1024, 768)
    }
}

impl VortexView {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            selected_task_id: None,
            active_filter: TaskFilter::All,
            inspector_tab: 0,
            show_about: false,
            width,
            height,
        }
    }

    /// Renders the complete Vortex desktop studio window into a 32-bit ARGB surface.
    pub fn render(&self, surface: &mut [u32], engine: &VortexEngine) {
        if surface.is_empty() || self.width == 0 || self.height == 0 {
            return;
        }

        // 1. Background fill
        surface.fill(0xFF0F172A); // Dark slate

        let surf_w = self.width;
        let surf_h = self.height;

        // 2. Top Toolbar (y: 0..46)
        let toolbar_rect = Rect::new(0, 0, surf_w as i32, 46);
        fill_rect(surface, surf_w, surf_h, toolbar_rect, 0xFF1E293B);
        fill_rect(surface, surf_w, surf_h, Rect::new(0, 45, surf_w as i32, 1), 0xFF334155);

        // App Logo & Title
        Font::draw_text(surface, surf_w, surf_h, 14, 16, "VORTEX", 0xFF38BDF8);
        Font::draw_text(surface, surf_w, surf_h, 72, 16, "STUDIO", 0xFFE2E8F0);

        // Action Buttons
        let btns = [
            ("+ Torrent", 140, 0xFF0284C7),
            ("+ Add URL", 230, 0xFF0284C7),
            ("Resume", 320, 0xFF10B981),
            ("Pause", 390, 0xFFF59E0B),
            ("Delete", 450, 0xFFEF4444),
        ];

        for &(lbl, bx, col) in &btns {
            let br = Rect::new(bx, 10, 80, 26);
            fill_rounded_rect(surface, surf_w, surf_h, br, 4, 0xFF0F172A);
            draw_rect_outline(surface, surf_w, surf_h, br, 1, col);
            Font::draw_text(surface, surf_w, surf_h, bx + 10, 16, lbl, 0xFFF8FAFC);
        }

        // Global Speed HUD (Right-aligned)
        let total_down = engine.total_download_rate_bps() as f32 / (1024.0 * 1024.0);
        let total_up = engine.total_upload_rate_bps() as f32 / (1024.0 * 1024.0);
        let speed_txt = format!("v {:.1} MB/s  ^ {:.1} MB/s", total_down, total_up);
        let speed_x = (surf_w as i32) - 260;
        Font::draw_text(surface, surf_w, surf_h, speed_x, 16, &speed_txt, 0xFF38BDF8);

        // About Button [?]
        let about_rect = Rect::new((surf_w as i32) - 40, 10, 28, 26);
        fill_rounded_rect(surface, surf_w, surf_h, about_rect, 4, 0xFF334155);
        Font::draw_text(surface, surf_w, surf_h, about_rect.x + 10, 16, "?", 0xFFFFFFFF);

        // 3. Left Sidebar Categories (x: 0..190, y: 46..surf_h)
        let sb_w = 190;
        let sb_rect = Rect::new(0, 46, sb_w, (surf_h as i32) - 46);
        fill_rect(surface, surf_w, surf_h, sb_rect, 0xFF1E293B);
        fill_rect(surface, surf_w, surf_h, Rect::new(sb_w - 1, 46, 1, sb_rect.h), 0xFF334155);

        Font::draw_text(surface, surf_w, surf_h, 14, 60, "CATEGORIES", 0xFF94A3B8);

        let categories = [
            (TaskFilter::All, "All Downloads"),
            (TaskFilter::Torrents, "Torrents"),
            (TaskFilter::Streams, "Streams & Video"),
            (TaskFilter::Active, "Active Queue"),
            (TaskFilter::Completed, "Completed"),
        ];

        let mut cat_y = 86;
        for (filter, label) in categories {
            let count = engine.filtered_tasks(filter).len();
            let is_sel = self.active_filter == filter;
            let row_r = Rect::new(8, cat_y, sb_w - 16, 26);
            if is_sel {
                fill_rounded_rect(surface, surf_w, surf_h, row_r, 4, 0xFF0284C7);
            }
            let text = format!("{} ({})", label, count);
            let col = if is_sel { 0xFFFFFFFF } else { 0xFFCBD5E1 };
            Font::draw_text(surface, surf_w, surf_h, 16, cat_y + 7, &text, col);
            cat_y += 32;
        }

        // Browser Sniffer Badge Status in sidebar
        let sniffer_box_y = (surf_h as i32) - 90;
        let sniffer_box = Rect::new(10, sniffer_box_y, sb_w - 20, 70);
        fill_rounded_rect(surface, surf_w, surf_h, sniffer_box, 6, 0xFF0F172A);
        draw_rect_outline(surface, surf_w, surf_h, sniffer_box, 1, 0xFF0284C7);
        Font::draw_text(surface, surf_w, surf_h, 18, sniffer_box_y + 10, "* SNIFFER ACTIVE", 0xFF38BDF8);
        Font::draw_text(surface, surf_w, surf_h, 18, sniffer_box_y + 28, "Aero Browser Bridge", 0xFF94A3B8);
        Font::draw_text(surface, surf_w, surf_h, 18, sniffer_box_y + 46, "Auto-detects videos", 0xFF64748B);

        // 4. Main Queue Table (x: 190..surf_w, y: 46..split_y)
        let split_y = ((surf_h as i32) - 260).max(200);
        let table_rect = Rect::new(sb_w, 46, (surf_w as i32) - sb_w, split_y - 46);

        // Table Header
        let th_rect = Rect::new(table_rect.x, 46, table_rect.w, 28);
        fill_rect(surface, surf_w, surf_h, th_rect, 0xFF162032);
        fill_rect(surface, surf_w, surf_h, Rect::new(table_rect.x, 73, table_rect.w, 1), 0xFF334155);

        Font::draw_text(surface, surf_w, surf_h, table_rect.x + 12, 54, "TYPE", 0xFF94A3B8);
        Font::draw_text(surface, surf_w, surf_h, table_rect.x + 80, 54, "NAME", 0xFF94A3B8);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 440, 54, "PROGRESS", 0xFF94A3B8);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 250, 54, "SPEED", 0xFF94A3B8);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 140, 54, "SIZE", 0xFF94A3B8);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 65, 54, "STATUS", 0xFF94A3B8);

        // Table Rows
        let tasks = engine.filtered_tasks(self.active_filter);
        let mut row_y = 76;
        for task in tasks {
            if row_y + 36 > split_y {
                break;
            }

            let is_sel = self.selected_task_id == Some(task.id());
            let row_r = Rect::new(table_rect.x + 2, row_y, table_rect.w - 4, 34);

            let row_bg = if is_sel { 0xFF1E293B } else { 0xFF0F172A };
            fill_rounded_rect(surface, surf_w, surf_h, row_r, 4, row_bg);
            if is_sel {
                draw_rect_outline(surface, surf_w, surf_h, row_r, 1, 0xFF0284C7);
            }

            // Type badge
            let (type_lbl, type_col) = if task.is_torrent() {
                ("[TORRENT]", 0xFF38BDF8)
            } else {
                ("[STREAM]", 0xFFFB923C)
            };
            Font::draw_text(surface, surf_w, surf_h, table_rect.x + 10, row_y + 11, type_lbl, type_col);

            // Name
            let mut name_trunc = task.name().to_string();
            if name_trunc.len() > 36 {
                name_trunc.truncate(33);
                name_trunc.push_str("...");
            }
            Font::draw_text(surface, surf_w, surf_h, table_rect.x + 80, row_y + 11, &name_trunc, 0xFFF1F5F9);

            // Progress bar
            let pbar_x = table_rect.right() - 440;
            let pbar_r = Rect::new(pbar_x, row_y + 10, 160, 14);
            let pct = task.progress_pct();
            let pbar_col = if task.is_torrent() { 0xFF0284C7 } else { 0xFF10B981 };
            draw_progress_bar(surface, surf_w, surf_h, pbar_r, pct, 0xFF1E293B, pbar_col);

            // Speed
            let speed_mb = task.download_rate_bps() as f32 / (1024.0 * 1024.0);
            let speed_str = if task.status() == TaskStatus::Downloading {
                format!("{:.1} MB/s", speed_mb)
            } else {
                "-".to_string()
            };
            Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 250, row_y + 11, &speed_str, 0xFFE2E8F0);

            // Size
            let size_mb = task.total_size() as f32 / (1024.0 * 1024.0);
            let size_str = format!("{:.1} MB", size_mb);
            Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 140, row_y + 11, &size_str, 0xFF94A3B8);

            // Status
            Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 65, row_y + 11, task.status().label(), 0xFF10B981);

            row_y += 38;
        }

        // 5. Bottom Inspector Panel (y: split_y..surf_h)
        let insp_rect = Rect::new(sb_w, split_y, (surf_w as i32) - sb_w, (surf_h as i32) - split_y);
        fill_rect(surface, surf_w, surf_h, insp_rect, 0xFF1E293B);
        fill_rect(surface, surf_w, surf_h, Rect::new(sb_w, split_y, insp_rect.w, 1), 0xFF334155);

        // Inspector Tabs
        let tabs = ["Visualizer", "Peers & Streams", "Metadata"];
        let mut tab_x = sb_w + 12;
        for (idx, &tab_name) in tabs.iter().enumerate() {
            let is_sel = self.inspector_tab == idx;
            let tab_r = Rect::new(tab_x, split_y + 6, 120, 24);
            let tab_col = if is_sel { 0xFF0284C7 } else { 0xFF0F172A };
            fill_rounded_rect(surface, surf_w, surf_h, tab_r, 4, tab_col);
            let text_col = if is_sel { 0xFFFFFFFF } else { 0xFF94A3B8 };
            Font::draw_text(surface, surf_w, surf_h, tab_x + 14, split_y + 12, tab_name, text_col);
            tab_x += 130;
        }

        // Inspector Content Area
        let content_rect = Rect::new(sb_w + 10, split_y + 36, insp_rect.w - 20, insp_rect.h - 44);

        let selected_task = self.selected_task_id.and_then(|id| engine.tasks.iter().find(|t| t.id() == id))
            .or_else(|| engine.tasks.first());

        if let Some(task) = selected_task {
            match task {
                DownloadTask::Torrent(tt) => {
                    PieceGridView::render(
                        surface,
                        surf_w,
                        surf_h,
                        content_rect,
                        &tt.piece_picker.bitfield,
                        &tt.piece_picker.states,
                        tt.piece_picker.piece_length,
                    );
                }
                DownloadTask::Stream(st) => {
                    ChunkBarsView::render(
                        surface,
                        surf_w,
                        surf_h,
                        content_rect,
                        &st.segmented.chunks,
                        st.total_size,
                    );
                }
            }
        }

        // 6. About Modal Dialog (if toggled)
        if self.show_about {
            let modal_w = 460;
            let modal_h = 240;
            let modal_x = ((surf_w as i32) - modal_w) / 2;
            let modal_y = ((surf_h as i32) - modal_h) / 2;
            let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

            // Backdrop tint
            fill_rect(surface, surf_w, surf_h, Rect::new(0, 0, surf_w as i32, surf_h as i32), 0x99000000);

            // Dialog Window
            fill_rounded_rect(surface, surf_w, surf_h, modal_rect, 8, 0xFF0F172A);
            draw_rect_outline(surface, surf_w, surf_h, modal_rect, 2, 0xFF0284C7);

            Font::draw_text(surface, surf_w, surf_h, modal_x + 20, modal_y + 20, "VORTEX DOWNLOAD STUDIO", 0xFF38BDF8);
            Font::draw_text(surface, surf_w, surf_h, modal_x + 20, modal_y + 40, "Version 1.0.0 (Pure Rust 1.99.1)", 0xFFE2E8F0);
            Font::draw_text(surface, surf_w, surf_h, modal_x + 20, modal_y + 70, "* BitTorrent Engine (Transmission/uTorrent architecture)", 0xFF94A3B8);
            Font::draw_text(surface, surf_w, surf_h, modal_x + 20, modal_y + 90, "* IDM Multi-Thread Stream Range Accelerator (4-16 chunks)", 0xFF94A3B8);
            Font::draw_text(surface, surf_w, surf_h, modal_x + 20, modal_y + 110, "* Intelligent Browser Video Sniffer & Floating Badge", 0xFF94A3B8);
            Font::draw_text(surface, surf_w, surf_h, modal_x + 20, modal_y + 130, "* Sovereign HLS / m3u8 Transport Stream Assembler", 0xFF94A3B8);
            Font::draw_text(surface, surf_w, surf_h, modal_x + 20, modal_y + 160, "Registered in NovaOS App Store (Internet Category)", 0xFF10B981);

            let close_btn = Rect::new(modal_x + modal_w - 90, modal_y + modal_h - 40, 70, 26);
            fill_rounded_rect(surface, surf_w, surf_h, close_btn, 4, 0xFF0284C7);
            Font::draw_text(surface, surf_w, surf_h, close_btn.x + 18, close_btn.y + 7, "CLOSE", 0xFFFFFFFF);
        }
    }

    /// Handles mouse clicks in toolbar, sidebar, table rows, and modals.
    pub fn handle_click(&mut self, px: i32, py: i32, engine: &mut VortexEngine) {
        // If About modal is open, clicking dismisses it
        if self.show_about {
            self.show_about = false;
            return;
        }

        // Toolbar Clicks (y: 0..46)
        if py >= 0 && py <= 46 {
            // ? About button (x: surf_w - 40 .. surf_w - 12)
            if px >= (self.width as i32) - 40 && px <= (self.width as i32) - 12 {
                self.show_about = true;
                return;
            }

            // + Torrent (x: 140..220)
            if px >= 140 && px <= 220 {
                let meta = vortex_core::TorrentMeta::create_single_file(
                    "NovaOS-2.0-Desktop.iso",
                    650 * 1024 * 1024,
                    512 * 1024,
                    Some("udp://tracker.novaos.org:6969/announce"),
                );
                let id = engine.add_torrent(meta);
                self.selected_task_id = Some(id);
                return;
            }

            // + Add URL (x: 230..310)
            if px >= 230 && px <= 310 {
                let id = engine.add_stream(
                    "YouTube: Rust Concurrency Deep Dive [1080p FHD].mp4",
                    "https://www.youtube.com/watch?v=kernel_deep_dive",
                    "1080p FHD",
                    92 * 1024 * 1024,
                    8,
                );
                self.selected_task_id = Some(id);
                return;
            }

            // Resume (x: 320..380)
            if px >= 320 && px <= 380 {
                if let Some(id) = self.selected_task_id {
                    engine.resume_task(id);
                }
                return;
            }

            // Pause (x: 390..440)
            if px >= 390 && px <= 440 {
                if let Some(id) = self.selected_task_id {
                    engine.pause_task(id);
                }
                return;
            }

            // Delete (x: 450..520)
            if px >= 450 && px <= 520 {
                if let Some(id) = self.selected_task_id {
                    engine.remove_task(id);
                    self.selected_task_id = None;
                }
                return;
            }
            return;
        }

        // Sidebar Category Clicks (x: 0..190, y: 86..240)
        if px >= 0 && px <= 190 && py >= 86 && py <= 246 {
            let cat_idx = (py - 86) / 32;
            self.active_filter = match cat_idx {
                0 => TaskFilter::All,
                1 => TaskFilter::Torrents,
                2 => TaskFilter::Streams,
                3 => TaskFilter::Active,
                4 => TaskFilter::Completed,
                _ => TaskFilter::All,
            };
            return;
        }

        // Main Table Row Selection (x: 190..surf_w, y: 76..split_y)
        let split_y = ((self.height as i32) - 260).max(200);
        if px >= 190 && py >= 76 && py < split_y {
            let row_idx = ((py - 76) / 38).max(0) as usize;
            let tasks = engine.filtered_tasks(self.active_filter);
            if let Some(task) = tasks.get(row_idx) {
                self.selected_task_id = Some(task.id());
            }
            return;
        }

        // Inspector Tab Selection (y: split_y..split_y + 36)
        if py >= split_y && py <= split_y + 36 {
            let sb_w = 190;
            if px >= sb_w + 12 && px < sb_w + 132 {
                self.inspector_tab = 0;
            } else if px >= sb_w + 142 && px < sb_w + 262 {
                self.inspector_tab = 1;
            } else if px >= sb_w + 272 && px < sb_w + 392 {
                self.inspector_tab = 2;
            }
        }
    }
}

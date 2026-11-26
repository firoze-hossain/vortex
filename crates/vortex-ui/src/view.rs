//! vortex-ui/src/view.rs
//! Interactive Desktop Studio View for Vortex (BitTorrent Swarm + IDM Video/File Accelerator).

use std::collections::HashMap;
use std::path::PathBuf;

use vortex_core::engine::{DownloadTask, TaskFilter, TaskStatus, VortexEngine};
use vortex_core::piece_picker::PickerStrategy;

use crate::chunk_bars::ChunkBarsView;
use crate::draw::{
    blit_image_fit, draw_down_arrow, draw_progress_bar, draw_rect_outline, draw_up_arrow, fill_rect,
    fill_rounded_rect,
};
use crate::font::Font;
use crate::geom::Rect;
use crate::image_decode::DecodedImage;
use crate::modals::{
    handle_add_torrent_click, handle_add_url_click, handle_settings_click, handle_video_preview_click,
    render_add_torrent_modal, render_add_url_modal, render_settings_modal, render_video_preview_modal,
    ActiveModal, AddTorrentModalState, AddUrlModalState, ModalAction, VideoPreviewModalState,
};
use crate::piece_grid::PieceGridView;
use crate::theme::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewCommand {
    None,
    Minimize,
    ToggleMaximize,
    ToggleFullscreen,
    Quit,
    OpenFolder(PathBuf),
    Modal(ModalAction),
}

pub struct VortexView {
    pub selected_task_id: Option<u64>,
    pub active_filter: TaskFilter,
    pub inspector_tab: usize, // 0: Visualizer, 1: Peers & Streams, 2: Metadata & Files
    pub modal: ActiveModal,
    pub show_about: bool, // kept for API compatibility; mirrors ActiveModal::About
    pub scroll_offset: usize,
    pub width: usize,
    pub height: usize,
    pub frame_counter: u64,
    pub is_maximized: bool,
    pub is_fullscreen: bool,
    pub thumb_cache: HashMap<u64, DecodedImage>,
}

impl Default for VortexView {
    fn default() -> Self {
        Self::new(1080, 760)
    }
}

impl VortexView {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            selected_task_id: None,
            active_filter: TaskFilter::All,
            inspector_tab: 0,
            modal: ActiveModal::None,
            show_about: false,
            scroll_offset: 0,
            width,
            height,
            frame_counter: 0,
            is_maximized: false,
            is_fullscreen: false,
            thumb_cache: HashMap::new(),
        }
    }

    pub fn open_add_url_with_text(&mut self, url: String) {
        let mut st = AddUrlModalState::default();
        st.url_input = url;
        self.modal = ActiveModal::AddUrl(st);
        self.show_about = false;
    }

    pub fn render(&self, surface: &mut [u32], engine: &VortexEngine) {
        if surface.is_empty() || self.width == 0 || self.height == 0 {
            return;
        }

        surface.fill(BG_SURFACE);
        let surf_w = self.width;
        let surf_h = self.height;

        // 1. Top Toolbar (y: 0..48)
        fill_rect(surface, surf_w, surf_h, Rect::new(0, 0, surf_w as i32, 48), BG_ELEVATED);
        fill_rect(surface, surf_w, surf_h, Rect::new(0, 47, surf_w as i32, 1), BORDER_SUBTLE);

        Font::draw_text(surface, surf_w, surf_h, 14, 18, "VORTEX", ACCENT_CYAN);
        Font::draw_text(surface, surf_w, surf_h, 64, 18, "STUDIO", TEXT_PRIMARY);

        let btns: [(&str, i32, i32, u32); 7] = [
            ("+ Torrent", 126, 86, ACCENT_BLUE),
            ("+ Add URL", 218, 86, ACCENT_BLUE),
            ("Resume", 310, 68, ACCENT_EMERALD),
            ("Pause", 384, 64, ACCENT_AMBER),
            ("Delete", 454, 68, ACCENT_ROSE),
            ("Folder", 528, 66, BORDER_STRONG),
            ("Settings", 600, 76, BORDER_STRONG),
        ];

        for &(lbl, bx, bw, col) in &btns {
            let br = Rect::new(bx, 10, bw, 28);
            fill_rounded_rect(surface, surf_w, surf_h, br, 5, BG_SURFACE);
            draw_rect_outline(surface, surf_w, surf_h, br, 1, col);
            Font::draw_text(surface, surf_w, surf_h, bx + 10, 19, lbl, TEXT_PRIMARY);
        }

        // Global Speed HUD (Right-aligned left of window controls)
        let speed_x = (surf_w as i32) - 378;
        if speed_x >= 686 {
            draw_down_arrow(surface, surf_w, surf_h, speed_x, 18, ACCENT_CYAN);
            let dl_txt = format_rate(engine.total_download_rate_bps());
            Font::draw_text(surface, surf_w, surf_h, speed_x + 14, 18, &dl_txt, ACCENT_CYAN);

            draw_up_arrow(surface, surf_w, surf_h, speed_x + 104, 18, ACCENT_EMERALD);
            let ul_txt = format_rate(engine.total_upload_rate_bps());
            Font::draw_text(surface, surf_w, surf_h, speed_x + 118, 18, &ul_txt, ACCENT_EMERALD);
        }

        // Rightmost Desktop Window Controls + Help: [?] [FS] [_] [Max] [X]
        let sw = surf_w as i32;

        // About Button [?]
        let about_rect = Rect::new(sw - 160, 11, 26, 26);
        fill_rounded_rect(surface, surf_w, surf_h, about_rect, 4, BORDER_SUBTLE);
        Font::draw_text(surface, surf_w, surf_h, about_rect.x + 9, 18, "?", 0xFFFFFFFF);

        // Fullscreen Toggle Button [FS]
        let fs_rect = Rect::new(sw - 128, 11, 26, 26);
        fill_rounded_rect(surface, surf_w, surf_h, fs_rect, 4, if self.is_fullscreen { ACCENT_BLUE } else { BG_SURFACE });
        draw_rect_outline(surface, surf_w, surf_h, fs_rect, 1, if self.is_fullscreen { ACCENT_CYAN } else { BORDER_STRONG });
        // Four corner brackets representing fullscreen
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 6, fs_rect.y + 6, 5, 2), TEXT_PRIMARY);
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 6, fs_rect.y + 6, 2, 5), TEXT_PRIMARY);
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 15, fs_rect.y + 6, 5, 2), TEXT_PRIMARY);
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 18, fs_rect.y + 6, 2, 5), TEXT_PRIMARY);
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 6, fs_rect.y + 18, 5, 2), TEXT_PRIMARY);
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 6, fs_rect.y + 15, 2, 5), TEXT_PRIMARY);
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 15, fs_rect.y + 18, 5, 2), TEXT_PRIMARY);
        fill_rect(surface, surf_w, surf_h, Rect::new(fs_rect.x + 18, fs_rect.y + 15, 2, 5), TEXT_PRIMARY);

        // Minimize Button [_]
        let min_rect = Rect::new(sw - 98, 11, 26, 26);
        fill_rounded_rect(surface, surf_w, surf_h, min_rect, 4, BG_SURFACE);
        draw_rect_outline(surface, surf_w, surf_h, min_rect, 1, BORDER_STRONG);
        fill_rect(surface, surf_w, surf_h, Rect::new(min_rect.x + 7, min_rect.y + 17, 12, 2), TEXT_PRIMARY);

        // Maximize / Restore Button []
        let max_rect = Rect::new(sw - 68, 11, 26, 26);
        fill_rounded_rect(surface, surf_w, surf_h, max_rect, 4, if self.is_maximized { BG_HOVER } else { BG_SURFACE });
        draw_rect_outline(surface, surf_w, surf_h, max_rect, 1, BORDER_STRONG);
        if self.is_maximized {
            // Overlapping restore squares
            draw_rect_outline(surface, surf_w, surf_h, Rect::new(max_rect.x + 9, max_rect.y + 6, 10, 10), 1, TEXT_PRIMARY);
            draw_rect_outline(surface, surf_w, surf_h, Rect::new(max_rect.x + 6, max_rect.y + 9, 10, 10), 1, ACCENT_CYAN);
        } else {
            draw_rect_outline(surface, surf_w, surf_h, Rect::new(max_rect.x + 7, max_rect.y + 7, 12, 12), 1, TEXT_PRIMARY);
        }

        // Main Window Close Button [X]
        let close_rect = Rect::new(sw - 38, 11, 28, 26);
        fill_rounded_rect(surface, surf_w, surf_h, close_rect, 4, 0xFF2B1420);
        draw_rect_outline(surface, surf_w, surf_h, close_rect, 1, ACCENT_ROSE);
        Font::draw_text(surface, surf_w, surf_h, close_rect.x + 10, 18, "X", ACCENT_ROSE);

        // 2. Left Sidebar Categories (x: 0..196, y: 48..surf_h)
        let sb_w = 196;
        let sb_rect = Rect::new(0, 48, sb_w, (surf_h as i32) - 48);
        fill_rect(surface, surf_w, surf_h, sb_rect, BG_ELEVATED);
        fill_rect(surface, surf_w, surf_h, Rect::new(sb_w - 1, 48, 1, sb_rect.h), BORDER_SUBTLE);

        Font::draw_text(surface, surf_w, surf_h, 14, 62, "CATEGORIES", TEXT_MUTED);

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
            let row_r = Rect::new(8, cat_y, sb_w - 16, 28);
            if is_sel {
                fill_rounded_rect(surface, surf_w, surf_h, row_r, 5, ACCENT_BLUE);
            }
            let text = format!("{} ({})", label, count);
            let col = if is_sel { 0xFFFFFFFF } else { TEXT_SECONDARY };
            Font::draw_text(surface, surf_w, surf_h, 16, cat_y + 9, &text, col);
            cat_y += 33;
        }

        // Pure Rust IPC Bridge & Clipboard Monitor Status Card in Sidebar
        let sniffer_box_y = (surf_h as i32) - 94;
        let sniffer_box = Rect::new(10, sniffer_box_y, sb_w - 20, 78);
        fill_rounded_rect(surface, surf_w, surf_h, sniffer_box, 6, BG_SURFACE);
        draw_rect_outline(surface, surf_w, surf_h, sniffer_box, 1, ACCENT_BLUE);
        fill_rounded_rect(surface, surf_w, surf_h, Rect::new(18, sniffer_box_y + 13, 8, 8), 4, ACCENT_EMERALD);
        Font::draw_text(surface, surf_w, surf_h, 32, sniffer_box_y + 12, "BROWSER BRIDGE", ACCENT_CYAN);
        Font::draw_text(
            surface,
            surf_w,
            surf_h,
            18,
            sniffer_box_y + 32,
            &format!("127.0.0.1:{}", engine.config.bridge_port),
            TEXT_SECONDARY,
        );
        let clip_txt = if engine.config.clipboard_monitor {
            "Clip Watch: ON"
        } else {
            "Clip Watch: OFF"
        };
        Font::draw_text(
            surface,
            surf_w,
            surf_h,
            18,
            sniffer_box_y + 52,
            clip_txt,
            if engine.config.clipboard_monitor { ACCENT_EMERALD } else { TEXT_DIM },
        );

        // 3. Main Queue Table (x: 196..surf_w, y: 48..split_y)
        let split_y = ((surf_h as i32) - 270).max(210);
        let table_rect = Rect::new(sb_w, 48, (surf_w as i32) - sb_w, split_y - 48);

        let th_rect = Rect::new(table_rect.x, 48, table_rect.w, 28);
        fill_rect(surface, surf_w, surf_h, th_rect, BG_PANEL);
        fill_rect(surface, surf_w, surf_h, Rect::new(table_rect.x, 75, table_rect.w, 1), BORDER_SUBTLE);

        Font::draw_text(surface, surf_w, surf_h, table_rect.x + 12, 56, "TYPE", TEXT_MUTED);
        Font::draw_text(surface, surf_w, surf_h, table_rect.x + 86, 56, "PREVIEW", TEXT_MUTED);
        Font::draw_text(surface, surf_w, surf_h, table_rect.x + 168, 56, "NAME", TEXT_MUTED);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 430, 56, "PROGRESS", TEXT_MUTED);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 250, 56, "SPEED", TEXT_MUTED);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 160, 56, "SIZE", TEXT_MUTED);
        Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 80, 56, "STATUS", TEXT_MUTED);

        let tasks = engine.filtered_tasks(self.active_filter);
        if tasks.is_empty() {
            let cy = table_rect.y + table_rect.h / 2 - 20;
            let cx = table_rect.x + 40;
            Font::draw_text(surface, surf_w, surf_h, cx, cy, "No downloads in this view.", TEXT_SECONDARY);
            Font::draw_text(
                surface,
                surf_w,
                surf_h,
                cx,
                cy + 22,
                "Click '+ Add URL' (or press Cmd+V / Ctrl+V) to paste any video/file link with real preview,",
                TEXT_MUTED,
            );
            Font::draw_text(
                surface,
                surf_w,
                surf_h,
                cx,
                cy + 40,
                "or click '+ Torrent' to load a .torrent / magnet link.",
                TEXT_MUTED,
            );
        } else {
            let mut row_y = 78;
            for task in tasks.iter().skip(self.scroll_offset) {
                if row_y + 36 > split_y {
                    break;
                }

                let is_sel = self.selected_task_id == Some(task.id());
                let row_r = Rect::new(table_rect.x + 4, row_y, table_rect.w - 8, 34);
                let row_bg = if is_sel { BG_ELEVATED } else { BG_SURFACE };
                fill_rounded_rect(surface, surf_w, surf_h, row_r, 4, row_bg);
                if is_sel {
                    draw_rect_outline(surface, surf_w, surf_h, row_r, 1, ACCENT_BLUE);
                }

                let (type_lbl, type_col) = if task.is_torrent() {
                    ("[TORRENT]", ACCENT_CYAN)
                } else {
                    ("[STREAM]", ACCENT_ORANGE)
                };
                Font::draw_text(surface, surf_w, surf_h, table_rect.x + 12, row_y + 12, type_lbl, type_col);

                // PREVIEW column (x: table_rect.x + 86 .. +158)
                let pv_r = Rect::new(table_rect.x + 86, row_y + 3, 72, 28);
                if is_video_task(task) {
                    fill_rounded_rect(surface, surf_w, surf_h, pv_r, 4, BG_ROOT);
                    draw_rect_outline(surface, surf_w, surf_h, pv_r, 1, ACCENT_BLUE);
                    if let Some(thumb) = self.thumb_cache.get(&task.id()) {
                        blit_image_fit(surface, surf_w, surf_h, Rect::new(pv_r.x + 1, pv_r.y + 1, 42, 26), thumb);
                        Font::draw_text(surface, surf_w, surf_h, pv_r.x + 48, pv_r.y + 9, ">PV", ACCENT_CYAN);
                    } else {
                        Font::draw_text(surface, surf_w, surf_h, pv_r.x + 7, pv_r.y + 9, "> Video", ACCENT_CYAN);
                    }
                } else {
                    fill_rounded_rect(surface, surf_w, surf_h, pv_r, 4, BG_ELEVATED);
                    Font::draw_text(surface, surf_w, surf_h, pv_r.x + 12, pv_r.y + 9, "[File]", TEXT_DIM);
                }

                let name_max_px = (table_rect.right() - 442) - (table_rect.x + 168);
                Font::draw_text_max_width(surface, surf_w, surf_h, table_rect.x + 168, row_y + 12, task.name(), name_max_px.max(60), TEXT_PRIMARY);

                let pbar_x = table_rect.right() - 430;
                let pbar_r = Rect::new(pbar_x, row_y + 10, 128, 14);
                let pct = task.progress_pct();
                let pbar_col = if task.is_torrent() { ACCENT_BLUE } else { ACCENT_EMERALD };
                draw_progress_bar(surface, surf_w, surf_h, pbar_r, pct, BG_ELEVATED, pbar_col);
                Font::draw_text(surface, surf_w, surf_h, pbar_x + 134, row_y + 12, &format!("{:.0}%", pct), TEXT_SECONDARY);

                let speed_str = if task.status() == TaskStatus::Downloading {
                    format_rate(task.download_rate_bps())
                } else {
                    "-".to_string()
                };
                Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 250, row_y + 12, &speed_str, TEXT_PRIMARY);

                let size_str = format_size(task.total_size());
                Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 160, row_y + 12, &size_str, TEXT_MUTED);

                let st_col = match task.status() {
                    TaskStatus::Downloading => ACCENT_CYAN,
                    TaskStatus::Seeding | TaskStatus::Completed => ACCENT_EMERALD,
                    TaskStatus::Paused => ACCENT_AMBER,
                    TaskStatus::Error => ACCENT_ROSE,
                    TaskStatus::Queued => TEXT_MUTED,
                };
                Font::draw_text(surface, surf_w, surf_h, table_rect.right() - 80, row_y + 12, task.status().label(), st_col);

                row_y += 38;
            }
        }

        // 4. Bottom Inspector Panel (y: split_y..surf_h)
        let insp_rect = Rect::new(sb_w, split_y, (surf_w as i32) - sb_w, (surf_h as i32) - split_y);
        fill_rect(surface, surf_w, surf_h, insp_rect, BG_ELEVATED);
        fill_rect(surface, surf_w, surf_h, Rect::new(sb_w, split_y, insp_rect.w, 1), BORDER_SUBTLE);

        let tabs = ["Visualizer", "Peers & Streams", "Metadata & Files"];
        let mut tab_x = sb_w + 12;
        for (idx, &tab_name) in tabs.iter().enumerate() {
            let is_sel = self.inspector_tab == idx;
            let tab_r = Rect::new(tab_x, split_y + 6, 134, 24);
            fill_rounded_rect(surface, surf_w, surf_h, tab_r, 4, if is_sel { ACCENT_BLUE } else { BG_SURFACE });
            Font::draw_text(surface, surf_w, surf_h, tab_x + 12, split_y + 13, tab_name, if is_sel { 0xFFFFFFFF } else { TEXT_MUTED });
            tab_x += 142;
        }

        let selected_task = self
            .selected_task_id
            .and_then(|id| engine.tasks.iter().find(|t| t.id() == id))
            .or_else(|| engine.tasks.first());

        // For Torrents in Tab 0, show Rarest-First / Sequential toggle button on the right of tab bar
        if let Some(DownloadTask::Torrent(tt)) = selected_task {
            let mode_r = Rect::new(insp_rect.right() - 186, split_y + 6, 174, 24);
            fill_rounded_rect(surface, surf_w, surf_h, mode_r, 4, BG_SURFACE);
            draw_rect_outline(surface, surf_w, surf_h, mode_r, 1, ACCENT_CYAN);
            let mode_lbl = match tt.piece_picker.strategy {
                PickerStrategy::RarestFirst => "Mode: Rarest-First",
                PickerStrategy::SequentialStreaming => "Mode: Sequential",
            };
            Font::draw_text(surface, surf_w, surf_h, mode_r.x + 12, mode_r.y + 7, mode_lbl, ACCENT_CYAN);
        }

        let content_rect = Rect::new(sb_w + 10, split_y + 36, insp_rect.w - 20, insp_rect.h - 44);
        if let Some(task) = selected_task {
            match self.inspector_tab {
                0 => match task {
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
                        if st.status == TaskStatus::Error {
                            fill_rounded_rect(surface, surf_w, surf_h, content_rect, 4, BG_SURFACE);
                            draw_rect_outline(surface, surf_w, surf_h, content_rect, 1, ACCENT_ROSE);
                            Font::draw_text(surface, surf_w, surf_h, content_rect.x + 14, content_rect.y + 16, "Download Error Details:", ACCENT_ROSE);
                            let err = st.error_msg.as_deref().unwrap_or("Stream transfer failed");
                            Font::draw_text_max_width(surface, surf_w, surf_h, content_rect.x + 14, content_rect.y + 38, err, content_rect.w - 28, TEXT_PRIMARY);
                            Font::draw_text_max_width(surface, surf_w, surf_h, content_rect.x + 14, content_rect.y + 62, &format!("URL: {}", st.url), content_rect.w - 28, TEXT_MUTED);
                        } else {
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
                },
                1 => render_peers_and_streams_tab(surface, surf_w, surf_h, content_rect, task),
                _ => render_metadata_tab(surface, surf_w, surf_h, content_rect, task),
            }
        } else {
            fill_rounded_rect(surface, surf_w, surf_h, content_rect, 4, BG_SURFACE);
            Font::draw_text(surface, surf_w, surf_h, content_rect.x + 16, content_rect.y + 18, "Select a download task to inspect live pieces, threads, peers, and file metadata.", TEXT_DIM);
        }

        // 5. Render Active Modal Overlay
        let blink = (self.frame_counter / 30) % 2 == 0;
        match &self.modal {
            ActiveModal::AddUrl(st) => render_add_url_modal(
                surface,
                surf_w,
                surf_h,
                st,
                blink,
                &engine.config.download_dir.to_string_lossy(),
            ),
            ActiveModal::AddTorrent(st) => render_add_torrent_modal(surface, surf_w, surf_h, st, blink),
            ActiveModal::VideoPreview(vp) => {
                if let Some(task) = engine.tasks.iter().find(|t| t.id() == vp.task_id) {
                    render_video_preview_modal(surface, surf_w, surf_h, task, self.thumb_cache.get(&vp.task_id));
                }
            }
            ActiveModal::Settings => render_settings_modal(
                surface,
                surf_w,
                surf_h,
                engine.config.default_threads,
                engine.global_download_limit_bps,
                &engine.config.download_dir.to_string_lossy(),
                engine.config.bridge_port,
                engine.config.clipboard_monitor,
            ),
            ActiveModal::About => render_about_modal(surface, surf_w, surf_h),
            ActiveModal::None => {}
        }
    }

    /// Handles mouse clicks across modals, toolbar, sidebar, table rows, and inspector tabs.
    pub fn handle_click_with_actions(&mut self, px: i32, py: i32, engine: &mut VortexEngine) -> ViewCommand {
        // 1. Handle open modal first
        match &mut self.modal {
            ActiveModal::About => {
                self.modal = ActiveModal::None;
                self.show_about = false;
                return ViewCommand::None;
            }
            ActiveModal::AddUrl(st) => {
                let act = handle_add_url_click(self.width, self.height, px, py, st);
                if act == ModalAction::Close {
                    self.modal = ActiveModal::None;
                    return ViewCommand::None;
                }
                return ViewCommand::Modal(act);
            }
            ActiveModal::AddTorrent(st) => {
                let act = handle_add_torrent_click(self.width, self.height, px, py, st);
                if act == ModalAction::Close {
                    self.modal = ActiveModal::None;
                    return ViewCommand::None;
                }
                return ViewCommand::Modal(act);
            }
            ActiveModal::VideoPreview(vp) => {
                if let Some(task) = engine.tasks.iter().find(|t| t.id() == vp.task_id) {
                    let save_path = task.save_path().clone();
                    let act = handle_video_preview_click(self.width, self.height, px, py, task);
                    match act {
                        ModalAction::Close => {
                            self.modal = ActiveModal::None;
                            return ViewCommand::None;
                        }
                        ModalAction::OpenDownloadFolder => {
                            return ViewCommand::OpenFolder(save_path);
                        }
                        _ => return ViewCommand::Modal(act),
                    }
                }
                self.modal = ActiveModal::None;
                return ViewCommand::None;
            }
            ActiveModal::Settings => {
                let act = handle_settings_click(self.width, self.height, px, py);
                match act {
                    ModalAction::Close => {
                        self.modal = ActiveModal::None;
                        return ViewCommand::None;
                    }
                    ModalAction::SetThreads(t) => {
                        engine.config.default_threads = t;
                        engine.save_persisted();
                        return ViewCommand::None;
                    }
                    ModalAction::SetSpeedLimit(lim) => {
                        engine.set_global_download_limit(lim);
                        engine.save_persisted();
                        return ViewCommand::None;
                    }
                    ModalAction::ToggleClipboardMonitor => {
                        engine.config.clipboard_monitor = !engine.config.clipboard_monitor;
                        engine.save_persisted();
                        return ViewCommand::None;
                    }
                    ModalAction::OpenDownloadFolder => {
                        return ViewCommand::OpenFolder(engine.config.download_dir.clone());
                    }
                    ModalAction::ChooseDefaultDownloadFolder => {
                        return ViewCommand::Modal(ModalAction::ChooseDefaultDownloadFolder);
                    }
                    _ => return ViewCommand::None,
                }
            }
            ActiveModal::None => {}
        }

        // 2. Toolbar Clicks (y: 0..48)
        if py >= 0 && py <= 48 {
            let sw = self.width as i32;
            if px >= sw - 38 && px <= sw - 10 {
                return ViewCommand::Quit;
            }
            if px >= sw - 68 && px <= sw - 42 {
                return ViewCommand::ToggleMaximize;
            }
            if px >= sw - 98 && px <= sw - 72 {
                return ViewCommand::Minimize;
            }
            if px >= sw - 128 && px <= sw - 102 {
                return ViewCommand::ToggleFullscreen;
            }
            if px >= sw - 160 && px <= sw - 134 {
                self.modal = ActiveModal::About;
                self.show_about = true;
                return ViewCommand::None;
            }
            if (126..=212).contains(&px) {
                self.modal = ActiveModal::AddTorrent(AddTorrentModalState::default());
                return ViewCommand::None;
            }
            if (218..=304).contains(&px) {
                self.modal = ActiveModal::AddUrl(AddUrlModalState::default());
                return ViewCommand::None;
            }
            if (310..=378).contains(&px) {
                if let Some(id) = self.selected_task_id {
                    engine.resume_task(id);
                }
                return ViewCommand::None;
            }
            if (384..=448).contains(&px) {
                if let Some(id) = self.selected_task_id {
                    engine.pause_task(id);
                }
                return ViewCommand::None;
            }
            if (454..=522).contains(&px) {
                if let Some(id) = self.selected_task_id {
                    engine.remove_task(id);
                    self.selected_task_id = None;
                }
                return ViewCommand::None;
            }
            if (528..=594).contains(&px) {
                let target = self
                    .selected_task_id
                    .and_then(|id| engine.tasks.iter().find(|t| t.id() == id))
                    .map(|t| t.save_path().clone())
                    .unwrap_or_else(|| engine.config.download_dir.clone());
                return ViewCommand::OpenFolder(target);
            }
            if (600..=676).contains(&px) {
                self.modal = ActiveModal::Settings;
                return ViewCommand::None;
            }
            return ViewCommand::None;
        }

        // 3. Sidebar Category Clicks (x: 0..196, y: 86..251)
        if px >= 0 && px <= 196 && py >= 86 && py <= 251 {
            let cat_idx = (py - 86) / 33;
            self.active_filter = match cat_idx {
                0 => TaskFilter::All,
                1 => TaskFilter::Torrents,
                2 => TaskFilter::Streams,
                3 => TaskFilter::Active,
                4 => TaskFilter::Completed,
                _ => TaskFilter::All,
            };
            self.scroll_offset = 0;
            return ViewCommand::None;
        }

        // 4. Main Table Row Selection & PREVIEW Column Click
        let split_y = ((self.height as i32) - 270).max(210);
        if px >= 196 && py >= 78 && py < split_y {
            let row_idx = ((py - 78) / 38).max(0) as usize + self.scroll_offset;
            let tasks = engine.filtered_tasks(self.active_filter);
            if let Some(task) = tasks.get(row_idx) {
                let tid = task.id();
                let is_vid = is_video_task(task);
                self.selected_task_id = Some(tid);
                // PREVIEW column hit area: x in 282..354 (table_rect.x 196 + 86 .. + 72)
                if is_vid && px >= 196 + 86 && px <= 196 + 86 + 72 {
                    self.modal = ActiveModal::VideoPreview(VideoPreviewModalState { task_id: tid });
                }
            }
            return ViewCommand::None;
        }

        // 5. Inspector Tab Selection & Torrent Strategy Toggle
        if py >= split_y && py <= split_y + 36 {
            let sb_w = 196;
            if px >= sb_w + 12 && px < sb_w + 146 {
                self.inspector_tab = 0;
            } else if px >= sb_w + 154 && px < sb_w + 288 {
                self.inspector_tab = 1;
            } else if px >= sb_w + 296 && px < sb_w + 430 {
                self.inspector_tab = 2;
            } else if px >= (self.width as i32) - 186 {
                if let Some(id) = self.selected_task_id.or_else(|| engine.tasks.first().map(|t| t.id())) {
                    engine.toggle_torrent_strategy(id);
                }
            }
        }

        ViewCommand::None
    }

    /// Backwards-compatible click handler wrapper.
    pub fn handle_click(&mut self, px: i32, py: i32, engine: &mut VortexEngine) {
        let _ = self.handle_click_with_actions(px, py, engine);
    }
}

fn render_peers_and_streams_tab(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    bounds: Rect,
    task: &DownloadTask,
) {
    fill_rounded_rect(surface, surf_w, surf_h, bounds, 4, BG_SURFACE);
    draw_rect_outline(surface, surf_w, surf_h, bounds, 1, BORDER_SUBTLE);

    match task {
        DownloadTask::Torrent(tt) => {
            let hdr = format!(
                "Swarm Peers: {} connected | Seeders: {} | Leechers: {} | Ratio: {:.2}",
                tt.peers.len(),
                tt.seeders,
                tt.leechers,
                tt.ratio()
            );
            Font::draw_text(surface, surf_w, surf_h, bounds.x + 12, bounds.y + 10, &hdr, ACCENT_CYAN);
            Font::draw_text(surface, surf_w, surf_h, bounds.x + 12, bounds.y + 32, "ADDRESS             CLIENT              PROGRESS   DOWN        UP          STATE", TEXT_MUTED);

            let mut ry = bounds.y + 50;
            if tt.peers.is_empty() {
                let msg = tt.last_message.as_deref().unwrap_or("Waiting for tracker peer list...");
                Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, ry + 6, msg, bounds.w - 24, TEXT_SECONDARY);
            } else {
                for peer in &tt.peers {
                    if ry + 18 > bounds.bottom() - 6 {
                        break;
                    }
                    let line = format!(
                        "{:<19} {:<19} {:>5.1}%     {:<11} {:<11} {}",
                        peer.addr,
                        peer.client,
                        peer.progress_pct,
                        format_rate(peer.down_bps),
                        format_rate(peer.up_bps),
                        peer.state
                    );
                    Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, ry, &line, bounds.w - 24, TEXT_PRIMARY);
                    ry += 18;
                }
            }
        }
        DownloadTask::Stream(st) => {
            let hdr = format!(
                "HTTP/HLS Connections: {} active threads | Resumable: {} | MIME: {}",
                st.segmented.chunks.len(),
                if st.segmented.resumable { "Yes" } else { "No" },
                st.segmented.content_type
            );
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, bounds.y + 10, &hdr, bounds.w - 24, ACCENT_CYAN);
            Font::draw_text(surface, surf_w, surf_h, bounds.x + 12, bounds.y + 32, "THREAD   BYTE RANGE                  PROGRESS       SPEED         STATUS", TEXT_MUTED);

            let mut ry = bounds.y + 50;
            for c in &st.segmented.chunks {
                if ry + 18 > bounds.bottom() - 6 {
                    break;
                }
                let line = format!(
                    "#{:<7} {:>10}-{:<12}    {:>5.1}%        {:<12}  {:?}",
                    c.id + 1,
                    c.start_byte,
                    c.end_byte,
                    c.progress_pct(),
                    format_rate(c.speed_bps),
                    c.status
                );
                Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, ry, &line, bounds.w - 24, TEXT_PRIMARY);
                ry += 18;
            }
        }
    }
}

fn render_metadata_tab(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    bounds: Rect,
    task: &DownloadTask,
) {
    fill_rounded_rect(surface, surf_w, surf_h, bounds, 4, BG_SURFACE);
    draw_rect_outline(surface, surf_w, surf_h, bounds, 1, BORDER_SUBTLE);

    let max_w = bounds.w - 24;
    let mut y = bounds.y + 12;

    match task {
        DownloadTask::Torrent(tt) => {
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Name: {}", tt.name), max_w, TEXT_PRIMARY);
            y += 20;
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Info Hash: {}", tt.info_hash_hex), max_w, ACCENT_CYAN);
            y += 20;
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Save Path: {}", tt.save_path.display()), max_w, TEXT_SECONDARY);
            y += 20;
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Tracker: {}", tt.meta.announce.as_deref().unwrap_or("None")), max_w, TEXT_MUTED);
            y += 24;
            Font::draw_text(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Files ({}):", tt.meta.files.len()), ACCENT_EMERALD);
            y += 18;
            for f in &tt.meta.files {
                if y + 18 > bounds.bottom() - 6 {
                    break;
                }
                let fline = format!("  - {} ({})", f.path, format_size(f.length));
                Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &fline, max_w, TEXT_SECONDARY);
                y += 18;
            }
        }
        DownloadTask::Stream(st) => {
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("File Name: {}", st.name), max_w, TEXT_PRIMARY);
            y += 20;
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Source URL: {}", st.url), max_w, ACCENT_CYAN);
            y += 20;
            Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Save Path: {}", st.save_path.display()), max_w, TEXT_SECONDARY);
            y += 20;
            Font::draw_text_max_width(
                surface,
                surf_w,
                surf_h,
                bounds.x + 12,
                y,
                &format!(
                    "Quality: {} | Threads: {} | HLS: {} | Downloaded: {} / {}",
                    st.resolution,
                    st.active_threads,
                    if st.is_hls { "Yes" } else { "No" },
                    format_size(st.downloaded_bytes),
                    format_size(st.total_size)
                ),
                max_w,
                TEXT_MUTED,
            );
            y += 22;
            if let Some(err) = &st.error_msg {
                Font::draw_text_max_width(surface, surf_w, surf_h, bounds.x + 12, y, &format!("Error: {}", err), max_w, ACCENT_ROSE);
            }
        }
    }
}

fn render_about_modal(surface: &mut [u32], surf_w: usize, surf_h: usize) {
    let modal_w = 500;
    let modal_h = 240;
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;
    let modal_rect = Rect::new(mx, my, modal_w, modal_h);

    fill_rect(surface, surf_w, surf_h, Rect::new(0, 0, surf_w as i32, surf_h as i32), 0xAA000000);
    fill_rounded_rect(surface, surf_w, surf_h, modal_rect, 8, BG_SURFACE);
    draw_rect_outline(surface, surf_w, surf_h, modal_rect, 2, ACCENT_BLUE);

    Font::draw_text(surface, surf_w, surf_h, mx + 20, my + 20, "VORTEX DOWNLOAD STUDIO", ACCENT_CYAN);
    Font::draw_text(surface, surf_w, surf_h, mx + 20, my + 40, "Version 2.0.0 (Real Network Engine in Pure Rust)", TEXT_PRIMARY);
    Font::draw_text(surface, surf_w, surf_h, mx + 20, my + 70, "* BitTorrent Client: HTTP/UDP Trackers, Peer Wire, SHA-1, Multi-File", TEXT_MUTED);
    Font::draw_text(surface, surf_w, surf_h, mx + 20, my + 90, "* IDM Stream Accelerator: Multi-Thread HTTP Range & HLS (.m3u8)", TEXT_MUTED);
    Font::draw_text(surface, surf_w, surf_h, mx + 20, my + 110, "* Live Video Preview: Real Title, Thumbnail (PNG/JPEG) & Qualities", TEXT_MUTED);
    Font::draw_text(surface, surf_w, surf_h, mx + 20, my + 130, "* Extensions (Chrome/Brave/Edge/Firefox) + Aero Browser + Clipboard", TEXT_MUTED);
    Font::draw_text(surface, surf_w, surf_h, mx + 20, my + 160, "Cross-Platform Desktop: macOS | Linux | Windows | NovaOS", ACCENT_EMERALD);

    let close_btn = Rect::new(mx + modal_w - 90, my + modal_h - 40, 70, 26);
    fill_rounded_rect(surface, surf_w, surf_h, close_btn, 4, ACCENT_BLUE);
    Font::draw_text(surface, surf_w, surf_h, close_btn.x + 18, close_btn.y + 8, "CLOSE", 0xFFFFFFFF);
}

pub fn is_video_task(task: &DownloadTask) -> bool {
    let low_name = task.name().to_ascii_lowercase();
    let has_vid_ext = [".mp4", ".webm", ".mkv", ".mov", ".m4v", ".ts", ".m3u8"]
        .iter()
        .any(|ext| low_name.contains(ext));
    let has_non_vid_ext = [".zip", ".tar", ".gz", ".iso", ".exe", ".deb", ".appimage", ".pdf", ".dmg"]
        .iter()
        .any(|ext| low_name.ends_with(ext));

    if has_vid_ext {
        return true;
    }
    if has_non_vid_ext {
        return false;
    }
    match task {
        DownloadTask::Stream(st) => !st.resolution.eq_ignore_ascii_case("Direct") || st.is_hls,
        DownloadTask::Torrent(_) => false,
    }
}

pub fn format_rate(bps: u64) -> String {
    if bps >= 1024 * 1024 {
        format!("{:.1} MB/s", bps as f32 / (1024.0 * 1024.0))
    } else if bps >= 1024 {
        format!("{} KB/s", bps / 1024)
    } else {
        format!("{} B/s", bps)
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        "Unknown".into()
    } else if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f32 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f32 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{} B", bytes)
    }
}

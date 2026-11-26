//! vortex-ui/src/modals.rs
//! Interactive Modal Dialogs: Add URL + Real Video Preview, Add Torrent / Magnet, Settings, and About.

use std::path::PathBuf;

use vortex_core::media::preview::{MediaPreview, PreviewKind};

use crate::draw::{blit_image_fit, draw_rect_outline, fill_rect, fill_rounded_rect};
use crate::font::Font;
use crate::geom::Rect;
use crate::image_decode::{decode_image, DecodedImage};
use crate::theme::*;

#[derive(Clone, Debug)]
pub enum PreviewFetchState {
    Idle,
    Loading(String),
    Ready {
        preview: MediaPreview,
        thumbnail: Option<DecodedImage>,
        selected_quality_idx: usize,
    },
    Error(String),
}

#[derive(Clone, Debug)]
pub struct AddUrlModalState {
    pub url_input: String,
    pub threads: usize,
    pub preview_state: PreviewFetchState,
    pub custom_save_dir: Option<PathBuf>,
}

impl Default for AddUrlModalState {
    fn default() -> Self {
        Self {
            url_input: String::new(),
            threads: 8,
            preview_state: PreviewFetchState::Idle,
            custom_save_dir: None,
        }
    }
}

impl AddUrlModalState {
    pub fn set_preview_result(&mut self, res: Result<MediaPreview, String>) {
        match res {
            Ok(preview) => {
                let thumb = preview.thumbnail_bytes.as_deref().and_then(decode_image);
                let best_idx = preview
                    .qualities
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, q)| (q.has_audio as u32, !q.is_audio_only as u32, q.resolution.0 * q.resolution.1))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.preview_state = PreviewFetchState::Ready {
                    preview,
                    thumbnail: thumb,
                    selected_quality_idx: best_idx,
                };
            }
            Err(e) => {
                self.preview_state = PreviewFetchState::Error(e);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct AddTorrentModalState {
    pub input: String,
    pub sequential: bool,
    pub error: Option<String>,
    pub custom_save_dir: Option<PathBuf>,
}

impl Default for AddTorrentModalState {
    fn default() -> Self {
        Self {
            input: String::new(),
            sequential: false,
            error: None,
            custom_save_dir: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VideoPreviewModalState {
    pub task_id: u64,
}

#[derive(Clone, Debug)]
pub enum ActiveModal {
    None,
    AddUrl(AddUrlModalState),
    AddTorrent(AddTorrentModalState),
    VideoPreview(VideoPreviewModalState),
    Settings,
    About,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalAction {
    None,
    Close,
    RequestPasteUrl,
    RequestPasteTorrent,
    RequestBrowseTorrent,
    ChooseDefaultDownloadFolder,
    ChooseModalSaveFolder,
    PlayMediaFile(PathBuf),
    FetchPreview(String),
    StartStream {
        url: String,
        filename: String,
        resolution: String,
        size: u64,
        threads: usize,
        is_hls: bool,
        referer: Option<String>,
        dest_dir: Option<PathBuf>,
    },
    StartTorrentOrMagnet {
        input: String,
        sequential: bool,
        dest_dir: Option<PathBuf>,
    },
    SetThreads(usize),
    SetSpeedLimit(u64),
    ToggleClipboardMonitor,
    OpenDownloadFolder,
}

fn add_url_modal_bounds(surf_w: usize, surf_h: usize) -> (i32, i32, i32, i32) {
    let modal_w = (surf_w as i32 - 60).clamp(560, 780);
    let modal_h = (surf_h as i32 - 60).clamp(470, 580);
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;
    (mx, my, modal_w, modal_h)
}

pub fn render_add_url_modal(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    state: &AddUrlModalState,
    cursor_blink: bool,
    default_download_dir: &str,
) {
    let (mx, my, modal_w, modal_h) = add_url_modal_bounds(surf_w, surf_h);

    fill_rect(surface, surf_w, surf_h, Rect::new(0, 0, surf_w as i32, surf_h as i32), 0xAA000000);
    let modal_r = Rect::new(mx, my, modal_w, modal_h);
    fill_rounded_rect(surface, surf_w, surf_h, modal_r, 8, BG_SURFACE);
    draw_rect_outline(surface, surf_w, surf_h, modal_r, 2, ACCENT_BLUE);

    // Title Bar
    fill_rect(surface, surf_w, surf_h, Rect::new(mx + 2, my + 2, modal_w - 4, 38), BG_PANEL);
    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 15, "Add URL & Live Video Preview", ACCENT_CYAN);

    let close_r = Rect::new(mx + modal_w - 34, my + 8, 24, 24);
    fill_rounded_rect(surface, surf_w, surf_h, close_r, 4, 0xFF2B1420);
    draw_rect_outline(surface, surf_w, surf_h, close_r, 1, ACCENT_ROSE);
    Font::draw_text(surface, surf_w, surf_h, close_r.x + 8, close_r.y + 7, "X", ACCENT_ROSE);

    // URL Input Row
    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 52, "Paste Video, Stream (.m3u8), Direct File, or Magnet URL:", TEXT_MUTED);
    let input_w = modal_w - 220;
    let input_r = Rect::new(mx + 16, my + 70, input_w, 30);
    fill_rounded_rect(surface, surf_w, surf_h, input_r, 4, BG_ROOT);
    draw_rect_outline(surface, surf_w, surf_h, input_r, 1, ACCENT_CYAN);

    let display_url = if state.url_input.is_empty() {
        "https://... (type or click Paste)".to_string()
    } else {
        let max_chars = ((input_w - 18) / 7).max(10) as usize;
        let len = state.url_input.chars().count();
        let tail: String = if len > max_chars {
            state.url_input.chars().skip(len - max_chars).collect()
        } else {
            state.url_input.clone()
        };
        if cursor_blink { format!("{}|", tail) } else { tail }
    };
    let text_col = if state.url_input.is_empty() { TEXT_DIM } else { TEXT_PRIMARY };
    Font::draw_text(surface, surf_w, surf_h, input_r.x + 8, input_r.y + 10, &display_url, text_col);

    // Paste Button
    let paste_r = Rect::new(mx + 16 + input_w + 8, my + 70, 74, 30);
    fill_rounded_rect(surface, surf_w, surf_h, paste_r, 4, BG_ELEVATED);
    draw_rect_outline(surface, surf_w, surf_h, paste_r, 1, BORDER_STRONG);
    Font::draw_text(surface, surf_w, surf_h, paste_r.x + 18, paste_r.y + 10, "Paste", TEXT_PRIMARY);

    // Preview / Analyze Button
    let fetch_r = Rect::new(paste_r.right() + 8, my + 70, 102, 30);
    fill_rounded_rect(surface, surf_w, surf_h, fetch_r, 4, ACCENT_BLUE);
    Font::draw_text(surface, surf_w, surf_h, fetch_r.x + 10, fetch_r.y + 10, "Get Preview", 0xFFFFFFFF);

    // Preview Card Area
    let card_r = Rect::new(mx + 16, my + 114, modal_w - 32, modal_h - 176);
    fill_rounded_rect(surface, surf_w, surf_h, card_r, 6, BG_PANEL);
    draw_rect_outline(surface, surf_w, surf_h, card_r, 1, BORDER_SUBTLE);

    let mut can_download = !state.url_input.trim().is_empty();
    let mut cta = "Analyze & Download";

    match &state.preview_state {
        PreviewFetchState::Idle => {
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 28, "Ready to inspect real video/file metadata.", TEXT_SECONDARY);
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 52, "* YouTube / Vimeo / HTML5 Video Pages -> extracts 1080p/720p/480p/360p/Audio", TEXT_MUTED);
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 72, "* Direct Files (.mp4, .zip, .iso)     -> probes exact size & multi-thread support", TEXT_MUTED);
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 92, "* HLS Playlists (.m3u8)               -> lists real resolution variants", TEXT_MUTED);
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 124, "Tip: Paste a URL and click 'Get Preview' (or press Enter) to choose quality!", ACCENT_CYAN);
        }
        PreviewFetchState::Loading(u) => {
            can_download = false;
            cta = "Resolving...";
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 30, "Fetching real preview & probing video qualities...", ACCENT_CYAN);
            Font::draw_text_max_width(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 54, u, card_r.w - 40, TEXT_MUTED);
        }
        PreviewFetchState::Error(err) => {
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 26, "Preview Error:", ACCENT_ROSE);
            Font::draw_text_max_width(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 48, err, card_r.w - 40, TEXT_SECONDARY);
            Font::draw_text(surface, surf_w, surf_h, card_r.x + 20, card_r.y + 80, "You can try 'Get Preview' again or click 'Download Direct' for direct file links.", TEXT_MUTED);
            cta = "Download Direct";
        }
        PreviewFetchState::Ready {
            preview,
            thumbnail,
            selected_quality_idx,
        } => {
            // Left: Real Thumbnail Box (220x124 16:9)
            let thumb_r = Rect::new(card_r.x + 12, card_r.y + 12, 220, 124);
            fill_rounded_rect(surface, surf_w, surf_h, thumb_r, 4, BG_ROOT);
            draw_rect_outline(surface, surf_w, surf_h, thumb_r, 1, BORDER_SUBTLE);
            if let Some(img) = thumbnail {
                blit_image_fit(surface, surf_w, surf_h, thumb_r.inset(2, 2), img);
            } else {
                let kind_lbl = format!("[{:?}]", preview.kind);
                Font::draw_text(surface, surf_w, surf_h, thumb_r.x + 48, thumb_r.y + 52, &kind_lbl, ACCENT_CYAN);
                Font::draw_text_max_width(surface, surf_w, surf_h, thumb_r.x + 16, thumb_r.y + 72, &preview.content_type, thumb_r.w - 32, TEXT_MUTED);
            }

            // Right: Video Title, Author, Duration, Resumability
            let info_x = thumb_r.right() + 14;
            let info_w = card_r.right() - info_x - 12;
            let disp_title = readable_ascii_title(&preview.title, &preview.suggested_filename);
            Font::draw_text_max_width(surface, surf_w, surf_h, info_x, card_r.y + 14, &disp_title, info_w, TEXT_PRIMARY);

            let author_line = format!(
                "Source: {}  {}",
                preview.source_domain,
                preview.author.as_deref().map(|a| format!("| {}", a)).unwrap_or_default()
            );
            Font::draw_text_max_width(surface, surf_w, surf_h, info_x, card_r.y + 36, &author_line, info_w, ACCENT_CYAN);

            let dur_str = preview
                .duration_secs
                .map(|s| format!("{}:{:02}", s / 60, s % 60))
                .unwrap_or_else(|| "N/A".into());
            let range_str = if preview.resumable { "Multi-Thread IDM OK" } else { "Single Stream" };
            let meta_line = format!("Duration: {}  |  {}", dur_str, range_str);
            Font::draw_text_max_width(surface, surf_w, surf_h, info_x, card_r.y + 56, &meta_line, info_w, ACCENT_EMERALD);

            let selected_q = preview.qualities.get(*selected_quality_idx);
            let is_gated = selected_q
                .map(|q| q.label.contains("[PO-Token Gated]"))
                .unwrap_or(false);
            let out_name = selected_q
                .map(|q| build_quality_filename(&preview.suggested_filename, q))
                .unwrap_or_else(|| preview.suggested_filename.clone());
            let file_line = format!("Save As: {}", out_name);
            Font::draw_text_max_width(surface, surf_w, surf_h, info_x, card_r.y + 76, &file_line, info_w, TEXT_SECONDARY);

            if is_gated {
                Font::draw_text_max_width(
                    surface,
                    surf_w,
                    surf_h,
                    info_x,
                    card_r.y + 98,
                    "PO-Token gated by YouTube (403). Select '360p MP4 [READY]' or install yt-dlp.",
                    info_w,
                    ACCENT_AMBER,
                );
            } else if let Some(note) = &preview.note {
                let note_col = if preview.qualities.is_empty() { ACCENT_ROSE } else { TEXT_MUTED };
                Font::draw_text_max_width(surface, surf_w, surf_h, info_x, card_r.y + 98, note, info_w, note_col);
            }

            // Bottom of Card: 2-Column Quality / Format Selection Grid (fits up to 10-12 qualities!)
            let q_y_start = thumb_r.bottom() + 10;
            Font::draw_text(
                surface,
                surf_w,
                surf_h,
                card_r.x + 12,
                q_y_start,
                "Choose Quality / Format to Download:",
                TEXT_MUTED,
            );

            if preview.qualities.is_empty() {
                can_download = false;
                cta = "No Stream Found";
                Font::draw_text_max_width(
                    surface,
                    surf_w,
                    surf_h,
                    card_r.x + 12,
                    q_y_start + 24,
                    "No downloadable video stream found on this page. Click 'Get Preview' to retry or paste a direct stream URL.",
                    card_r.w - 24,
                    ACCENT_ROSE,
                );
            } else {
                if is_gated {
                    can_download = false;
                    cta = "PO-Token Gated";
                } else {
                    cta = "Download Selected";
                }
                let col_gap = 8;
                let col_w = (card_r.w - 24 - col_gap) / 2;
                let row_h = 26;
                let grid_y0 = q_y_start + 18;

                for (i, q) in preview.qualities.iter().enumerate() {
                    let row = (i / 2) as i32;
                    let col = (i % 2) as i32;
                    let qx = card_r.x + 12 + col * (col_w + col_gap);
                    let qy = grid_y0 + row * (row_h + 4);
                    if qy + row_h > card_r.bottom() - 6 {
                        break;
                    }

                    let is_sel = i == *selected_quality_idx;
                    let qr = Rect::new(qx, qy, col_w, row_h);
                    fill_rounded_rect(surface, surf_w, surf_h, qr, 4, if is_sel { BG_HOVER } else { BG_SURFACE });
                    draw_rect_outline(
                        surface,
                        surf_w,
                        surf_h,
                        qr,
                        1,
                        if is_sel { ACCENT_CYAN } else { BORDER_SUBTLE },
                    );

                    let size_str = if q.estimated_size_bytes > 0 {
                        format!("{:.1} MB", q.estimated_size_bytes as f32 / (1024.0 * 1024.0))
                    } else {
                        "Stream".into()
                    };
                    let tag = if q.label.contains("[PO-Token Gated]") {
                        "GATED"
                    } else if q.is_audio_only {
                        "AUDIO"
                    } else if q.has_audio {
                        "READY"
                    } else {
                        "VID"
                    };
                    let line = format!(
                        "{} {} [{} · {}]",
                        if is_sel { ">" } else { " " },
                        q.label.replace(" [PO-Token Gated]", ""),
                        tag,
                        size_str
                    );
                    let row_col = if q.label.contains("[PO-Token Gated]") {
                        TEXT_DIM
                    } else if is_sel {
                        TEXT_PRIMARY
                    } else {
                        TEXT_SECONDARY
                    };
                    Font::draw_text_max_width(
                        surface,
                        surf_w,
                        surf_h,
                        qr.x + 8,
                        qr.y + 8,
                        &line,
                        qr.w - 16,
                        row_col,
                    );
                }
            }
        }
    }

    // Bottom Bar: Thread Count Selector + Save Folder Picker + Download CTA
    let foot_y = my + modal_h - 46;
    Font::draw_text(surface, surf_w, surf_h, mx + 16, foot_y + 10, "Threads:", TEXT_MUTED);
    for (idx, &t) in [1usize, 4, 8, 16].iter().enumerate() {
        let bx = mx + 82 + (idx as i32) * 42;
        let br = Rect::new(bx, foot_y + 4, 36, 24);
        let active = state.threads == t;
        fill_rounded_rect(surface, surf_w, surf_h, br, 4, if active { ACCENT_BLUE } else { BG_ELEVATED });
        Font::draw_text(surface, surf_w, surf_h, br.x + 10, br.y + 7, &format!("{}", t), TEXT_PRIMARY);
    }

    let dl_r = Rect::new(mx + modal_w - 186, foot_y + 2, 170, 28);
    let folder_btn_x = mx + 256;
    let folder_btn_w = (dl_r.x - folder_btn_x - 10).max(90);
    let folder_r = Rect::new(folder_btn_x, foot_y + 4, folder_btn_w, 24);
    fill_rounded_rect(surface, surf_w, surf_h, folder_r, 4, BG_ELEVATED);
    draw_rect_outline(
        surface,
        surf_w,
        surf_h,
        folder_r,
        1,
        if state.custom_save_dir.is_some() { ACCENT_CYAN } else { BORDER_STRONG },
    );
    let folder_disp = state
        .custom_save_dir
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| default_download_dir.to_string());
    let folder_short = folder_disp
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(&folder_disp);
    let folder_lbl = format!("Save: {} [Set]", folder_short);
    Font::draw_text_max_width(
        surface,
        surf_w,
        surf_h,
        folder_r.x + 8,
        folder_r.y + 7,
        &folder_lbl,
        folder_r.w - 16,
        if state.custom_save_dir.is_some() { ACCENT_CYAN } else { TEXT_SECONDARY },
    );

    fill_rounded_rect(surface, surf_w, surf_h, dl_r, 4, if can_download { ACCENT_EMERALD } else { BG_ELEVATED });
    Font::draw_text(surface, surf_w, surf_h, dl_r.x + 16, dl_r.y + 9, cta, if can_download { 0xFFFFFFFF } else { TEXT_DIM });
}

pub fn handle_add_url_click(
    surf_w: usize,
    surf_h: usize,
    px: i32,
    py: i32,
    state: &mut AddUrlModalState,
) -> ModalAction {
    let (mx, my, modal_w, modal_h) = add_url_modal_bounds(surf_w, surf_h);

    let close_r = Rect::new(mx + modal_w - 34, my + 8, 24, 24);
    if close_r.contains_coords(px, py) {
        return ModalAction::Close;
    }

    let input_w = modal_w - 220;
    let paste_r = Rect::new(mx + 16 + input_w + 8, my + 70, 74, 30);
    if paste_r.contains_coords(px, py) {
        return ModalAction::RequestPasteUrl;
    }

    let fetch_r = Rect::new(paste_r.right() + 8, my + 70, 102, 30);
    if fetch_r.contains_coords(px, py) {
        let u = state.url_input.trim().to_string();
        if !u.is_empty() {
            state.preview_state = PreviewFetchState::Loading(u.clone());
            return ModalAction::FetchPreview(u);
        }
        return ModalAction::None;
    }

    // 2-column quality grid click handling
    let card_r = Rect::new(mx + 16, my + 114, modal_w - 32, modal_h - 176);
    if let PreviewFetchState::Ready {
        preview,
        selected_quality_idx,
        ..
    } = &mut state.preview_state
    {
        let grid_y0 = card_r.y + 12 + 124 + 10 + 18;
        let col_gap = 8;
        let col_w = (card_r.w - 24 - col_gap) / 2;
        let row_h = 26;
        if px >= card_r.x + 12 && px <= card_r.right() - 12 && py >= grid_y0 && py < card_r.bottom() - 6 {
            let row = ((py - grid_y0) / (row_h + 4)).max(0) as usize;
            let rel_x = px - (card_r.x + 12);
            let col = if rel_x >= col_w + col_gap { 1 } else { 0 };
            let idx = row * 2 + col;
            if idx < preview.qualities.len() {
                *selected_quality_idx = idx;
                return ModalAction::None;
            }
        }
    }

    // Thread selector & Save Folder picker clicks
    let foot_y = my + modal_h - 46;
    for (idx, &t) in [1usize, 4, 8, 16].iter().enumerate() {
        let bx = mx + 82 + (idx as i32) * 42;
        let br = Rect::new(bx, foot_y + 4, 36, 24);
        if br.contains_coords(px, py) {
            state.threads = t;
            return ModalAction::None;
        }
    }

    let dl_r = Rect::new(mx + modal_w - 186, foot_y + 2, 170, 28);
    let folder_btn_x = mx + 256;
    let folder_btn_w = (dl_r.x - folder_btn_x - 10).max(90);
    if Rect::new(folder_btn_x, foot_y + 4, folder_btn_w, 24).contains_coords(px, py) {
        return ModalAction::ChooseModalSaveFolder;
    }

    // Download CTA
    if dl_r.contains_coords(px, py) && !state.url_input.trim().is_empty() {
        match &state.preview_state {
            PreviewFetchState::Idle => {
                // First resolve the URL preview instead of blindly downloading a webpage!
                let u = state.url_input.trim().to_string();
                state.preview_state = PreviewFetchState::Loading(u.clone());
                return ModalAction::FetchPreview(u);
            }
            PreviewFetchState::Loading(_) => return ModalAction::None,
            PreviewFetchState::Ready {
                preview,
                selected_quality_idx,
                ..
            } => {
                if preview.original_url.starts_with("magnet:?") {
                    return ModalAction::StartTorrentOrMagnet {
                        input: preview.original_url.clone(),
                        sequential: false,
                        dest_dir: state.custom_save_dir.clone(),
                    };
                }
                if let Some(q) = preview.qualities.get(*selected_quality_idx) {
                    if q.label.contains("[PO-Token Gated]") {
                        return ModalAction::None;
                    }
                    let filename = build_quality_filename(&preview.suggested_filename, q);
                    return ModalAction::StartStream {
                        url: q.url.clone(),
                        filename,
                        resolution: q.label.clone(),
                        size: q.estimated_size_bytes,
                        threads: state.threads,
                        is_hls: q.is_hls,
                        referer: Some(preview.original_url.clone()),
                        dest_dir: state.custom_save_dir.clone(),
                    };
                }
                // Do NOT download an HTML page (e.g. YouTube watch page) when qualities is empty!
                if matches!(preview.kind, PreviewKind::YouTube | PreviewKind::WebVideoPage) {
                    return ModalAction::None;
                }
            }
            PreviewFetchState::Error(_) => {
                let u = state.url_input.trim().to_string();
                if u.starts_with("magnet:?") {
                    return ModalAction::StartTorrentOrMagnet {
                        input: u,
                        sequential: false,
                        dest_dir: state.custom_save_dir.clone(),
                    };
                }
                let fname = vortex_core::network::http::filename_from_url(&u);
                return ModalAction::StartStream {
                    is_hls: u.to_ascii_lowercase().contains(".m3u8"),
                    url: u,
                    filename: fname,
                    resolution: "Direct".into(),
                    size: 0,
                    threads: state.threads,
                    referer: None,
                    dest_dir: state.custom_save_dir.clone(),
                };
            }
        }
    }

    ModalAction::None
}

fn readable_ascii_title(title: &str, fallback_filename: &str) -> String {
    if title.chars().any(|c| c.is_ascii_alphanumeric()) {
        title.to_string()
    } else {
        format!("Video ({})", fallback_filename)
    }
}

fn build_quality_filename(base_filename: &str, q: &vortex_core::sniffer::MediaStreamQuality) -> String {
    let ext = if q.is_audio_only {
        if q.mime_type.contains("webm") { "webm" } else { "m4a" }
    } else if q.mime_type.contains("webm") {
        "webm"
    } else {
        "mp4"
    };

    let stem = base_filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(base_filename);

    let q_tag = if q.is_audio_only {
        "Audio".to_string()
    } else if q.resolution.1 > 0 {
        format!("{}p", q.resolution.1)
    } else {
        String::new()
    };

    if q_tag.is_empty() {
        format!("{}.{}", stem, ext)
    } else {
        format!("{} [{}].{}", stem, q_tag, ext)
    }
}

pub fn render_add_torrent_modal(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    state: &AddTorrentModalState,
    cursor_blink: bool,
) {
    let modal_w = 560;
    let modal_h = 250;
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;

    fill_rect(surface, surf_w, surf_h, Rect::new(0, 0, surf_w as i32, surf_h as i32), 0xAA000000);
    let modal_r = Rect::new(mx, my, modal_w, modal_h);
    fill_rounded_rect(surface, surf_w, surf_h, modal_r, 8, BG_SURFACE);
    draw_rect_outline(surface, surf_w, surf_h, modal_r, 2, ACCENT_BLUE);

    fill_rect(surface, surf_w, surf_h, Rect::new(mx + 2, my + 2, modal_w - 4, 38), BG_PANEL);
    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 15, "Add Torrent File or Magnet URI", ACCENT_CYAN);

    let close_r = Rect::new(mx + modal_w - 34, my + 8, 24, 24);
    fill_rounded_rect(surface, surf_w, surf_h, close_r, 4, 0xFF2B1420);
    draw_rect_outline(surface, surf_w, surf_h, close_r, 1, ACCENT_ROSE);
    Font::draw_text(surface, surf_w, surf_h, close_r.x + 8, close_r.y + 7, "X", ACCENT_ROSE);

    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 56, "Enter path to .torrent file OR paste magnet:?xt=urn:btih:... link:", TEXT_MUTED);

    let input_r = Rect::new(mx + 16, my + 76, modal_w - 32, 30);
    fill_rounded_rect(surface, surf_w, surf_h, input_r, 4, BG_ROOT);
    draw_rect_outline(surface, surf_w, surf_h, input_r, 1, ACCENT_CYAN);
    let disp = if state.input.is_empty() {
        "magnet:?xt=urn:btih:... or /path/to/file.torrent".to_string()
    } else {
        let max_c = ((input_r.w - 18) / 7) as usize;
        let len = state.input.chars().count();
        let tail: String = if len > max_c {
            state.input.chars().skip(len - max_c).collect()
        } else {
            state.input.clone()
        };
        if cursor_blink { format!("{}|", tail) } else { tail }
    };
    Font::draw_text(
        surface,
        surf_w,
        surf_h,
        input_r.x + 8,
        input_r.y + 10,
        &disp,
        if state.input.is_empty() { TEXT_DIM } else { TEXT_PRIMARY },
    );

    let paste_r = Rect::new(mx + 16, my + 116, 110, 26);
    fill_rounded_rect(surface, surf_w, surf_h, paste_r, 4, BG_ELEVATED);
    draw_rect_outline(surface, surf_w, surf_h, paste_r, 1, BORDER_STRONG);
    Font::draw_text(surface, surf_w, surf_h, paste_r.x + 10, paste_r.y + 8, "Paste Magnet", TEXT_PRIMARY);

    let browse_r = Rect::new(paste_r.right() + 10, my + 116, 150, 26);
    fill_rounded_rect(surface, surf_w, surf_h, browse_r, 4, BG_ELEVATED);
    draw_rect_outline(surface, surf_w, surf_h, browse_r, 1, BORDER_STRONG);
    Font::draw_text(surface, surf_w, surf_h, browse_r.x + 12, browse_r.y + 8, "Browse .torrent...", TEXT_PRIMARY);

    let save_r = Rect::new(browse_r.right() + 10, my + 116, modal_w - (browse_r.right() - mx) - 26, 26);
    fill_rounded_rect(surface, surf_w, surf_h, save_r, 4, BG_ELEVATED);
    draw_rect_outline(
        surface,
        surf_w,
        surf_h,
        save_r,
        1,
        if state.custom_save_dir.is_some() { ACCENT_CYAN } else { BORDER_STRONG },
    );
    let save_lbl = state
        .custom_save_dir
        .as_ref()
        .and_then(|p| p.file_name().map(|n| format!("Save: {}", n.to_string_lossy())))
        .unwrap_or_else(|| "Save Folder...".into());
    Font::draw_text_max_width(
        surface,
        surf_w,
        surf_h,
        save_r.x + 10,
        save_r.y + 8,
        &save_lbl,
        save_r.w - 20,
        if state.custom_save_dir.is_some() { ACCENT_CYAN } else { TEXT_PRIMARY },
    );

    let seq_r = Rect::new(mx + 16, my + 158, 16, 16);
    fill_rounded_rect(surface, surf_w, surf_h, seq_r, 3, if state.sequential { ACCENT_BLUE } else { BG_ROOT });
    draw_rect_outline(surface, surf_w, surf_h, seq_r, 1, BORDER_STRONG);
    if state.sequential {
        Font::draw_text(surface, surf_w, surf_h, seq_r.x + 4, seq_r.y + 3, "v", 0xFFFFFFFF);
    }
    Font::draw_text(surface, surf_w, surf_h, seq_r.right() + 10, seq_r.y + 3, "Sequential Streaming Mode (preview video while downloading)", TEXT_SECONDARY);

    if let Some(err) = &state.error {
        Font::draw_text_max_width(surface, surf_w, surf_h, mx + 16, my + 186, err, modal_w - 32, ACCENT_ROSE);
    }

    let start_r = Rect::new(mx + modal_w - 166, my + modal_h - 42, 150, 28);
    fill_rounded_rect(surface, surf_w, surf_h, start_r, 4, ACCENT_EMERALD);
    Font::draw_text(surface, surf_w, surf_h, start_r.x + 24, start_r.y + 9, "Start Torrent", 0xFFFFFFFF);
}

pub fn handle_add_torrent_click(
    surf_w: usize,
    surf_h: usize,
    px: i32,
    py: i32,
    state: &mut AddTorrentModalState,
) -> ModalAction {
    let modal_w = 560;
    let modal_h = 250;
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;

    if Rect::new(mx + modal_w - 34, my + 8, 24, 24).contains_coords(px, py) {
        return ModalAction::Close;
    }
    if Rect::new(mx + 16, my + 116, 110, 26).contains_coords(px, py) {
        return ModalAction::RequestPasteTorrent;
    }
    if Rect::new(mx + 136, my + 116, 150, 26).contains_coords(px, py) {
        return ModalAction::RequestBrowseTorrent;
    }
    if Rect::new(mx + 296, my + 116, modal_w - 312, 26).contains_coords(px, py) {
        return ModalAction::ChooseModalSaveFolder;
    }
    if Rect::new(mx + 16, my + 154, 420, 24).contains_coords(px, py) {
        state.sequential = !state.sequential;
        return ModalAction::None;
    }
    if Rect::new(mx + modal_w - 166, my + modal_h - 42, 150, 28).contains_coords(px, py) {
        let inp = state.input.trim().to_string();
        if inp.is_empty() {
            state.error = Some("Please paste a magnet link or choose a .torrent file".into());
            return ModalAction::None;
        }
        return ModalAction::StartTorrentOrMagnet {
            input: inp,
            sequential: state.sequential,
            dest_dir: state.custom_save_dir.clone(),
        };
    }
    ModalAction::None
}

pub fn render_settings_modal(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    threads: usize,
    speed_limit_bps: u64,
    download_dir: &str,
    bridge_port: u16,
    clipboard_monitor: bool,
) {
    let modal_w = 520;
    let modal_h = 336;
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;

    fill_rect(surface, surf_w, surf_h, Rect::new(0, 0, surf_w as i32, surf_h as i32), 0xAA000000);
    let modal_r = Rect::new(mx, my, modal_w, modal_h);
    fill_rounded_rect(surface, surf_w, surf_h, modal_r, 8, BG_SURFACE);
    draw_rect_outline(surface, surf_w, surf_h, modal_r, 2, ACCENT_BLUE);

    fill_rect(surface, surf_w, surf_h, Rect::new(mx + 2, my + 2, modal_w - 4, 38), BG_PANEL);
    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 15, "Vortex Settings & Bandwidth Throttler", ACCENT_CYAN);

    let close_r = Rect::new(mx + modal_w - 34, my + 8, 24, 24);
    fill_rounded_rect(surface, surf_w, surf_h, close_r, 4, 0xFF2B1420);
    draw_rect_outline(surface, surf_w, surf_h, close_r, 1, ACCENT_ROSE);
    Font::draw_text(surface, surf_w, surf_h, close_r.x + 8, close_r.y + 7, "X", ACCENT_ROSE);

    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 54, "Default Download Folder (Files & Videos):", TEXT_MUTED);
    let path_r = Rect::new(mx + 16, my + 70, modal_w - 224, 24);
    fill_rounded_rect(surface, surf_w, surf_h, path_r, 4, BG_ROOT);
    draw_rect_outline(surface, surf_w, surf_h, path_r, 1, BORDER_SUBTLE);
    Font::draw_text_max_width(surface, surf_w, surf_h, path_r.x + 8, path_r.y + 7, download_dir, path_r.w - 16, TEXT_PRIMARY);

    let change_r = Rect::new(mx + modal_w - 200, my + 70, 90, 24);
    fill_rounded_rect(surface, surf_w, surf_h, change_r, 4, ACCENT_BLUE);
    Font::draw_text(surface, surf_w, surf_h, change_r.x + 12, change_r.y + 7, "Change...", 0xFFFFFFFF);

    let open_r = Rect::new(mx + modal_w - 102, my + 70, 86, 24);
    fill_rounded_rect(surface, surf_w, surf_h, open_r, 4, BG_ELEVATED);
    draw_rect_outline(surface, surf_w, surf_h, open_r, 1, BORDER_STRONG);
    Font::draw_text(surface, surf_w, surf_h, open_r.x + 12, open_r.y + 7, "Open Dir", TEXT_PRIMARY);

    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 108, "Default IDM Connections per Download:", TEXT_MUTED);
    for (i, &t) in [1usize, 4, 8, 16].iter().enumerate() {
        let br = Rect::new(mx + 16 + (i as i32) * 60, my + 128, 52, 26);
        fill_rounded_rect(surface, surf_w, surf_h, br, 4, if threads == t { ACCENT_BLUE } else { BG_ELEVATED });
        Font::draw_text(surface, surf_w, surf_h, br.x + 14, br.y + 8, &format!("{}x", t), TEXT_PRIMARY);
    }

    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 170, "Global Download Speed Limit:", TEXT_MUTED);
    let limits: [(u64, &str); 5] = [
        (0, "Unlimited"),
        (2 * 1024 * 1024, "2 MB/s"),
        (5 * 1024 * 1024, "5 MB/s"),
        (10 * 1024 * 1024, "10 MB/s"),
        (25 * 1024 * 1024, "25 MB/s"),
    ];
    for (i, &(lim, lbl)) in limits.iter().enumerate() {
        let br = Rect::new(mx + 16 + (i as i32) * 96, my + 190, 88, 26);
        fill_rounded_rect(surface, surf_w, surf_h, br, 4, if speed_limit_bps == lim { ACCENT_BLUE } else { BG_ELEVATED });
        Font::draw_text(surface, surf_w, surf_h, br.x + 10, br.y + 8, lbl, TEXT_PRIMARY);
    }

    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 232, "Extensionless Clipboard Auto-Capture (any browser):", TEXT_MUTED);
    let clip_r = Rect::new(mx + 16, my + 250, 210, 26);
    fill_rounded_rect(surface, surf_w, surf_h, clip_r, 4, if clipboard_monitor { ACCENT_EMERALD } else { BG_ELEVATED });
    draw_rect_outline(surface, surf_w, surf_h, clip_r, 1, if clipboard_monitor { ACCENT_EMERALD } else { BORDER_STRONG });
    let clip_lbl = if clipboard_monitor { "Clipboard Monitor: ON" } else { "Clipboard Monitor: OFF" };
    Font::draw_text(surface, surf_w, surf_h, clip_r.x + 12, clip_r.y + 8, clip_lbl, TEXT_PRIMARY);

    let bridge_line = format!("Pure Rust IPC Bridge: Active on http://127.0.0.1:{}", bridge_port);
    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 292, &bridge_line, ACCENT_EMERALD);
    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 312, "Chrome/Brave/Edge/Firefox extensions + Aero Browser + Clipboard.", TEXT_MUTED);
}

pub fn handle_settings_click(surf_w: usize, surf_h: usize, px: i32, py: i32) -> ModalAction {
    let modal_w = 520;
    let modal_h = 336;
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;

    if Rect::new(mx + modal_w - 34, my + 8, 24, 24).contains_coords(px, py) {
        return ModalAction::Close;
    }
    if Rect::new(mx + modal_w - 200, my + 70, 90, 24).contains_coords(px, py) {
        return ModalAction::ChooseDefaultDownloadFolder;
    }
    if Rect::new(mx + modal_w - 102, my + 70, 86, 24).contains_coords(px, py) {
        return ModalAction::OpenDownloadFolder;
    }
    for (i, &t) in [1usize, 4, 8, 16].iter().enumerate() {
        if Rect::new(mx + 16 + (i as i32) * 60, my + 128, 52, 26).contains_coords(px, py) {
            return ModalAction::SetThreads(t);
        }
    }
    let limits = [0, 2 * 1024 * 1024, 5 * 1024 * 1024, 10 * 1024 * 1024, 25 * 1024 * 1024];
    for (i, &lim) in limits.iter().enumerate() {
        if Rect::new(mx + 16 + (i as i32) * 96, my + 190, 88, 26).contains_coords(px, py) {
            return ModalAction::SetSpeedLimit(lim);
        }
    }
    if Rect::new(mx + 16, my + 250, 210, 26).contains_coords(px, py) {
        return ModalAction::ToggleClipboardMonitor;
    }
    ModalAction::None
}

pub fn render_video_preview_modal(
    surface: &mut [u32],
    surf_w: usize,
    surf_h: usize,
    task: &vortex_core::engine::DownloadTask,
    thumbnail: Option<&DecodedImage>,
) {
    let modal_w = 560;
    let modal_h = 280;
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;

    fill_rect(surface, surf_w, surf_h, Rect::new(0, 0, surf_w as i32, surf_h as i32), 0xAA000000);
    let modal_r = Rect::new(mx, my, modal_w, modal_h);
    fill_rounded_rect(surface, surf_w, surf_h, modal_r, 8, BG_SURFACE);
    draw_rect_outline(surface, surf_w, surf_h, modal_r, 2, ACCENT_CYAN);

    fill_rect(surface, surf_w, surf_h, Rect::new(mx + 2, my + 2, modal_w - 4, 38), BG_PANEL);
    Font::draw_text(surface, surf_w, surf_h, mx + 16, my + 15, "Video Preview & Stream Inspector", ACCENT_CYAN);

    let close_r = Rect::new(mx + modal_w - 34, my + 8, 24, 24);
    fill_rounded_rect(surface, surf_w, surf_h, close_r, 4, 0xFF2B1420);
    draw_rect_outline(surface, surf_w, surf_h, close_r, 1, ACCENT_ROSE);
    Font::draw_text(surface, surf_w, surf_h, close_r.x + 8, close_r.y + 7, "X", ACCENT_ROSE);

    // Left 16:9 Thumbnail / Preview Card
    let thumb_r = Rect::new(mx + 16, my + 54, 224, 126);
    fill_rounded_rect(surface, surf_w, surf_h, thumb_r, 6, BG_ROOT);
    draw_rect_outline(surface, surf_w, surf_h, thumb_r, 1, BORDER_STRONG);
    if let Some(img) = thumbnail {
        blit_image_fit(surface, surf_w, surf_h, thumb_r.inset(2, 2), img);
    } else {
        Font::draw_text(surface, surf_w, surf_h, thumb_r.x + 76, thumb_r.y + 48, "[ VIDEO ]", ACCENT_CYAN);
        if let vortex_core::engine::DownloadTask::Stream(st) = task {
            Font::draw_text_max_width(
                surface,
                surf_w,
                surf_h,
                thumb_r.x + 16,
                thumb_r.y + 72,
                &st.resolution,
                thumb_r.w - 32,
                TEXT_SECONDARY,
            );
        }
    }

    // Play badge overlay bottom-right of thumbnail
    let tag_r = Rect::new(thumb_r.right() - 62, thumb_r.bottom() - 24, 54, 18);
    fill_rounded_rect(surface, surf_w, surf_h, tag_r, 4, 0xCC0D1523);
    draw_rect_outline(surface, surf_w, surf_h, tag_r, 1, ACCENT_EMERALD);
    Font::draw_text(surface, surf_w, surf_h, tag_r.x + 8, tag_r.y + 4, "> PLAY", ACCENT_EMERALD);

    // Right: Type, Name, Size, Status, Path
    let info_x = thumb_r.right() + 14;
    let info_w = mx + modal_w - info_x - 16;
    let kind_lbl = if task.is_torrent() { "Type: TORRENT VIDEO" } else { "Type: STREAM / VIDEO" };
    Font::draw_text(surface, surf_w, surf_h, info_x, my + 56, kind_lbl, ACCENT_ORANGE);
    Font::draw_text_max_width(surface, surf_w, surf_h, info_x, my + 78, task.name(), info_w, TEXT_PRIMARY);

    let qual_txt = match task {
        vortex_core::engine::DownloadTask::Stream(st) => {
            format!("Quality: {} | Threads: {}", st.resolution, st.active_threads)
        }
        vortex_core::engine::DownloadTask::Torrent(tt) => {
            format!("Pieces: {}/{}", tt.piece_picker.bitfield.count_completed(), tt.piece_picker.num_pieces)
        }
    };
    Font::draw_text_max_width(surface, surf_w, surf_h, info_x, my + 102, &qual_txt, info_w, ACCENT_CYAN);

    let size_mb = task.total_size() as f32 / (1024.0 * 1024.0);
    let dl_mb = task.downloaded_bytes() as f32 / (1024.0 * 1024.0);
    let stats = format!(
        "Progress: {:.0}% ({:.1} / {:.1} MB) | {}",
        task.progress_pct(),
        dl_mb,
        size_mb,
        task.status().label()
    );
    Font::draw_text_max_width(surface, surf_w, surf_h, info_x, my + 124, &stats, info_w, ACCENT_EMERALD);

    Font::draw_text(surface, surf_w, surf_h, info_x, my + 150, "Saved To:", TEXT_MUTED);
    Font::draw_text_max_width(
        surface,
        surf_w,
        surf_h,
        info_x,
        my + 168,
        &task.save_path().to_string_lossy(),
        info_w,
        TEXT_SECONDARY,
    );

    // Bottom Actions
    let foot_y = my + modal_h - 46;
    let play_r = Rect::new(mx + 16, foot_y + 2, 136, 28);
    fill_rounded_rect(surface, surf_w, surf_h, play_r, 4, ACCENT_EMERALD);
    Font::draw_text(surface, surf_w, surf_h, play_r.x + 18, play_r.y + 9, "> Play Video", 0xFFFFFFFF);

    let folder_r = Rect::new(play_r.right() + 10, foot_y + 2, 120, 28);
    fill_rounded_rect(surface, surf_w, surf_h, folder_r, 4, BG_ELEVATED);
    draw_rect_outline(surface, surf_w, surf_h, folder_r, 1, BORDER_STRONG);
    Font::draw_text(surface, surf_w, surf_h, folder_r.x + 16, folder_r.y + 9, "Open Folder", TEXT_PRIMARY);

    let done_r = Rect::new(mx + modal_w - 106, foot_y + 2, 90, 28);
    fill_rounded_rect(surface, surf_w, surf_h, done_r, 4, ACCENT_BLUE);
    Font::draw_text(surface, surf_w, surf_h, done_r.x + 24, done_r.y + 9, "Close", 0xFFFFFFFF);
}

pub fn handle_video_preview_click(
    surf_w: usize,
    surf_h: usize,
    px: i32,
    py: i32,
    task: &vortex_core::engine::DownloadTask,
) -> ModalAction {
    let modal_w = 560;
    let modal_h = 280;
    let mx = ((surf_w as i32) - modal_w) / 2;
    let my = ((surf_h as i32) - modal_h) / 2;
    let foot_y = my + modal_h - 46;

    if Rect::new(mx + modal_w - 34, my + 8, 24, 24).contains_coords(px, py)
        || Rect::new(mx + modal_w - 106, foot_y + 2, 90, 28).contains_coords(px, py)
    {
        return ModalAction::Close;
    }

    if Rect::new(mx + 16, my + 54, 224, 126).contains_coords(px, py)
        || Rect::new(mx + 16, foot_y + 2, 136, 28).contains_coords(px, py)
    {
        return ModalAction::PlayMediaFile(task.save_path().clone());
    }

    if Rect::new(mx + 162, foot_y + 2, 120, 28).contains_coords(px, py) {
        return ModalAction::OpenDownloadFolder;
    }

    ModalAction::None
}

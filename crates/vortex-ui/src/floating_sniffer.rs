//! vortex-ui/src/floating_sniffer.rs
//! IDM-Style Floating Video Download Badge & Quality Selection Dropdown.

use crate::draw::{draw_rect_outline, fill_rect, fill_rounded_rect};
use crate::font::Font;
use crate::geom::{Point, Rect};
use vortex_core::sniffer::{MediaStreamQuality, SniffedMedia};

pub struct FloatingSnifferBadge;

impl FloatingSnifferBadge {
    /// Renders the floating download badge and optional dropdown menu.
    pub fn render(
        surface: &mut [u32],
        surf_w: usize,
        surf_h: usize,
        anchor: Point,
        media: &SniffedMedia,
        dropdown_open: bool,
    ) {
        let btn_w = 210;
        let btn_h = 28;
        let btn_rect = Rect::new(anchor.x, anchor.y, btn_w, btn_h);

        // 1. Main Floating Pill Button
        fill_rounded_rect(surface, surf_w, surf_h, btn_rect, 6, 0xFF0284C7);
        draw_rect_outline(surface, surf_w, surf_h, btn_rect, 1, 0xFF38BDF8);

        // Download Icon & Label
        Font::draw_text(surface, surf_w, surf_h, btn_rect.x + 8, btn_rect.y + 8, "v Download Video", 0xFFFFFFFF);

        // Best quality tag
        let tag = media.best_video_quality().map(|q| q.label.as_str()).unwrap_or("FHD");
        let tag_txt = format!("[{}] v", tag);
        Font::draw_text(surface, surf_w, surf_h, btn_rect.right() - 68, btn_rect.y + 8, &tag_txt, 0xFFE0F2FE);

        // 2. Dropdown Menu (if expanded)
        if dropdown_open {
            let menu_y = btn_rect.bottom() + 4;
            let menu_h = (media.qualities.len() as i32) * 26 + 8;
            let menu_rect = Rect::new(btn_rect.x, menu_y, btn_w + 30, menu_h);

            // Menu container with drop shadow & border
            fill_rounded_rect(surface, surf_w, surf_h, menu_rect, 6, 0xFF0F172A);
            draw_rect_outline(surface, surf_w, surf_h, menu_rect, 1, 0xFF0284C7);

            for (i, q) in media.qualities.iter().enumerate() {
                let row_y = menu_rect.y + 4 + (i as i32) * 26;
                let row_rect = Rect::new(menu_rect.x + 4, row_y, menu_rect.w - 8, 24);

                let is_hover = i == 0;
                let bg_color = if is_hover { 0xFF1E293B } else { 0xFF0F172A };
                fill_rounded_rect(surface, surf_w, surf_h, row_rect, 4, bg_color);

                let icon = if q.is_audio_only { "[Audio]" } else { "[Video]" };
                let size_mb = q.estimated_size_bytes as f32 / (1024.0 * 1024.0);
                let label = format!("{} {} ({:.1} MB)", icon, q.label, size_mb);

                let text_col = if is_hover { 0xFF38BDF8 } else { 0xFFE2E8F0 };
                Font::draw_text(surface, surf_w, surf_h, row_rect.x + 6, row_rect.y + 6, &label, text_col);

                if i + 1 < media.qualities.len() {
                    fill_rect(surface, surf_w, surf_h, Rect::new(menu_rect.x + 6, row_y + 25, menu_rect.w - 12, 1), 0xFF1E293B);
                }
            }
        }
    }

    /// Evaluates click within floating badge or dropdown options.
    pub fn handle_click(
        anchor: Point,
        media: &SniffedMedia,
        dropdown_open: bool,
        px: i32,
        py: i32,
    ) -> Option<FloatingClickResult> {
        let btn_w = 210;
        let btn_h = 28;
        let btn_rect = Rect::new(anchor.x, anchor.y, btn_w, btn_h);

        // Click on main button -> toggle dropdown
        if btn_rect.contains_coords(px, py) {
            return Some(FloatingClickResult::ToggleDropdown);
        }

        // Click on dropdown items
        if dropdown_open {
            let menu_y = btn_rect.bottom() + 4;
            let menu_h = (media.qualities.len() as i32) * 26 + 8;
            let menu_rect = Rect::new(btn_rect.x, menu_y, btn_w + 30, menu_h);

            if menu_rect.contains_coords(px, py) {
                let item_idx = ((py - (menu_rect.y + 4)) / 26).max(0) as usize;
                if let Some(quality) = media.qualities.get(item_idx) {
                    return Some(FloatingClickResult::StartDownload(quality.clone()));
                }
            }
        }

        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FloatingClickResult {
    ToggleDropdown,
    StartDownload(MediaStreamQuality),
}

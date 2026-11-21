//! vortex-ui/src/piece_grid.rs
//! Iconic BitTorrent Piece Map Visualizer (uTorrent & Transmission style).

use crate::draw::{draw_rect_outline, fill_rect, fill_rounded_rect};
use crate::font::Font;
use crate::geom::Rect;
use vortex_core::piece_picker::{Bitfield, PieceState};

pub struct PieceGridView;

impl PieceGridView {
    /// Renders a responsive 2D piece block matrix.
    pub fn render(
        surface: &mut [u32],
        surf_w: usize,
        surf_h: usize,
        bounds: Rect,
        bitfield: &Bitfield,
        states: &[PieceState],
        piece_length: u32,
    ) {
        if bounds.w <= 0 || bounds.h <= 0 {
            return;
        }

        // 1. Header / Legend bar
        let header_h = 24;
        let header_rect = Rect::new(bounds.x, bounds.y, bounds.w, header_h);
        fill_rounded_rect(surface, surf_w, surf_h, header_rect, 4, 0xFF1E293B);

        let completed = bitfield.count_completed();
        let total = bitfield.num_pieces();
        let pct = bitfield.completion_pct();

        let legend_txt = format!(
            "Piece Map: {}/{} ({:.1}%) - {} KB/piece",
            completed,
            total,
            pct,
            piece_length / 1024
        );
        Font::draw_text(surface, surf_w, surf_h, bounds.x + 8, bounds.y + 7, &legend_txt, 0xFFE2E8F0);

        // Swatch indicators
        let swatch_x = bounds.right() - 220;
        fill_rect(surface, surf_w, surf_h, Rect::new(swatch_x, bounds.y + 7, 10, 10), 0xFF00B4D8);
        Font::draw_text(surface, surf_w, surf_h, swatch_x + 14, bounds.y + 7, "Done", 0xFF94A3B8);

        fill_rect(surface, surf_w, surf_h, Rect::new(swatch_x + 60, bounds.y + 7, 10, 10), 0xFFFF9E00);
        Font::draw_text(surface, surf_w, surf_h, swatch_x + 74, bounds.y + 7, "Loading", 0xFF94A3B8);

        fill_rect(surface, surf_w, surf_h, Rect::new(swatch_x + 140, bounds.y + 7, 10, 10), 0xFF334155);
        Font::draw_text(surface, surf_w, surf_h, swatch_x + 154, bounds.y + 7, "Missing", 0xFF94A3B8);

        // 2. Grid Canvas
        let grid_y = bounds.y + header_h + 6;
        let grid_h = bounds.h - header_h - 6;
        if grid_h <= 10 {
            return;
        }

        let grid_rect = Rect::new(bounds.x, grid_y, bounds.w, grid_h);
        fill_rounded_rect(surface, surf_w, surf_h, grid_rect, 4, 0xFF0F172A);
        draw_rect_outline(surface, surf_w, surf_h, grid_rect, 1, 0xFF334155);

        let cols = 32;
        let pad = 2;
        let cell_w = ((grid_rect.w - 12 - (cols - 1) * pad) / cols).max(4);
        let cell_h = 10;
        let max_rows = (grid_rect.h - 12) / (cell_h + pad);

        let max_display_pieces = (cols * max_rows) as usize;
        let display_count = total.min(max_display_pieces);

        for i in 0..display_count {
            let col = (i as i32) % cols;
            let row = (i as i32) / cols;

            let cx = grid_rect.x + 6 + col * (cell_w + pad);
            let cy = grid_rect.y + 6 + row * (cell_h + pad);

            let cell_rect = Rect::new(cx, cy, cell_w, cell_h);

            let color = if bitfield.has_piece(i) {
                0xFF00B4D8 // Cyan (Verified)
            } else if let Some(PieceState::Downloading { .. }) = states.get(i) {
                0xFFFF9E00 // Amber (Downloading)
            } else {
                0xFF1E293B // Dark slate (Missing)
            };

            fill_rect(surface, surf_w, surf_h, cell_rect, color);
        }
    }
}

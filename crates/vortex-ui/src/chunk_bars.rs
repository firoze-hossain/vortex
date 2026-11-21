//! vortex-ui/src/chunk_bars.rs
//! Iconic IDM-Style Multi-Threaded Chunk Stream Progress Bars.

use crate::draw::{draw_progress_bar, draw_rect_outline, fill_rounded_rect};
use crate::font::Font;
use crate::geom::Rect;
use vortex_core::segmented::SegmentChunk;

pub struct ChunkBarsView;

impl ChunkBarsView {
    /// Renders parallel thread streams for an IDM-style accelerated download.
    pub fn render(
        surface: &mut [u32],
        surf_w: usize,
        surf_h: usize,
        bounds: Rect,
        chunks: &[SegmentChunk],
        total_size: u64,
    ) {
        if bounds.w <= 0 || bounds.h <= 0 || chunks.is_empty() {
            return;
        }

        // 1. Header Bar
        let header_h = 24;
        let header_rect = Rect::new(bounds.x, bounds.y, bounds.w, header_h);
        fill_rounded_rect(surface, surf_w, surf_h, header_rect, 4, 0xFF1E293B);

        let active_threads = chunks.iter().filter(|c| c.downloaded_bytes < c.total_bytes()).count();
        let header_txt = format!(
            "IDM Stream Acceleration: {} Parallel Connections | Total: {:.1} MB",
            chunks.len(),
            total_size as f32 / (1024.0 * 1024.0)
        );
        Font::draw_text(surface, surf_w, surf_h, bounds.x + 8, bounds.y + 7, &header_txt, 0xFFE2E8F0);

        let active_txt = format!("Active: {}/{}", active_threads, chunks.len());
        Font::draw_text(surface, surf_w, surf_h, bounds.right() - 110, bounds.y + 7, &active_txt, 0xFF38BDF8);

        // 2. Thread progress bars list
        let list_y = bounds.y + header_h + 6;
        let list_h = bounds.h - header_h - 6;
        let list_rect = Rect::new(bounds.x, list_y, bounds.w, list_h);
        fill_rounded_rect(surface, surf_w, surf_h, list_rect, 4, 0xFF0F172A);
        draw_rect_outline(surface, surf_w, surf_h, list_rect, 1, 0xFF334155);

        let bar_h = 16;
        let bar_spacing = 20;

        for (i, chunk) in chunks.iter().enumerate() {
            let row_y = list_rect.y + 8 + (i as i32) * bar_spacing;
            if row_y + bar_h > list_rect.bottom() - 4 {
                break;
            }

            // Thread label
            let thread_lbl = format!("Th #{}:", i + 1);
            Font::draw_text(surface, surf_w, surf_h, list_rect.x + 8, row_y + 3, &thread_lbl, 0xFF94A3B8);

            // Progress Bar
            let pbar_x = list_rect.x + 56;
            let pbar_w = (list_rect.w - 240).max(60);
            let pbar_rect = Rect::new(pbar_x, row_y + 2, pbar_w, 12);
            let pct = chunk.progress_pct();

            draw_progress_bar(surface, surf_w, surf_h, pbar_rect, pct, 0xFF1E293B, 0xFF10B981);

            // Speed and percentage
            let speed_kb = chunk.speed_bps / 1024;
            let status_txt = if chunk.is_complete() {
                "100% [Done]".to_string()
            } else {
                format!("{:.1}% ({} KB/s)", pct, speed_kb)
            };
            Font::draw_text(surface, surf_w, surf_h, pbar_x + pbar_w + 12, row_y + 3, &status_txt, 0xFFE2E8F0);
        }
    }
}

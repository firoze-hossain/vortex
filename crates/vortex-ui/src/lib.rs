//! vortex-ui - UI Component Library and Visualizer Widgets in Pure Rust.

pub mod chunk_bars;
pub mod draw;
pub mod floating_sniffer;
pub mod font;
pub mod geom;
pub mod piece_grid;
pub mod view;

pub use chunk_bars::ChunkBarsView;
pub use floating_sniffer::{FloatingClickResult, FloatingSnifferBadge};
pub use font::Font;
pub use geom::{Point, Rect};
pub use piece_grid::PieceGridView;
pub use view::VortexView;

#[cfg(test)]
mod tests {
    use super::*;
    use vortex_core::engine::VortexEngine;
    use vortex_core::metainfo::TorrentMeta;

    #[test]
    fn test_vortex_view_creation_and_render() {
        let mut engine = VortexEngine::new();
        let meta = TorrentMeta::create_single_file("ubuntu-24.04-desktop.iso", 2_000_000_000, 1024 * 1024, None);
        engine.add_torrent(meta);
        engine.add_stream("YouTube: Building an OS in Rust [1080p].mp4", "https://youtube.com/watch?v=os", "1080p FHD", 120_000_000, 8);

        let view = VortexView::new(1024, 768);
        let mut surface = vec![0u32; 1024 * 768];
        view.render(&mut surface, &engine);

        // Verify non-empty render
        assert!(surface.iter().any(|&p| p != 0));
        assert!(surface.iter().any(|&p| p == 0xFF38BDF8)); // Cyan brand color
    }

    #[test]
    fn test_floating_sniffer_badge_interaction() {
        let mut sniffer = vortex_core::sniffer::MediaSniffer::new();
        let detected = sniffer.inspect_url(
            "https://www.youtube.com/watch?v=rust_tour",
            "https://www.youtube.com/watch?v=rust_tour",
            "A Tour of Rust 1.99",
        ).unwrap();

        let anchor = Point::new(300, 100);
        let mut surface = vec![0u32; 800 * 600];

        // Render badge
        FloatingSnifferBadge::render(&mut surface, 800, 600, anchor, detected, false);
        assert!(surface.iter().any(|&p| p != 0));

        // Click badge to toggle dropdown
        let res1 = FloatingSnifferBadge::handle_click(anchor, detected, false, 350, 110);
        assert_eq!(res1, Some(FloatingClickResult::ToggleDropdown));

        // Click item 0 in dropdown
        let res2 = FloatingSnifferBadge::handle_click(anchor, detected, true, 350, 140);
        match res2 {
            Some(FloatingClickResult::StartDownload(q)) => {
                assert_eq!(q.label, "1080p FHD");
            }
            _ => panic!("Expected start download with 1080p FHD"),
        }
    }

    #[test]
    fn test_vortex_view_click_dispatch() {
        let mut engine = VortexEngine::new();
        let mut view = VortexView::new(1024, 768);

        // Click + Torrent in toolbar
        view.handle_click(160, 20, &mut engine);
        assert_eq!(engine.tasks.len(), 1);
        assert!(view.selected_task_id.is_some());

        // Click + Add URL in toolbar
        view.handle_click(250, 20, &mut engine);
        assert_eq!(engine.tasks.len(), 2);

        // Click Category "Torrents" in sidebar (y = 86 + 32 = 118)
        view.handle_click(50, 120, &mut engine);
        assert_eq!(view.active_filter, vortex_core::engine::TaskFilter::Torrents);

        // Click About button [?] (x: 1024 - 30 = 994, y: 20)
        view.handle_click(1024 - 30, 20, &mut engine);
        assert!(view.show_about);

        // Click to dismiss About
        view.handle_click(100, 100, &mut engine);
        assert!(!view.show_about);
    }
}

//! vortex-ui/src/lib.rs
//! UI Component Library, Thumbnail Image Decoders, Interactive Modals, and Visualizer Widgets.

pub mod chunk_bars;
pub mod draw;
pub mod floating_sniffer;
pub mod font;
pub mod geom;
pub mod image_decode;
pub mod modals;
pub mod piece_grid;
pub mod theme;
pub mod view;

pub use chunk_bars::ChunkBarsView;
pub use floating_sniffer::{FloatingClickResult, FloatingSnifferBadge};
pub use font::Font;
pub use geom::{Point, Rect};
pub use image_decode::{decode_image, DecodedImage};
pub use modals::{ActiveModal, AddTorrentModalState, AddUrlModalState, ModalAction, PreviewFetchState};
pub use piece_grid::PieceGridView;
pub use view::{ViewCommand, VortexView};

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
        engine.add_stream(
            "YouTube: Building an OS in Rust [1080p].mp4",
            "https://youtube.com/watch?v=os",
            "1080p FHD",
            120_000_000,
            8,
        );

        let view = VortexView::new(1080, 760);
        let mut surface = vec![0u32; 1080 * 760];
        view.render(&mut surface, &engine);

        assert!(surface.iter().any(|&p| p != 0));
        assert!(surface.iter().any(|&p| p == theme::ACCENT_CYAN));
    }

    #[test]
    fn test_floating_sniffer_badge_interaction() {
        let mut sniffer = vortex_core::sniffer::MediaSniffer::new();
        let detected = sniffer
            .inspect_url(
                "https://www.youtube.com/watch?v=rust_tour",
                "https://www.youtube.com/watch?v=rust_tour",
                "A Tour of Rust 1.99",
            )
            .unwrap();

        let anchor = Point::new(300, 100);
        let mut surface = vec![0u32; 800 * 600];

        FloatingSnifferBadge::render(&mut surface, 800, 600, anchor, detected, false);
        assert!(surface.iter().any(|&p| p != 0));

        let res1 = FloatingSnifferBadge::handle_click(anchor, detected, false, 350, 110);
        assert_eq!(res1, Some(FloatingClickResult::ToggleDropdown));

        let res2 = FloatingSnifferBadge::handle_click(anchor, detected, true, 350, 140);
        match res2 {
            Some(FloatingClickResult::StartDownload(_)) => {}
            _ => panic!("Expected start download"),
        }
    }

    #[test]
    fn test_vortex_view_click_dispatch_opens_modals_and_filters() {
        let mut engine = VortexEngine::new();
        let mut view = VortexView::new(1080, 760);

        // Click + Torrent in toolbar -> opens AddTorrent modal
        view.handle_click(160, 20, &mut engine);
        assert!(matches!(view.modal, ActiveModal::AddTorrent(_)));
        // Close modal
        view.modal = ActiveModal::None;

        // Click + Add URL in toolbar -> opens AddUrl modal
        view.handle_click(250, 20, &mut engine);
        assert!(matches!(view.modal, ActiveModal::AddUrl(_)));
        view.modal = ActiveModal::None;

        // Click Category "Torrents" in sidebar (y = 86 + 33 = 119)
        view.handle_click(50, 125, &mut engine);
        assert_eq!(view.active_filter, vortex_core::engine::TaskFilter::Torrents);

        // Click About button [?]
        view.handle_click(1080 - 147, 20, &mut engine);
        assert!(view.show_about);

        // Click to dismiss About
        view.handle_click(100, 100, &mut engine);
        assert!(!view.show_about);

        // Click Fullscreen, Minimize, Maximize, Close [X] window controls
        assert_eq!(view.handle_click_with_actions(1080 - 115, 20, &mut engine), ViewCommand::ToggleFullscreen);
        assert_eq!(view.handle_click_with_actions(1080 - 85, 20, &mut engine), ViewCommand::Minimize);
        assert_eq!(view.handle_click_with_actions(1080 - 55, 20, &mut engine), ViewCommand::ToggleMaximize);
        assert_eq!(view.handle_click_with_actions(1080 - 24, 20, &mut engine), ViewCommand::Quit);
    }

    #[test]
    fn test_add_url_modal_quality_selection_and_empty_guard() {
        use vortex_core::media::preview::{MediaPreview, PreviewKind};
        use vortex_core::sniffer::MediaStreamQuality;

        let mut st = AddUrlModalState::default();
        st.url_input = "https://www.youtube.com/watch?v=Xxar--p3WdU".into();

        // 1. Empty qualities on YouTube should NOT download the /watch HTML page
        st.set_preview_result(Ok(MediaPreview {
            original_url: st.url_input.clone(),
            kind: PreviewKind::YouTube,
            title: "Test".into(),
            author: None,
            duration_secs: Some(120),
            source_domain: "www.youtube.com".into(),
            content_type: "video/mp4".into(),
            resumable: true,
            suggested_filename: "Test.mp4".into(),
            thumbnail_url: None,
            thumbnail_bytes: None,
            qualities: vec![],
            note: None,
        }));
        // Click Download CTA (bottom-right of modal 1080x760: dl_r is around x=744..914, y=626..654)
        let act = modals::handle_add_url_click(1080, 760, 820, 640, &mut st);
        assert_eq!(act, ModalAction::None);

        // 2. Populated qualities allow picking 720p (index 1) and downloading that exact stream URL
        st.set_preview_result(Ok(MediaPreview {
            original_url: st.url_input.clone(),
            kind: PreviewKind::YouTube,
            title: "Test Video".into(),
            author: Some("Author".into()),
            duration_secs: Some(120),
            source_domain: "www.youtube.com".into(),
            content_type: "video/mp4".into(),
            resumable: true,
            suggested_filename: "Test_Video.mp4".into(),
            thumbnail_url: None,
            thumbnail_bytes: None,
            qualities: vec![
                MediaStreamQuality {
                    label: "1080p FHD".into(),
                    resolution: (1920, 1080),
                    mime_type: "video/mp4".into(),
                    estimated_size_bytes: 90_000_000,
                    url: "https://example.com/1080.m3u8".into(),
                    bitrate_kbps: 4000,
                    is_audio_only: false,
                    is_hls: true,
                    has_audio: true,
                },
                MediaStreamQuality {
                    label: "720p HD".into(),
                    resolution: (1280, 720),
                    mime_type: "video/mp4".into(),
                    estimated_size_bytes: 45_000_000,
                    url: "https://example.com/720.m3u8".into(),
                    bitrate_kbps: 2000,
                    is_audio_only: false,
                    is_hls: true,
                    has_audio: true,
                },
            ],
            note: None,
        }));

        // Select column 1, row 0 (720p HD) in 2-column grid (grid_y0 = 368, col1 x = 544..902)
        let _ = modals::handle_add_url_click(1080, 760, 600, 380, &mut st);
        if let PreviewFetchState::Ready { selected_quality_idx, .. } = st.preview_state {
            assert_eq!(selected_quality_idx, 1);
        } else {
            panic!("Expected Ready state");
        }
        // Verify clicking Save Folder inside AddUrl modal triggers ChooseModalSaveFolder
        let modal_w = (1080 - 60).clamp(560, 780);
        let modal_h = (760 - 60).clamp(470, 580);
        let mx = (1080 - modal_w) / 2;
        let my = (760 - modal_h) / 2;
        let foot_y = my + modal_h - 46;
        assert_eq!(
            modals::handle_add_url_click(1080, 760, mx + 280, foot_y + 12, &mut st),
            ModalAction::ChooseModalSaveFolder
        );

        // Verify Settings modal "Change..." button triggers ChooseDefaultDownloadFolder
        let s_mx = (1080 - 520) / 2;
        let s_my = (760 - 336) / 2;
        assert_eq!(
            modals::handle_settings_click(1080, 760, s_mx + 520 - 150, s_my + 80),
            ModalAction::ChooseDefaultDownloadFolder
        );

        // Verify clicking PREVIEW column on a video task row opens VideoPreview modal
        let mut engine = VortexEngine::new();
        let vid_id = engine.add_stream(
            "Sample Video.mp4",
            "https://example.com/video.mp4",
            "1080p MP4",
            10_000_000,
            8,
        );
        let mut view = VortexView::new(1080, 760);
        // PREVIEW cell in row 0: x = 196 + 86 + 20 = 302, y = 78 + 14 = 92
        view.handle_click(302, 92, &mut engine);
        assert!(matches!(view.modal, ActiveModal::VideoPreview(ref vp) if vp.task_id == vid_id));
    }
}

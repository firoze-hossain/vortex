//! vortex-core/src/media/preview.rs
//! Real URL Video & File Preview Resolver:
//! Probes direct files, parses HLS playlists, extracts real YouTube (Innertube + HLS + Formats),
//! Vimeo/HTML5 video metadata, formats/sizes, and downloads real thumbnail images.

use std::collections::HashSet;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::media::hls::HlsPlaylist;
use crate::media::sniffer::{
    extract_domain, extract_filename, extract_youtube_id, is_youtube_url, MediaStreamQuality,
};
use crate::network::http::{get_bytes, get_text, post_json, probe_url, sanitize_filename};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewKind {
    DirectFile,
    DirectVideo,
    HlsStream,
    WebVideoPage,
    YouTube,
    Magnet,
    TorrentFile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaPreview {
    pub original_url: String,
    pub kind: PreviewKind,
    pub title: String,
    pub author: Option<String>,
    pub duration_secs: Option<u64>,
    pub source_domain: String,
    pub content_type: String,
    pub resumable: bool,
    pub suggested_filename: String,
    pub thumbnail_url: Option<String>,
    #[serde(skip)]
    pub thumbnail_bytes: Option<Vec<u8>>,
    pub qualities: Vec<MediaStreamQuality>,
    pub note: Option<String>,
}

/// Fetches a live metadata + thumbnail preview for ANY pasted URL (YouTube, Vimeo, HTML5 page,
/// `.m3u8` HLS playlist, direct `.mp4`/`.zip`/`.iso`, or `magnet:` link).
pub fn fetch_url_preview(raw_url: &str) -> Result<MediaPreview, String> {
    let url = raw_url.trim();
    if url.is_empty() {
        return Err("URL is empty".into());
    }

    if url.starts_with("magnet:?") {
        let mag = crate::magnet::Magnet::parse(url)?;
        let name = mag
            .display_name
            .unwrap_or_else(|| format!("Torrent_{}", &mag.info_hash_hex[..8]));
        return Ok(MediaPreview {
            original_url: url.to_string(),
            kind: PreviewKind::Magnet,
            title: name.clone(),
            author: Some(format!("{} trackers", mag.trackers.len())),
            duration_secs: None,
            source_domain: "BitTorrent Magnet".into(),
            content_type: "application/x-bittorrent".into(),
            resumable: true,
            suggested_filename: name,
            thumbnail_url: None,
            thumbnail_bytes: None,
            qualities: vec![MediaStreamQuality {
                label: "BitTorrent Swarm".into(),
                resolution: (0, 0),
                mime_type: "application/x-bittorrent".into(),
                estimated_size_bytes: mag.exact_length.unwrap_or(0),
                url: url.to_string(),
                bitrate_kbps: 0,
                is_audio_only: false,
                is_hls: false,
                has_audio: true,
                audio_url: None,
            }],
            note: Some(format!("InfoHash: {}", mag.info_hash_hex)),
        });
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("URL must start with http://, https://, or magnet:?".into());
    }

    let domain = extract_domain(url);

    // 1. Optional yt-dlp integration if installed on host
    if is_youtube_url(url)
        || url.contains("vimeo.com/")
        || url.contains("tiktok.com/")
        || url.contains("twitch.tv/")
        || url.contains("dailymotion.com/")
    {
        if let Some(preview) = try_yt_dlp_preview(url, &domain) {
            return Ok(preview);
        }
    }

    // 2. YouTube Pure-Rust Innertube + oEmbed + HLS Multi-Quality Extractor
    if is_youtube_url(url) {
        return fetch_youtube_preview(url, &domain);
    }

    // 3. Probe URL headers to distinguish direct binary/video/m3u8 vs HTML webpage
    let probe = probe_url(url, Some(url));
    let (content_type, final_url, content_length, accept_ranges, filename) = match probe {
        Ok(p) => (
            p.content_type.to_ascii_lowercase(),
            p.final_url,
            p.content_length.unwrap_or(0),
            p.accept_ranges,
            p.filename,
        ),
        Err(e) => {
            if let Ok(html) = get_text(url, None) {
                return parse_html_video_preview(url, &domain, &html);
            }
            return Err(e);
        }
    };

    // 4. HLS (.m3u8) playlist
    if final_url.to_ascii_lowercase().contains(".m3u8")
        || content_type.contains("mpegurl")
        || content_type.contains("x-mpegurl")
    {
        return fetch_hls_preview(&final_url, &domain, &filename);
    }

    // 5. HTML Webpage -> scrape OpenGraph video/thumbnail & <video>/<source> tags
    if content_type.contains("text/html") || content_type.contains("application/xhtml") {
        let html = get_text(&final_url, None)?;
        return parse_html_video_preview(&final_url, &domain, &html);
    }

    // 6. Direct Video or Direct File Download
    let is_video = content_type.starts_with("video/") || content_type.starts_with("audio/");
    let kind = if is_video {
        PreviewKind::DirectVideo
    } else {
        PreviewKind::DirectFile
    };

    let clean_name = sanitize_filename(&filename);
    let label = if is_video {
        format!("Direct Stream ({})", content_type)
    } else {
        format!("Direct File ({})", content_type)
    };

    Ok(MediaPreview {
        original_url: final_url.clone(),
        kind,
        title: clean_name.clone(),
        author: Some(domain.clone()),
        duration_secs: None,
        source_domain: domain,
        content_type: content_type.clone(),
        resumable: accept_ranges,
        suggested_filename: clean_name,
        thumbnail_url: None,
        thumbnail_bytes: None,
        qualities: vec![MediaStreamQuality {
            label,
            resolution: (0, 0),
            mime_type: content_type,
            estimated_size_bytes: content_length,
            url: final_url,
            bitrate_kbps: 0,
            is_audio_only: false,
            is_hls: false,
            has_audio: true,
            audio_url: None,
        }],
        note: Some(if accept_ranges {
            "Server supports HTTP Range (multi-thread acceleration enabled)".into()
        } else {
            "Server does not advertise Accept-Ranges (single-stream mode)".into()
        }),
    })
}

fn fetch_hls_preview(url: &str, domain: &str, filename: &str) -> Result<MediaPreview, String> {
    let body = get_text(url, Some(url))?;
    let parsed = HlsPlaylist::parse(&body, url)?;
    let mut qualities = Vec::new();
    let mut duration_secs = None;

    match parsed {
        HlsPlaylist::Master { mut variants, .. } => {
            variants.sort_by_key(|v| std::cmp::Reverse((v.resolution.1, v.bandwidth)));
            for v in variants {
                let res_label = if v.resolution.1 > 0 {
                    format!("{}p ({} kbps)", v.resolution.1, v.bandwidth / 1000)
                } else {
                    format!("Stream ({} kbps)", v.bandwidth / 1000)
                };
                qualities.push(MediaStreamQuality {
                    label: res_label,
                    resolution: v.resolution,
                    mime_type: "video/mp4".into(),
                    estimated_size_bytes: 0,
                    url: v.uri,
                    bitrate_kbps: v.bandwidth / 1000,
                    is_audio_only: false,
                    is_hls: true,
                    has_audio: true,
                    audio_url: None,
                });
            }
        }
        HlsPlaylist::Media {
            segments,
            total_duration,
            init_uri,
            ..
        } => {
            duration_secs = Some(total_duration as u64);
            let mime = if init_uri.is_some() { "video/mp4" } else { "video/mp2t" };
            qualities.push(MediaStreamQuality {
                label: format!("HLS Stream ({} segments)", segments.len()),
                resolution: (1920, 1080),
                mime_type: mime.into(),
                estimated_size_bytes: 0,
                url: url.to_string(),
                bitrate_kbps: 0,
                is_audio_only: false,
                is_hls: true,
                has_audio: true,
                audio_url: None,
            });
        }
    }

    let base_name = if filename.ends_with(".m3u8") {
        format!("{}.mp4", filename.trim_end_matches(".m3u8"))
    } else {
        "hls_stream.mp4".into()
    };

    Ok(MediaPreview {
        original_url: url.to_string(),
        kind: PreviewKind::HlsStream,
        title: base_name.clone(),
        author: Some(domain.to_string()),
        duration_secs,
        source_domain: domain.to_string(),
        content_type: "application/vnd.apple.mpegurl".into(),
        resumable: true,
        suggested_filename: sanitize_filename(&base_name),
        thumbnail_url: None,
        thumbnail_bytes: None,
        qualities,
        note: Some("HLS m3u8 stream detected - concurrent segment assembler ready".into()),
    })
}

fn fetch_youtube_preview(url: &str, domain: &str) -> Result<MediaPreview, String> {
    let video_id = extract_youtube_id(url);

    let mut title = "YouTube Video".to_string();
    let mut author = None;
    let mut duration_secs = None;
    let mut thumbnail_url = video_id
        .as_ref()
        .map(|id| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", id));

    // 1. Fetch oEmbed for fast title/author/thumbnail
    let oembed_url = format!(
        "https://www.youtube.com/oembed?url={}&format=json",
        url_encode_simple(url)
    );
    if let Ok(oembed_str) = get_text(&oembed_url, None) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&oembed_str) {
            if let Some(t) = val.get("title").and_then(|v| v.as_str()) {
                title = t.to_string();
            }
            if let Some(a) = val.get("author_name").and_then(|v| v.as_str()) {
                author = Some(a.to_string());
            }
            if let Some(th) = val.get("thumbnail_url").and_then(|v| v.as_str()) {
                thumbnail_url = Some(th.to_string());
            }
        }
    }

    let mut qualities = Vec::new();
    let mut last_err = None;

    // 2. Query YouTube Innertube Player API across ANDROID -> IOS -> ANDROID_VR clients
    //    ANDROID returns muxed formats (360p MP4 Video+Audio) + adaptive HD/SD + audio;
    //    IOS can also return hlsManifestUrl (all muxed qualities) and unciphered adaptive streams.
    if let Some(ref vid) = video_id {
        let android_body = serde_json::json!({
            "videoId": vid,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "context": {
                "client": {
                    "clientName": "ANDROID",
                    "clientVersion": "20.10.38",
                    "androidSdkVersion": 34,
                    "hl": "en",
                    "gl": "US"
                }
            }
        })
        .to_string();

        let ios_body = serde_json::json!({
            "videoId": vid,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "context": {
                "client": {
                    "clientName": "IOS",
                    "clientVersion": "20.10.4",
                    "deviceMake": "Apple",
                    "deviceModel": "iPhone16,2",
                    "osName": "iPhone",
                    "osVersion": "18.3.1.22D72",
                    "hl": "en",
                    "gl": "US"
                }
            }
        })
        .to_string();

        let android_vr_body = serde_json::json!({
            "videoId": vid,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "context": {
                "client": {
                    "clientName": "ANDROID_VR",
                    "clientVersion": "1.61.48",
                    "deviceMake": "Oculus",
                    "deviceModel": "Quest 3",
                    "osName": "Android",
                    "osVersion": "12L",
                    "androidSdkVersion": 32,
                    "hl": "en",
                    "gl": "US"
                }
            }
        })
        .to_string();

        let attempts = [
            (
                "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
                ios_body,
                "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_1 like Mac OS X;)",
                "5",
                "20.10.4",
            ),
            (
                "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
                android_vr_body,
                "com.google.android.apps.youtube.vr.oculus/1.61.48 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
                "28",
                "1.61.48",
            ),
            (
                "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
                android_body,
                "com.google.android.youtube/20.10.38 (Linux; U; Android 14) gzip",
                "3",
                "20.10.38",
            ),
        ];

        for (endpoint, body, ua, client_id, client_ver) in attempts {
            match post_json(
                endpoint,
                &body,
                Some(ua),
                &[
                    ("Origin", "https://www.youtube.com"),
                    ("X-YouTube-Client-Name", client_id),
                    ("X-YouTube-Client-Version", client_ver),
                ],
            ) {
                Ok(resp_str) => {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp_str) {
                        extract_youtube_player_json(
                            &val,
                            &mut title,
                            &mut author,
                            &mut duration_secs,
                            &mut qualities,
                        );
                        // Stop early only once we already have an unlocked HD (720p+) muxed/HLS stream
                        let has_hd_hls = qualities
                            .iter()
                            .any(|q| q.is_hls && q.has_audio && q.resolution.1 >= 720);
                        let has_muxed = qualities.iter().any(|q| !q.is_audio_only && q.has_audio);
                        if has_hd_hls && has_muxed {
                            break;
                        }
                    }
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }
    }

    // 2B. Merge any live browser-authenticated HD/4K streams captured by Chrome/Brave/Edge/Firefox
    let mut browser_unlocked_hd = false;
    for mut bq in crate::media::sniffer::browser_streams_for_url(url) {
        if !bq.label.contains("[Browser Auth]") {
            bq.label = format!("{} [Browser Auth]", bq.label);
        }
        if !bq.is_audio_only && bq.resolution.1 >= 720 {
            browser_unlocked_hd = true;
        }
        qualities.push(bq);
    }

    // 3. Fallback to watch page HTML if Innertube yielded nothing
    if qualities.is_empty() {
        if let Ok(html) = get_text(url, Some("https://www.youtube.com/")) {
            if let Some(player_json) = extract_json_object_after(&html, "ytInitialPlayerResponse = ") {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&player_json) {
                    extract_youtube_player_json(
                        &val,
                        &mut title,
                        &mut author,
                        &mut duration_secs,
                        &mut qualities,
                    );
                }
            }
            if let Some(og_title) = extract_meta_content(&html, "og:title") {
                if !has_ascii_alnum(&title) && has_ascii_alnum(&og_title) {
                    title = og_title;
                }
            }
        }
    }

    // Sort & deduplicate qualities cleanly (Browser Auth & Unlocked Muxed/HLS first, then 2160p..144p, Audio)
    let raw_before_dedup = qualities.clone();
    qualities = finalize_youtube_qualities(qualities);

    // Quick tail-byte verification on standalone Innertube adaptive streams (ignore HLS & [Browser Auth])
    let mut adaptive_gated = false;
    if let Some(sample_adapt) = qualities
        .iter()
        .find(|q| !q.is_hls && !q.has_audio && !q.label.contains("[Browser Auth]") && q.estimated_size_bytes > 2_000_000)
    {
        if !check_tail_byte_accessible(&sample_adapt.url, sample_adapt.estimated_size_bytes) {
            adaptive_gated = true;
        }
    }
    if adaptive_gated {
        for q in &mut qualities {
            let is_browser = q.label.contains("[Browser Auth]");
            if !is_browser && !q.is_hls && (!q.has_audio || q.is_audio_only) {
                q.label = format!("{} [PO-Token Gated]", q.label.split(" (").next().unwrap_or(&q.label));
            }
        }
    } else {
        attach_companion_audio_urls(&mut qualities, &raw_before_dedup);
    }

    // 4. Fetch real thumbnail bytes
    let thumbnail_bytes = thumbnail_url
        .as_deref()
        .and_then(|u| get_bytes(u, Some("https://www.youtube.com/"), 2 * 1024 * 1024).ok());

    let has_unlocked_hd = browser_unlocked_hd
        || qualities
            .iter()
            .any(|q| !q.is_audio_only && q.resolution.1 >= 720 && !q.label.contains("[PO-Token Gated]"));

    let note = if qualities.is_empty() {
        Some(format!(
            "Could not extract unciphered streams ({}). Install yt-dlp or play in Brave/Chrome with Vortex extension.",
            last_err.unwrap_or_else(|| "ciphered/restricted".into())
        ))
    } else if has_unlocked_hd {
        Some(format!(
            "HD Ready: {} quality option(s) unlocked (up to {}p)",
            qualities.len(),
            qualities.iter().map(|q| q.resolution.1).max().unwrap_or(720)
        ))
    } else if adaptive_gated {
        Some("360p MP4 Ready! Play at 1080p in Brave/Chrome/Firefox (extension) or install yt-dlp to unlock 1080p/4K.".into())
    } else {
        Some(format!(
            "Ready: {} downloadable quality option(s) extracted",
            qualities.len()
        ))
    };

    let clean_name = build_youtube_filename(&title, video_id.as_deref());
    Ok(MediaPreview {
        original_url: url.to_string(),
        kind: PreviewKind::YouTube,
        title,
        author,
        duration_secs,
        source_domain: domain.to_string(),
        content_type: "video/mp4".into(),
        resumable: true,
        suggested_filename: clean_name,
        thumbnail_url,
        thumbnail_bytes,
        qualities,
        note,
    })
}

pub fn extract_youtube_player_json(
    val: &serde_json::Value,
    title: &mut String,
    author: &mut Option<String>,
    duration_secs: &mut Option<u64>,
    qualities: &mut Vec<MediaStreamQuality>,
) {
    if let Some(vd) = val.get("videoDetails") {
        if let Some(t) = vd.get("title").and_then(|v| v.as_str()) {
            if has_ascii_alnum(t) || title == "YouTube Video" {
                *title = t.to_string();
            }
        }
        if let Some(a) = vd.get("author").and_then(|v| v.as_str()) {
            *author = Some(a.to_string());
        }
        if let Some(len) = vd
            .get("lengthSeconds")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
        {
            *duration_secs = Some(len);
        }
    }

    let Some(sd) = val.get("streamingData") else {
        return;
    };

    // A. Parse HLS Master Manifest (provides all resolutions 1080p..144p WITH AUDIO!)
    if let Some(hls_manifest) = sd.get("hlsManifestUrl").and_then(|v| v.as_str()) {
        if let Ok(m3u8_text) = get_text(hls_manifest, Some("https://www.youtube.com/")) {
            if let Ok(HlsPlaylist::Master { variants, .. }) = HlsPlaylist::parse(&m3u8_text, hls_manifest) {
                for v in variants {
                    let h = v.resolution.1;
                    if h == 0 {
                        continue;
                    }
                    let est_bytes = duration_secs
                        .map(|d| (v.bandwidth as u64).saturating_mul(d) / 8)
                        .unwrap_or(0);
                    let tag = match h {
                        2160.. => "4K UHD",
                        1440..=2159 => "1440p QHD",
                        1080..=1439 => "1080p FHD",
                        720..=1079 => "720p HD",
                        _ => "SD",
                    };
                    let label = if h >= 720 {
                        format!("{} ({}p MP4)", tag, h)
                    } else {
                        format!("{}p MP4", h)
                    };
                    qualities.push(MediaStreamQuality {
                        label,
                        resolution: v.resolution,
                        mime_type: "video/mp4".into(),
                        estimated_size_bytes: est_bytes,
                        url: v.uri,
                        bitrate_kbps: v.bandwidth / 1000,
                        is_audio_only: false,
                        is_hls: true,
                        has_audio: true,
                        audio_url: None,
                    });
                }
            }
        }
    }

    // B. Parse direct formats (muxed MP4) and adaptiveFormats (audio + direct video)
    for arr_key in ["formats", "adaptiveFormats"] {
        if let Some(arr) = sd.get(arr_key).and_then(|v| v.as_array()) {
            for fmt in arr {
                let Some(stream_url) = fmt.get("url").and_then(|v| v.as_str()) else {
                    continue;
                };
                let mime = fmt.get("mimeType").and_then(|v| v.as_str()).unwrap_or("video/mp4");
                let is_audio = mime.starts_with("audio/");
                let w = fmt.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let h = fmt.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let bitrate = (fmt.get("bitrate").and_then(|v| v.as_u64()).unwrap_or(0) / 1000) as u32;
                let size = fmt
                    .get("contentLength")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let q_label = fmt
                    .get("qualityLabel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        if is_audio {
                            format!("Audio {}kbps", bitrate)
                        } else {
                            format!("{}p", h)
                        }
                    });
                let ext = if mime.contains("webm") {
                    "webm"
                } else if is_audio {
                    "m4a"
                } else {
                    "mp4"
                };
                let has_audio = arr_key == "formats" || is_audio;
                let label = if arr_key == "formats" {
                    format!("{} {} (Direct Full)", q_label, ext.to_uppercase())
                } else if is_audio {
                    format!("Audio {} ({} kbps)", ext.to_uppercase(), bitrate)
                } else {
                    format!("{} {} (HD Video)", q_label, ext.to_uppercase())
                };

                qualities.push(MediaStreamQuality {
                    label,
                    resolution: (w, h),
                    mime_type: mime.split(';').next().unwrap_or(mime).to_string(),
                    estimated_size_bytes: size,
                    url: stream_url.to_string(),
                    bitrate_kbps: bitrate,
                    is_audio_only: is_audio,
                    is_hls: false,
                    has_audio,
                    audio_url: None,
                });
            }
        }
    }
}

fn finalize_youtube_qualities(mut raw: Vec<MediaStreamQuality>) -> Vec<MediaStreamQuality> {
    // Prefer audio-inclusive video streams per height; keep video-only only if height has no muxed/HLS stream
    let muxed_heights: HashSet<u32> = raw
        .iter()
        .filter(|q| !q.is_audio_only && q.has_audio && q.resolution.1 > 0)
        .map(|q| q.resolution.1)
        .collect();

    raw.retain(|q| q.is_audio_only || q.has_audio || !muxed_heights.contains(&q.resolution.1));

    // Sort descending: non-audio first, [Browser Auth] first, highest resolution/audio next, MP4 over WebM
    raw.sort_by_key(|q| {
        let is_browser = q.label.contains("[Browser Auth]") as u8;
        let is_mp4 = q.mime_type.contains("mp4") as u8;
        std::cmp::Reverse((
            !q.is_audio_only as u8,
            is_browser,
            q.has_audio as u8,
            q.resolution.1,
            is_mp4,
            !q.is_hls as u8,
            q.bitrate_kbps,
        ))
    });

    // Deduplicate by (is_audio_only, height, is_hls)
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for q in raw {
        let key = if q.is_audio_only {
            (true, 0u32, q.mime_type.clone())
        } else {
            (false, q.resolution.1, format!("{}", q.has_audio))
        };
        if seen.insert(key) {
            out.push(q);
        }
        if out.len() >= 10 {
            break;
        }
    }
    out
}

fn attach_companion_audio_urls(qualities: &mut [MediaStreamQuality], pool: &[MediaStreamQuality]) {
    let best_m4a = pool
        .iter()
        .chain(qualities.iter())
        .filter(|q| {
            q.is_audio_only
                && !q.is_hls
                && !q.label.contains("[PO-Token Gated]")
                && (q.mime_type.contains("m4a") || q.mime_type.contains("mp4"))
        })
        .max_by_key(|q| q.bitrate_kbps)
        .map(|q| q.url.clone());

    let Some(aud_url) = best_m4a else {
        return;
    };

    for q in qualities.iter_mut() {
        if !q.is_audio_only && !q.is_hls && !q.has_audio && q.mime_type.contains("mp4") {
            q.audio_url = Some(aud_url.clone());
            if q.label.contains("(Video)") {
                q.label = q.label.replace("(Video)", "(+Audio)");
            } else if q.label.contains("(HD Video)") {
                q.label = q.label.replace("(HD Video)", "(+Audio)");
            }
        }
    }
}

fn build_youtube_filename(title: &str, video_id: Option<&str>) -> String {
    let cleaned = sanitize_filename(title);
    if has_ascii_alnum(&cleaned) {
        format!("{}.mp4", cleaned)
    } else if let Some(vid) = video_id {
        if cleaned.is_empty() {
            format!("youtube_{}.mp4", vid)
        } else {
            format!("{} [{}].mp4", cleaned, vid)
        }
    } else {
        "youtube_video.mp4".into()
    }
}

fn check_tail_byte_accessible(url: &str, total_len: u64) -> bool {
    if total_len < 64 {
        return true;
    }
    let ua = crate::network::http::user_agent_for_url(url);
    let start = total_len - 64;
    let end = total_len - 1;
    match minreq::get(url)
        .with_header("User-Agent", ua)
        .with_header("Range", format!("bytes={}-{}", start, end))
        .with_timeout(4)
        .send_lazy()
    {
        Ok(resp) => (200..300).contains(&resp.status_code),
        Err(_) => false,
    }
}

fn is_supported_baseline_thumb(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return true;
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        // Baseline JPEG uses SOF0 (0xFF 0xC0); progressive JPEG uses SOF2 (0xFF 0xC2)
        return bytes.windows(2).any(|w| w == [0xFF, 0xC0]);
    }
    false
}

fn has_ascii_alnum(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_alphanumeric())
}

fn parse_html_video_preview(page_url: &str, domain: &str, html: &str) -> Result<MediaPreview, String> {
    let title = extract_meta_content(html, "og:title")
        .or_else(|| extract_html_title(html))
        .unwrap_or_else(|| format!("Media from {}", domain));
    let author = extract_meta_content(html, "og:site_name").or_else(|| Some(domain.to_string()));
    let thumb_url = extract_meta_content(html, "og:image").map(|u| resolve_page_url(page_url, &u));
    let thumbnail_bytes = thumb_url
        .as_deref()
        .and_then(|u| get_bytes(u, Some(page_url), 2 * 1024 * 1024).ok());

    let mut candidate_urls = Vec::new();
    for key in ["og:video:secure_url", "og:video:url", "og:video"] {
        if let Some(u) = extract_meta_content(html, key) {
            let full = resolve_page_url(page_url, &u);
            if !candidate_urls.contains(&full) {
                candidate_urls.push(full);
            }
        }
    }

    for pat in ["src=\"", "src='", "https://", "http://"] {
        let mut rest = html;
        while let Some(idx) = rest.find(pat) {
            let after = if pat.starts_with("src=") {
                &rest[idx + pat.len()..]
            } else {
                &rest[idx..]
            };
            let end_char = if pat.ends_with('\'') { '\'' } else { '"' };
            let token_end = after
                .find(|c: char| c == end_char || c.is_whitespace() || c == '<' || c == '>' || c == '\\')
                .unwrap_or(after.len().min(512));
            let raw_cand = &after[..token_end];
            let lower = raw_cand.split('?').next().unwrap_or(raw_cand).to_ascii_lowercase();
            if lower.ends_with(".mp4") || lower.ends_with(".webm") || lower.ends_with(".m3u8") || lower.ends_with(".mkv") {
                let full = resolve_page_url(page_url, raw_cand);
                if !candidate_urls.contains(&full) {
                    candidate_urls.push(full);
                }
                if candidate_urls.len() >= 8 {
                    break;
                }
            }
            rest = &after[token_end.min(after.len())..];
        }
        if candidate_urls.len() >= 8 {
            break;
        }
    }

    let mut qualities = Vec::new();
    for cand in candidate_urls {
        let is_hls = cand.to_ascii_lowercase().contains(".m3u8");
        let (size, mime) = if is_hls {
            (0, "application/vnd.apple.mpegurl".to_string())
        } else {
            match probe_url(&cand, Some(page_url)) {
                Ok(p) => (p.content_length.unwrap_or(0), p.content_type),
                Err(_) => (0, "video/mp4".to_string()),
            }
        };
        let fname = extract_filename(&cand);
        qualities.push(MediaStreamQuality {
            label: if is_hls {
                format!("HLS: {}", fname)
            } else {
                format!("Video: {}", fname)
            },
            resolution: (1920, 1080),
            mime_type: mime,
            estimated_size_bytes: size,
            url: cand,
            bitrate_kbps: 0,
            is_audio_only: false,
            is_hls,
            has_audio: true,
            audio_url: None,
        });
    }

    let clean_title = sanitize_filename(&title);
    let note = if qualities.is_empty() {
        Some("No direct <video> URL found in static HTML. Use Aero Browser Bridge or paste direct .m3u8/.mp4 URL.".into())
    } else {
        Some(format!("Detected {} video stream(s) on page", qualities.len()))
    };

    Ok(MediaPreview {
        original_url: page_url.to_string(),
        kind: PreviewKind::WebVideoPage,
        title,
        author,
        duration_secs: None,
        source_domain: domain.to_string(),
        content_type: "text/html".into(),
        resumable: true,
        suggested_filename: format!("{}.mp4", clean_title),
        thumbnail_url: thumb_url,
        thumbnail_bytes,
        qualities,
        note,
    })
}

fn try_yt_dlp_preview(url: &str, domain: &str) -> Option<MediaPreview> {
    let mut cmds: Vec<(String, Vec<String>)> = Vec::new();
    let has_node = Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    let mut ytdlp_args = vec![
        "-J".to_string(),
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
    ];
    if has_node {
        ytdlp_args.push("--js-runtimes".to_string());
        ytdlp_args.push("node".to_string());
    }
    ytdlp_args.push(url.to_string());

    for bin in [
        "yt-dlp",
        "/opt/homebrew/bin/yt-dlp",
        "/usr/local/bin/yt-dlp",
    ] {
        cmds.push((bin.to_string(), ytdlp_args.clone()));
    }
    if let Ok(home) = std::env::var("HOME") {
        cmds.push((format!("{}/.local/bin/yt-dlp", home), ytdlp_args.clone()));
        cmds.push((format!("{}/.local/share/uv/tools/yt-dlp/bin/yt-dlp", home), ytdlp_args.clone()));
        cmds.push((format!("{}/.cargo/bin/yt-dlp", home), ytdlp_args.clone()));
        let mut uv_args = vec!["tool".to_string(), "run".to_string(), "yt-dlp".to_string()];
        uv_args.extend(ytdlp_args.clone());
        cmds.push((format!("{}/.local/bin/uv", home), uv_args.clone()));
        cmds.push(("uv".to_string(), uv_args));
    } else {
        let mut uv_args = vec!["tool".to_string(), "run".to_string(), "yt-dlp".to_string()];
        uv_args.extend(ytdlp_args);
        cmds.push(("uv".to_string(), uv_args));
    }

    let mut output_opt = None;
    for (bin, args) in &cmds {
        if let Ok(out) = Command::new(bin).args(args).output() {
            if out.status.success() {
                output_opt = Some(out);
                break;
            }
        }
    }
    let output = output_opt?;
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let title = val.get("title").and_then(|v| v.as_str()).unwrap_or("Video").to_string();
    let author = val.get("uploader").and_then(|v| v.as_str()).map(|s| s.to_string());
    let duration_secs = val.get("duration").and_then(|v| v.as_f64()).map(|d| d as u64);
    let mut thumb_candidates: Vec<String> = Vec::new();
    if let Some(vid) = extract_youtube_id(url) {
        // `hqdefault.jpg` is Baseline JPEG (supported by pure-Rust baseline JPEG decoder);
        // `yt-dlp`'s top `thumbnail` on YouTube defaults to `.webp` or progressive `maxresdefault.jpg`.
        thumb_candidates.push(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", vid));
    }
    if let Some(arr) = val.get("thumbnails").and_then(|v| v.as_array()) {
        for item in arr.iter().rev() {
            if let Some(u) = item.get("url").and_then(|v| v.as_str()) {
                let low = u.to_ascii_lowercase();
                if (low.contains(".jpg") || low.contains(".jpeg") || low.contains(".png")) && !low.contains(".webp") {
                    if !thumb_candidates.iter().any(|c| c == u) {
                        thumb_candidates.push(u.to_string());
                    }
                }
            }
        }
    }
    if let Some(top) = val.get("thumbnail").and_then(|v| v.as_str()) {
        let low = top.to_ascii_lowercase();
        if !low.contains(".webp") && !thumb_candidates.iter().any(|c| c == top) {
            thumb_candidates.push(top.to_string());
        }
    }

    let mut thumb_url = thumb_candidates.first().cloned();
    let mut thumbnail_bytes = None;
    for cand in &thumb_candidates {
        if let Ok(bytes) = get_bytes(cand, Some(url), 2 * 1024 * 1024) {
            if is_supported_baseline_thumb(&bytes) {
                thumb_url = Some(cand.clone());
                thumbnail_bytes = Some(bytes);
                break;
            }
        }
    }

    let mut qualities = Vec::new();
    for mut bq in crate::media::sniffer::browser_streams_for_url(url) {
        if !bq.label.contains("[Browser Auth]") {
            bq.label = format!("{} [Browser Auth]", bq.label);
        }
        qualities.push(bq);
    }

    if let Some(formats) = val.get("formats").and_then(|v| v.as_array()) {
        for fmt in formats {
            let Some(u) = fmt.get("url").and_then(|v| v.as_str()) else {
                continue;
            };
            if !u.starts_with("http") {
                continue;
            }
            let ext = fmt.get("ext").and_then(|v| v.as_str()).unwrap_or("mp4");
            // Skip storyboard sprite images (`mhtml`)
            if ext == "mhtml" {
                continue;
            }
            let vcodec = fmt.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
            let acodec = fmt.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
            if vcodec == "none" && acodec == "none" {
                continue;
            }
            let is_audio = vcodec == "none" && acodec != "none";
            let has_audio = acodec != "none";
            let w = fmt.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let h = fmt.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let size = fmt
                .get("filesize")
                .or_else(|| fmt.get("filesize_approx"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let tbr = fmt.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
            let proto = fmt.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
            let is_hls = proto.contains("m3u8") || u.contains(".m3u8");

            let codec_bonus_kbps = if vcodec.starts_with("avc") { tbr + 50_000 } else { tbr };

            let label = if is_audio {
                format!("Audio {} ({} kbps)", ext.to_uppercase(), tbr)
            } else if h > 0 && has_audio {
                format!("{}p {} (Full)", h, ext.to_uppercase())
            } else if h > 0 {
                format!("{}p {} (Video)", h, ext.to_uppercase())
            } else {
                format!("Stream {}", ext.to_uppercase())
            };

            qualities.push(MediaStreamQuality {
                label,
                resolution: (w, h),
                mime_type: format!("{}/{}", if is_audio { "audio" } else { "video" }, ext),
                estimated_size_bytes: size,
                url: u.to_string(),
                bitrate_kbps: codec_bonus_kbps,
                is_audio_only: is_audio,
                is_hls,
                has_audio,
                audio_url: None,
            });
        }
    }

    let raw_before_dedup = qualities.clone();
    qualities = finalize_youtube_qualities(qualities);
    attach_companion_audio_urls(&mut qualities, &raw_before_dedup);

    if qualities.is_empty() {
        return None;
    }

    let clean_title = sanitize_filename(&title);
    let max_h = qualities.iter().map(|q| q.resolution.1).max().unwrap_or(720);
    Some(MediaPreview {
        original_url: url.to_string(),
        kind: if is_youtube_url(url) {
            PreviewKind::YouTube
        } else {
            PreviewKind::WebVideoPage
        },
        title,
        author,
        duration_secs,
        source_domain: domain.to_string(),
        content_type: "video/mp4".into(),
        resumable: true,
        suggested_filename: format!("{}.mp4", clean_title),
        thumbnail_url: thumb_url,
        thumbnail_bytes,
        qualities,
        note: Some(format!("Unlocked all resolutions (up to {}p)!", max_h)),
    })
}

fn extract_html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let gt = lower[start..].find('>')? + start + 1;
    let end = lower[gt..].find("</title>")? + gt;
    let raw = html[gt..end].trim();
    if raw.is_empty() {
        None
    } else {
        Some(html_unescape(raw))
    }
}

fn extract_meta_content(html: &str, property: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let prop_lower = property.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(idx) = lower[pos..].find("<meta") {
        let abs = pos + idx;
        let end = lower[abs..].find('>').map(|e| abs + e + 1)?;
        let tag = &html[abs..end];
        let tag_l = &lower[abs..end];
        if tag_l.contains(&prop_lower) {
            if let Some(c) = extract_attr(tag, "content") {
                return Some(html_unescape(&c));
            }
        }
        pos = end;
    }
    None
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{}=", attr);
    let idx = lower.find(&needle)?;
    let rest = tag[idx + needle.len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let inner = &rest[1..];
        let end = inner.find(quote)?;
        Some(inner[..end].to_string())
    } else {
        None
    }
}

fn extract_json_object_after(haystack: &str, marker: &str) -> Option<String> {
    let start = haystack.find(marker)? + marker.len();
    let bytes = haystack[start..].as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(haystack[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn resolve_page_url(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") {
        return rel.to_string();
    }
    if rel.starts_with("//") {
        let scheme = if base.starts_with("http://") { "http:" } else { "https:" };
        return format!("{}{}", scheme, rel);
    }
    let origin = if let Some(scheme_end) = base.find("://") {
        let after = &base[scheme_end + 3..];
        let slash = after.find('/').map(|s| scheme_end + 3 + s).unwrap_or(base.len());
        &base[..slash]
    } else {
        base
    };
    if rel.starts_with('/') {
        format!("{}{}", origin, rel)
    } else {
        let base_dir = base.rfind('/').map(|i| &base[..i]).unwrap_or(origin);
        format!("{}/{}", base_dir, rel)
    }
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn url_encode_simple(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_video_preview_scraper() {
        let html = r#"
        <html>
          <head>
            <title>Awesome Demo</title>
            <meta property="og:title" content="Big Buck Bunny 1080p" />
            <meta property="og:image" content="/poster.jpg" />
          </head>
          <body>
            <video controls src="https://cdn.example.com/bunny.mp4"></video>
          </body>
        </html>"#;
        let preview = parse_html_video_preview("https://videos.example.com/watch", "videos.example.com", html).unwrap();
        assert_eq!(preview.title, "Big Buck Bunny 1080p");
        assert_eq!(preview.thumbnail_url.as_deref(), Some("https://videos.example.com/poster.jpg"));
        assert!(!preview.qualities.is_empty());
        assert_eq!(preview.qualities[0].url, "https://cdn.example.com/bunny.mp4");
    }

    #[test]
    fn test_youtube_player_json_multi_quality_extraction() {
        let sample_json = serde_json::json!({
            "videoDetails": {
                "title": "Sample Video",
                "author": "TestChannel",
                "lengthSeconds": "300"
            },
            "streamingData": {
                "formats": [
                    {
                        "url": "https://rr1.googlevideo.com/360p.mp4",
                        "mimeType": "video/mp4; codecs=\"avc1\"",
                        "width": 640,
                        "height": 360,
                        "bitrate": 550000,
                        "contentLength": "20625000",
                        "qualityLabel": "360p"
                    }
                ],
                "adaptiveFormats": [
                    {
                        "url": "https://rr1.googlevideo.com/1080p.mp4",
                        "mimeType": "video/mp4",
                        "width": 1920,
                        "height": 1080,
                        "bitrate": 3500000,
                        "contentLength": "131250000",
                        "qualityLabel": "1080p"
                    },
                    {
                        "url": "https://rr1.googlevideo.com/audio.m4a",
                        "mimeType": "audio/mp4",
                        "bitrate": 128000,
                        "contentLength": "4800000"
                    }
                ]
            }
        });
        let mut title = String::new();
        let mut author = None;
        let mut dur = None;
        let mut qualities = Vec::new();
        extract_youtube_player_json(&sample_json, &mut title, &mut author, &mut dur, &mut qualities);
        let final_q = finalize_youtube_qualities(qualities);

        assert_eq!(title, "Sample Video");
        assert_eq!(dur, Some(300));
        assert_eq!(final_q.len(), 3);
        assert_eq!(final_q[0].resolution.1, 360); // muxed first
        assert_eq!(final_q[1].resolution.1, 1080); // adaptive video
        assert!(final_q[2].is_audio_only);
    }
}

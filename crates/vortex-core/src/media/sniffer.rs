//! vortex-core/src/media/sniffer.rs
//! Intelligent Media Stream Sniffer & Video Format Detector (IDM-style).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamQuality {
    pub label: String,
    pub resolution: (u32, u32),
    pub mime_type: String,
    pub estimated_size_bytes: u64,
    pub url: String,
    pub bitrate_kbps: u32,
    pub is_audio_only: bool,
    #[serde(default)]
    pub is_hls: bool,
    #[serde(default)]
    pub has_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SniffedMedia {
    pub id: u64,
    pub page_url: String,
    pub title: String,
    pub qualities: Vec<MediaStreamQuality>,
    pub thumbnail_url: Option<String>,
    pub source_domain: String,
    pub detected_timestamp: u64,
}

impl SniffedMedia {
    pub fn best_video_quality(&self) -> Option<&MediaStreamQuality> {
        self.qualities
            .iter()
            .filter(|q| !q.is_audio_only)
            .max_by_key(|q| (q.has_audio as u32, q.resolution.0 * q.resolution.1, q.bitrate_kbps))
    }

    pub fn audio_only_quality(&self) -> Option<&MediaStreamQuality> {
        self.qualities.iter().find(|q| q.is_audio_only)
    }
}

pub struct MediaSniffer {
    pub sniffed_items: Vec<SniffedMedia>,
    next_id: u64,
}

impl Default for MediaSniffer {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaSniffer {
    pub fn new() -> Self {
        Self {
            sniffed_items: Vec::new(),
            next_id: 1,
        }
    }

    /// Registers a sniffed media entry (e.g. received from browser extension or preview resolver).
    pub fn register_media(
        &mut self,
        page_url: &str,
        title: &str,
        thumbnail_url: Option<String>,
        qualities: Vec<MediaStreamQuality>,
    ) -> &SniffedMedia {
        let id = self.next_id;
        self.next_id += 1;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let media = SniffedMedia {
            id,
            page_url: page_url.to_string(),
            title: title.to_string(),
            qualities,
            thumbnail_url,
            source_domain: extract_domain(page_url),
            detected_timestamp: now,
        };
        self.sniffed_items.push(media);
        self.sniffed_items.last().unwrap()
    }

    /// Quick offline/heuristic inspection of a URL (for fast classification or fallback).
    pub fn inspect_url(&mut self, url: &str, page_url: &str, page_title: &str) -> Option<&SniffedMedia> {
        let clean_url = url.trim();
        let domain = extract_domain(page_url);

        if is_youtube_url(clean_url) {
            let vid_id = extract_youtube_id(clean_url).unwrap_or_default();
            let title = if page_title.is_empty() || page_title == "YouTube" {
                "YouTube Video".to_string()
            } else {
                page_title.to_string()
            };
            let thumb = if vid_id.is_empty() {
                None
            } else {
                Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", vid_id))
            };
            let q = vec![MediaStreamQuality {
                label: "Source Page (Analyze for streams)".to_string(),
                resolution: (1920, 1080),
                mime_type: "video/mp4".to_string(),
                estimated_size_bytes: 0,
                url: clean_url.to_string(),
                bitrate_kbps: 0,
                is_audio_only: false,
                is_hls: false,
                has_audio: true,
            }];
            return Some(self.register_media(page_url, &title, thumb, q));
        }

        if is_direct_media_url(clean_url) {
            let filename = extract_filename(clean_url);
            let title = if !page_title.is_empty() { page_title } else { &filename };
            let is_hls = clean_url.to_ascii_lowercase().contains(".m3u8");
            let mime = if is_hls {
                "application/vnd.apple.mpegurl"
            } else if clean_url.to_ascii_lowercase().contains(".webm") {
                "video/webm"
            } else {
                "video/mp4"
            };
            let q = vec![MediaStreamQuality {
                label: if is_hls { "HLS Stream".into() } else { "Direct Video".into() },
                resolution: (1920, 1080),
                mime_type: mime.to_string(),
                estimated_size_bytes: 0,
                url: clean_url.to_string(),
                bitrate_kbps: 0,
                is_audio_only: false,
                is_hls,
                has_audio: true,
            }];
            let id = self.next_id;
            self.next_id += 1;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            self.sniffed_items.push(SniffedMedia {
                id,
                page_url: page_url.to_string(),
                title: title.to_string(),
                qualities: q,
                thumbnail_url: None,
                source_domain: domain,
                detected_timestamp: now,
            });
            return self.sniffed_items.last();
        }

        None
    }
}

pub fn is_youtube_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("youtube.com/watch")
        || lower.contains("youtu.be/")
        || lower.contains("youtube.com/shorts/")
        || lower.contains("googlevideo.com")
}

pub fn is_direct_media_url(url: &str) -> bool {
    let lower = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    lower.ends_with(".mp4")
        || lower.ends_with(".webm")
        || lower.ends_with(".mkv")
        || lower.ends_with(".mov")
        || lower.ends_with(".m3u8")
        || lower.ends_with(".flv")
        || lower.ends_with(".avi")
        || lower.ends_with(".mp3")
        || lower.ends_with(".m4a")
}

pub fn extract_youtube_id(url: &str) -> Option<String> {
    if let Some(pos) = url.find("youtu.be/") {
        let rest = &url[pos + 9..];
        let id = rest.split(&['?', '&', '/'][..]).next().unwrap_or("");
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    if let Some(pos) = url.find("/shorts/") {
        let rest = &url[pos + 8..];
        let id = rest.split(&['?', '&', '/'][..]).next().unwrap_or("");
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    if let Some(q) = url.split('?').nth(1) {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("v=") {
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

pub fn extract_domain(url: &str) -> String {
    let s = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    s.split('/').next().unwrap_or(s).to_string()
}

pub fn extract_filename(url: &str) -> String {
    let clean = url.split('?').next().unwrap_or(url);
    let last = clean.rsplit('/').next().unwrap_or("");
    if last.is_empty() {
        "media_stream.mp4".to_string()
    } else {
        last.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_sniffer_detection() {
        let mut sniffer = MediaSniffer::new();
        let detected = sniffer
            .inspect_url(
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "Rick Astley - Never Gonna Give You Up",
            )
            .unwrap();

        assert_eq!(
            detected.thumbnail_url.as_deref(),
            Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg")
        );
        assert_eq!(extract_youtube_id("https://youtu.be/abc123XYZ?t=10").as_deref(), Some("abc123XYZ"));
    }
}

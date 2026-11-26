//! vortex-core/src/media/hls.rs
//! HTTP Live Streaming (HLS / m3u8) playlist parser supporting Master playlists,
//! Media playlists, and fMP4 #EXT-X-MAP initialization segments.

#[derive(Clone, Debug, PartialEq)]
pub struct HlsSegment {
    pub index: usize,
    pub uri: String,
    pub duration_sec: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HlsStreamVariant {
    pub bandwidth: u32,
    pub resolution: (u32, u32),
    pub codecs: String,
    pub uri: String,
    pub audio_group: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HlsPlaylist {
    Master {
        variants: Vec<HlsStreamVariant>,
        audio_uris: Vec<(String, String)>, // (group_id, uri)
    },
    Media {
        target_duration: u32,
        init_uri: Option<String>,
        segments: Vec<HlsSegment>,
        total_duration: f32,
    },
}

impl HlsPlaylist {
    pub fn parse(content: &str, base_url: &str) -> Result<Self, String> {
        let lines: Vec<&str> = content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() || !lines[0].starts_with("#EXTM3U") {
            return Err("Not a valid HLS m3u8 playlist (missing #EXTM3U header)".into());
        }

        let has_stream_inf = lines.iter().any(|l| l.starts_with("#EXT-X-STREAM-INF"));
        if has_stream_inf {
            let mut variants = Vec::new();
            let mut audio_uris = Vec::new();
            let mut i = 0;

            for line in &lines {
                if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA:") {
                    if rest.contains("TYPE=AUDIO") {
                        let group = extract_m3u8_attr(rest, "GROUP-ID").unwrap_or_default();
                        if let Some(uri) = extract_m3u8_attr(rest, "URI") {
                            audio_uris.push((group, resolve_url(base_url, &uri)));
                        }
                    }
                }
            }

            while i < lines.len() {
                if let Some(info) = lines[i].strip_prefix("#EXT-X-STREAM-INF:") {
                    let bandwidth = extract_m3u8_attr(info, "BANDWIDTH")
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(0);
                    let resolution = extract_m3u8_attr(info, "RESOLUTION")
                        .map(|v| {
                            let mut res = v.splitn(2, 'x');
                            let w = res.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                            let h = res.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                            (w, h)
                        })
                        .unwrap_or((0, 0));
                    let codecs = extract_m3u8_attr(info, "CODECS").unwrap_or_default();
                    let audio_group = extract_m3u8_attr(info, "AUDIO");

                    if i + 1 < lines.len() && !lines[i + 1].starts_with('#') {
                        let rel_uri = lines[i + 1];
                        let full_uri = resolve_url(base_url, rel_uri);
                        variants.push(HlsStreamVariant {
                            bandwidth,
                            resolution,
                            codecs,
                            uri: full_uri,
                            audio_group,
                        });
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            Ok(HlsPlaylist::Master { variants, audio_uris })
        } else {
            let mut target_duration = 10;
            let mut init_uri = None;
            let mut segments = Vec::new();
            let mut cur_duration = 0.0f32;
            let mut seg_idx = 0;

            for line in lines {
                if let Some(val) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
                    target_duration = val.parse().unwrap_or(10);
                } else if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
                    if let Some(uri) = extract_m3u8_attr(rest, "URI") {
                        init_uri = Some(resolve_url(base_url, &uri));
                    }
                } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
                    let dur_str = rest.split(',').next().unwrap_or("0");
                    cur_duration = dur_str.parse().unwrap_or(0.0);
                } else if !line.starts_with('#') {
                    let full_uri = resolve_url(base_url, line);
                    segments.push(HlsSegment {
                        index: seg_idx,
                        uri: full_uri,
                        duration_sec: cur_duration,
                    });
                    seg_idx += 1;
                }
            }

            let total_duration = segments.iter().map(|s| s.duration_sec).sum();
            Ok(HlsPlaylist::Media {
                target_duration,
                init_uri,
                segments,
                total_duration,
            })
        }
    }
}

fn extract_m3u8_attr(attrs: &str, key: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] == b',' || bytes[i] == b' ') {
            i += 1;
        }
        let eq = attrs[i..].find('=')?;
        let k = attrs[i..i + eq].trim();
        let val_start = i + eq + 1;
        if val_start >= attrs.len() {
            break;
        }
        let (val, next_i) = if bytes[val_start] == b'"' {
            let rest = &attrs[val_start + 1..];
            let end_q = rest.find('"').unwrap_or(rest.len());
            (&rest[..end_q], val_start + 1 + end_q + 1)
        } else {
            let rest = &attrs[val_start..];
            let end_c = rest.find(',').unwrap_or(rest.len());
            (rest[..end_c].trim(), val_start + end_c)
        };
        if k.eq_ignore_ascii_case(key) {
            return Some(val.to_string());
        }
        i = next_i;
    }
    None
}

fn resolve_url(base: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }
    let base_clean = base.split('?').next().unwrap_or(base);
    if relative.starts_with('/') {
        if let Some(scheme_end) = base_clean.find("://") {
            let after = &base_clean[scheme_end + 3..];
            let slash = after.find('/').map(|s| scheme_end + 3 + s).unwrap_or(base_clean.len());
            return format!("{}{}", &base_clean[..slash], relative);
        }
    }
    if let Some(pos) = base_clean.rfind('/') {
        format!("{}/{}", &base_clean[..pos], relative)
    } else {
        format!("{}/{}", base, relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hls_master_and_media_parsing() {
        let master_m3u8 = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=4500000,RESOLUTION=1920x1080,CODECS=\"avc1.64002a,mp4a.40.2\"\n1080p/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2200000,RESOLUTION=1280x720\n720p/index.m3u8";
        let parsed = HlsPlaylist::parse(master_m3u8, "https://stream.example.com/live/master.m3u8").unwrap();
        match parsed {
            HlsPlaylist::Master { variants, .. } => {
                assert_eq!(variants.len(), 2);
                assert_eq!(variants[0].resolution, (1920, 1080));
                assert_eq!(variants[0].uri, "https://stream.example.com/live/1080p/index.m3u8");
            }
            _ => panic!("Expected master playlist"),
        }

        let media_m3u8 = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\nseg0.m4s\n#EXTINF:6.0,\nseg1.m4s";
        let parsed_media = HlsPlaylist::parse(media_m3u8, "https://stream.example.com/live/1080p/index.m3u8").unwrap();
        match parsed_media {
            HlsPlaylist::Media { init_uri, segments, .. } => {
                assert_eq!(init_uri.as_deref(), Some("https://stream.example.com/live/1080p/init.mp4"));
                assert_eq!(segments.len(), 2);
            }
            _ => panic!("Expected media playlist"),
        }
    }
}

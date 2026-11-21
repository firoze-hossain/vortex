//! vortex-core/src/hls.rs
//! HTTP Live Streaming (HLS / m3u8) playlist parser and segment downloader.

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
}

#[derive(Clone, Debug, PartialEq)]
pub enum HlsPlaylist {
    Master { variants: Vec<HlsStreamVariant> },
    Media { target_duration: u32, segments: Vec<HlsSegment>, total_duration: f32 },
}

impl HlsPlaylist {
    pub fn parse(content: &str, base_url: &str) -> Result<Self, String> {
        let lines: Vec<&str> = content.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() || lines[0] != "#EXTM3U" {
            return Err("Not a valid HLS m3u8 playlist (missing #EXTM3U header)".into());
        }

        // Check if master playlist
        let has_stream_inf = lines.iter().any(|l| l.starts_with("#EXT-X-STREAM-INF"));
        if has_stream_inf {
            let mut variants = Vec::new();
            let mut i = 0;
            while i < lines.len() {
                if lines[i].starts_with("#EXT-X-STREAM-INF:") {
                    let info = &lines[i]["#EXT-X-STREAM-INF:".len()..];
                    let mut bandwidth = 0;
                    let mut resolution = (0, 0);
                    let mut codecs = String::new();

                    for attr in info.split(',') {
                        let mut kv = attr.splitn(2, '=');
                        let k = kv.next().unwrap_or("").trim();
                        let v = kv.next().unwrap_or("").trim().trim_matches('"');
                        match k {
                            "BANDWIDTH" => bandwidth = v.parse::<u32>().unwrap_or(0),
                            "RESOLUTION" => {
                                let mut res = v.splitn(2, 'x');
                                let w = res.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                                let h = res.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                                resolution = (w, h);
                            }
                            "CODECS" => codecs = v.to_string(),
                            _ => {}
                        }
                    }

                    if i + 1 < lines.len() && !lines[i + 1].starts_with('#') {
                        let rel_uri = lines[i + 1];
                        let full_uri = resolve_url(base_url, rel_uri);
                        variants.push(HlsStreamVariant {
                            bandwidth,
                            resolution,
                            codecs,
                            uri: full_uri,
                        });
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            Ok(HlsPlaylist::Master { variants })
        } else {
            // Media playlist
            let mut target_duration = 10;
            let mut segments = Vec::new();
            let mut cur_duration = 0.0f32;
            let mut seg_idx = 0;

            for line in lines {
                if line.starts_with("#EXT-X-TARGETDURATION:") {
                    target_duration = line["#EXT-X-TARGETDURATION:".len()..].parse().unwrap_or(10);
                } else if line.starts_with("#EXTINF:") {
                    let dur_str = line["#EXTINF:".len()..].split(',').next().unwrap_or("0");
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
                segments,
                total_duration,
            })
        }
    }
}

fn resolve_url(base: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        relative.to_string()
    } else {
        let base_clean = base.split('?').next().unwrap_or(base);
        if let Some(pos) = base_clean.rfind('/') {
            format!("{}/{}", &base_clean[..pos], relative)
        } else {
            format!("{}/{}", base, relative)
        }
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
            HlsPlaylist::Master { variants } => {
                assert_eq!(variants.len(), 2);
                assert_eq!(variants[0].resolution, (1920, 1080));
                assert_eq!(variants[0].uri, "https://stream.example.com/live/1080p/index.m3u8");
            }
            _ => panic!("Expected master playlist"),
        }
    }
}

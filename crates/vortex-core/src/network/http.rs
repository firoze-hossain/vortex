//! vortex-core/src/network/http.rs
//! Real HTTP/HTTPS client layer built over minreq (rustls + webpki-roots).

use std::io::Read;

pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub fn user_agent_for_url(url: &str) -> &'static str {
    if url.contains("googlevideo.com") {
        if url.contains("c=ANDROID_VR") {
            return "com.google.android.apps.youtube.vr.oculus/1.61.48 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";
        }
        if url.contains("c=ANDROID") {
            return "com.google.android.youtube/20.10.38 (Linux; U; Android 14) gzip";
        }
        if url.contains("c=IOS") {
            return "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_1 like Mac OS X;)";
        }
    }
    DEFAULT_USER_AGENT
}

fn should_send_referer(url: &str) -> bool {
    !(url.contains("googlevideo.com")
        && (url.contains("c=ANDROID") || url.contains("c=IOS")))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    pub final_url: String,
    pub status_code: u16,
    pub content_length: Option<u64>,
    pub accept_ranges: bool,
    pub content_type: String,
    pub etag: Option<String>,
    pub filename: String,
}

/// Probes a URL via HEAD (falling back to GET Range: bytes=0-0) to discover real file size,
/// range-resume capability, MIME type, and suggested filename.
pub fn probe_url(url: &str, referer: Option<&str>) -> Result<ProbeResult, String> {
    let ua = user_agent_for_url(url);
    let send_ref = should_send_referer(url);
    let mut req = minreq::head(url)
        .with_header("User-Agent", ua)
        .with_timeout(12);
    if send_ref {
        if let Some(r) = referer {
            if !r.is_empty() {
                req = req.with_header("Referer", r);
            }
        }
    }

    if let Ok(resp) = req.send() {
        if (200..400).contains(&resp.status_code) {
            let content_length = header_ci(&resp.headers, "content-length").and_then(|v| v.parse::<u64>().ok());
            let accept_ranges = header_ci(&resp.headers, "accept-ranges")
                .map(|v| v.to_ascii_lowercase().contains("bytes"))
                .unwrap_or(false);
            let content_type = header_ci(&resp.headers, "content-type")
                .unwrap_or("application/octet-stream")
                .to_string();
            let etag = header_ci(&resp.headers, "etag").map(|s| s.to_string());
            let disp = header_ci(&resp.headers, "content-disposition");
            let filename = parse_disposition_filename(disp).unwrap_or_else(|| filename_from_url(&resp.url));

            if content_length.is_some() || accept_ranges {
                return Ok(ProbeResult {
                    final_url: resp.url,
                    status_code: resp.status_code,
                    content_length,
                    accept_ranges,
                    content_type,
                    etag,
                    filename,
                });
            }
        }
    }

    // Fallback: GET with Range: bytes=0-0 (many CDNs disable HEAD or omit Content-Length on HEAD)
    let mut get_req = minreq::get(url)
        .with_header("User-Agent", ua)
        .with_header("Range", "bytes=0-0")
        .with_timeout(12);
    if send_ref {
        if let Some(r) = referer {
            if !r.is_empty() {
                get_req = get_req.with_header("Referer", r);
            }
        }
    }

    let resp = get_req
        .send_lazy()
        .map_err(|e| format!("HTTP probe failed: {}", e))?;

    let status_code = resp.status_code;
    if status_code < 200 || status_code >= 400 {
        return Err(format!("HTTP error status {}", status_code));
    }

    let content_range = header_ci(&resp.headers, "content-range").map(|s| s.to_string());
    let total_from_range = content_range.as_deref().and_then(parse_content_range_total);
    let content_length = total_from_range
        .or_else(|| header_ci(&resp.headers, "content-length").and_then(|v| v.parse::<u64>().ok()));

    let accept_ranges = status_code == 206
        || header_ci(&resp.headers, "accept-ranges")
            .map(|v| v.to_ascii_lowercase().contains("bytes"))
            .unwrap_or(false);

    let content_type = header_ci(&resp.headers, "content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    let etag = header_ci(&resp.headers, "etag").map(|s| s.to_string());
    let disp = header_ci(&resp.headers, "content-disposition");
    let filename = parse_disposition_filename(disp).unwrap_or_else(|| filename_from_url(&resp.url));
    let final_url = if resp.url.is_empty() { url.to_string() } else { resp.url };

    Ok(ProbeResult {
        final_url,
        status_code,
        content_length,
        accept_ranges,
        content_type,
        etag,
        filename,
    })
}

pub fn get_text(url: &str, referer: Option<&str>) -> Result<String, String> {
    let mut req = minreq::get(url)
        .with_header("User-Agent", DEFAULT_USER_AGENT)
        .with_header("Accept-Language", "en-US,en;q=0.9")
        .with_timeout(12);
    if let Some(r) = referer {
        if !r.is_empty() {
            req = req.with_header("Referer", r);
        }
    }
    let resp = req.send().map_err(|e| format!("HTTP GET failed: {}", e))?;
    if resp.status_code < 200 || resp.status_code >= 400 {
        return Err(format!("HTTP status {}", resp.status_code));
    }
    resp.as_str()
        .map(|s| s.to_string())
        .map_err(|e| format!("UTF-8 decode error: {}", e))
}

pub fn post_json(url: &str, json_body: &str, user_agent: Option<&str>, extra_headers: &[(&str, &str)]) -> Result<String, String> {
    let mut req = minreq::post(url)
        .with_header("Content-Type", "application/json")
        .with_header("User-Agent", user_agent.unwrap_or(DEFAULT_USER_AGENT))
        .with_body(json_body.as_bytes().to_vec())
        .with_timeout(12);
    for &(k, v) in extra_headers {
        req = req.with_header(k, v);
    }
    let resp = req.send().map_err(|e| format!("HTTP POST failed: {}", e))?;
    if resp.status_code < 200 || resp.status_code >= 400 {
        return Err(format!("HTTP POST status {}", resp.status_code));
    }
    resp.as_str()
        .map(|s| s.to_string())
        .map_err(|e| format!("UTF-8 decode error: {}", e))
}

pub fn get_bytes(url: &str, referer: Option<&str>, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut req = minreq::get(url)
        .with_header("User-Agent", DEFAULT_USER_AGENT)
        .with_timeout(12);
    if let Some(r) = referer {
        if !r.is_empty() {
            req = req.with_header("Referer", r);
        }
    }
    let resp = req.send_lazy().map_err(|e| format!("HTTP GET failed: {}", e))?;
    if resp.status_code < 200 || resp.status_code >= 400 {
        return Err(format!("HTTP status {}", resp.status_code));
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    let mut reader = resp;
    while out.len() < max_bytes {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let take = n.min(max_bytes - out.len());
                out.extend_from_slice(&buf[..take]);
            }
            Err(e) => return Err(format!("Stream read error: {}", e)),
        }
    }
    Ok(out)
}

pub fn open_range_stream(
    url: &str,
    range: Option<(u64, u64)>,
    referer: Option<&str>,
) -> Result<minreq::ResponseLazy, String> {
    let ua = user_agent_for_url(url);
    let mut req = minreq::get(url)
        .with_header("User-Agent", ua)
        .with_timeout(120);
    if let Some((start, end)) = range {
        req = req.with_header("Range", format!("bytes={}-{}", start, end));
    }
    if should_send_referer(url) {
        if let Some(r) = referer {
            if !r.is_empty() {
                req = req.with_header("Referer", r);
            }
        }
    }
    let resp = req.send_lazy().map_err(|e| format!("HTTP stream connect error: {}", e))?;
    if resp.status_code < 200 || resp.status_code >= 400 {
        if resp.status_code == 403 && url.contains("googlevideo.com") {
            return Err("YouTube 403 (HD stream requires PO-Token). Use '360p MP4 (Direct)' or install yt-dlp / Browser Extension.".into());
        }
        return Err(format!("HTTP stream returned status {}", resp.status_code));
    }
    Ok(resp)
}

fn header_ci<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let target = name.to_ascii_lowercase();
    headers
        .iter()
        .find(|(k, _)| k.to_ascii_lowercase() == target)
        .map(|(_, v)| v.as_str())
}

fn parse_content_range_total(val: &str) -> Option<u64> {
    let slash = val.rfind('/')?;
    let total_str = val[slash + 1..].trim();
    if total_str == "*" {
        None
    } else {
        total_str.parse::<u64>().ok()
    }
}

fn parse_disposition_filename(disp: Option<&str>) -> Option<String> {
    let d = disp?;
    let lower = d.to_ascii_lowercase();
    let idx = lower.find("filename=")?;
    let raw = d[idx + 9..].split(';').next()?.trim().trim_matches('"').trim_matches('\'');
    if raw.is_empty() {
        None
    } else {
        Some(sanitize_filename(raw))
    }
}

pub fn filename_from_url(url: &str) -> String {
    let no_frag = url.split('#').next().unwrap_or(url);
    let no_query = no_frag.split('?').next().unwrap_or(no_frag);
    let seg = no_query.rsplit('/').next().unwrap_or("");
    let cleaned = sanitize_filename(seg);
    if cleaned.is_empty() {
        "download.bin".to_string()
    } else {
        cleaned
    }
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filename_and_range_parsing() {
        assert_eq!(parse_content_range_total("bytes 0-0/1048576"), Some(1048576));
        assert_eq!(parse_content_range_total("bytes 0-0/*"), None);
        assert_eq!(
            parse_disposition_filename(Some("attachment; filename=\"video_1080p.mp4\"")),
            Some("video_1080p.mp4".into())
        );
        assert_eq!(
            filename_from_url("https://cdn.example.com/media/clip.mp4?token=abc"),
            "clip.mp4"
        );
    }
}

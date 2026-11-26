//! vortex-core/src/bridge/server.rs
//! Lightweight pure-Rust HTTP/1.1 + CORS JSON & Status Bridge on 127.0.0.1:47391
//! connecting Aero Browser, CLI tools, and Rust/WASM frontends to the Vortex desktop engine.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::engine::{DownloadTask, VortexEngine};
use crate::media::preview::fetch_url_preview;
use crate::media::sniffer::MediaStreamQuality;
use crate::network::http::{filename_from_url, sanitize_filename};

pub struct BridgeServerHandle {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl BridgeServerHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Deserialize)]
struct PreviewRequest {
    url: String,
}

#[derive(Deserialize)]
struct SniffReportRequest {
    page_url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail_url: Option<String>,
    #[serde(default)]
    qualities: Vec<MediaStreamQuality>,
}

#[derive(Deserialize)]
struct StreamDownloadRequest {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    quality: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    threads: usize,
    #[serde(default)]
    is_hls: bool,
    #[serde(default)]
    referer: Option<String>,
}

#[derive(Deserialize)]
struct TorrentDownloadRequest {
    url: String,
    #[serde(default)]
    sequential: bool,
}

#[derive(Serialize)]
struct TaskSummaryDto {
    id: u64,
    kind: &'static str,
    name: String,
    progress_pct: f32,
    speed_bps: u64,
    total_size: u64,
    downloaded_bytes: u64,
    status: &'static str,
    save_path: String,
}

pub fn start_bridge_server(engine: Arc<Mutex<VortexEngine>>, port: u16) -> Result<BridgeServerHandle, String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("Failed to bind Vortex Bridge on 127.0.0.1:{}: {}", port, e))?;
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    listener.set_nonblocking(true).ok();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    thread::Builder::new()
        .name("vortex-bridge".into())
        .spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let eng = Arc::clone(&engine);
                        thread::spawn(move || handle_client(stream, eng));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(40));
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| format!("Failed to spawn bridge thread: {}", e))?;

    Ok(BridgeServerHandle {
        port: actual_port,
        stop,
    })
}

fn handle_client(mut stream: TcpStream, engine: Arc<Mutex<VortexEngine>>) {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
    let cloned = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(cloned);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let path = parts[1].split('?').next().unwrap_or(parts[1]);

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut body = vec![0u8; content_length.min(2 * 1024 * 1024)];
    if !body.is_empty() && reader.read_exact(&mut body).is_err() {
        send_json(&mut stream, 400, r#"{"error":"Incomplete request body"}"#);
        return;
    }

    if method == "OPTIONS" {
        let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    match (method, path) {
        ("GET", "/") => {
            let html = render_status_html(&engine);
            send_html(&mut stream, 200, &html);
        }
        ("GET", "/api/health") => {
            let (count, dl_bps) = {
                let mut eng = engine.lock().unwrap();
                eng.bridge_connected_clients += 1;
                (eng.tasks.len(), eng.total_download_rate_bps())
            };
            let json = serde_json::json!({
                "status": "ok",
                "app": "Vortex Studio",
                "version": "2.0.0",
                "task_count": count,
                "download_rate_bps": dl_bps
            });
            send_json(&mut stream, 200, &json.to_string());
        }
        ("GET", "/api/tasks") => {
            let eng = engine.lock().unwrap();
            let list: Vec<TaskSummaryDto> = eng
                .tasks
                .iter()
                .map(|t| TaskSummaryDto {
                    id: t.id(),
                    kind: match t {
                        DownloadTask::Torrent(_) => "torrent",
                        DownloadTask::Stream(_) => "stream",
                    },
                    name: t.name().to_string(),
                    progress_pct: t.progress_pct(),
                    speed_bps: t.download_rate_bps(),
                    total_size: t.total_size(),
                    downloaded_bytes: t.downloaded_bytes(),
                    status: t.status().label(),
                    save_path: t.save_path().to_string_lossy().to_string(),
                })
                .collect();
            let json = serde_json::json!({
                "tasks": list,
                "total_download_bps": eng.total_download_rate_bps(),
                "total_upload_bps": eng.total_upload_rate_bps(),
            });
            send_json(&mut stream, 200, &json.to_string());
        }
        ("GET", "/api/sniffed") => {
            let eng = engine.lock().unwrap();
            let json = serde_json::json!({ "items": eng.sniffer.sniffed_items });
            send_json(&mut stream, 200, &json.to_string());
        }
        ("POST", "/api/sniff") => {
            let Ok(req) = serde_json::from_slice::<SniffReportRequest>(&body) else {
                send_json(&mut stream, 400, r#"{"error":"Invalid JSON body"}"#);
                return;
            };
            let mut eng = engine.lock().unwrap();
            let id = eng
                .sniffer
                .register_media(&req.page_url, &req.title, req.thumbnail_url, req.qualities)
                .id;
            let json = serde_json::json!({ "ok": true, "id": id });
            send_json(&mut stream, 200, &json.to_string());
        }
        ("POST", "/api/preview") => {
            let Ok(req) = serde_json::from_slice::<PreviewRequest>(&body) else {
                send_json(&mut stream, 400, r#"{"error":"Invalid JSON body"}"#);
                return;
            };
            match fetch_url_preview(&req.url) {
                Ok(preview) => {
                    let json = serde_json::to_string(&preview).unwrap_or_else(|_| "{}".into());
                    send_json(&mut stream, 200, &json);
                }
                Err(e) => {
                    let json = serde_json::json!({ "error": e }).to_string();
                    send_json(&mut stream, 422, &json);
                }
            }
        }
        ("POST", "/api/download/stream") => {
            let Ok(req) = serde_json::from_slice::<StreamDownloadRequest>(&body) else {
                send_json(&mut stream, 400, r#"{"error":"Invalid JSON body"}"#);
                return;
            };
            let mut eng = engine.lock().unwrap();
            let default_threads = eng.config.default_threads;
            let threads = if req.threads == 0 { default_threads } else { req.threads };
            let raw_name = if !req.filename.is_empty() {
                req.filename
            } else if !req.title.is_empty() {
                let ext = if req.is_hls || req.url.contains(".m3u8") {
                    "ts"
                } else if req.url.contains(".webm") {
                    "webm"
                } else {
                    "mp4"
                };
                format!("{}.{}", sanitize_filename(&req.title), ext)
            } else {
                filename_from_url(&req.url)
            };

            let res_lbl = if req.quality.is_empty() {
                "Auto".to_string()
            } else {
                req.quality
            };

            match eng.start_real_stream(
                &raw_name,
                &req.url,
                &res_lbl,
                req.size,
                threads,
                req.is_hls,
                req.referer,
                None,
            ) {
                Ok(id) => {
                    let json = serde_json::json!({ "ok": true, "id": id, "filename": raw_name });
                    send_json(&mut stream, 200, &json.to_string());
                }
                Err(e) => {
                    let json = serde_json::json!({ "ok": false, "error": e });
                    send_json(&mut stream, 500, &json.to_string());
                }
            }
        }
        ("POST", "/api/download/torrent") => {
            let Ok(req) = serde_json::from_slice::<TorrentDownloadRequest>(&body) else {
                send_json(&mut stream, 400, r#"{"error":"Invalid JSON body"}"#);
                return;
            };
            let mut eng = engine.lock().unwrap();
            let res = if req.url.starts_with("magnet:") {
                eng.start_real_magnet(&req.url, None, req.sequential)
            } else {
                Err("Only magnet: URIs are accepted over JSON; load .torrent files directly".into())
            };
            match res {
                Ok(id) => {
                    let json = serde_json::json!({ "ok": true, "id": id });
                    send_json(&mut stream, 200, &json.to_string());
                }
                Err(e) => {
                    let json = serde_json::json!({ "ok": false, "error": e });
                    send_json(&mut stream, 400, &json.to_string());
                }
            }
        }
        ("POST", p) if p.starts_with("/api/tasks/") => {
            let rest = &p["/api/tasks/".len()..];
            let mut segs = rest.split('/');
            let id = segs.next().and_then(|s| s.parse::<u64>().ok());
            let action = segs.next();
            if let (Some(id), Some(act)) = (id, action) {
                let mut eng = engine.lock().unwrap();
                match act {
                    "pause" => eng.pause_task(id),
                    "resume" => eng.resume_task(id),
                    "delete" => eng.remove_task(id),
                    _ => {
                        send_json(&mut stream, 404, r#"{"error":"Unknown action"}"#);
                        return;
                    }
                }
                send_json(&mut stream, 200, r#"{"ok":true}"#);
            } else {
                send_json(&mut stream, 400, r#"{"error":"Invalid task route"}"#);
            }
        }
        _ => {
            send_json(&mut stream, 404, r#"{"error":"Not found"}"#);
        }
    }
}

/// Pure-Rust server-rendered HTML live status page (zero JavaScript).
fn render_status_html(engine: &Arc<Mutex<VortexEngine>>) -> String {
    let eng = engine.lock().unwrap();
    let mut rows = String::new();
    for t in &eng.tasks {
        let kind = if t.is_torrent() { "TORRENT" } else { "STREAM" };
        let name = html_escape(t.name());
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{:.1}%</td><td>{}</td></tr>",
            kind,
            name,
            t.progress_pct(),
            t.status().label()
        ));
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"4\">Queue is empty. Paste a URL or Magnet link in Vortex Studio.</td></tr>");
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"3\"><title>Vortex Studio Bridge</title>\
<style>body{{background:#0b1120;color:#f8fafc;font-family:system-ui,sans-serif;padding:24px}}table{{width:100%;border-collapse:collapse;margin-top:16px}}\
th,td{{border:1px solid #334155;padding:8px 12px;text-align:left}}th{{background:#162032;color:#38bdf8}}</style></head>\
<body><h1>Vortex Studio Pure-Rust IPC Bridge</h1><p>Active tasks: {} | Download: {} B/s | Upload: {} B/s</p>\
<table><thead><tr><th>Type</th><th>Name</th><th>Progress</th><th>Status</th></tr></thead><tbody>{}</tbody></table></body></html>",
        eng.tasks.len(),
        eng.total_download_rate_bps(),
        eng.total_upload_rate_bps(),
        rows
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn send_html(stream: &mut TcpStream, status: u16, html: &str) {
    let resp = format!(
        "HTTP/1.1 {} OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        html.len(),
        html
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn send_json(stream: &mut TcpStream, status: u16, json: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        _ => "Internal Server Error",
    };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        reason,
        json.len(),
        json
    );
    let _ = stream.write_all(resp.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_server_health_and_tasks() {
        let engine = Arc::new(Mutex::new(VortexEngine::new()));
        let srv = start_bridge_server(Arc::clone(&engine), 0).unwrap();
        let port = srv.port;
        let resp = minreq::get(format!("http://127.0.0.1:{}/api/health", port))
            .with_timeout(3)
            .send()
            .unwrap();
        assert_eq!(resp.status_code, 200);
        assert!(resp.as_str().unwrap().contains("Vortex Studio"));

        let html_resp = minreq::get(format!("http://127.0.0.1:{}/", port))
            .with_timeout(3)
            .send()
            .unwrap();
        assert_eq!(html_resp.status_code, 200);
        assert!(!html_resp.as_str().unwrap().contains("<script"));
        srv.stop();
    }
}

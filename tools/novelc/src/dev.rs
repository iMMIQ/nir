//! Developer-friendly HTTP dev server (with live SSE hot-reload & HTTP Range streaming)
//! and static preview server.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use tiny_http::{Header, Request, Response, Server, StatusCode};

/// Parses a single `Range: bytes=start-end` header into inclusive `(start, end)`.
fn parse_byte_range(req: &Request, total_len: u64) -> Option<(u64, u64)> {
    if total_len == 0 {
        return None;
    }
    let val = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))?
        .value
        .as_str()
        .trim();
    let spec = val.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // Multi-range falls back to full response
    }
    let (start_s, end_s) = spec.split_once('-')?;
    if start_s.is_empty() {
        let suffix: u64 = end_s.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = total_len.saturating_sub(suffix);
        return Some((start, total_len - 1));
    }
    let start: u64 = start_s.parse().ok()?;
    if start >= total_len {
        return None;
    }
    let end = if end_s.is_empty() {
        total_len - 1
    } else {
        end_s.parse::<u64>().ok()?.min(total_len - 1)
    };
    (start <= end).then_some((start, end))
}

fn respond_file_with_range(req: Request, path: &Path, cache_control: &str) {
    let Ok(mut file) = File::open(path) else {
        let _ = req.respond(Response::from_string("read error").with_status_code(500));
        return;
    };
    let Ok(meta) = file.metadata() else {
        let _ = req.respond(Response::from_string("stat error").with_status_code(500));
        return;
    };
    let total = meta.len();
    let mime = mime_for_path(path);

    if let Some((start, end)) = parse_byte_range(&req, total) {
        let len = (end - start + 1) as usize;
        if file.seek(SeekFrom::Start(start)).is_ok() {
            let mut buf = vec![0u8; len];
            if file.read_exact(&mut buf).is_ok() {
                let mut resp = Response::from_data(buf).with_status_code(StatusCode(206));
                if let Ok(h) = Header::from_bytes(b"Content-Type", mime.as_bytes()) {
                    resp.add_header(h);
                }
                if let Ok(h) = Header::from_bytes(b"Accept-Ranges", b"bytes") {
                    resp.add_header(h);
                }
                if let Ok(h) = Header::from_bytes(
                    b"Content-Range",
                    format!("bytes {start}-{end}/{total}").as_bytes(),
                ) {
                    resp.add_header(h);
                }
                if let Ok(h) = Header::from_bytes(b"Cache-Control", cache_control.as_bytes()) {
                    resp.add_header(h);
                }
                let _ = req.respond(resp);
                return;
            }
        }
    }

    let mut resp = Response::from_file(file);
    if let Ok(h) = Header::from_bytes(b"Content-Type", mime.as_bytes()) {
        resp.add_header(h);
    }
    if let Ok(h) = Header::from_bytes(b"Accept-Ranges", b"bytes") {
        resp.add_header(h);
    }
    if let Ok(h) = Header::from_bytes(b"Cache-Control", cache_control.as_bytes()) {
        resp.add_header(h);
    }
    let _ = req.respond(resp);
}

struct ArcSliceReader {
    data: Arc<Vec<u8>>,
    pos: usize,
    end: usize,
}

impl Read for ArcSliceReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let rem = self.end.saturating_sub(self.pos);
        if rem == 0 {
            return Ok(0);
        }
        let n = rem.min(buf.len());
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn respond_arc_bytes_with_range(req: Request, data: Arc<Vec<u8>>, cache_control: &str) {
    let total = data.len() as u64;
    let mut headers = Vec::with_capacity(4);
    if let Ok(h) = Header::from_bytes(b"Content-Type", b"application/octet-stream") {
        headers.push(h);
    }
    if let Ok(h) = Header::from_bytes(b"Accept-Ranges", b"bytes") {
        headers.push(h);
    }
    if let Ok(h) = Header::from_bytes(b"Cache-Control", cache_control.as_bytes()) {
        headers.push(h);
    }

    if let Some((start, end)) = parse_byte_range(&req, total) {
        let len = (end - start + 1) as usize;
        if let Ok(h) = Header::from_bytes(
            b"Content-Range",
            format!("bytes {start}-{end}/{total}").as_bytes(),
        ) {
            headers.push(h);
        }
        let reader = ArcSliceReader {
            data,
            pos: start as usize,
            end: (end + 1) as usize,
        };
        let resp = Response::new(StatusCode(206), headers, reader, Some(len), None);
        let _ = req.respond(resp);
        return;
    }

    let len = data.len();
    let reader = ArcSliceReader {
        data,
        pos: 0,
        end: len,
    };
    let resp = Response::new(StatusCode(200), headers, reader, Some(len), None);
    let _ = req.respond(resp);
}

/// Serves a static directory (e.g. a built Web bundle) over HTTP with `Range` support.
pub fn serve_static(dir: &Path, host: &str, port: u16) -> Result<()> {
    let addr = format!("{host}:{port}");
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("binding http://{addr}: {e}"))?;
    println!("NIR static preview: http://{addr}/");
    for req in server.incoming_requests() {
        let url_path = req.url().split('?').next().unwrap_or("/");
        let rel = if url_path == "/" {
            "index.html"
        } else {
            url_path.trim_start_matches('/')
        };
        if rel.contains("..") {
            let _ = req.respond(Response::from_string("forbidden").with_status_code(403));
            continue;
        }
        let path = dir.join(rel);
        if path.is_file() {
            respond_file_with_range(req, &path, "no-cache");
        } else {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
    Ok(())
}

/// Starts `novelc dev`: watches project sources, rebuilds `.nirpak` on edit,
/// serves Web host + in-memory `game.nirpak` (with zero-clone Range support),
/// and notifies connected browsers via Server-Sent Events (`/__nir_events`).
pub fn run_dev_server(project_dir: &Path, sdk_web_dir: &Path, host: &str, port: u16) -> Result<()> {
    let initial = nir_build::compile_project_fast(project_dir)
        .with_context(|| format!("initial compile of `{}`", project_dir.display()))?;
    println!(
        "Built initial game.nirpak ({:.1} KiB, {} ops, {} assets)",
        initial.nirpak.len() as f64 / 1024.0,
        initial.program.ops.len(),
        initial.program.assets.len()
    );

    let pak_buf = Arc::new(RwLock::new(Arc::new(initial.nirpak)));
    let version = Arc::new(AtomicU64::new(1));

    // File watcher thread (polling mtimes every 250ms, zero OS-specific native deps)
    {
        let watch_root = project_dir.to_path_buf();
        let pak_buf = pak_buf.clone();
        let version = version.clone();
        std::thread::Builder::new()
            .name("nir-watch".into())
            .spawn(move || {
                let mut prev = snapshot_mtimes(&watch_root);
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    let next = snapshot_mtimes(&watch_root);
                    if next != prev {
                        prev = next;
                        match nir_build::compile_project_fast(&watch_root) {
                            Ok(rebuilt) => {
                                let size_kb = rebuilt.nirpak.len() as f64 / 1024.0;
                                if let Ok(mut guard) = pak_buf.write() {
                                    *guard = Arc::new(rebuilt.nirpak);
                                }
                                let v = version.fetch_add(1, Ordering::SeqCst) + 1;
                                println!(
                                    "[hot-reload #{v}] rebuilt game.nirpak ({size_kb:.1} KiB)"
                                );
                            }
                            Err(e) => {
                                eprintln!("[hot-reload error] {e:#}");
                            }
                        }
                    }
                }
            })
            .context("spawning dev file watcher")?;
    }

    let addr = format!("{host}:{port}");
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("binding http://{addr}: {e}"))?;
    println!("NIR dev server listening at http://{addr}/ (live hot-reload active)");

    for req in server.incoming_requests() {
        let url_path = req.url().split('?').next().unwrap_or("/").to_owned();
        if url_path == "/__nir_events" {
            let ver = version.clone();
            std::thread::spawn(move || {
                serve_sse_client(req, ver);
            });
            continue;
        }
        if url_path == "/game.nirpak" {
            let data = pak_buf
                .read()
                .map(|g| Arc::clone(&g))
                .unwrap_or_else(|_| Arc::new(Vec::new()));
            respond_arc_bytes_with_range(req, data, "no-store");
            continue;
        }

        let rel = if url_path == "/" {
            "index.html"
        } else {
            url_path.trim_start_matches('/')
        };
        if rel.contains("..") {
            let _ = req.respond(Response::from_string("forbidden").with_status_code(403));
            continue;
        }
        let file = sdk_web_dir.join(rel);
        if file.is_file() {
            respond_file_with_range(req, &file, "no-cache");
        } else {
            let _ = req.respond(
                Response::from_string(
                    "Web player SDK files not found; run `cargo xtask sdk` or pass `--sdk <dir>`",
                )
                .with_status_code(404),
            );
        }
    }
    Ok(())
}

fn serve_sse_client(req: tiny_http::Request, version: Arc<AtomicU64>) {
    let mut writer = req.into_writer();
    let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n: connected\n\n";
    if writer.write_all(header.as_bytes()).is_err() || writer.flush().is_err() {
        return;
    }
    let mut seen = version.load(Ordering::SeqCst);
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let cur = version.load(Ordering::SeqCst);
        if cur != seen {
            seen = cur;
            let msg = format!("event: reload\ndata: {cur}\n\n");
            if writer.write_all(msg.as_bytes()).is_err() || writer.flush().is_err() {
                break;
            }
        }
    }
}

fn snapshot_mtimes(root: &Path) -> BTreeMap<PathBuf, Option<SystemTime>> {
    let mut out = BTreeMap::new();
    let toml = root.join("game.toml");
    if toml.is_file() {
        out.insert(
            toml.clone(),
            fs::metadata(&toml).ok().and_then(|m| m.modified().ok()),
        );
    }
    for sub in ["scripts", "assets", "locales", "ui"] {
        let dir = root.join(sub);
        if dir.is_dir() {
            collect_mtimes(&dir, &mut out);
        }
    }
    out
}

fn collect_mtimes(dir: &Path, out: &mut BTreeMap<PathBuf, Option<SystemTime>>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_mtimes(&p, out);
        } else if p.is_file() {
            let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
            out.insert(p, mtime);
        }
    }
}

fn mime_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json; charset=utf-8",
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

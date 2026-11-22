//! Developer-friendly HTTP dev server (with live SSE hot-reload) and static preview server.

use anyhow::{Context, Result};
use nir_build::optimize::OptimizeConfig;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use tiny_http::{Header, Response, Server};

/// Serves a static directory (e.g. a built Web bundle) over HTTP.
pub fn serve_static(dir: &Path, host: &str, port: u16) -> Result<()> {
    let addr = format!("{host}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("binding http://{addr}: {e}"))?;
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
            match fs::read(&path) {
                Ok(bytes) => {
                    let mime = mime_for_path(&path);
                    let mut resp = Response::from_data(bytes);
                    if let Ok(h) = Header::from_bytes(b"Content-Type", mime.as_bytes()) {
                        resp.add_header(h);
                    }
                    if let Ok(h) = Header::from_bytes(b"Cache-Control", b"no-cache") {
                        resp.add_header(h);
                    }
                    let _ = req.respond(resp);
                }
                Err(_) => {
                    let _ = req.respond(Response::from_string("read error").with_status_code(500));
                }
            }
        } else {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
        }
    }
    Ok(())
}

/// Starts `novelc dev`: watches project sources, rebuilds `.nirpak` on edit,
/// serves Web host + in-memory `game.nirpak`, and notifies connected browsers
/// via Server-Sent Events (`/__nir_events`) for instant hot-reload!
pub fn run_dev_server(project_dir: &Path, sdk_web_dir: &Path, host: &str, port: u16) -> Result<()> {
    let fast_opt = OptimizeConfig {
        enabled: false, // Ultra-fast iteration in dev mode!
        ..Default::default()
    };
    let initial = nir_build::compile_project(project_dir, &fast_opt)
        .with_context(|| format!("initial compile of `{}`", project_dir.display()))?;
    println!(
        "Built initial game.nirpak ({:.1} KiB, {} ops, {} assets)",
        initial.nirpak.len() as f64 / 1024.0,
        initial.program.ops.len(),
        initial.program.assets.len()
    );

    let pak_buf = Arc::new(RwLock::new(initial.nirpak));
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
                        match nir_build::compile_project(&watch_root, &fast_opt) {
                            Ok(rebuilt) => {
                                let size_kb = rebuilt.nirpak.len() as f64 / 1024.0;
                                if let Ok(mut guard) = pak_buf.write() {
                                    *guard = rebuilt.nirpak;
                                }
                                let v = version.fetch_add(1, Ordering::SeqCst) + 1;
                                println!("[hot-reload #{v}] rebuilt game.nirpak ({size_kb:.1} KiB)");
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
    let server = Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("binding http://{addr}: {e}"))?;
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
            let bytes = pak_buf.read().map(|g| g.clone()).unwrap_or_default();
            let mut resp = Response::from_data(bytes);
            if let Ok(h) = Header::from_bytes(b"Content-Type", b"application/octet-stream") {
                resp.add_header(h);
            }
            if let Ok(h) = Header::from_bytes(b"Cache-Control", b"no-store") {
                resp.add_header(h);
            }
            let _ = req.respond(resp);
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
            if let Ok(bytes) = fs::read(&file) {
                let mut resp = Response::from_data(bytes);
                if let Ok(h) = Header::from_bytes(b"Content-Type", mime_for_path(&file).as_bytes()) {
                    resp.add_header(h);
                }
                let _ = req.respond(resp);
            } else {
                let _ = req.respond(Response::from_string("read error").with_status_code(500));
            }
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
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

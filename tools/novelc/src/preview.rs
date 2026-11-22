use anyhow::{Context, Result};
use nir_compiler::{build_profile, diagnostic, OptimizeOptions};
use nir_format::ReleaseManifest;
use std::{
    fs,
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static SERVE_TIMING: OnceLock<bool> = OnceLock::new();
static SERVE_REQUEST_ID: AtomicU64 = AtomicU64::new(0);

fn serve_timing() -> bool {
    *SERVE_TIMING.get_or_init(|| std::env::var_os("NIR_SERVE_TIMING").is_some())
}

fn wall_time_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
}

fn log_serve_timing(id: u64, phase: &str, path: &str, elapsed: Duration, bytes: u64) {
    eprintln!(
        "nir_serve id={id} phase={phase} wall_us={} elapsed_us={} bytes={bytes} path={path}",
        wall_time_us(),
        elapsed.as_micros(),
    );
}

#[derive(Default)]
struct DevStatus {
    release: String,
    building: bool,
    error: Option<String>,
    scenario: Option<nir_compiler::ScenarioReplayPlan>,
}
impl DevStatus {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"format":1,"release":self.release,"building":self.building,"stale":self.building || self.error.is_some(),"error":self.error,"scenario":self.scenario})
    }
}
// Poll metadata rather than decoding or hashing every large media file on every turn.
// An editor's atomic rename, deletion or new file changes the sorted signature too.
fn fingerprint(root: &Path) -> Result<String> {
    const MAX_DEPTH: usize = 32;
    const MAX_ENTRIES: usize = 65_536;
    fn walk(
        root: &Path,
        dir: &Path,
        depth: usize,
        entries: &mut usize,
        out: &mut String,
    ) -> Result<()> {
        anyhow::ensure!(
            depth <= MAX_DEPTH,
            "E_WATCH_LIMIT: directory nesting too deep"
        );
        let mut files: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
        files.sort_by_key(|f| f.file_name());
        for file in files {
            *entries += 1;
            anyhow::ensure!(*entries <= MAX_ENTRIES, "E_WATCH_LIMIT: too many files");
            let name = file.file_name();
            if [
                ".git",
                ".nir",
                "dist",
                "reports",
                "target",
                "node_modules",
                "game.local.toml",
            ]
            .iter()
            .any(|n| name == *n)
            {
                continue;
            }
            let kind = file.file_type()?;
            if kind.is_dir() {
                walk(root, &file.path(), depth + 1, entries, out)?;
            } else {
                let meta = fs::symlink_metadata(file.path())?;
                let at = meta
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                out.push_str(&format!(
                    "{:?}:{}:{at}:{}\n",
                    file.path().strip_prefix(root)?,
                    meta.len(),
                    kind.is_symlink()
                ));
            }
        }
        Ok(())
    }
    let mut text = String::new();
    let mut entries = 0usize;
    walk(root, root, 0, &mut entries, &mut text)?;
    // Caches remain ignored, but finishing/recovering a text transaction must
    // retry a candidate rejected while the journal was present.
    if root.join(".nir/text-transaction.json").try_exists()? {
        text.push_str("text-transaction:pending\n");
    }
    Ok(nir_content::digest(text.as_bytes()))
}
fn publish(candidate: &Path, out: &Path, release: &str) -> Result<()> {
    fn copy_atomic(from: &Path, to: &Path) -> Result<()> {
        fs::create_dir_all(to.parent().unwrap())?;
        let name = to
            .file_name()
            .and_then(|n| n.to_str())
            .context("E_PREVIEW_PATH")?;
        let temporary = to.with_file_name(format!(".{name}.{}.next", std::process::id()));
        if let Err(err) = fs::copy(from, &temporary).and_then(|_| fs::rename(&temporary, to)) {
            let _ = fs::remove_file(&temporary);
            return Err(err.into());
        }
        Ok(())
    }
    let release_path = format!("releases/{release}.json");
    let bytes = fs::read(candidate.join(&release_path))?;
    nir_content::verify(&bytes, release)?;
    let manifest: ReleaseManifest = nir_content::parse(&bytes, "preview release")?;
    nir_content::validate_release(&manifest)?;
    for (hash, object) in &manifest.objects {
        let source = candidate.join(&object.path);
        let target = out.join(&object.path);
        nir_content::verify(&fs::read(&source)?, hash)?;
        let already_present =
            fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() == object.bytes);
        if !already_present {
            copy_atomic(&source, &target)?;
        }
        let compressed = candidate.join(format!("{}.gz", object.path));
        if compressed.try_exists()? {
            let mut decoded = Vec::new();
            flate2::read::MultiGzDecoder::new(fs::File::open(&compressed)?)
                .take(object.bytes + 1)
                .read_to_end(&mut decoded)?;
            anyhow::ensure!(decoded.len() as u64 == object.bytes, "E_GZIP_SIZE");
            nir_content::verify(&decoded, hash)?;
            let gz_target = out.join(format!("{}.gz", object.path));
            let gz_src_len = fs::metadata(&compressed).map(|m| m.len()).ok();
            let gz_present = gz_src_len.is_some_and(|len| {
                fs::metadata(&gz_target).is_ok_and(|m| m.is_file() && m.len() == len)
            });
            if !gz_present {
                copy_atomic(&compressed, &gz_target)?;
            }
        }
    }
    copy_atomic(&candidate.join(&release_path), &out.join(&release_path))?;
    let mut launch_items: Vec<(&str, &String)> = vec![
        ("index.html", &manifest.launch.html),
        ("bootstrap.js", &manifest.launch.bootstrap),
    ];
    if let Some(style) = &manifest.launch.style {
        launch_items.push(("style.css", style));
    }
    for (name, hash) in launch_items {
        let source = candidate.join(format!("releases/{release}/{name}"));
        nir_content::verify(&fs::read(&source)?, hash)?;
        copy_atomic(&source, &out.join(format!("releases/{release}/{name}")))?;
    }
    let mut root_items = vec!["index.html", "bootstrap.js", "NOTICE.txt"];
    if manifest.launch.style.is_some() {
        root_items.push("style.css");
    }
    for name in root_items {
        copy_atomic(&candidate.join(name), &out.join(name))?;
    }
    // The only mutable runtime pointer is replaced after the complete object closure exists.
    copy_atomic(
        &candidate.join("channels/stable.json"),
        &out.join("channels/stable.json"),
    )?;
    Ok(())
}
pub fn dev(
    root: &Path,
    sdk: &Path,
    port: u16,
    scenario: Option<nir_compiler::ScenarioReplayPlan>,
) -> Result<()> {
    let root = fs::canonicalize(root)?;
    let sdk = fs::canonicalize(sdk)?;
    let out = root.join("dist/full/web");
    // An edit during the initial build must still be noticed by the first watch turn.
    let initial_fingerprint = fingerprint(&root)?;
    let initial = build_profile(
        &root,
        &sdk,
        &out,
        "dev",
        true,
        true,
        &OptimizeOptions::default(),
    )?;
    let status = Arc::new(Mutex::new(DevStatus {
        release: initial.release,
        scenario,
        ..Default::default()
    }));
    let watched = status.clone();
    let served = out.clone();
    thread::spawn(move || {
        let mut committed = initial_fingerprint;
        let mut pending: Option<(String, Instant)> = None;
        loop {
            thread::sleep(Duration::from_millis(250));
            let current = match fingerprint(&root) {
                Ok(v) => v,
                Err(e) => {
                    let mut s = watched.lock().unwrap();
                    s.building = false;
                    s.error = Some(format!("E_WATCH: {e}"));
                    continue;
                }
            };
            if current == committed {
                if pending.take().is_some() {
                    watched.lock().unwrap().building = false;
                }
                continue;
            }
            if pending.as_ref().is_none_or(|(p, _)| p != &current) {
                pending = Some((current.clone(), Instant::now()));
                watched.lock().unwrap().building = true;
                continue;
            }
            if pending.as_ref().unwrap().1.elapsed() < Duration::from_millis(500) {
                continue;
            }
            let candidate = root.join(".nir/preview-candidate");
            let result = build_profile(
                &root,
                &sdk,
                &candidate,
                "dev",
                true,
                true,
                &OptimizeOptions::default(),
            );
            // Edits during compilation require another stable build before promotion.
            if fingerprint(&root).ok().as_ref() != Some(&current) {
                pending = None;
                continue;
            }
            let result = result.and_then(|report| {
                publish(&candidate, &out, &report.release)?;
                Ok(report.release)
            });
            committed = current;
            pending = None;
            let mut state = watched.lock().unwrap();
            state.building = false;
            match result {
                Ok(release) => {
                    println!("Preview ready: {release}");
                    state.release = release;
                    state.error = None;
                }
                Err(error) => {
                    let d = diagnostic(&error);
                    let relative = d
                        .to_string()
                        .replace(&root.to_string_lossy().to_string(), ".");
                    eprintln!("Preview kept: {relative}");
                    state.error = Some(relative);
                }
            }
        }
    });
    println!("Watching author files; valid changes reload the preview from its entry. Ctrl-C stops preview.");
    serve_inner(&served, "127.0.0.1", port, Some(status))
}

pub fn serve(directory: &Path, host: &str, port: u16) -> Result<()> {
    serve_inner(directory, host, port, None)
}
const PREVIEW_CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self'; style-src-elem 'self'; style-src-attr 'unsafe-inline'; img-src 'self' blob:; media-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

fn is_loopback_bind(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

fn valid_loopback_host(header: &str) -> bool {
    let trimmed = header.trim();
    if let Some(rest) = trimmed.strip_prefix("[::1]") {
        return rest.is_empty()
            || rest
                .strip_prefix(':')
                .is_some_and(|p| p.parse::<u16>().is_ok());
    }
    let host = trimmed.split(':').next().unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost")
        && trimmed
            .split_once(':')
            .is_none_or(|(_, p)| p.parse::<u16>().is_ok())
}

fn serve_inner(
    directory: &Path,
    host: &str,
    port: u16,
    dev: Option<Arc<Mutex<DevStatus>>>,
) -> Result<()> {
    let root = Arc::new(fs::canonicalize(directory).context("E_SERVE: directory missing")?);
    let server = Arc::new(
        tiny_http::Server::http((host, port)).map_err(|e| anyhow::anyhow!("E_LISTEN: {e}"))?,
    );
    let loopback_only = is_loopback_bind(host);
    println!(
        "NIR preview: http://{host}:{port}/\nServing {}",
        root.display()
    );
    let workers = thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(2, 8);
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let server = server.clone();
        let root = root.clone();
        let dev = dev.clone();
        handles.push(thread::spawn(move || {
            for request in server.incoming_requests() {
                if let Err(err) =
                    serve_request_with_policy(request, &root, dev.as_ref(), loopback_only)
                {
                    eprintln!("NIR preview request error: {err}");
                }
            }
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}

// An explicit gzip quality overrides a wildcard, including gzip;q=0.
fn accepts_gzip(value: &str) -> bool {
    let mut gzip = None::<f32>;
    let mut wildcard = None::<f32>;
    for entry in value.split(',') {
        let mut parts = entry.split(';');
        let coding = parts.next().unwrap_or("").trim();
        let mut quality = 1.0_f32;
        for parameter in parts {
            if let Some((name, value)) = parameter.trim().split_once('=') {
                if name.trim().eq_ignore_ascii_case("q") {
                    quality = value
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|q| q.is_finite() && (0.0..=1.0).contains(q))
                        .unwrap_or(0.0);
                }
            }
        }
        let target = if coding.eq_ignore_ascii_case("gzip") {
            &mut gzip
        } else if coding == "*" {
            &mut wildcard
        } else {
            continue;
        };
        *target = Some(target.unwrap_or(0.0).max(quality));
    }
    gzip.or(wildcard).unwrap_or(0.0) > 0.0
}

#[cfg(test)]
fn serve_request(
    request: tiny_http::Request,
    root: &Path,
    dev: Option<&Arc<Mutex<DevStatus>>>,
) -> Result<()> {
    serve_request_with_policy(request, root, dev, true)
}

fn serve_request_with_policy(
    request: tiny_http::Request,
    root: &Path,
    dev: Option<&Arc<Mutex<DevStatus>>>,
    loopback_only: bool,
) -> Result<()> {
    // Reject DNS-rebinding Host headers when serving on loopback.
    if loopback_only {
        let host_ok = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Host"))
            .is_none_or(|h| valid_loopback_host(h.value.as_str()));
        if !host_ok {
            let r = tiny_http::Response::from_string("Forbidden host")
                .with_status_code(403)
                .with_header(
                    tiny_http::Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap(),
                )
                .with_header(
                    tiny_http::Header::from_bytes("Content-Security-Policy", PREVIEW_CSP).unwrap(),
                );
            let _ = request.respond(r);
            return Ok(());
        }
    }
    let is_head = match request.method() {
        tiny_http::Method::Get => false,
        tiny_http::Method::Head => true,
        _ => {
            let r = tiny_http::Response::from_string("Method not allowed")
                .with_status_code(405)
                .with_header(tiny_http::Header::from_bytes("Allow", "GET, HEAD").unwrap())
                .with_header(
                    tiny_http::Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap(),
                );
            let _ = request.respond(r);
            return Ok(());
        }
    };
    // Windows canonicalize yields \\?\ prefixed paths; comparisons against a
    // plain root would otherwise reject every served file.
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let root = canonical_root.as_path();
    let raw = request.url().split('?').next().unwrap_or("/").to_owned();
    let timing = serve_timing();
    let received = Instant::now();
    let timing_id = timing.then(|| SERVE_REQUEST_ID.fetch_add(1, Ordering::Relaxed) + 1);
    if let Some(id) = timing_id {
        log_serve_timing(id, "received", &raw, received.elapsed(), 0);
    }
    if let Some(dev) = &dev {
        if raw == "/__nir_dev/status" || raw == "/__nir_dev/client.js" {
            let (body, mime) = if raw.ends_with("status") {
                (
                    dev.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .json()
                        .to_string(),
                    "application/json",
                )
            } else {
                (
                    include_str!("preview-client.js").into(),
                    "text/javascript; charset=utf-8",
                )
            };
            let len = body.len();
            let mut r = if is_head {
                tiny_http::Response::empty(200)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Length", len.to_string()).unwrap(),
                    )
                    .boxed()
            } else {
                tiny_http::Response::from_string(body).boxed()
            };
            for (k, v) in [
                ("Content-Type", mime),
                ("Cache-Control", "no-store"),
                ("X-Content-Type-Options", "nosniff"),
                ("Content-Security-Policy", PREVIEW_CSP),
            ] {
                r.add_header(tiny_http::Header::from_bytes(k, v).unwrap());
            }
            let _ = request.respond(r);
            return Ok(());
        }
    }
    let path = if raw == "/" {
        "index.html"
    } else {
        raw.trim_start_matches('/')
    };
    let full = if raw.ends_with('/') && raw != "/" {
        root.join(path).join("index.html")
    } else {
        root.join(path)
    };
    let safe = !path.contains('%')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.bytes().any(|b| b < 0x20 || b == 0x7f)
        && !path.split('/').any(|p| p == "..");
    let resolved = if safe {
        fs::canonicalize(&full).ok()
    } else {
        None
    };
    if let Some(file) = resolved.filter(|p| p.starts_with(root) && p.is_file()) {
        let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("");
        let mime = match ext {
            "html" => "text/html; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "json" => "application/json",
            "txt" => "text/plain; charset=utf-8",
            "wasm" => "application/wasm",
            "png" => "image/png",
            "webp" => "image/webp",
            "wav" => "audio/wav",
            "mp3" => "audio/mpeg",
            "otf" => "font/otf",
            "ttf" => "font/ttf",
            _ => "application/octet-stream",
        };
        let cache = if path
            .split('/')
            .any(|part| matches!(part, "objects" | "releases"))
        {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        let negotiable =
            path.split('/').any(|part| part == "objects") && matches!(ext, "wasm" | "js" | "json");
        let accepted = request
            .headers()
            .iter()
            .filter(|header| header.field.equiv("Accept-Encoding"))
            .map(|header| header.value.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let gzip = negotiable
            .then(|| {
                full.with_file_name(format!(
                    "{}.gz",
                    full.file_name().unwrap().to_string_lossy()
                ))
            })
            .filter(|_| accepts_gzip(&accepted))
            .and_then(|sidecar| fs::canonicalize(sidecar).ok())
            .filter(|sidecar| sidecar.starts_with(root) && sidecar.is_file());
        let source = gzip.as_ref().unwrap_or(&file);
        let meta_len = match fs::metadata(source) {
            Ok(m) => m.len(),
            Err(_) => {
                let r = tiny_http::Response::from_string("Not found")
                    .with_status_code(404)
                    .with_header(
                        tiny_http::Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap(),
                    );
                let _ = request.respond(r);
                return Ok(());
            }
        };
        let bytes = if timing { meta_len } else { 0 };
        let mut r = if is_head {
            tiny_http::Response::empty(200)
                .with_header(
                    tiny_http::Header::from_bytes("Content-Length", meta_len.to_string()).unwrap(),
                )
                .boxed()
        } else {
            match fs::File::open(source) {
                Ok(opened) => tiny_http::Response::from_file(opened).boxed(),
                Err(_) => {
                    let r = tiny_http::Response::from_string("Not found")
                        .with_status_code(404)
                        .with_header(
                            tiny_http::Header::from_bytes("X-Content-Type-Options", "nosniff")
                                .unwrap(),
                        );
                    let _ = request.respond(r);
                    return Ok(());
                }
            }
        };
        for (k, v) in [
            ("Content-Type", mime),
            ("Cache-Control", cache),
            ("X-Content-Type-Options", "nosniff"),
            ("Content-Security-Policy", PREVIEW_CSP),
        ] {
            r.add_header(tiny_http::Header::from_bytes(k, v).unwrap());
        }
        if let Some(id) = timing_id {
            r.add_header(tiny_http::Header::from_bytes("X-NIR-Serve-Id", id.to_string()).unwrap());
        }
        if negotiable {
            r.add_header(tiny_http::Header::from_bytes("Vary", "Accept-Encoding").unwrap());
        }
        if gzip.is_some() {
            r.add_header(tiny_http::Header::from_bytes("Content-Encoding", "gzip").unwrap());
        }
        if let Some(id) = timing_id {
            log_serve_timing(id, "ready", &raw, received.elapsed(), bytes);
        }
        let result = request.respond(r);
        if let Some(id) = timing_id {
            log_serve_timing(
                id,
                if result.is_ok() { "done" } else { "send_error" },
                &raw,
                received.elapsed(),
                bytes,
            );
        }
        let _ = result;
    } else {
        let result = request.respond(
            tiny_http::Response::from_string("Not found")
                .with_status_code(404)
                .with_header(
                    tiny_http::Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap(),
                )
                .with_header(
                    tiny_http::Header::from_bytes("Content-Security-Policy", PREVIEW_CSP).unwrap(),
                ),
        );
        if let Some(id) = timing_id {
            log_serve_timing(id, "not_found", &raw, received.elapsed(), 0);
        }
        let _ = result;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpStream};

    fn get(root: &Path, path: &str, encoding: Option<&str>) -> (String, Vec<u8>) {
        get_with_dev(root, path, encoding, None)
    }

    fn get_with_dev(
        root: &Path,
        path: &str,
        encoding: Option<&str>,
        dev: Option<Arc<Mutex<DevStatus>>>,
    ) -> (String, Vec<u8>) {
        let server = tiny_http::Server::http(("127.0.0.1", 0)).unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let root = root.to_path_buf();
        let worker = thread::spawn(move || {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            serve_request(request, &root, dev.as_ref()).unwrap();
        });
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let encoding = encoding
            .map(|v| format!("Accept-Encoding: {v}\r\n"))
            .unwrap_or_default();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{encoding}\r\n"
        )
        .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        worker.join().unwrap();
        let split = response
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        (
            String::from_utf8(response[..split].to_vec())
                .unwrap()
                .to_lowercase(),
            response[split + 4..].to_vec(),
        )
    }

    #[test]
    fn dev_serves_fixed_entry_bytes_without_injection() {
        let t = Temp::new();
        let digest = "a".repeat(64);
        let fixed = t.0.join(format!("releases/{digest}"));
        fs::create_dir_all(&fixed).unwrap();
        let html = b"<!doctype html><body>hashed launch</body>";
        fs::write(fixed.join("index.html"), html).unwrap();
        let status = Arc::new(Mutex::new(DevStatus {
            release: digest.clone(),
            ..Default::default()
        }));
        let (headers, bytes) = get_with_dev(
            &t.0,
            &format!("/releases/{digest}/index.html"),
            None,
            Some(status),
        );
        assert_eq!(bytes, html);
        assert!(headers.contains("cache-control: public, max-age=31536000, immutable"));
        assert!(!String::from_utf8(bytes).unwrap().contains("__nir_dev"));
    }

    #[test]
    fn gzip_negotiation_preserves_mime_cache_length_and_identity() {
        let t = Temp::new();
        fs::create_dir(t.0.join("objects")).unwrap();
        let original = b"sample module content ".repeat(100);
        let mut encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::new(6));
        encoder.write_all(&original).unwrap();
        let gzip = encoder.finish().unwrap();
        fs::write(t.0.join("objects/test.json"), &original).unwrap();
        fs::write(t.0.join("objects/test.json.gz"), &gzip).unwrap();
        for encoding in ["gzip", "br, gzip;q=0.5", "*", "GZIP;Q=1"] {
            let (headers, body) = get(&t.0, "/objects/test.json", Some(encoding));
            assert!(headers.contains("content-encoding: gzip"));
            assert!(headers.contains("content-type: application/json"));
            assert!(headers.contains("vary: accept-encoding"));
            assert!(headers.contains("cache-control: public, max-age=31536000, immutable"));
            assert!(headers.contains(&format!("content-length: {}", gzip.len())));
            assert_eq!(body, gzip);
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(body.as_slice())
                .read_to_end(&mut decoded)
                .unwrap();
            assert_eq!(decoded, original);
        }
        for encoding in [
            None,
            Some("br"),
            Some("gzip;q=0, *;q=1"),
            Some("gzip;q=NaN"),
            Some("gzip;q=2"),
        ] {
            let (headers, body) = get(&t.0, "/objects/test.json", encoding);
            assert!(!headers.contains("content-encoding:"));
            assert!(headers.contains("vary: accept-encoding"));
            assert_eq!(body, original);
        }
        fs::remove_file(t.0.join("objects/test.json.gz")).unwrap();
        assert_eq!(get(&t.0, "/objects/test.json", Some("gzip")).1, original);
        for path in [
            "/../objects/test.json",
            "/%2e%2e/objects/test.json",
            "/objects/missing.json",
        ] {
            assert!(get(&t.0, path, Some("gzip")).0.starts_with("http/1.1 404"));
        }
        #[cfg(unix)]
        {
            let outside = Temp::new();
            fs::write(outside.0.join("secret"), "outside").unwrap();
            std::os::unix::fs::symlink(outside.0.join("secret"), t.0.join("objects/test.json.gz"))
                .unwrap();
            assert_eq!(get(&t.0, "/objects/test.json", Some("gzip")).1, original);
        }
    }
    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("nir-preview-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn watcher_detects_edits_deletions_and_ignores_its_output() {
        let t = Temp::new();
        fs::write(t.0.join("story.json"), "old").unwrap();
        let a = fingerprint(&t.0).unwrap();
        for dir in ["dist", "reports", ".nir"] {
            fs::create_dir(t.0.join(dir)).unwrap();
            fs::write(t.0.join(dir).join("generated"), "new").unwrap();
        }
        assert_eq!(a, fingerprint(&t.0).unwrap());
        let journal = t.0.join(".nir/text-transaction.json");
        fs::write(&journal, "[]").unwrap();
        assert_ne!(a, fingerprint(&t.0).unwrap());
        fs::remove_file(journal).unwrap();
        assert_eq!(a, fingerprint(&t.0).unwrap());
        fs::write(t.0.join("story.json"), "new and longer").unwrap();
        let b = fingerprint(&t.0).unwrap();
        assert_ne!(a, b);
        fs::remove_file(t.0.join("story.json")).unwrap();
        assert_ne!(b, fingerprint(&t.0).unwrap());
    }
    #[test]
    fn preview_promotion_rejects_corruption_before_changing_channel() {
        let t = Temp::new();
        let project = t.0.join("project");
        nir_compiler::copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rain-letters"),
            &project,
        )
        .unwrap();
        let sdk = t.0.join("sdk");
        fs::create_dir(&sdk).unwrap();
        for f in [
            "player_web.js",
            "player_web_bg.wasm",
            "host.js",
            "runtime-worker.js",
            "asset-worker.js",
            "index.html",
            "bootstrap.js",
            "style.css",
            "THIRD-PARTY.txt",
            "compiler.sha256",
        ] {
            fs::write(sdk.join(f), format!("fixture {f}")).unwrap();
        }
        nir_compiler::resolve(&project, &sdk).unwrap();
        let candidate = t.0.join("candidate");
        let live = t.0.join("live");
        let report = build_profile(
            &project,
            &sdk,
            &candidate,
            "dev",
            true,
            true,
            &OptimizeOptions::default(),
        )
        .unwrap();
        publish(&candidate, &live, &report.release).unwrap();
        let old = fs::read(live.join("channels/stable.json")).unwrap();
        let r: ReleaseManifest = serde_json::from_slice(
            &fs::read(candidate.join(format!("releases/{}.json", report.release))).unwrap(),
        )
        .unwrap();
        let compressed = r
            .objects
            .values()
            .find_map(|o| {
                let p = candidate.join(format!("{}.gz", o.path));
                p.exists().then_some(p)
            })
            .unwrap();
        let original_gzip = fs::read(&compressed).unwrap();
        fs::write(&compressed, b"corrupt gzip").unwrap();
        assert!(publish(&candidate, &live, &report.release).is_err());
        assert_eq!(fs::read(live.join("channels/stable.json")).unwrap(), old);
        fs::write(compressed, original_gzip).unwrap();
        fs::write(
            candidate.join(&r.objects.values().next().unwrap().path),
            "corrupt",
        )
        .unwrap();
        assert!(publish(&candidate, &live, &report.release).is_err());
        assert_eq!(fs::read(live.join("channels/stable.json")).unwrap(), old);
    }
}

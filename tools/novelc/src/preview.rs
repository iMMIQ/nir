use anyhow::{Context, Result};
use nir_compiler::{build, diagnostic};
use nir_format::ReleaseManifest;
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

#[derive(Default)]
struct DevStatus {
    release: String,
    building: bool,
    error: Option<String>,
}
impl DevStatus {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"format":1,"release":self.release,"building":self.building,"stale":self.building || self.error.is_some(),"error":self.error})
    }
}
// Poll metadata rather than decoding or hashing every large media file on every turn.
// An editor's atomic rename, deletion or new file changes the sorted signature too.
fn fingerprint(root: &Path) -> Result<String> {
    fn walk(root: &Path, dir: &Path, out: &mut String) -> Result<()> {
        let mut files: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
        files.sort_by_key(|f| f.file_name());
        for file in files {
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
                walk(root, &file.path(), out)?;
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
    walk(root, root, &mut text)?;
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
        let temporary = to.with_extension("next");
        fs::copy(from, &temporary)?;
        fs::rename(temporary, to)?;
        Ok(())
    }
    let release_path = format!("releases/{release}.json");
    let bytes = fs::read(candidate.join(&release_path))?;
    nir_content::verify(&bytes, release)?;
    let manifest: ReleaseManifest = nir_content::parse(&bytes, "preview release")?;
    nir_content::validate_release(&manifest)?;
    for (hash, object) in &manifest.objects {
        let source = candidate.join(&object.path);
        nir_content::verify(&fs::read(&source)?, hash)?;
        copy_atomic(&source, &out.join(&object.path))?;
    }
    copy_atomic(&candidate.join(&release_path), &out.join(&release_path))?;
    for name in ["index.html", "bootstrap.js", "NOTICE.txt"] {
        copy_atomic(&candidate.join(name), &out.join(name))?;
    }
    // The only mutable runtime pointer is replaced after the complete object closure exists.
    copy_atomic(
        &candidate.join("channels/stable.json"),
        &out.join("channels/stable.json"),
    )?;
    Ok(())
}
pub fn dev(root: &Path, sdk: &Path, port: u16) -> Result<()> {
    let root = fs::canonicalize(root)?;
    let sdk = fs::canonicalize(sdk)?;
    let out = root.join("dist/full/web");
    // An edit during the initial build must still be noticed by the first watch turn.
    let initial_fingerprint = fingerprint(&root)?;
    let initial = build(&root, &sdk, &out, true)?;
    let status = Arc::new(Mutex::new(DevStatus {
        release: initial.release,
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
            let result = build(&root, &sdk, &candidate, true);
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
    serve_inner(&served, port, Some(status))
}

pub fn serve(directory: &Path, port: u16) -> Result<()> {
    serve_inner(directory, port, None)
}
fn serve_inner(directory: &Path, port: u16, dev: Option<Arc<Mutex<DevStatus>>>) -> Result<()> {
    let root = fs::canonicalize(directory).context("E_SERVE: directory missing")?;
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("E_LISTEN: {e}"))?;
    println!(
        "NIR preview: http://127.0.0.1:{port}/\nServing {}",
        root.display()
    );
    for request in server.incoming_requests() {
        let raw = request.url().split('?').next().unwrap_or("/");
        if let Some(dev) = &dev {
            if raw == "/__nir_dev/status" || raw == "/__nir_dev/client.js" {
                let (body, mime) = if raw.ends_with("status") {
                    (dev.lock().unwrap().json().to_string(), "application/json")
                } else {
                    (
                        include_str!("preview-client.js").into(),
                        "text/javascript; charset=utf-8",
                    )
                };
                let r = tiny_http::Response::from_string(body)
                    .with_header(tiny_http::Header::from_bytes("Content-Type", mime).unwrap())
                    .with_header(
                        tiny_http::Header::from_bytes("Cache-Control", "no-store").unwrap(),
                    );
                let _ = request.respond(r);
                continue;
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
        let safe =
            !path.contains('%') && !path.contains('\\') && !path.split('/').any(|p| p == "..");
        let resolved = if safe {
            fs::canonicalize(&full).ok()
        } else {
            None
        };
        if let Some(file) = resolved.filter(|p| p.starts_with(&root) && p.is_file()) {
            let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("");
            let mime = match ext {
                "html" => "text/html; charset=utf-8",
                "js" => "text/javascript; charset=utf-8",
                "json" => "application/json",
                "txt" => "text/plain; charset=utf-8",
                "wasm" => "application/wasm",
                "png" => "image/png",
                "wav" => "audio/wav",
                "otf" => "font/otf",
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
            let mut r = if let Some(dev) = dev.as_ref().filter(|_| ext == "html") {
                let release = dev.lock().unwrap().release.clone();
                let html = fs::read_to_string(&file)?.replace("</body>", &format!("<script src=\"/__nir_dev/client.js\" data-release=\"{release}\"></script></body>"));
                tiny_http::Response::from_string(html).boxed()
            } else {
                tiny_http::Response::from_file(fs::File::open(file)?).boxed()
            };
            for (k,v) in [("Content-Type",mime),("Cache-Control",cache),("X-Content-Type-Options","nosniff"),("Content-Security-Policy","default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; media-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'")]{r.add_header(tiny_http::Header::from_bytes(k,v).unwrap());}
            let _ = request.respond(r);
        } else {
            let _ = request
                .respond(tiny_http::Response::from_string("Not found").with_status_code(404));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            "index.html",
            "bootstrap.js",
            "THIRD-PARTY.txt",
            "compiler.sha256",
        ] {
            fs::write(sdk.join(f), format!("fixture {f}")).unwrap();
        }
        nir_compiler::resolve(&project, &sdk).unwrap();
        let candidate = t.0.join("candidate");
        let live = t.0.join("live");
        let report = build(&project, &sdk, &candidate, true).unwrap();
        publish(&candidate, &live, &report.release).unwrap();
        let old = fs::read(live.join("channels/stable.json")).unwrap();
        let r: ReleaseManifest = serde_json::from_slice(
            &fs::read(candidate.join(format!("releases/{}.json", report.release))).unwrap(),
        )
        .unwrap();
        fs::write(
            candidate.join(&r.objects.values().next().unwrap().path),
            "corrupt",
        )
        .unwrap();
        assert!(publish(&candidate, &live, &report.release).is_err());
        assert_eq!(fs::read(live.join("channels/stable.json")).unwrap(), old);
    }
}

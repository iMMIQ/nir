use anyhow::{bail, Context, Result};
use nir_format::{ContentKey, ReleaseManifest, RuntimeExecutable, RUNTIME_FORMAT_VERSION};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn release_bytes(root: &Path, digest: &str) -> Result<(Vec<u8>, ReleaseManifest)> {
    if !hash(digest) {
        bail!("E_RELEASE_DIGEST: invalid release identity");
    }
    let bytes = fs::read(root.join(format!("releases/{digest}.json")))?;
    nir_content::verify(&bytes, digest)?;
    let manifest: ReleaseManifest = nir_content::parse(&bytes, "release")?;
    nir_content::validate_release(&manifest)?;
    Ok((bytes, manifest))
}
fn channel(root: &Path) -> Result<Option<String>> {
    let path = root.join("channels/stable.json");
    if !path.exists() {
        return Ok(None);
    }
    let v: serde_json::Value = nir_content::parse(&fs::read(path)?, "channel")?;
    if v.as_object().is_none_or(|o| o.len() != 2)
        || v["format"] != 1
        || v["release"].as_str().is_none_or(|s| !hash(s))
    {
        bail!("E_CHANNEL_SCHEMA: invalid current pointer");
    }
    Ok(Some(v["release"].as_str().unwrap().to_owned()))
}
enum Reader {
    Local(PathBuf),
    Remote(reqwest::Url, reqwest::blocking::Client),
}
impl Reader {
    fn new(location: &str) -> Result<Self> {
        if location.starts_with("http://") || location.starts_with("https://") {
            let base = reqwest::Url::parse(&format!("{}/", location.trim_end_matches('/')))?;
            let client = reqwest::blocking::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(30))
                .build()?;
            Ok(Self::Remote(base, client))
        } else {
            Ok(Self::Local(fs::canonicalize(location)?))
        }
    }
    fn get(&self, path: &str, mime: &str, immutable: bool) -> Result<Vec<u8>> {
        if path.starts_with('/')
            || path.contains('\\')
            || path.contains(':')
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            bail!("E_VERIFY_PATH: {path}");
        }
        match self {
            Self::Local(base) => {
                let resolved = fs::canonicalize(base.join(path))?;
                if !resolved.starts_with(base) {
                    bail!("E_VERIFY_PATH: {path}");
                }
                Ok(fs::read(resolved)?)
            }
            Self::Remote(base, client) => {
                let url = base.join(path)?;
                if !url.as_str().starts_with(base.as_str()) {
                    bail!("E_VERIFY_URL: {path}");
                }
                let response = client
                    .get(url)
                    .header("Accept-Encoding", "identity")
                    .header(reqwest::header::CACHE_CONTROL, "no-cache")
                    .send()?;
                if response.status() != reqwest::StatusCode::OK {
                    bail!("E_VERIFY_HTTP: {} {path}", response.status());
                }
                let actual = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim();
                if actual != mime {
                    bail!("E_VERIFY_MIME: {path}: {actual} != {mime}");
                }
                let cache = response
                    .headers()
                    .get(reqwest::header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if immutable && !(cache.contains("immutable") && cache.contains("max-age")) {
                    bail!("E_VERIFY_CACHE: immutable policy missing for {path}");
                }
                if !immutable && !(cache.contains("no-cache") || cache.contains("no-store")) {
                    bail!("E_VERIFY_CACHE: mutable policy missing for {path}");
                }
                Ok(response.bytes()?.to_vec())
            }
        }
    }
    fn missing(&self, path: &str) -> Result<()> {
        if let Self::Remote(base, client) = self {
            let response = client.get(base.join(path)?).send()?;
            if response.status() != reqwest::StatusCode::NOT_FOUND {
                bail!("E_VERIFY_MISSING: {path} returned {}", response.status());
            }
        }
        Ok(())
    }
}
fn mime_for(path: &str) -> Result<&'static str> {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => Ok("text/html"),
        "js" => Ok("text/javascript"),
        "json" => Ok("application/json"),
        "wasm" => Ok("application/wasm"),
        "png" => Ok("image/png"),
        "wav" => Ok("audio/wav"),
        "otf" => Ok("font/otf"),
        "ttf" => Ok("font/ttf"),
        "txt" => Ok("text/plain"),
        _ => bail!("E_VERIFY_MIME: unknown extension in {path}"),
    }
}
pub fn verify(location: &str, requested: Option<&str>) -> Result<serde_json::Value> {
    let reader = Reader::new(location)?;
    let digest = if let Some(id) = requested {
        id.to_owned()
    } else {
        let bytes = reader.get("channels/stable.json", "application/json", false)?;
        let pointer: serde_json::Value = nir_content::parse(&bytes, "channel")?;
        if pointer.as_object().is_none_or(|o| o.len() != 2) || pointer["format"] != 1 {
            bail!("E_CHANNEL_SCHEMA");
        }
        pointer["release"]
            .as_str()
            .context("E_CHANNEL_SCHEMA")?
            .to_owned()
    };
    if !hash(&digest) {
        bail!("E_RELEASE_DIGEST: invalid release identity");
    }
    let raw = reader.get(&format!("releases/{digest}.json"), "application/json", true)?;
    nir_content::verify(&raw, &digest)?;
    let manifest: ReleaseManifest = nir_content::parse(&raw, "release")?;
    nir_content::validate_release(&manifest)?;
    for (id, object) in &manifest.objects {
        if object.path
            != format!(
                "objects/{id}.{}",
                object.path.rsplit('.').next().unwrap_or("")
            )
        {
            bail!("E_OBJECT_PATH: {}", object.path);
        }
        let mime = mime_for(&object.path)?;
        if object.media_type.split(';').next().unwrap_or("") != mime {
            bail!("E_OBJECT_MIME: {}", object.path);
        }
        let bytes = reader.get(&object.path, mime, true)?;
        if bytes.len() as u64 != object.bytes {
            bail!("E_OBJECT_SIZE: {id}");
        }
        nir_content::verify(&bytes, id)?;
        if let Reader::Local(base) = &reader {
            let sidecar = base.join(format!("{}.gz", object.path));
            if sidecar.exists() {
                use std::io::Read;
                let mut decoded = Vec::new();
                flate2::read::MultiGzDecoder::new(fs::File::open(sidecar)?)
                    .take(object.bytes.saturating_add(1))
                    .read_to_end(&mut decoded)?;
                if decoded != bytes {
                    bail!("E_GZIP_DIGEST: {id}");
                }
            }
        }
    }
    for (name, id) in [
        ("index.html", &manifest.launch.html),
        ("bootstrap.js", &manifest.launch.bootstrap),
    ] {
        let bytes = reader.get(&format!("releases/{digest}/{name}"), mime_for(name)?, true)?;
        nir_content::verify(&bytes, id)?;
        if bytes != reader.get(&manifest.objects[id].path, mime_for(name)?, true)? {
            bail!("E_LAUNCH_BYTES: {name}");
        }
        if requested.is_none() {
            nir_content::verify(&reader.get(name, mime_for(name)?, false)?, id)?;
        }
    }
    let wasm = reader.get(
        &manifest.objects[&manifest.engine.wasm].path,
        "application/wasm",
        true,
    )?;
    if !wasm.starts_with(b"\0asm\x01\0\0\0") {
        bail!("E_WASM_MAGIC");
    }
    let program_bytes = reader.get(
        &manifest.objects[&manifest.program].path,
        "application/json",
        true,
    )?;
    let runtime: RuntimeExecutable = nir_content::parse(&program_bytes, "runtime root")?;
    if runtime.format != RUNTIME_FORMAT_VERSION || runtime.program.format != RUNTIME_FORMAT_VERSION
    {
        bail!("E_RUNTIME_VERSION");
    }
    let mut keys = Vec::new();
    for (module, index) in &runtime.program.modules {
        keys.push(ContentKey::Static {
            module: module.clone(),
        });
        keys.push(ContentKey::Code {
            module: module.clone(),
        });
        keys.extend(index.locales.keys().map(|locale| ContentKey::Text {
            module: module.clone(),
            locale: locale.clone(),
        }));
    }
    keys.extend(
        runtime
            .program
            .catalogs
            .keys()
            .map(|catalog| ContentKey::Catalog {
                catalog: catalog.clone(),
            }),
    );
    for key in keys {
        let requirement = runtime
            .program
            .content_requirement(&key)
            .context("E_RUNTIME_INDEX")?;
        let object = manifest
            .objects
            .get(&requirement.digest)
            .context("E_RUNTIME_OBJECT")?;
        let bytes = reader.get(&object.path, "application/json", true)?;
        nir_content::parse_runtime_object(&runtime.program, &key, &bytes)?;
    }
    let absent = if digest == "0".repeat(64) {
        "f".repeat(64)
    } else {
        "0".repeat(64)
    };
    reader.missing(&format!("releases/{absent}.json"))?;
    reader.missing(&format!("objects/{}.json", "0".repeat(64)))?;
    if let Reader::Local(base) = &reader {
        if requested.is_none() {
            let notice = reader.get("NOTICE.txt", "text/plain", false)?;
            if manifest.notices.len() != 1 || nir_content::digest(&notice) != manifest.notices[0] {
                bail!("E_NOTICE");
            }
        }
        for forbidden in [
            "game.toml",
            "game.lock",
            "content",
            "tests",
            "assets/source",
        ] {
            if base.join(forbidden).exists() {
                bail!("E_PUBLIC_SOURCE: {forbidden}");
            }
        }
    }
    Ok(serde_json::json!({
        "format": 1,
        "location": location,
        "release": digest,
        "profile": manifest.profile,
        "objects": manifest.objects.len(),
        "status": "PASS"
    }))
}
fn copy_immutable(source: &Path, target: &Path, digest: &str) -> Result<()> {
    let bytes =
        fs::read(source).with_context(|| format!("E_STAGE_MISSING: {}", source.display()))?;
    nir_content::verify(&bytes, digest)?;
    if target.exists() {
        nir_content::verify(&fs::read(target)?, digest)?;
    } else {
        fs::create_dir_all(target.parent().unwrap())?;
        let tmp = target.with_extension(format!("stage-{}", uuid::Uuid::new_v4()));
        fs::write(&tmp, bytes)?;
        let linked = fs::hard_link(&tmp, target);
        fs::remove_file(tmp)?;
        match linked {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                nir_content::verify(&fs::read(target)?, digest)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn copy_sidecar(source: &Path, target: &Path, digest: &str, expected: u64) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    let encoded = fs::read(source)?;
    let mut decoded = Vec::new();
    use std::io::Read;
    flate2::read::MultiGzDecoder::new(encoded.as_slice())
        .take(expected.saturating_add(1))
        .read_to_end(&mut decoded)?;
    if decoded.len() as u64 != expected {
        bail!("E_GZIP_SIZE");
    }
    nir_content::verify(&decoded, digest)?;
    fs::create_dir_all(target.parent().unwrap())?;
    let tmp = target.with_extension("stage-next");
    fs::write(&tmp, encoded)?;
    fs::rename(tmp, target)?;
    Ok(())
}
pub fn stage(source: &Path, directory: &Path, requested: Option<&str>) -> Result<String> {
    let source = fs::canonicalize(source)?;
    let digest = match requested {
        Some(id) => id.to_owned(),
        None => channel(&source)?.context("E_CHANNEL_MISSING: specify --release")?,
    };
    verify(source.to_str().context("E_PATH_UTF8")?, Some(&digest))?;
    let (bytes, manifest) = release_bytes(&source, &digest)?;
    if manifest.profile != "release" {
        bail!("E_RELEASE_PROFILE: staging requires a release build");
    }
    fs::create_dir_all(directory)?;
    let directory = fs::canonicalize(directory)?;
    let _guard = StageLock::acquire(&directory)?;
    for (id, object) in &manifest.objects {
        copy_immutable(
            &source.join(&object.path),
            &directory.join(&object.path),
            id,
        )?;
        let sidecar = format!("{}.gz", object.path);
        copy_sidecar(
            &source.join(&sidecar),
            &directory.join(&sidecar),
            id,
            object.bytes,
        )?;
    }
    for (name, id) in [
        ("index.html", &manifest.launch.html),
        ("bootstrap.js", &manifest.launch.bootstrap),
    ] {
        copy_immutable(
            &source.join(format!("releases/{digest}/{name}")),
            &directory.join(format!("releases/{digest}/{name}")),
            id,
        )?;
    }
    let release_path = directory.join(format!("releases/{digest}.json"));
    // The source manifest was authenticated before any import; install it last.
    let staged_manifest = directory.join(format!(".manifest-{digest}"));
    fs::write(&staged_manifest, bytes)?;
    copy_immutable(&staged_manifest, &release_path, &digest)?;
    fs::remove_file(staged_manifest)?;
    verify(directory.to_str().context("E_PATH_UTF8")?, Some(&digest))?;
    Ok(digest)
}
struct StageLock(PathBuf);
impl StageLock {
    fn acquire(root: &Path) -> Result<Self> {
        let path = root.join(".stage.lock");
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .context("E_STAGE_BUSY: another import is in progress")?;
        Ok(Self(path))
    }
}
impl Drop for StageLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct PointerLock(PathBuf);
impl PointerLock {
    fn acquire(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("channels"))?;
        let path = root.join("channels/.stable.lock");
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .context("E_CHANNEL_BUSY: another local promotion is in progress")?;
        Ok(Self(path))
    }
}
impl Drop for PointerLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn promote(directory: &Path, target: &str, expected: &str) -> Result<()> {
    if !hash(target) {
        bail!("E_RELEASE_DIGEST: invalid target");
    }
    if expected != "none" && !hash(expected) {
        bail!("E_CHANNEL_EXPECT: expected digest or none");
    }
    let directory = fs::canonicalize(directory)?;
    let _guard = PointerLock::acquire(&directory)?;
    let actual = channel(&directory)?;
    if actual.as_deref().unwrap_or("none") != expected {
        bail!(
            "E_CHANNEL_CAS: current release is {}, expected {expected}",
            actual.as_deref().unwrap_or("none")
        );
    }
    verify(directory.to_str().context("E_PATH_UTF8")?, Some(target))?;
    let (_, manifest) = release_bytes(&directory, target)?;
    if manifest.profile != "release" {
        bail!("E_RELEASE_PROFILE: promotion requires a release build");
    }
    for (name, id) in [
        ("index.html", &manifest.launch.html),
        ("bootstrap.js", &manifest.launch.bootstrap),
    ] {
        let bytes = fs::read(directory.join(&manifest.objects[id].path))?;
        nir_content::verify(&bytes, id)?;
        let tmp = directory.join(format!("{name}.next"));
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, directory.join(name))?;
    }
    let notice = manifest.notices.first().context("E_NOTICE")?;
    let bytes = fs::read(directory.join(&manifest.objects[notice].path))?;
    nir_content::verify(&bytes, notice)?;
    let tmp = directory.join("NOTICE.txt.next");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, directory.join("NOTICE.txt"))?;
    let tmp = directory.join("channels/stable.json.next");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec(
        &serde_json::json!({"format":1,"release":target}),
    )?)?;
    file.sync_all()?;
    fs::rename(tmp, directory.join("channels/stable.json"))?;
    Ok(())
}

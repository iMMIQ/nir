//! Desktop-only host integrations for `player-native` (Linux / Windows / macOS):
//! - Clipboard / Texthooker (`arboard` with Linux `wl-copy`/`xclip`/`xsel` fallback)
//! - Linux battery low-power detector (`/sys/class/power_supply/BAT*`)
//! - Lightweight external video host (`mpv` / `ffplay`) for `.nirpak` embedded movies
//! - Save thumbnail DataURL (`data:image/webp;base64,...`) encoding & decoding

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use nir_render::{decode_image, encode_thumbnail_webp};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Encodes a stage RGBA8 capture as a compact 160x90 WebP DataURL (100% compatible with Web saves!).
pub fn encode_thumb_data_url(rgba: &[u8], width: u32, height: u32) -> Option<String> {
    let webp = encode_thumbnail_webp(rgba, width, height, 160, 90).ok()?;
    let b64 = BASE64.encode(&webp);
    let url = format!("data:image/webp;base64,{b64}");
    (url.len() < 48 * 1024).then_some(url)
}

/// Decodes a `data:image/...;base64,...` save thumbnail into premultiplied RGBA8 for GPU upload.
pub fn decode_thumb_data_url(data_url: &str) -> Option<(u32, u32, Vec<u8>)> {
    let (_, b64) = data_url.split_once(";base64,")?;
    let raw = BASE64.decode(b64.trim()).ok()?;
    decode_image(&raw).ok()
}

/// Sanitizes a game title into a safe cross-platform filename stem for screenshots.
pub fn safe_filename_stem(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_control()
                || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                || c.is_whitespace()
            {
                '_'
            } else {
                c
            }
        })
        .take(40)
        .collect();
    let trimmed = s.trim_matches('_');
    if trimmed.is_empty() {
        "nir-cg".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Detects whether a Linux laptop is currently unplugged on low battery (< 20%) via sysfs (zero deps).
pub fn read_low_battery_hint() -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else {
            return false;
        };
        for entry in rd.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("BAT") {
                continue;
            }
            let dir = entry.path();
            let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
            if !status.trim().eq_ignore_ascii_case("Discharging") {
                continue;
            }
            if let Ok(cap_str) = std::fs::read_to_string(dir.join("capacity")) {
                if let Ok(cap) = cap_str.trim().parse::<u32>() {
                    if cap < 20 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Persistent desktop clipboard wrapper for Texthooker (`KeyC` / `Shift+C`).
/// Keeps `arboard::Clipboard` alive across calls (required on X11) and falls back
/// to `wl-copy` / `xclip` / `xsel` on Linux compositors without `wlr-data-control`.
#[cfg(not(target_os = "android"))]
pub struct DesktopClipboard {
    inner: Option<arboard::Clipboard>,
}

#[cfg(not(target_os = "android"))]
impl DesktopClipboard {
    pub fn new() -> Self {
        Self {
            inner: arboard::Clipboard::new().ok(),
        }
    }

    pub fn copy_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        if let Some(cb) = &mut self.inner {
            if cb.set_text(text.to_owned()).is_ok() {
                return true;
            }
        }
        #[cfg(target_os = "linux")]
        {
            if copy_via_linux_cli(text) {
                return true;
            }
        }
        false
    }
}

#[cfg(not(target_os = "android"))]
impl Default for DesktopClipboard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
fn copy_via_linux_cli(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let candidates: &[(&str, &[&str])] = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    } else {
        &[
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
            ("wl-copy", &[]),
        ]
    };

    for &(bin, args) in candidates {
        let Ok(mut child) = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        // `wl-copy` / `xclip` fork into background to serve selection requests;
        // do not block waiting forever — check immediate spawn/exit cleanly.
        if bin == "wl-copy" {
            return true;
        }
        if let Ok(status) = child.wait() {
            if status.success() {
                return true;
            }
        }
    }
    false
}

/// Lightweight desktop video host spawning `mpv` (preferred) or `ffplay` when available,
/// cleaning up the extracted temporary movie file automatically on finish/stop/drop.
#[cfg(not(target_os = "android"))]
pub struct DesktopVideoPlayer {
    child: Option<std::process::Child>,
    temp_file: Option<PathBuf>,
    skippable: bool,
}

#[cfg(not(target_os = "android"))]
impl DesktopVideoPlayer {
    pub fn new() -> Self {
        Self {
            child: None,
            temp_file: None,
            skippable: true,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.child.is_some()
    }

    pub fn is_skippable(&self) -> bool {
        self.skippable
    }

    /// Attempts to play an embedded `.nirpak` video via `mpv` or `ffplay`.
    /// Returns `Ok(())` when playback started, or `Err(reason)` if no helper player is installed.
    pub fn play(
        &mut self,
        bytes: &[u8],
        ext: &str,
        title: &str,
        skippable: bool,
    ) -> Result<(), String> {
        self.stop();
        if bytes.is_empty() {
            return Err("empty video payload".into());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let safe_ext = if ext.is_empty() { "mp4" } else { ext };
        let path = std::env::temp_dir().join(format!(
            "nir-video-{}-{stamp}.{safe_ext}",
            std::process::id()
        ));
        std::fs::write(&path, bytes).map_err(|e| format!("writing temp video: {e}"))?;

        let mut mpv = std::process::Command::new("mpv");
        mpv.args([
            "--no-terminal",
            "--force-window=yes",
            "--keep-open=no",
            "--osc=no",
            "--ontop",
        ])
        .arg(format!("--title={title}"))
        .arg(&path);
        if !skippable {
            mpv.arg("--input-default-bindings=no");
        }
        if let Ok(child) = mpv.spawn() {
            self.child = Some(child);
            self.temp_file = Some(path);
            self.skippable = skippable;
            return Ok(());
        }

        let mut ffplay = std::process::Command::new("ffplay");
        ffplay
            .args(["-autoexit", "-loglevel", "quiet", "-window_title", title])
            .arg(&path);
        if let Ok(child) = ffplay.spawn() {
            self.child = Some(child);
            self.temp_file = Some(path);
            self.skippable = skippable;
            return Ok(());
        }

        let _ = std::fs::remove_file(&path);
        Err("no system video helper (`mpv` or `ffplay`) found on PATH".into())
    }

    /// Polls whether the active video process has finished. Returns `true` once upon completion.
    pub fn poll_finished(&mut self) -> bool {
        let Some(child) = &mut self.child else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => {
                self.child = None;
                self.cleanup_temp();
                true
            }
            Ok(None) => false,
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.cleanup_temp();
    }

    fn cleanup_temp(&mut self) {
        if let Some(p) = self.temp_file.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(not(target_os = "android"))]
impl Default for DesktopVideoPlayer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_os = "android"))]
impl Drop for DesktopVideoPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_data_url_roundtrips_webp_base64() {
        let mut rgba = vec![0u8; 32 * 18 * 4];
        for px in rgba.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[64, 140, 220, 255]);
        }
        let url = encode_thumb_data_url(&rgba, 32, 18).expect("encode webp thumb data url");
        assert!(url.starts_with("data:image/webp;base64,"));
        let (w, h, decoded) = decode_thumb_data_url(&url).expect("decode thumb data url");
        assert_eq!((w, h), (160, 90));
        assert_eq!(decoded.len(), 160 * 90 * 4);
    }

    #[test]
    fn filename_stem_sanitizes_unsafe_chars() {
        assert_eq!(
            safe_filename_stem("雨后书简: Part 1/2"),
            "雨后书简__Part_1_2"
        );
        assert_eq!(safe_filename_stem("   ///  "), "nir-cg");
    }
}

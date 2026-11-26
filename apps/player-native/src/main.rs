//! Thin Desktop binary entry for NIR v2 (`player-native`).
//!
//! Usage:
//!   player-native [path/to/game.nirpak] [--watch] [--resume] [--patch <locale>=<file.json>]...

use anyhow::{Context, Result};
use player_native::DesktopOptions;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut pak_path: Option<PathBuf> = None;
    let mut opts = DesktopOptions::default();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--watch" {
            opts.watch_hot_reload = true;
        } else if arg == "--resume" {
            opts.resume_latest = true;
        } else if arg == "--patch" {
            if let Some(spec) = args.next() {
                if let Some((loc, file)) = spec.split_once('=') {
                    opts.patches.push((loc.to_owned(), PathBuf::from(file)));
                } else {
                    let p = PathBuf::from(&spec);
                    let loc = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("en")
                        .to_owned();
                    opts.patches.push((loc, p));
                }
            }
        } else if !arg.starts_with('-') && pak_path.is_none() {
            pak_path = Some(PathBuf::from(arg));
        }
    }

    let path = match pak_path {
        Some(p) => p,
        None => {
            let exe = std::env::current_exe().context("locating executable")?;
            let sibling = exe
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("game.nirpak");
            if sibling.is_file() {
                sibling
            } else if PathBuf::from("game.nirpak").is_file() {
                PathBuf::from("game.nirpak")
            } else {
                #[cfg(not(target_os = "android"))]
                {
                    if let Some(picked) = rfd::FileDialog::new()
                        .set_title("Open NIR Game Package (.nirpak)")
                        .add_filter("NIR Game Package", &["nirpak"])
                        .pick_file()
                    {
                        picked
                    } else {
                        return Ok(());
                    }
                }
                #[cfg(target_os = "android")]
                PathBuf::from("game.nirpak")
            }
        }
    };
    player_native::run_desktop_with_options(&path, opts)
}

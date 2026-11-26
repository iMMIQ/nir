//! Thin Desktop binary entry for NIR v2 (`player-native`).
//!
//! Usage:
//!   player-native [path/to/game.nirpak] [--shelf [dir]] [--watch] [--resume] [--patch <locale>=<file.json>]...

use anyhow::{Context, Result};
use player_native::DesktopOptions;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut pak_path: Option<PathBuf> = None;
    let mut shelf_dir: Option<PathBuf> = None;
    let mut force_shelf = false;
    let mut opts = DesktopOptions::default();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--watch" {
            opts.watch_hot_reload = true;
        } else if arg == "--resume" {
            opts.resume_latest = true;
        } else if arg == "--shelf" {
            force_shelf = true;
            if let Some(next) = args.next() {
                if !next.starts_with('-') {
                    shelf_dir = Some(PathBuf::from(next));
                }
            }
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

    if force_shelf {
        return player_native::run_desktop_shelf(shelf_dir.as_deref(), opts);
    }

    let path = match pak_path {
        Some(p) => Some(p),
        None => {
            let exe = std::env::current_exe().context("locating executable")?;
            let sibling = exe
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("game.nirpak");
            if sibling.is_file() {
                Some(sibling)
            } else if PathBuf::from("game.nirpak").is_file() {
                Some(PathBuf::from("game.nirpak"))
            } else {
                None
            }
        }
    };

    match path {
        Some(p) => player_native::run_desktop_with_options(&p, opts),
        None => player_native::run_desktop_shelf(None, opts),
    }
}

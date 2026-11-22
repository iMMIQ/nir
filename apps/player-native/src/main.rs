//! Thin Desktop binary entry for NIR v2 (`player-native`).
//!
//! Usage:
//!   player-native [path/to/game.nirpak] [--watch]

use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut pak_path: Option<PathBuf> = None;
    let mut watch = false;
    for arg in std::env::args().skip(1) {
        if arg == "--watch" {
            watch = true;
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
            } else {
                PathBuf::from("game.nirpak")
            }
        }
    };
    player_native::run_desktop(&path, watch)
}

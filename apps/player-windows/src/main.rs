#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod desktop;

#[cfg(windows)]
fn main() {
    if let Err(error) = desktop::run() {
        let text = format!("无法启动 / Unable to start\n\n{error:#}");
        let args: Vec<_> = std::env::args_os().collect();
        if let Some(at) = args.iter().position(|a| a == "--smoke-report") {
            if let Some(path) = args.get(at + 1) {
                let _ = player_windows::atomic_write(
                    std::path::Path::new(path),
                    serde_json::json!({"ok":false,"error":text})
                        .to_string()
                        .as_bytes(),
                );
            }
        } else if args.iter().any(|a| a == "--verify") {
            eprintln!("{text}");
        } else {
            rfd::MessageDialog::new()
                .set_title("NIR")
                .set_description(&text)
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("player-windows requires Windows");
    std::process::exit(1);
}

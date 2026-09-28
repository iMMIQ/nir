#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = player_desktop::desktop::run() {
        // No bundled dialog backend on Linux: report on stderr so terminal,
        // launcher and CI invocations all see the failure.
        let text = format!("无法启动 / Unable to start\n\n{error:#}");
        let args: Vec<_> = std::env::args_os().collect();
        if let Some(at) = args.iter().position(|a| a == "--smoke-report") {
            if let Some(path) = args.get(at + 1) {
                let _ = player_desktop::atomic_write(
                    std::path::Path::new(path),
                    serde_json::json!({"ok":false,"error":text})
                        .to_string()
                        .as_bytes(),
                );
            }
        } else {
            eprintln!("{text}");
        }
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("player-linux requires Linux");
    std::process::exit(1);
}

//! Workspace automation CLI (`cargo xtask sdk | check-architecture | test`).

use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("starting {cmd:?}"))?;
    if !status.success() {
        bail!("command failed: {cmd:?}");
    }
    Ok(())
}

fn android_tool(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_owned()
    }
}

fn android_ndk_tools() -> Option<PathBuf> {
    let host_tags: &[&str] = if cfg!(target_os = "macos") {
        &["darwin-x86_64", "darwin-arm64"]
    } else if cfg!(windows) {
        &["windows-x86_64"]
    } else {
        &["linux-x86_64"]
    };
    ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_NDK"]
        .iter()
        .filter_map(std::env::var_os)
        .filter_map(|root| {
            let prebuilt = Path::new(&root).join("toolchains/llvm/prebuilt");
            host_tags
                .iter()
                .map(|tag| prebuilt.join(tag).join("bin"))
                .find(|bin| {
                    bin.join(android_tool("aarch64-linux-android26-clang"))
                        .is_file()
                })
        })
        .next()
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    std::env::set_current_dir(root)?;
    fs::create_dir_all("target/tmp")?;

    match std::env::args().nth(1).as_deref() {
        Some("sdk") => {
            fs::create_dir_all("dist/sdk")?;
            // 1. Build Web player WASM + wasm-bindgen (+ optional wasm-opt -Os)
            run(Command::new("cargo").args([
                "build",
                "-p",
                "player-web",
                "--target",
                "wasm32-unknown-unknown",
                "--profile",
                "release-wasm",
            ]))?;
            let bindgen = std::env::var_os("WASM_BINDGEN")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let home = std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .unwrap_or_default();
                    let p = Path::new(&home).join(".cargo/bin/wasm-bindgen");
                    if cfg!(windows) {
                        p.with_extension("exe")
                    } else {
                        p
                    }
                });
            run(Command::new(bindgen).args([
                "--target",
                "web",
                "--out-dir",
                "dist/sdk",
                "target/wasm32-unknown-unknown/release-wasm/player_web.wasm",
            ]))?;
            let wasm_opt = std::env::var_os("WASM_OPT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("wasm-opt"));
            if Command::new(&wasm_opt).arg("--version").output().is_ok() {
                let wasm_path = "dist/sdk/player_web_bg.wasm";
                run(Command::new(&wasm_opt).args(["-Os", "--enable-bulk-memory", wasm_path, "-o", wasm_path]))?;
            } else {
                println!("SDK: wasm-opt not found on PATH; skipping WASM size pass");
            }
            for name in ["index.html", "style.css", "app.js"] {
                fs::copy(
                    format!("apps/player-web/host/{name}"),
                    format!("dist/sdk/{name}"),
                )?;
            }

            // 2. Build unified native player
            run(Command::new("cargo").args(["build", "-p", "player-native", "--release"]))?;
            fs::create_dir_all("dist/sdk/native")?;
            let bin = if cfg!(windows) {
                "player-native.exe"
            } else {
                "player-native"
            };
            fs::copy(format!("target/release/{bin}"), format!("dist/sdk/native/{bin}"))?;

            // 3. Optional Android cdylib cross-compile when NDK is present
            if let Some(tools) = android_ndk_tools() {
                let mut build = Command::new("cargo");
                build
                    .args([
                        "build",
                        "-p",
                        "player-android",
                        "--target",
                        "aarch64-linux-android",
                        "--release",
                    ])
                    .env(
                        "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER",
                        tools.join(android_tool("aarch64-linux-android26-clang")),
                    );
                let ar = tools.join(android_tool("llvm-ar"));
                if ar.is_file() {
                    build.env("CARGO_TARGET_AARCH64_LINUX_ANDROID_AR", ar);
                }
                run(&mut build)?;
                fs::create_dir_all("dist/sdk/android/lib/arm64-v8a")?;
                fs::copy(
                    "target/aarch64-linux-android/release/libplayer_android.so",
                    "dist/sdk/android/lib/arm64-v8a/libplayer.so",
                )?;
            } else {
                println!("SDK: Android player skipped (ANDROID_NDK_HOME not set)");
            }

            // 4. Build `novelc` CLI
            run(Command::new("cargo").args(["build", "-p", "novelc", "--release"]))?;
            let novelc = if cfg!(windows) { "novelc.exe" } else { "novelc" };
            fs::copy(format!("target/release/{novelc}"), format!("dist/{novelc}"))?;
            println!("SDK ready: dist/sdk & dist/{novelc}");
        }
        Some("check-architecture") => {
            run(Command::new("python3").arg("scripts/check_architecture.py"))?;
        }
        Some("test") => {
            run(Command::new("cargo").args(["test", "--workspace"]))?;
            run(Command::new("python3").arg("scripts/check_architecture.py"))?;
        }
        _ => println!("Usage: cargo xtask sdk | check-architecture | test"),
    }
    Ok(())
}

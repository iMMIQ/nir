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

fn android_ndk_tools() -> Option<(PathBuf, PathBuf)> {
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
            let root_path = PathBuf::from(root);
            let prebuilt = root_path.join("toolchains/llvm/prebuilt");
            host_tags
                .iter()
                .map(|tag| prebuilt.join(tag).join("bin"))
                .find(|bin| {
                    bin.join(android_tool("aarch64-linux-android26-clang"))
                        .is_file()
                })
                .map(|bin| (root_path, bin))
        })
        .next()
}

fn copy_android_gradle_template(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if matches!(
            name_str.as_ref(),
            "build" | ".gradle" | "local.properties" | "jniLibs" | ".idea"
        ) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_android_gradle_template(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
            #[cfg(unix)]
            if name_str == "gradlew" {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&to, fs::Permissions::from_mode(0o755));
            }
        }
    }
    Ok(())
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
                run(Command::new(&wasm_opt).args([
                    "-Os",
                    "--enable-bulk-memory",
                    wasm_path,
                    "-o",
                    wasm_path,
                ]))?;
            } else {
                println!("SDK: wasm-opt not found on PATH; skipping WASM size pass");
            }
            for name in [
                "index.html",
                "style.css",
                "app.js",
                "manifest.webmanifest",
                "sw.js",
            ] {
                let src = format!("apps/player-web/host/{name}");
                if Path::new(&src).is_file() {
                    fs::copy(src, format!("dist/sdk/{name}"))?;
                }
            }

            // 2. Build unified native player
            run(Command::new("cargo").args(["build", "-p", "player-native", "--release"]))?;
            fs::create_dir_all("dist/sdk/native")?;
            let bin = if cfg!(windows) {
                "player-native.exe"
            } else {
                "player-native"
            };
            fs::copy(
                format!("target/release/{bin}"),
                format!("dist/sdk/native/{bin}"),
            )?;
            if !cfg!(windows) {
                for name in ["nir-player.desktop", "nir-nirpak.xml", "install-desktop.sh"] {
                    let src = format!("apps/player-native/desktop/{name}");
                    let dst = format!("dist/sdk/native/{name}");
                    if Path::new(&src).is_file() {
                        fs::copy(&src, &dst)?;
                        #[cfg(unix)]
                        if name == "install-desktop.sh" {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o755));
                        }
                    }
                }
            }

            // 3. Optional Android cdylib cross-compile when NDK is present
            // Always link with 16 KiB ELF max-page-size for Android 15+ compatibility!
            if let Some((ndk_root, tools)) = android_ndk_tools() {
                let build_android_abi =
                    |triple: &str, clang_prefix: &str, abi_dir: &str| -> Result<()> {
                        let clang = tools.join(android_tool(&format!("{clang_prefix}26-clang")));
                        let clangxx =
                            tools.join(android_tool(&format!("{clang_prefix}26-clang++")));
                        let env_triple = triple.replace('-', "_");
                        let env_triple_upper = env_triple.to_ascii_uppercase();
                        let mut build = Command::new("cargo");
                        build
                            .args([
                                "build",
                                "-p",
                                "player-android",
                                "--target",
                                triple,
                                "--release",
                            ])
                            .env("ANDROID_NDK_ROOT", &ndk_root)
                            .env(format!("CARGO_TARGET_{env_triple_upper}_LINKER"), &clang)
                            .env(
                                format!("CARGO_TARGET_{env_triple_upper}_RUSTFLAGS"),
                                "-C link-arg=-Wl,-z,max-page-size=16384",
                            )
                            .env(format!("CC_{env_triple}"), &clang)
                            .env(format!("CXX_{env_triple}"), &clangxx);
                        let ar = tools.join(android_tool("llvm-ar"));
                        if ar.is_file() {
                            build
                                .env(format!("CARGO_TARGET_{env_triple_upper}_AR"), &ar)
                                .env(format!("AR_{env_triple}"), &ar);
                        }
                        let ranlib = tools.join(android_tool("llvm-ranlib"));
                        if ranlib.is_file() {
                            build.env(format!("RANLIB_{env_triple}"), &ranlib);
                        }
                        if cfg!(windows)
                            && std::env::var_os(format!("CMAKE_GENERATOR_{env_triple}")).is_none()
                        {
                            build.env(format!("CMAKE_GENERATOR_{env_triple}"), "Ninja");
                        }
                        run(&mut build)?;
                        let out_dir = format!("dist/sdk/android/lib/{abi_dir}");
                        fs::create_dir_all(&out_dir)?;
                        fs::copy(
                            format!("target/{triple}/release/libplayer_android.so"),
                            format!("{out_dir}/libplayer.so"),
                        )?;
                        Ok(())
                    };

                build_android_abi(
                    "aarch64-linux-android",
                    "aarch64-linux-android",
                    "arm64-v8a",
                )?;
                if std::env::var_os("NIR_BUILD_ANDROID_X86_64").is_some() {
                    build_android_abi("x86_64-linux-android", "x86_64-linux-android", "x86_64")?;
                }
                let gradle_src = Path::new("apps/player-android/android");
                if gradle_src.join("settings.gradle.kts").is_file() {
                    let gradle_dst = Path::new("dist/sdk/android/gradle-project");
                    let _ = fs::remove_dir_all(gradle_dst);
                    copy_android_gradle_template(gradle_src, gradle_dst)?;
                }
            } else {
                println!("SDK: Android player skipped (ANDROID_NDK_HOME not set)");
            }

            // 4. Build `novelc` CLI
            run(Command::new("cargo").args(["build", "-p", "novelc", "--release"]))?;
            let novelc = if cfg!(windows) {
                "novelc.exe"
            } else {
                "novelc"
            };
            fs::copy(format!("target/release/{novelc}"), format!("dist/{novelc}"))?;

            // 5. Generate compact shared Pan-CJK Web fallback font if not yet present
            let fallback_font = Path::new("dist/sdk/fallback-cjk.otf");
            if !fallback_font.is_file() {
                let status = Command::new(format!("dist/{novelc}"))
                    .args(["build-fallback-font", "--out", "dist/sdk/fallback-cjk.otf"])
                    .status();
                if !status.is_ok_and(|s| s.success()) {
                    println!("SDK: skipped fallback-cjk.otf (no system Noto Sans CJK found)");
                }
            }
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

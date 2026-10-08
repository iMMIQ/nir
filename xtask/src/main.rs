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
/// Windows NDK toolchain wrappers are `.cmd` batch files; other hosts ship
/// plain executables.
fn android_tool(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_owned()
    }
}
/// Locates the NDK toolchain `bin` directory from `ANDROID_NDK_HOME` /
/// `ANDROID_NDK_ROOT` / `ANDROID_NDK`, probing the prebuilt host tags in
/// descending likelihood. None means "no NDK here" and skips the Android arm.
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
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let e = entry?;
        let name = e.file_name();
        if [
            "dist",
            "reports",
            ".nir",
            "game.lock",
            // Machine-local Android signing identity, never redistributed.
            "android-signing.pem",
        ]
        .iter()
        .any(|x| name == *x)
        {
            continue;
        }
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dst.join(name))?;
        } else {
            fs::copy(e.path(), dst.join(name))?;
        }
    }
    Ok(())
}
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    std::env::set_current_dir(root)?;
    fs::create_dir_all("target/tmp")?;
    std::env::set_var("TMPDIR", root.join("target/tmp"));
    match std::env::args().nth(1).as_deref() {
        Some("sdk") => {
            // Keep developer usernames and checkout locations out of distributed binaries.
            // Use Cargo's encoded flags so paths containing spaces remain a single flag.
            let mut flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
            for (from, to) in [
                (
                    std::env::var_os("HOME").map(std::path::PathBuf::from),
                    "/build-home",
                ),
                (Some(root.to_path_buf()), "/nir"),
            ] {
                if let Some(from) = from {
                    if !flags.is_empty() {
                        flags.push('\x1f');
                    }
                    flags.push_str(&format!("--remap-path-prefix={}={to}", from.display()));
                }
            }
            std::env::set_var("CARGO_ENCODED_RUSTFLAGS", flags);
            // Bundled native subsetter must also omit developer build paths.
            let mut cxx = std::env::var("CXXFLAGS").unwrap_or_default();
            if let Some(home) = std::env::var_os("HOME") {
                cxx.push_str(&format!(
                    " -ffile-prefix-map={}=/build-home",
                    Path::new(&home).display()
                ));
            }
            cxx.push_str(&format!(" -ffile-prefix-map={}=/nir", root.display()));
            std::env::set_var("CXXFLAGS", cxx);
            run(Command::new("cargo").args([
                "build",
                "--locked",
                "-p",
                "player-web",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ]))?;
            fs::create_dir_all("dist/sdk")?;
            let bindgen = std::env::var_os("WASM_BINDGEN")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    let home = std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .unwrap_or_default();
                    let bindgen = Path::new(&home).join(".cargo/bin/wasm-bindgen");
                    if cfg!(windows) {
                        bindgen.with_extension("exe")
                    } else {
                        bindgen
                    }
                });
            let version = Command::new(&bindgen)
                .arg("--version")
                .output()
                .context("install wasm-bindgen-cli 0.2.100")?;
            if !String::from_utf8_lossy(&version.stdout).contains("0.2.100") {
                bail!("E_BINDGEN_VERSION: expected 0.2.100");
            }
            run(Command::new(bindgen).args([
                "--target",
                "web",
                "--out-dir",
                "dist/sdk",
                "--no-typescript",
                "target/wasm32-unknown-unknown/release/player_web.wasm",
            ]))?;
            for name in ["index.html", "bootstrap.js"] {
                fs::copy(
                    format!("apps/player-web/host/{name}"),
                    format!("dist/sdk/{name}"),
                )?;
            }
            fs::copy("crates/nir-platform-web/host.js", "dist/sdk/host.js")?;
            for name in ["runtime-worker.js", "asset-worker.js"] {
                fs::copy(
                    format!("crates/nir-platform-web/{name}"),
                    format!("dist/sdk/{name}"),
                )?;
            }
            fs::copy("docs/IMPORT.md", "dist/sdk/IMPORT.md")?;
            if cfg!(windows) {
                run(Command::new("cargo").args([
                    "build",
                    "--locked",
                    "-p",
                    "player-windows",
                    "--release",
                ]))?;
                fs::create_dir_all("dist/sdk/windows")?;
                fs::copy(
                    "target/release/player-windows.exe",
                    "dist/sdk/windows/player-windows.exe",
                )?;
            }
            if cfg!(target_os = "linux") {
                run(Command::new("cargo").args([
                    "build",
                    "--locked",
                    "-p",
                    "player-linux",
                    "--release",
                ]))?;
                fs::create_dir_all("dist/sdk/linux")?;
                fs::copy("target/release/player-linux", "dist/sdk/linux/player-linux")?;
            }
            // The Android player cross-compiles from any host, but only when
            // an NDK is discoverable; otherwise the SDK ships without it, the
            // same way a Windows host ships no Linux player.
            if let Some(tools) = android_ndk_tools() {
                let mut build = Command::new("cargo");
                build
                    .args([
                        "build",
                        "--locked",
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
                println!(
                    "SDK: Android player skipped (set ANDROID_NDK_HOME to an NDK r27+ install)"
                );
            }
            run(Command::new("python3")
                .args(["scripts/third_party.py", "dist/sdk/THIRD-PARTY.txt"]))?;
            copy_dir(
                Path::new("examples/rain-letters"),
                Path::new("dist/sdk/template"),
            )?;
            copy_dir(
                Path::new("templates/minimal"),
                Path::new("dist/sdk/templates/minimal"),
            )?;
            for template in ["dist/sdk/template", "dist/sdk/templates/minimal"] {
                let docs = Path::new(template).join("docs");
                fs::create_dir_all(&docs)?;
                for name in [
                    "AUTHOR-FONTS.md",
                    "LOCALE-FONTS.md",
                    "TEXT-REVISIONS.md",
                    "PROJECT-THEMES.md",
                    "AUDIO-SEMANTICS.md",
                    "TWEEN-SEMANTICS.md",
                    "TIME-DOMAINS.md",
                    "READING-SEMANTICS.md",
                    "MENU-ELEMENTS-SEMANTICS.md",
                    "MENU-STATE-SEMANTICS.md",
                    "MENU-SERVICES-SEMANTICS.md",
                    "MENU-NAVIGATION-SEMANTICS.md",
                    "MENU-VALUES-SEMANTICS.md",
                    "STORAGE-SERVICE-SEMANTICS.md",
                    "MENU-HISTORY-SEMANTICS.md",
                    "MENU-HISTORY-FLOW-SEMANTICS.md",
                    "MENU-HISTORY-SCROLLBAR-SEMANTICS.md",
                    "MENU-STACK-SEMANTICS.md",
                    "MENU-TEXT-BUTTON-SEMANTICS.md",
                    "MENU-STORY-SEMANTICS.md",
                    "MODULE-WORKFLOW.md",
                    "CONTENT-RESIDENCY.md",
                    "M3-PREPARATION.md",
                    "M4-RELEASE.md",
                ] {
                    fs::copy(Path::new("docs").join(name), docs.join(name))?;
                }
                let validation = docs.join("validation/m4");
                fs::create_dir_all(&validation)?;
                fs::copy(
                    "docs/validation/m4/summary.json",
                    validation.join("summary.json"),
                )?;
            }
            run(Command::new("cargo").args(["build", "--locked", "-p", "novelc", "--release"]))?;
            let novelc = if cfg!(windows) {
                "novelc.exe"
            } else {
                "novelc"
            };
            // A preview server may still be executing the previous CLI inode.
            fs::copy(format!("target/release/{novelc}"), "dist/novelc.next")?;
            fs::rename("dist/novelc.next", format!("dist/{novelc}"))?;
            let fingerprint = format!(
                "import hashlib,pathlib; pathlib.Path('dist/sdk/compiler.sha256').write_text(hashlib.sha256(pathlib.Path('dist/{novelc}').read_bytes()).hexdigest()+'\\n')"
            );
            run(Command::new("python3").args(["-c", &fingerprint]))?;
            run(Command::new(format!("dist/{novelc}")).args([
                "schemas",
                "--out",
                "dist/sdk/template/schemas",
            ]))?;
            run(Command::new(format!("dist/{novelc}")).args([
                "schemas",
                "--out",
                "dist/sdk/templates/minimal/schemas",
            ]))?;
            println!("SDK: dist/sdk; CLI: dist/novelc");
        }
        Some("check-architecture") => {
            run(Command::new("python3").arg("scripts/check_architecture.py"))?
        }
        Some("test") => {
            let mut args = std::env::args().skip(2).peekable();
            let quick = args.peek().is_some_and(|arg| arg == "--quick");
            if quick {
                args.next();
            }
            let mut command = Command::new("cargo");
            command.args(["test", "--locked"]);
            for package in [
                "nir-core",
                "nir-content",
                "nir-assets",
                "nir-player",
                "nir-presentation",
                "nir-render-wgpu",
                "nir-apk",
                "player-desktop",
            ] {
                command.args(["-p", package]);
            }
            if !quick {
                command.args(["-p", "nir-compiler", "-p", "novelc"]);
            }
            run(command.args(args))?;
            if !quick {
                run(Command::new("python3").arg("scripts/check_architecture.py"))?;
                run(Command::new("python3").arg("scripts/verify_capabilities.py"))?;
            }
        }
        _ => println!("cargo xtask sdk | check-architecture | test [--quick] [cargo test args...]"),
    }
    Ok(())
}

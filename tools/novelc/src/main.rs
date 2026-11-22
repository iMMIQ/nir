//! `novelc` — NIR v2 Developer CLI.
//!
//! Commands:
//!   novelc init <dir>                   Scaffold a zero-boilerplate NIR v2 project
//!   novelc check [project]              Compile & validate scripts + assets
//!   novelc build [project]              Build optimized .nirpak / web / native / android
//!   novelc convert <source> --out <dir> Auto-detect & convert LiveMaker / KiriKiri2 games
//!   novelc dev [project]                Run live hot-reload Web preview server
//!   novelc serve <dir>                  Serve a built Web bundle locally
//!   novelc i18n extract [project]       Extract/sync locale translation tables
#![forbid(unsafe_code)]

mod dev;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use nir_build::optimize::{AudioCodecMode, OptimizeConfig};
use nir_import_core::{emit_nirs_project, DecodedPixels, ImportOptions, ImporterRegistry, RawAssetData};
use nir_import_krkr::KrkrImporter;
use nir_import_livemaker::LiveMakerImporter;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, ValueEnum)]
enum CliAudioCodec {
    Mp3,
    Opus,
}

impl From<CliAudioCodec> for AudioCodecMode {
    fn from(c: CliAudioCodec) -> Self {
        match c {
            CliAudioCodec::Mp3 => AudioCodecMode::Mp3Vbr,
            CliAudioCodec::Opus => AudioCodecMode::Opus,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "novelc",
    version,
    about = "NIR v2 Galgame Engine CLI — build, auto-optimize, live-reload, and convert games"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum BuildTarget {
    Pak,
    Web,
    Native,
    Android,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a minimal zero-boilerplate NIR v2 project
    Init {
        dir: PathBuf,
        #[arg(long, default_value = "My Story")]
        title: String,
    },
    /// Compile and validate scripts and assets without writing output files
    Check {
        #[arg(default_value = ".")]
        project: PathBuf,
    },
    /// Compile, auto-optimize, and package a NIR v2 project
    Build {
        #[arg(default_value = ".")]
        project: PathBuf,
        #[arg(long, value_enum, default_value_t = BuildTarget::Pak)]
        target: BuildTarget,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        no_optimize: bool,
        #[arg(long)]
        lossless: bool,
        #[arg(long, default_value_t = 85)]
        webp_quality: u8,
        /// Audio transcoding profile (`mp3` universal VBR, `opus` extreme 48k/96k)
        #[arg(long, value_enum, default_value_t = CliAudioCodec::Mp3)]
        audio_codec: CliAudioCodec,
        /// Disable 16×16 tile-masked CG/sprite differential compression
        #[arg(long)]
        no_delta: bool,
        /// Disable automatic 256×144 gallery thumbnail generation
        #[arg(long)]
        no_thumbs: bool,
        /// Encode large images as KTX2 BC1/BC3 hardware-compressed GPU textures
        #[arg(long)]
        ktx2: bool,
        #[arg(long)]
        sdk: Option<PathBuf>,
    },
    /// Auto-detect and convert a third-party visual novel (LiveMaker / KiriKiri2) to NIR v2
    Convert {
        source: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Engine plugin id (`auto`, `livemaker`, `krkr`)
        #[arg(long, default_value = "auto")]
        engine: String,
        /// Fail if any fallback diagnostic occurs
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        locale: Option<String>,
        /// Optionally compile & package the converted project immediately
        #[arg(long, value_enum)]
        build: Option<BuildTarget>,
        #[arg(long)]
        no_optimize: bool,
        #[arg(long)]
        lossless: bool,
        #[arg(long, default_value_t = 85)]
        webp_quality: u8,
        /// Audio transcoding profile (`mp3` universal VBR, `opus` extreme 48k/96k)
        #[arg(long, value_enum, default_value_t = CliAudioCodec::Mp3)]
        audio_codec: CliAudioCodec,
        /// Disable 16×16 tile-masked CG/sprite differential compression
        #[arg(long)]
        no_delta: bool,
        /// Disable automatic 256×144 gallery thumbnail generation
        #[arg(long)]
        no_thumbs: bool,
        /// Encode large images as KTX2 BC1/BC3 hardware-compressed GPU textures
        #[arg(long)]
        ktx2: bool,
        /// Also emit editable `.nirs` project files even when direct-to-IR is available
        #[arg(long)]
        emit_nirs: bool,
        #[arg(long)]
        sdk: Option<PathBuf>,
    },
    /// Launch a live hot-reload Web preview server
    Dev {
        #[arg(default_value = ".")]
        project: PathBuf,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4173)]
        port: u16,
        #[arg(long)]
        sdk: Option<PathBuf>,
    },
    /// Serve a built Web output directory over HTTP
    Serve {
        dir: PathBuf,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4173)]
        port: u16,
    },
    /// Translation management
    I18n {
        #[command(subcommand)]
        action: I18nCmd,
    },
}

#[derive(Subcommand)]
enum I18nCmd {
    /// Extract all dialogue/choice strings into `locales/<lang>.toml`
    Extract {
        #[arg(default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        lang: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Init { dir, title } => cmd_init(&dir, &title),
        Cmd::Check { project } => cmd_check(&project),
        Cmd::Build {
            project,
            target,
            out,
            no_optimize,
            lossless,
            webp_quality,
            audio_codec,
            no_delta,
            no_thumbs,
            ktx2,
            sdk,
        } => {
            let opt = OptimizeConfig {
                enabled: !no_optimize,
                lossless_images: lossless,
                webp_quality,
                audio_codec: audio_codec.into(),
                delta_images: !no_delta,
                gallery_thumbnails: !no_thumbs,
                ktx2_textures: ktx2,
                ..Default::default()
            };
            cmd_build(&project, target, out.as_deref(), &opt, sdk.as_deref())
        }
        Cmd::Convert {
            source,
            out,
            engine,
            strict,
            locale,
            build,
            no_optimize,
            lossless,
            webp_quality,
            audio_codec,
            no_delta,
            no_thumbs,
            ktx2,
            emit_nirs,
            sdk,
        } => {
            let opt = OptimizeConfig {
                enabled: !no_optimize,
                lossless_images: lossless,
                webp_quality,
                audio_codec: audio_codec.into(),
                delta_images: !no_delta,
                gallery_thumbnails: !no_thumbs,
                ktx2_textures: ktx2,
                ..Default::default()
            };
            cmd_convert(
                &source,
                &out,
                &engine,
                strict,
                locale,
                build,
                &opt,
                emit_nirs,
                sdk.as_deref(),
            )
        }
        Cmd::Dev {
            project,
            host,
            port,
            sdk,
        } => {
            let sdk_dir = resolve_sdk_dir(sdk.as_deref());
            dev::run_dev_server(&project, &sdk_dir, &host, port)
        }
        Cmd::Serve { dir, host, port } => dev::serve_static(&dir, &host, port),
        Cmd::I18n {
            action: I18nCmd::Extract { project, lang },
        } => {
            let path = nir_build::extract_translations(&project, &lang)?;
            println!("Updated translation file: {}", path.display());
            Ok(())
        }
    }
}

fn cmd_init(dir: &Path, title: &str) -> Result<()> {
    fs::create_dir_all(dir.join("scripts"))?;
    fs::create_dir_all(dir.join("assets"))?;
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "story" } else { slug };

    let manifest = format!(
        "[game]\nid = \"org.example.{slug}\"\ntitle = \"{title}\"\nversion = \"0.1.0\"\nlocale = \"zh-Hans\"\nstage = [1280, 720]\n"
    );
    fs::write(dir.join("game.toml"), manifest)?;

    let script = format!(
        "# {title}\ncharacter narrator \"旁白\" color=#a8c6e8\nvar visited = 0\n\nlabel start:\n\"欢迎来到 {title}。\"\n\"只需在 scripts/ 写 .nirs，把素材放进 assets/，即可一键构建多端！\"\n\nmenu:\n  - \"继续探索\" -> explore\n  - \"结束旅程\" -> ending\n\nlabel explore:\n$ visited += 1\nnarrator: \"你迈出了第一步（已探索 {{visited}} 次）。\"\njump ending\n\nlabel ending:\n\"故事暂告一段落。\"\nend_game good\n"
    );
    fs::write(dir.join("scripts/main.nirs"), script)?;
    println!("Initialized NIR v2 project at `{}`", dir.display());
    println!("Next steps:\n  novelc check {}\n  novelc build {} --target pak", dir.display(), dir.display());
    Ok(())
}

fn cmd_check(project: &Path) -> Result<()> {
    let opt = OptimizeConfig {
        enabled: false,
        ..Default::default()
    };
    let built = nir_build::compile_project(project, &opt)?;
    let s = &built.summary;
    println!(
        "OK `{}` ({}) — {} ops, {} texts, {} active assets ({} unused tree-shaken), {:.1} KiB",
        s.title,
        s.game_id,
        s.ops,
        s.texts,
        s.packed_assets,
        s.pruned_assets,
        s.nirpak_bytes as f64 / 1024.0
    );
    Ok(())
}

fn print_summary(s: &nir_build::BuildSummary) {
    println!(
        "Compiled `{}`: {} ops | {} assets ({} trimmed, {} delta, {} thumbs, {} pruned, saved {:.1} KiB) | {:.1} KiB (.nirpak)",
        s.title,
        s.ops,
        s.packed_assets,
        s.trimmed_sprites,
        s.delta_patched,
        s.generated_thumbs,
        s.pruned_assets,
        s.saved_bytes as f64 / 1024.0,
        s.nirpak_bytes as f64 / 1024.0,
    );
}

fn package_built(
    project: &Path,
    built: &nir_build::BuiltBundle,
    target: BuildTarget,
    out: Option<&Path>,
    sdk_override: Option<&Path>,
) -> Result<()> {
    print_summary(&built.summary);

    match target {
        BuildTarget::Pak => {
            let dest = out
                .map(PathBuf::from)
                .unwrap_or_else(|| project.join("dist/game.nirpak"));
            if let Some(p) = dest.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(&dest, &built.nirpak)?;
            println!("Wrote package: {}", dest.display());
        }
        BuildTarget::Web => {
            let dest = out
                .map(PathBuf::from)
                .unwrap_or_else(|| project.join("dist/web"));
            let sdk_dir = resolve_sdk_dir(sdk_override);
            nir_build::package_web(built, &sdk_dir, &dest)?;
            println!("Packaged Web bundle: {}", dest.display());
        }
        BuildTarget::Native => {
            let dest = out
                .map(PathBuf::from)
                .unwrap_or_else(|| project.join("dist/native"));
            let sdk_dir = resolve_sdk_dir(sdk_override);
            let bin_name = if cfg!(windows) {
                "player-native.exe"
            } else {
                "player-native"
            };
            let candidate = sdk_dir.join("native").join(bin_name);
            nir_build::package_native(built, &candidate, &dest, bin_name)?;
            println!("Packaged Native bundle: {}", dest.display());
        }
        BuildTarget::Android => {
            let dest = out
                .map(PathBuf::from)
                .unwrap_or_else(|| project.join("dist/game.apk"));
            let sdk_dir = resolve_sdk_dir(sdk_override);
            let so_path = sdk_dir.join("android/lib/arm64-v8a/libplayer.so");
            if !so_path.is_file() {
                bail!(
                    "missing Android player library at `{}` (run `cargo xtask sdk` with ANDROID_NDK_HOME)",
                    so_path.display()
                );
            }
            nir_build::package_android(project, built, &so_path, &dest)?;
            println!("Packaged signed Android APK: {}", dest.display());
        }
    }
    Ok(())
}

fn cmd_build(
    project: &Path,
    target: BuildTarget,
    out: Option<&Path>,
    opt: &OptimizeConfig,
    sdk_override: Option<&Path>,
) -> Result<()> {
    let built = nir_build::compile_project(project, opt)?;
    package_built(project, &built, target, out, sdk_override)
}

#[allow(clippy::too_many_arguments)]
fn cmd_convert(
    source: &Path,
    out: &Path,
    engine: &str,
    strict: bool,
    locale: Option<String>,
    build_target: Option<BuildTarget>,
    opt: &OptimizeConfig,
    emit_nirs: bool,
    sdk_override: Option<&Path>,
) -> Result<()> {
    let mut registry = ImporterRegistry::new();
    registry.register(LiveMakerImporter);
    registry.register(KrkrImporter);

    let opts = ImportOptions {
        strict,
        source_locale: locale,
        ..Default::default()
    };

    if !emit_nirs {
        if let Some((direct, report)) = registry
            .import_direct_auto(source, Some(engine), &opts)
            .with_context(|| format!("direct-converting `{}`", source.display()))?
        {
            fs::create_dir_all(out)?;
            let report_json = serde_json::to_string_pretty(&report)?;
            fs::write(out.join("conversion_report.json"), report_json)?;

            let manifest = format!(
                "[game]\nid = {:?}\ntitle = {:?}\nversion = {:?}\nlocale = {:?}\nstage = [{}, {}]\n",
                direct.program.meta.game_id,
                direct.program.meta.title,
                direct.program.meta.version,
                direct.program.meta.default_locale,
                direct.program.meta.stage_width,
                direct.program.meta.stage_height,
            );
            fs::write(out.join("game.toml"), manifest)?;

            let sources: Vec<nir_build::DirectAssetSource> = direct
                .raw_assets
                .into_iter()
                .map(|a| {
                    let fp = a.source_fingerprint;
                    match a.data {
                        RawAssetData::LazyImage(px_fn) => nir_build::DirectAssetSource {
                            name: a.name,
                            kind: a.kind,
                            codec: a.codec,
                            loader: std::sync::Arc::new(|| {
                                anyhow::bail!("pixel asset should use pixel_loader")
                            }),
                            pixel_loader: Some(std::sync::Arc::new(move || {
                                match (px_fn)()? {
                                    DecodedPixels::Rgb {
                                        width,
                                        height,
                                        data,
                                    } => Ok(nir_build::DirectPixels::Rgb {
                                        width,
                                        height,
                                        data,
                                    }),
                                    DecodedPixels::Rgba {
                                        width,
                                        height,
                                        data,
                                    } => Ok(nir_build::DirectPixels::Rgba {
                                        width,
                                        height,
                                        data,
                                    }),
                                }
                            })),
                            source_fingerprint: fp,
                        },
                        other => nir_build::DirectAssetSource {
                            name: a.name,
                            kind: a.kind,
                            codec: a.codec,
                            loader: std::sync::Arc::new(move || other.load()),
                            pixel_loader: None,
                            source_fingerprint: fp,
                        },
                    }
                })
                .collect();
            let cache_dir = out.join(".nir").join("cache");
            let target = build_target.unwrap_or(BuildTarget::Pak);
            println!("{}", report.summary_text());
            if matches!(target, BuildTarget::Pak) {
                // Streaming direct-to-file `.nirpak` build (slashes peak RSS by ~89%!)
                let dest = out.join("dist/game.nirpak");
                let summary = nir_build::build_direct_program_to_file(
                    direct.program,
                    sources,
                    &direct.keep_assets,
                    &cache_dir,
                    opt,
                    &dest,
                )?;
                print_summary(&summary);
                println!("Wrote package: {}", dest.display());
            } else {
                let built = nir_build::build_direct_program(
                    direct.program,
                    sources,
                    &direct.keep_assets,
                    &cache_dir,
                    opt,
                )?;
                package_built(out, &built, target, None, sdk_override)?;
            }
            return Ok(());
        }
    }

    let (project, report) = registry
        .import_auto(source, Some(engine), &opts)
        .with_context(|| format!("converting `{}`", source.display()))?;

    emit_nirs_project(&project, out)?;
    let report_json = serde_json::to_string_pretty(&report)?;
    fs::write(out.join("conversion_report.json"), report_json)?;

    println!("{}", report.summary_text());
    println!("Emitted NIR v2 project: {}", out.display());

    if let Some(target) = build_target {
        cmd_build(out, target, None, opt, sdk_override)?;
    }
    Ok(())
}

fn resolve_sdk_dir(override_dir: Option<&Path>) -> PathBuf {
    if let Some(d) = override_dir {
        return d.to_path_buf();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sibling = parent.join("sdk");
            if sibling.is_dir() {
                return sibling;
            }
        }
    }
    let dist_sdk = PathBuf::from("dist/sdk");
    if dist_sdk.is_dir() {
        return dist_sdk;
    }
    PathBuf::from("apps/player-web/host")
}

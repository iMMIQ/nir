use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use nir_compiler::*;
use std::{fs, path::PathBuf};
mod preview;
mod release_lifecycle;
use preview::{dev, serve};
#[derive(Parser)]
#[command(version, about = "NIR content compiler and Web preview")]
struct Cli {
    #[arg(short, long, global = true, default_value = ".")]
    project: PathBuf,
    #[arg(long, global = true)]
    sdk: Option<PathBuf>,
    #[arg(long, global = true, value_parser = ["text", "json"], default_value = "text")]
    diagnostics: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Inspect and migrate projects from other engines (offline, experimental).
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
    Schemas {
        #[arg(long, default_value = "schemas")]
        out: PathBuf,
    },
    Init {
        path: PathBuf,
        #[arg(long, default_value = "minimal")]
        template: String,
    },
    Resolve,
    /// Track source revisions and explicitly review translations.
    Text {
        #[command(subcommand)]
        command: TextCommand,
    },
    /// Print effective author configuration and the source of each field.
    Config,
    Doctor,
    Check {
        #[arg(long)]
        locked: bool,
    },
    Test,
    Build {
        #[arg(long, default_value = "web")]
        target: String,
        #[arg(long, default_value = "full")]
        edition: String,
        #[arg(long, default_value = "release")]
        profile: String,
        #[arg(long)]
        locked: bool,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Packaged image container: webp (lossy), webp-lossless, png.
        #[arg(long, default_value = "webp")]
        image_format: String,
        /// Lossy WebP quality, 1-100 (default 92).
        #[arg(long, default_value_t = 92)]
        image_quality: u8,
        /// Packaged audio container: mp3 (CBR), wav.
        #[arg(long, default_value = "mp3")]
        audio_format: String,
        /// MP3 CBR bitrate in kbps (default 160).
        #[arg(long, default_value_t = 160)]
        audio_bitrate: u16,
        /// Package the original PNG/WAV bytes untouched.
        #[arg(long)]
        no_optimize: bool,
    },
    Dev {
        #[arg(long, default_value_t = 4173)]
        port: u16,
        #[arg(long)]
        scenario: Option<PathBuf>,
    },
    Serve {
        directory: PathBuf,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4173)]
        port: u16,
    },
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
}
#[derive(Subcommand)]
enum ImportCommand {
    /// Inventory extracted LiveMaker LSB scripts without exporting game content.
    Inspect { source: PathBuf },
    /// Convert LiveMaker 116; recognized LiveNovel profiles include events, menus and media.
    Livemaker {
        source: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        entry: Option<String>,
        /// Source label LineNo, not an index into the command array.
        #[arg(long, default_value_t = 0)]
        line: u32,
        /// Write an incomplete migration (fault blocks or a source-derived UI preview).
        #[arg(long)]
        draft: bool,
        /// Comma-separated approximate mapping rule ids to accept explicitly
        /// (see the mappings array in import-report.json).
        #[arg(long, value_delimiter = ',')]
        accept_approximate: Vec<String>,
        #[arg(long, default_value = "org.nir.imported.livemaker")]
        game_id: String,
        #[arg(long, default_value = "Imported LiveMaker story")]
        title: String,
        #[arg(long, value_parser = ["ja", "en", "zh-Hans"], default_value = "ja")]
        locale: String,
    },
}
#[derive(Subcommand)]
enum ReleaseCommand {
    Stage {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        release: Option<String>,
    },
    Verify {
        #[arg(long, conflicts_with = "url", required_unless_present = "url")]
        directory: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "directory",
            required_unless_present = "directory"
        )]
        url: Option<String>,
        #[arg(long)]
        release: Option<String>,
    },
    Promote {
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        release: String,
        #[arg(long)]
        expect: String,
    },
    Rollback {
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        to: String,
        #[arg(long)]
        expect: String,
    },
}
#[derive(Subcommand)]
enum TextCommand {
    Status {
        #[arg(long)]
        json: bool,
    },
    Update {
        #[arg(long)]
        id: String,
        #[arg(long, value_parser=["preserve","bump"])]
        meaning: String,
    },
    Review {
        #[arg(long)]
        id: String,
        #[arg(long)]
        locale: String,
    },
    Migrate {
        #[arg(long)]
        out: PathBuf,
    },
    Recover,
}
/// Validates the build flags into packaging options; defaults match every
/// player's decode matrix (WebP q92 images, MP3 CBR 160 audio).
fn parse_optimize(
    image_format: &str,
    image_quality: u8,
    audio_format: &str,
    audio_bitrate: u16,
    no_optimize: bool,
) -> Result<OptimizeOptions> {
    if no_optimize {
        return Ok(OptimizeOptions::none());
    }
    let image_format = match image_format {
        "webp" => ImageFormat::Webp,
        "webp-lossless" => ImageFormat::WebpLossless,
        "png" => ImageFormat::Png,
        other => bail!("E_OPTIMIZE: --image-format must be webp|webp-lossless|png, got {other}"),
    };
    if !(1..=100).contains(&image_quality) {
        bail!("E_OPTIMIZE: --image-quality must be 1..=100, got {image_quality}");
    }
    let audio_format = match audio_format {
        "mp3" => AudioFormat::Mp3,
        "wav" => AudioFormat::Wav,
        other => bail!("E_OPTIMIZE: --audio-format must be mp3|wav, got {other}"),
    };
    if !MP3_BITRATES.contains(&audio_bitrate) {
        bail!(
            "E_OPTIMIZE: --audio-bitrate must be one of {} kbps, got {audio_bitrate}",
            MP3_BITRATES.map(|kbps| kbps.to_string()).join(", ")
        );
    }
    Ok(OptimizeOptions {
        image_format,
        image_quality,
        audio_format,
        audio_bitrate_kbps: audio_bitrate,
    })
}
fn main() {
    let cli = Cli::parse();
    let json = cli.diagnostics == "json";
    if let Err(e) = run(cli) {
        let d = diagnostic(&e);
        if json {
            eprintln!("{}", serde_json::json!({"format":1,"diagnostic":d}));
        } else {
            eprintln!("{d}");
            if let Some(details) = &d.details {
                if let Some(s) = &details.source {
                    eprintln!("  {}:{}:{} {}", s.file, s.line, s.column, s.pointer);
                }
                for r in &details.references {
                    eprintln!("  reference: {r}");
                }
                if let Some(h) = &details.hint {
                    eprintln!("  action: {h}");
                }
            }
        }
        std::process::exit(1);
    }
}
fn run(cli: Cli) -> Result<()> {
    let sdk = cli.sdk.unwrap_or_else(default_sdk);
    if matches!(
        &cli.command,
        Command::Init { .. }
            | Command::Import {
                command: ImportCommand::Livemaker { .. }
            }
            | Command::Resolve
            | Command::Doctor
            | Command::Check { locked: true }
            | Command::Build { .. }
            | Command::Dev { .. }
    ) {
        let expected = fs::read_to_string(sdk.join("compiler.sha256"))
            .context("E_SDK_MISSING: compiler identity; run cargo xtask sdk")?;
        let actual = nir_content::digest(&fs::read(std::env::current_exe()?)?);
        if expected.trim() != actual {
            bail!("E_COMPILER_IDENTITY: use the novelc binary distributed with this SDK");
        }
    }
    match cli.command {
        Command::Import { command } => match command {
            ImportCommand::Inspect { source } => {
                let report = import::inspect(&source)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                if !report.errors.is_empty() {
                    bail!("E_IMPORT_INSPECT: some scripts could not be parsed; see report");
                }
            }
            ImportCommand::Livemaker {
                source,
                out,
                entry,
                line,
                draft,
                accept_approximate,
                game_id,
                title,
                locale,
            } => {
                let report = import::convert(
                    &import::ImportOptions {
                        source,
                        out,
                        entry,
                        line,
                        draft,
                        accept_approximate,
                        game_id,
                        title,
                        locale,
                    },
                    &sdk,
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                if report.errors > 0 {
                    bail!("E_IMPORT_INCOMPLETE: {} unsupported locations; project written: {}; see import report", report.errors, report.written);
                }
            }
        },
        Command::Schemas { out } => write_schemas(&out)?,
        Command::Text { command } => match command {
            TextCommand::Status { json } => {
                let report = text_status(&cli.project)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!(
                        "{} texts / {} locales / {}",
                        report.texts,
                        report.locales,
                        if report.ready {
                            "ready"
                        } else {
                            "needs review"
                        }
                    );
                    for issue in report.issues {
                        println!(
                            "{} {} [{}] {}#{}: {}",
                            issue.code,
                            issue.text_id,
                            issue.locale,
                            issue.file,
                            issue.pointer,
                            issue.message
                        );
                    }
                }
            }
            TextCommand::Update { id, meaning } => {
                text_update(&cli.project, &id, meaning == "bump")?;
                println!("Recorded source {id}; review translations explicitly");
            }
            TextCommand::Review { id, locale } => {
                text_review(&cli.project, &id, &locale)?;
                println!("Reviewed {locale}/{id}");
            }
            TextCommand::Migrate { out } => {
                text_migrate(&cli.project, &out)?;
                println!("Migrated to {}; legacy texts imported as the initial review baseline. Use the new SDK and resolve; old saves are not migrated.",out.display());
            }
            TextCommand::Recover => {
                text_recover(&cli.project)?;
                println!("Text transaction rolled back");
            }
        },
        Command::Init { path, template } => {
            init_template(
                &path,
                &sdk,
                &format!("org.nir.game.{}", uuid::Uuid::new_v4()),
                &template,
            )?;
            println!(
                "Created {}\nNext: novelc -p {} resolve",
                path.display(),
                path.display()
            );
        }
        Command::Config => {
            let p = load_project(&cli.project)?;
            println!("{}", serde_json::to_string_pretty(&p.resolved_config)?);
        }
        Command::Resolve => {
            let l = resolve(&cli.project, &sdk)?;
            println!("Locked SDK {}", l.sdk_digest);
        }
        Command::Doctor => {
            let p = load_project(&cli.project)?;
            println!(
                "Content: valid ({} assets, {} locales)",
                p.program.assets.len(),
                p.program.locales.len()
            );
            let m = sdk_manifest(&sdk)?;
            println!("SDK: {} ({} files)\nRuntime: desktop WebGPU; PNG/WebP images, PCM WAV/MP3 audio; zh-Hans/en\nMedia defaults: images -> WebP q92, audio -> MP3 160kbps (see build --help)\nPreview: http://127.0.0.1:4173",sdk.display(),m.files.len());
        }
        Command::Check { locked } => {
            let p = load_project(&cli.project)?;
            compile(&p.program)?;
            if locked {
                check_lock(&p, &sdk)?;
            }
            println!(
                "Valid: {} / {} functions / {} cues / {} locales",
                p.manifest.game.title,
                p.program.functions.len(),
                p.program.cues.len(),
                p.program.locales.len()
            );
        }
        Command::Test => {
            for s in test_project(&cli.project)? {
                println!("PASS {s}");
            }
        }
        Command::Build {
            target,
            edition,
            profile,
            locked,
            out,
            image_format,
            image_quality,
            audio_format,
            audio_bitrate,
            no_optimize,
        } => {
            if !matches!(target.as_str(), "web" | "windows" | "linux" | "android")
                || edition != "full"
                || !matches!(profile.as_str(), "dev" | "release")
            {
                bail!("E_CAPABILITY: supported --target web|windows|linux|android --edition full --profile dev|release");
            }
            let optimize = parse_optimize(
                &image_format,
                image_quality,
                &audio_format,
                audio_bitrate,
                no_optimize,
            )?;
            let out = out.unwrap_or_else(|| cli.project.join(format!("dist/full/{target}")));
            let report = if target == "windows" {
                build_windows(
                    &cli.project,
                    &sdk,
                    &out,
                    &profile,
                    locked || profile == "release",
                    &optimize,
                )?
            } else if target == "linux" {
                build_linux(
                    &cli.project,
                    &sdk,
                    &out,
                    &profile,
                    locked || profile == "release",
                    &optimize,
                )?
            } else if target == "android" {
                build_android(
                    &cli.project,
                    &sdk,
                    &out,
                    &profile,
                    locked || profile == "release",
                    &optimize,
                )?
            } else {
                build_profile(
                    &cli.project,
                    &sdk,
                    &out,
                    &profile,
                    locked || profile == "release",
                    true,
                    &optimize,
                )?
            };
            println!(
                "Built {}\nRelease {}\n{} objects / {} bytes\nMedia: {} images converted ({} -> {} bytes), {} audio converted ({} -> {} bytes)",
                out.display(),
                report.release,
                report.objects,
                report.total_bytes,
                report.optimization.images.converted,
                report.optimization.images.bytes_before,
                report.optimization.images.bytes_after,
                report.optimization.audio.converted,
                report.optimization.audio.bytes_before,
                report.optimization.audio.bytes_after,
            );
        }
        Command::Dev { port, scenario } => {
            if let Some(s) = scenario {
                let p = load_project(&cli.project)?;
                let s = s.to_string_lossy();
                if !p.manifest.inputs.scenarios.iter().any(|x| x == s.as_ref()) {
                    bail!("E_SCENARIO: scenario must be registered in game.toml");
                }
                for id in test_project(&cli.project)? {
                    println!("PASS {id}");
                }
                println!("Scenario verified from new_game; preview opens at its legal entry.");
            }
            dev(&cli.project, &sdk, port)?;
        }
        Command::Serve {
            directory,
            host,
            port,
        } => serve(&directory, &host, port)?,
        Command::Release { command } => match command {
            ReleaseCommand::Stage {
                source,
                directory,
                release,
            } => {
                let digest = release_lifecycle::stage(&source, &directory, release.as_deref())?;
                println!("Staged {digest} in {}", directory.display());
            }
            ReleaseCommand::Verify {
                directory,
                url,
                release,
            } => {
                let location = match (directory, url) {
                    (Some(path), None) => path.to_string_lossy().into_owned(),
                    (None, Some(url)) => url,
                    _ => bail!("E_VERIFY_LOCATION: specify --directory or --url"),
                };
                let report = release_lifecycle::verify(&location, release.as_deref())?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            ReleaseCommand::Promote {
                directory,
                release,
                expect,
            } => {
                release_lifecycle::promote(&directory, &release, &expect)?;
                println!("Promoted {release}");
            }
            ReleaseCommand::Rollback {
                directory,
                to,
                expect,
            } => {
                release_lifecycle::promote(&directory, &to, &expect)?;
                println!("Rolled back to {to}");
            }
        },
    }
    Ok(())
}

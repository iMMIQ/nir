use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use nir_compiler::*;
use std::{fs, path::PathBuf};
mod preview;
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
    },
    Dev {
        #[arg(long, default_value_t = 4173)]
        port: u16,
        #[arg(long)]
        scenario: Option<PathBuf>,
    },
    Serve {
        directory: PathBuf,
        #[arg(long, default_value_t = 4173)]
        port: u16,
    },
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
        Command::Schemas { out } => write_schemas(&out)?,
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
            println!("SDK: {} ({} files)\nRuntime: desktop WebGPU; PCM WAV; zh-Hans/en\nPreview: http://127.0.0.1:4173",sdk.display(),m.files.len());
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
        } => {
            if target != "web"
                || edition != "full"
                || !matches!(profile.as_str(), "dev" | "release")
            {
                bail!("E_CAPABILITY: supported --target web --edition full --profile dev|release");
            }
            let out = out.unwrap_or_else(|| cli.project.join("dist/full/web"));
            let report = build(&cli.project, &sdk, &out, locked)?;
            println!(
                "Built {}\nRelease {}\n{} objects / {} bytes",
                out.display(),
                report.release,
                report.objects,
                report.total_bytes
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
        Command::Serve { directory, port } => serve(&directory, port)?,
    }
    Ok(())
}

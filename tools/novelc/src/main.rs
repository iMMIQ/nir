use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use nir_compiler::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Parser)]
#[command(version, about = "NIR content compiler and Web preview")]
struct Cli {
    #[arg(short, long, global = true, default_value = ".")]
    project: PathBuf,
    #[arg(long, global = true)]
    sdk: Option<PathBuf>,
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
        #[arg(long, default_value = "web-basic")]
        template: String,
    },
    Resolve,
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
    if let Err(e) = run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let cli = Cli::parse();
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
            if template != "web-basic" {
                bail!("E_TEMPLATE: only web-basic is supported");
            }
            init(
                &path,
                &sdk,
                &format!("org.nir.game.{}", uuid::Uuid::new_v4()),
            )?;
            println!(
                "Created {}\nNext: novelc -p {} resolve",
                path.display(),
                path.display()
            );
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
            let out = cli.project.join("dist/full/web");
            build(&cli.project, &sdk, &out, true)?;
            serve(&out, port)?;
        }
        Command::Serve { directory, port } => serve(&directory, port)?,
    }
    Ok(())
}
fn serve(directory: &Path, port: u16) -> Result<()> {
    let root = fs::canonicalize(directory).context("E_SERVE: directory missing")?;
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("E_LISTEN: {e}"))?;
    println!(
        "NIR preview: http://127.0.0.1:{port}/\nServing {}",
        root.display()
    );
    for request in server.incoming_requests() {
        let raw = request.url().split('?').next().unwrap_or("/");
        let path = if raw == "/" {
            "index.html"
        } else {
            raw.trim_start_matches('/')
        };
        let full = if raw.ends_with('/') && raw != "/" {
            root.join(path).join("index.html")
        } else {
            root.join(path)
        };
        let safe =
            !path.contains('%') && !path.contains('\\') && !path.split('/').any(|p| p == "..");
        let resolved = if safe {
            fs::canonicalize(&full).ok()
        } else {
            None
        };
        if let Some(file) = resolved.filter(|p| p.starts_with(&root) && p.is_file()) {
            let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("");
            let mime = match ext {
                "html" => "text/html; charset=utf-8",
                "js" => "text/javascript; charset=utf-8",
                "json" => "application/json",
                "txt" => "text/plain; charset=utf-8",
                "wasm" => "application/wasm",
                "png" => "image/png",
                "wav" => "audio/wav",
                "otf" => "font/otf",
                _ => "application/octet-stream",
            };
            let cache = if path
                .split('/')
                .any(|part| matches!(part, "objects" | "releases"))
            {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            let mut r = tiny_http::Response::from_file(fs::File::open(file)?);
            for (k,v) in [("Content-Type",mime),("Cache-Control",cache),("X-Content-Type-Options","nosniff"),("Content-Security-Policy","default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; media-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'")]{r.add_header(tiny_http::Header::from_bytes(k,v).unwrap());}
            let _ = request.respond(r);
        } else {
            let _ = request
                .respond(tiny_http::Response::from_string("Not found").with_status_code(404));
        }
    }
    Ok(())
}

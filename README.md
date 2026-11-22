# NIR v2 — High-Performance Multi-Platform Visual Novel / Galgame Engine

NIR v2 is a clean-slate, high-performance Galgame playback engine and toolchain built in Rust, designed around four core goals:

1. **High-Performance Multi-Platform Playback** — Web (WebGPU / WebGL2 + single WASM + `createImageBitmap` zero-copy), Windows / Linux / macOS (`player-native` with `memmap2` zero-copy `.nirpak` + background image decode + gapless `rodio`/`symphonia` audio), and Android (`player-android` NDK reading STORED `assets/game.nirpak` directly from `AssetManager`).
2. **Developer-Friendly Authoring** — Zero-boilerplate project layout (`game.toml` + `scripts/*.nirs` + `assets/`), clean `.nirs` script DSL, built-in responsive Galgame UI, and live hot-reload via `novelc dev`.
3. **Automatic Asset Optimization** — Alpha Auto-Trim for sprites (with exact `SpriteTrim` placement restoration), WebP encoding with content-hash caching, WAV → gapless MP3 CBR (with MP3/OGG passthrough), HarfBuzz font subsetting directly from the compiled string pool, and dead-asset tree-shaking into a single `.nirpak` archive.
4. **Automatic Multi-Engine Conversion** — Pluggable converter architecture (`nir-import-core`) with layered case-insensitive VFS, auto CJK encoding detection, engine-agnostic `VN-IR`, Playable-First `ConversionReport`, and built-in **LiveMaker** (`nir-import-livemaker`) and **KiriKiri2 / KAG3** (`nir-import-krkr`, including `.xp3` & `.ks`) plugins.

## Quick Start

```bash
# 1. Create a new zero-boilerplate project
cargo run -p novelc -- init my-story --title "雨后书简"

# 2. Validate scripts & assets
cargo run -p novelc -- check examples/rain-letters

# 3. Build an auto-optimized .nirpak package
cargo run -p novelc -- build examples/rain-letters --target pak

# 4. Build a Web bundle (or Native / Android APK)
cargo xtask sdk
cargo run -p novelc -- build examples/rain-letters --target web

# 5. Live hot-reload development server
cargo run -p novelc -- dev examples/rain-letters

# 6. Auto-convert a LiveMaker or KiriKiri2/KAG3 game into editable NIR v2 + .nirpak
cargo run -p novelc -- convert /path/to/source-game --out ./converted-game --build pak
```

## Workspace Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the complete crate layering diagram, `.nirpak` container format, Persistent Scene Graph model, and converter pipeline.

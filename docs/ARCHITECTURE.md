# NIR v2 Architecture

## Crate Layering

```
Core Runtime Stack (no GPU / browser / compiler deps):
  nir-ir          Flat u32-indexed bytecode, Persistent Scene Graph types, native 64-bit Value,
                  declarative UiTheme/UiNode/UiAction, binary .nirpak container, LAME gapless parser
  nir-vm          Stack/register bytecode VM, Persistent Scene Graph + Tween/Shake/Transition evaluator,
                  version-resilient SaveSnapshot (vars by name + label relocation), hot-reload, asset lookahead
  nir-ui          Zero-alloc u64-key TextEngine (cosmic-text), DrawPacket projection, responsive built-in Galgame UI
  nir-runtime     Single-owner Runtime orchestrating VM, Auto/Skip/Rollback(32)/Replay, exact dirty gating,
                  predictive lookahead HostCommands, live hot-reload

GPU Renderer:
  nir-render      wgpu 25 + glyphon 0.9 compositor, 4 blend pipelines (Normal/Add/Multiply/Screen),
                  RuleMask transitions, LRU texture cache, wasm32 createImageBitmap zero-copy upload

Thin Platform Hosts (never link compiler/importer/hb-subset):
  player-native   Unified Windows / Linux / macOS winit host (memmap2 .nirpak, decode worker, rodio+symphonia, atomic JSON saves)
  player-android  Thin Android NDK cdylib wrapping player-native (direct AssetManager game.nirpak read)
  player-web      Single-WASM wasm-bindgen host + modern single-page host/{index.html,style.css,app.js}

Build, Optimization & Multi-Engine Conversion Toolchain:
  nir-apk         Pure-Rust Android binary manifest (AXML), aligned ZIP, and APK Signature Scheme v2 signer
  nir-build       Zero-boilerplate project scanner, .nirs compiler, Alpha Auto-Trim, WebP, gapless MP3,
                  HarfBuzz font subsetting, dead-asset tree-shaking, Web/Native/Android packagers
  nir-import-core Pluggable EngineImporter trait, layered ArchiveVfs, auto CJK decoder, VN-IR,
                  Playable-First ConversionReport, .nirs project emitter
  nir-import-livemaker  LiveMaker / LiveNovel (LSB 116/117, GAL/GaleX, Cinema) plugin
  nir-import-krkr       KiriKiri2 / KAG3 (XP3 archive reader, .ks parser, macros, TJS expressions) plugin
  novelc          Developer CLI (init / check / build / convert / dev / serve / i18n)
```

Enforced automatically by `python3 scripts/check_architecture.py`.

## Key Design Principles

- **Persistent Scene Graph**: Scripts mutate persistent named layers (`bg`, `aki`, `layer_0`, ...) and commit transitions with `StageCommit`. This eliminates state explosion and provides a direct 1:1 target for LiveMaker, KAG3, Ren'Py, and custom scripts.
- **Exact Dirty Gating**: Frames are re-projected and rendered strictly when VM state, animations, hover state, or newly decoded assets mark `Runtime::dirty` (idle CPU/GPU usage drops to ~0%).
- **Single `.nirpak` Binary Container**: Header (zlib-compressed `NirProgram`) + CRC32 + 64-byte aligned raw asset blobs, enabling zero-copy `mmap` slicing on native, direct `AssetManager` reads on Android, and single-request streaming fetch on Web.
- **Playable-First Multi-Engine Import**: Importers emit readable `.nirs` + `assets/` projects and record `Exact` / `Adapted` / `Fallback` diagnostics in `conversion_report.json` (`--strict` fails on any `Fallback`).

# wgpu 25.0.2 compatibility patch

`wgpu/` is the crates.io wgpu 25.0.2 source, under its upstream MIT/Apache-2.0 licenses. One change is maintained in `src/backend/webgpu.rs`, `set_device_lost_callback`:

Upstream attaches a stack-owned `Closure::once` to `GPUDevice.lost`, then drops it before the promise resolves. A real device loss consequently throws “closure invoked recursively or after being dropped”. The callback is now transferred to JavaScript with `Closure::once_into_js`, and attached through the promise's `then` function. This keeps the callback valid without unsafe code or an unconditional Rust closure leak.

The renderer also retains its wgpu Instance for the device/surface lifetime. Browser tests must exercise actual device destruction and reconstruction before this patch can be removed. The rest of the crate is unchanged.

# tiny_http 0.12.0 connection scheduling patch

`tiny_http/` is the crates.io tiny_http 0.12.0 source under its upstream MIT/Apache-2.0 licenses. In `src/util/task_pool.rs`, queued jobs now reserve waiting workers when deciding whether to start another thread. Previously, a burst could enqueue more connections than there were waiting workers; HTTP keep-alive readers then occupied every worker indefinitely, leaving another connection unanswered.

`scripts/verify_preview_connections.py` exercises simultaneous keep-alive connections against the actual SDK CLI, including a paused accept loop to reproduce a connection burst. Other upstream source is unchanged.

# hb-subset 0.3.0 Windows portability patch

`hb-subset/` is the crates.io hb-subset 0.3.0 source under its upstream MIT license. Three changes make the crate compile and run on MSVC, where libclang generates unsigned underlying types for HarfBuzz enums and no Unix `OsStr` extension exists:

- `src/blob.rs` builds the `CString` path with `OsStr::as_encoded_bytes` instead of the Unix-only `OsStrExt::as_bytes`; the byte sequence is identical on Unix.
- `src/sys.rs` and `src/subset/flags.rs` widen enum payloads with `as _`, so the cast adapts to the signedness bindgen chose per platform instead of assuming `i32`.

Building the bundled HarfBuzz source on MSVC also requires the `/bigobj` option (for example `CXXFLAGS=-bigobj` with cc-rs) because `harfbuzz-subset.cc` exceeds the COFF section limit.

To keep the vendored tree minimal, files that `build.rs` never touches were removed: the upstream generator scripts (`*.py`), autotools/pkg-config/cmake templates, the standalone `test-*.cc` drivers, Arabic PUA source data for those generators, and crate metadata (`CHANGELOG.md`, `README.tpl`, `cliff.toml`, `Cargo.toml.orig`). The compile entry point remains the untouched upstream unity file `harfbuzz/src/harfbuzz-subset.cc` plus every header it includes.

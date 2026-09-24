# wgpu 25.0.2 compatibility patch

`wgpu/` is the crates.io wgpu 25.0.2 source, under its upstream MIT/Apache-2.0 licenses. One change is maintained in `src/backend/webgpu.rs`, `set_device_lost_callback`:

Upstream attaches a stack-owned `Closure::once` to `GPUDevice.lost`, then drops it before the promise resolves. A real device loss consequently throws “closure invoked recursively or after being dropped”. The callback is now transferred to JavaScript with `Closure::once_into_js`, and attached through the promise's `then` function. This keeps the callback valid without unsafe code or an unconditional Rust closure leak.

The renderer also retains its wgpu Instance for the device/surface lifetime. Browser tests must exercise actual device destruction and reconstruction before this patch can be removed. The rest of the crate is unchanged.

# tiny_http 0.12.0 connection scheduling patch

`tiny_http/` is the crates.io tiny_http 0.12.0 source under its upstream MIT/Apache-2.0 licenses. In `src/util/task_pool.rs`, queued jobs now reserve waiting workers when deciding whether to start another thread. Previously, a burst could enqueue more connections than there were waiting workers; HTTP keep-alive readers then occupied every worker indefinitely, leaving another connection unanswered.

`scripts/verify_preview_connections.py` exercises simultaneous keep-alive connections against the actual SDK CLI, including a paused accept loop to reproduce a connection burst. Other upstream source is unchanged.

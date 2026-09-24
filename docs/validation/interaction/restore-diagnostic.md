# Restore request diagnostic

The 2026-09-24 diagnostic reports contain two long restore requests. With the
default HTTP cache, one object took 16,901.5 ms from CDP request to finish and
was marked `fromDiskCache=true`; its reported header interval ended after
46.3 ms. With the cache disabled, a pair of objects each took about 9,341 ms
and transferred 728 and 885 encoded bytes. One response's headers arrived
within 1 ms, while the other reported a 3,060 ms header wait. These timings
locate waits along the fetch path, but do not identify whether the server,
browser network service, renderer delivery, or host scheduling caused them.
Disabling the cache did not remove the tail.

`RestoreSession` asks for complete canonical units even when their keys are
resident in the live player. The player installs each unit into an independent
empty scratch view and checks it before producing a single-use proof. Reusing
live content as the proof source would change that verification contract, so
the current diagnostic adds no cache or restore bypass.

## Correlate a request

Set `NIR_SERVE_TIMING=1` on `novelc serve` (or `novelc dev`). Each static request
then emits `nir_serve` lines with an ID, object hash path, wall clock time in
microseconds, elapsed time since acceptance, and byte count. The `received`,
`ready`, and `done` phases bracket path lookup/open and `request.respond`.
The response also carries `X-NIR-Serve-Id` while timing is enabled.

Set `NIR_PERF_NETWORK_TRACE=1` for `interactions.spec.js`. Its CDP request row
records `wallTime` in Unix seconds; the response row records `serveId`. Match
the latter to the server log ID. Convert CDP event timestamps to wall time by
adding their difference from the request event's monotonic `at` to `wallTime`.
If CDP records a request but the server's `received` line is late, inspect
browser dispatch and server accept queue. A large server `ready` or `done`
interval points to filesystem work or sending. If `done` is prompt but CDP
completion is late, inspect browser response delivery and renderer scheduling.
The wall clocks must be on the same machine or synchronized.

```sh
node tests/performance/correlate-serve-timing.mjs \
  reports/performance-interactions.json reports/<round>-server.log 12 \
  reports/<round>-pressure.json
```

The output lists the longest object requests with server lookup/send intervals
and the delay between server acceptance and CDP finish. A cache hit replays
the cached response header without a new server request; the correlator ignores
its `X-NIR-Serve-Id`. Set `NIR_PERF_PRESSURE_LOG` to the pressure JSON
path during the browser run to sample Linux I/O and memory PSI totals every
500 ms. The join reports system-wide I/O stall growth across each request;
these totals are context, not per-process attribution.

The scale fixture accepts `NIR_PERF_SERVE_MODE=node` to serve the same build
with an independent Node implementation. The default is `novelc`. Set
`NIR_PERF_SERVE_CLI` to a newly built server binary if fixture compilation
uses a preserved baseline `NIR_PERF_CLI`. Set
`NIR_PERF_SERVE_LOG=reports/<round>-server.log` for each round; the fixture
captures both server output streams there before deleting its temporary build.
Use `NIR_SERVE_TIMING=1` with either server to include per-request timing and
the correlation response header. The report records the mode and log path.

## Four browser rounds

The same baseline CLI/SDK built the 32-module fixture for each round. Each run
used one Chromium context, 30 restores, the same WebGPU hardware check,
runtime diagnostics, CDP Network/Tracing, and 500 ms Linux PSI sampling.
Playwright's trace was off. Only the named server or Chromium temporary-file
setting changed. Raw reports, compressed traces, server logs, and PSI samples
are under `reports/m3-<round>-diagnostic/`. Tracked scalar results and artifact
SHA-256 digests are in [restore-summary.json](restore-summary.json).

| Round | Server | Chromium temporary files | Restore median | P95 | Maximum | Over 1 s |
|---|---|---|---:|---:|---:|---:|
| `novelc` | timed `novelc serve` | default `/tmp`, disk-backed shared memory | 254.05 ms | 13,341.8 ms | 14,299.0 ms | 4/30 |
| `node` | independent Node static server | default `/tmp`, disk-backed shared memory | 263.75 ms | 16,658.3 ms | 19,322.1 ms | 3/30 |
| `tmpfs` | timed `novelc serve` | `TMPDIR=/dev/shm/nir-m3-profile` | 205.6 ms | 224.6 ms | 227.9 ms | 0/30 |
| `native-shm` | timed `novelc serve` | `TMPDIR=/tmp`, Chromium native `/dev/shm` | 205.05 ms | 243.2 ms | 333.9 ms | 0/30 |

The first three rounds used Playwright's `--disable-dev-shm-usage` default; the
fourth explicitly removed that flag with `NIR_PERF_NATIVE_SHM=1`. The hardware
runner now defaults to native shared memory on Linux. To reproduce the first
three diagnostic conditions, set `NIR_PERF_NATIVE_SHM=0` explicitly; use
`NIR_PERF_TEMP_STORAGE=memory` for a runner-managed tmpfs directory. This
new default is a measurement-environment correction and does not alter
production Chromium launch or HTTP cache policy.

All four rounds passed save consistency, cleanup, and adapter checks. The
server switch did not remove the long tail. The two slowest object fetches in
the `novelc` round took about 13.16 s; the slowest in the Node round took
16.05 s. All were CDP disk-cache hits with zero transferred bytes. Cached
responses replay the original `X-NIR-Serve-Id`, so their header ID must not be
matched to a live server log entry. Each round had only 107 live object
requests; the remaining repeated requests came from Chromium's HTTP cache.
The independent Node server reproduced long tails without `novelc serve`.
For live object requests, the maximum server acceptance-to-send completion
was 2.0 ms with `novelc` and 7.0 ms with Node. Those measurements do not time
cached requests, which never reach either server.

During the longest 13.16 s request in the `novelc` round, system-wide I/O PSI
`full` grew by 9.49 s across enclosing 500 ms samples. During the longest
16.05 s Node request, it grew by 11.31 s. These overlapping stalls support a
temporary-storage I/O hypothesis but do not attribute pressure to Chromium
alone. Moving Chromium temporary files to tmpfs eliminated >1 s samples in
this small run. Enabling native shared memory while keeping `TMPDIR=/tmp` did
the same, making disk-backed Chromium shared memory a stronger candidate than
the HTTP disk cache alone. The two fast rounds were later in time, and other
system load was not controlled; replication is needed before treating this as
a general performance guarantee. Neither production cache policy nor restore
verification was changed.

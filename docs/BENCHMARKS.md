# Performance and Resource Benchmarking

The workspace includes a permanent, opt-in benchmark runner for measuring
changes before optimizing them. It uses only dependencies already required by
the application and does not ship in the `tz-player` binary.

## Coverage

| Scenario | Representative workload | Reported resources |
|---|---|---|
| `analysis` | Envelope, waveform proxy, beat detection, and 48-band spectrum over deterministic decoded PCM | Median/p95 latency, audio throughput, Rust allocations, allocated bytes, and peak live heap |
| `database` | Count, viewport fetch, sorted fetch, and FTS search over a 10,000-track SQLite playlist | Median/p95 latency, query throughput, peak live heap, DB bytes, and bytes/track |
| `disk` | Persist a fixed four-minute track's scalar, spectrum, beat, and waveform cache products; inspect compiled executables | Total/incremental cache bytes, bytes/audio-minute, runner size, and player size when built |
| `tui` | Production draw functions on Ratatui's headless backend with basic, spectrum, and hidden-visualizer idle frames | Median/p95 frame time, frames/s, Rust allocations, allocated bytes, and peak live heap |

Every scenario records process resident memory before and after it, plus the
process high-water mark where the operating system exposes it. Because the
high-water mark cannot be reset portably, compare runs with identical scenario
selection and order.

## Run the suite

Always use a release build for numbers you intend to compare:

```powershell
cargo run --release -p tz-bench -- run
```

Build the release player first when you want the `disk` scenario to include its
executable size:

```powershell
cargo build --release -p tz-player
cargo run --release -p tz-bench -- run --scenario disk
```

Results are printed as a table and written to a timestamped JSON artifact under
`.local/perf_results/`, which is git-ignored. Run one or more focused scenarios:

```powershell
cargo run --release -p tz-bench -- run --scenario analysis
cargo run --release -p tz-bench -- run --scenario database --scenario tui
```

### Presets for constrained hardware

`standard` uses 30 seconds of decoded audio, 10,000 playlist rows, and 25
samples per timed case. On an old or low-memory machine, use the `ancient`
preset: 10 seconds of audio, 2,000 playlist rows, an 80x24 terminal, and nine
samples. It retains every scenario and metric while reducing the runner's own
memory and CPU demand:

```powershell
cargo run --release -p tz-bench -- run --preset ancient
```

Compilation needs substantially more RAM than the benchmark itself. On a
machine that is too constrained to compile the workspace comfortably, build
`tz-bench` on a compatible machine and copy the release executable. If the old
machine must compile locally, serialize Cargo's work before running it:

```powershell
$env:CARGO_BUILD_JOBS = "1"
cargo build --release -p tz-bench
.\target\release\tz-bench.exe run --preset ancient
```

Keep the same target architecture and CPU-feature settings when moving a
binary between machines. Benchmark results from different builds or targets
should not be compared as if only the hardware changed.

Use `smoke` to verify the harness after changing benchmark code. Its three
seconds of audio, 500 playlist rows, and five samples are intended for
correctness rather than performance conclusions:

```powershell
cargo run --release -p tz-bench -- run --preset smoke
```

`--quick` remains a shorthand for `--preset smoke`:

```powershell
cargo run --release -p tz-bench -- run --quick
```

Useful controls:

```powershell
cargo run --release -p tz-bench -- list
cargo run --release -p tz-bench -- run --samples 40 --label before-change
cargo run --release -p tz-bench -- run --output .local/perf_results/baseline.json
```

## Compare two runs

Run the same scenarios, profile, and fixture sizes on the same otherwise-idle
machine and compare their artifacts:

```powershell
cargo run --release -p tz-bench -- compare `
  .local/perf_results/baseline.json `
  .local/perf_results/candidate.json
```

The default classification threshold is 5% for the median. Every JSON metric
is lower-is-better: latency, allocation count, and allocated bytes. Throughput
is retained as a counter for context and is not incorrectly classified using
that rule. Override the threshold with `--threshold 3` when a workload is stable
enough to support it.

The artifact schema matches version 1 of the Python reference's opt-in perf
artifacts. The comparison command can therefore read reference artifacts, but
only common metrics produced from equivalent configurations are meaningful.
The deterministic Rust scenarios are primarily intended for Rust branch and
commit comparisons; local-media and live-playback comparisons require a shared
corpus and separate controlled runs.

## Interpreting resource numbers

- Allocation metrics count calls and requested bytes through Rust's global
  allocator during the measured operation. They include returned result data.
- Peak-live metrics track the largest increase in live Rust heap bytes above
  the sample's starting point. This distinguishes temporary working memory
  from cumulative allocation churn.
- SQLite, VLC, Rodio, operating-system audio, and other native code may allocate
  outside Rust's allocator. Those allocations are not attributed per operation.
- Resident-memory snapshots cover the entire benchmark process, including the
  runner and fixtures. Compare only runs with the same selected scenarios.
- Playlist disk usage includes the SQLite database and active sidecars. The
  cache footprint always stores all four products for the same synthetic
  four-minute track and reports exact bytes plus bytes/audio-minute.
- The projected cache size for 1,000 four-minute tracks is a linear planning
  estimate. SQLite page packing and real track lengths will change the result.
- Release player size excludes external VLC/FFmpeg installs, music, config,
  logs, and platform packaging overhead.
- Hosted CI machines are noisy, so CI compiles and tests the harness but does
  not enforce timing thresholds. Record baselines on representative hardware.
- Battery and real playback CPU/RSS need longer OS-level observations with the
  same media, backend, audio device, volume, and sample window. Do not infer
  those costs from the headless microbenchmarks.

## Optimization workflow

1. Build and run the full relevant scenario two or three times on an idle
   machine; keep the median-looking artifact as the baseline.
2. Make one focused optimization.
3. Re-run with the same toolchain, power mode, scenarios, and inputs.
4. Compare artifacts and confirm that latency and resource changes agree.
5. Run the normal correctness and quality gates before keeping the change.

Benchmark artifacts are evidence, not correctness tests. Never loosen bounded
analysis limits, cache validity, or playback behavior solely to improve a
number.

For hardware that may not comfortably run `standard`, establish and retain an
`ancient` baseline instead. Do not compare `ancient`, `standard`, and `smoke`
artifacts against one another; their fixture sizes intentionally differ.

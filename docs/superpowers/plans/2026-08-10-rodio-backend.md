# Experimental Rodio Playback Backend Implementation Plan

Status: Ready for review
Date: 2026-08-10
Spec: `docs/superpowers/specs/2026-08-10-rodio-backend-design.md`

## Goal

Add an opt-in `RodioPlaybackBackend` beside VLC and Fake, validate it against
the existing playback contract and representative music formats, and record
enough evidence to decide later whether Rodio should become the default. VLC
remains the default throughout this plan.

## Constraints

- Preserve existing VLC and Fake behavior and tests.
- Use the existing `PlaybackBackend` boundary; the TUI must not import Rodio,
  Symphonia, or CPAL.
- Stream files rather than loading complete tracks into memory.
- Do not require an audio device for normal unit tests or hosted CI.
- Do not silently switch between Rodio and VLC.
- Keep each task in a focused explanatory commit. If a task uncovers an
  unrelated problem, commit that fix separately or defer it.
- Do not change the default backend or remove VLC without a later approval and
  ADR.

## Proposed commit sequence

### Task 1 — Record the additive architecture decision and dependency policy

**Files:**

- Add `docs/adr/ADR-0003-add-experimental-rodio-backend.md`
- Modify workspace `Cargo.toml`
- Modify `crates/tz-playback/Cargo.toml`
- Update `Cargo.lock`
- Modify `deny.toml` only if an explicitly reviewed license allowance is needed

**Work:**

- Record that ADR-0001's VLC-only listen-path decision is amended only to allow
  an experimental Rodio backend; its crate boundaries and analysis split remain.
- Add Rodio with default features disabled and only playback, tracing,
  Symphonia stable-all, and stable SIMD features enabled.
- Confirm the resolved CPAL/Symphonia versions preserve Rust 1.88 support.
- Inspect the complete new dependency and license graph before source work.
- Do not add a direct CPAL dependency unless doctor/device functionality cannot
  be expressed through Rodio; document the reason if it becomes necessary.

**Tests/verification:**

```text
cargo generate-lockfile --offline
cargo check -p tz-playback --all-targets
cargo audit --locked
cargo deny check
```

Use an online lockfile update only if the crates are not already cached.

**Commit:** `build(tz-playback): add the reviewed Rodio playback stack`

### Task 2 — Add a hardware-independent Rodio transport core

**Files:**

- Add `crates/tz-playback/src/rodio_backend.rs`
- Add `crates/tz-playback/src/rodio_engine.rs` if separating device ownership
  from the public backend keeps tests focused
- Modify `crates/tz-playback/src/lib.rs`
- Add tiny generated/redistributable test fixtures only if runtime generation
  is insufficient

**Work:**

- Introduce an internal transport/output seam that can be exercised without a
  physical audio device.
- Model Idle, Loading, Playing, Paused, Stopped, and Error transitions.
- Track current item, position, duration, volume, speed, explicit stop, and the
  one-shot natural-end latch.
- Decode generated WAV through Rodio/Symphonia and prove duration and seek
  semantics without opening the system device.
- Convert all internal failures into `PlaybackError`; do not panic on media or
  worker input.

**Tests:**

- Valid and corrupt WAV decode.
- Replacing an active source clears the prior state.
- Pause/resume, seek/clamp, volume, speed, explicit stop, and natural end.
- Natural end is published exactly once.
- Failed commands do not mutate the last valid snapshot.

**Verification:** `cargo test -p tz-playback rodio_engine`

**Commit:** `feat(tz-playback): model Rodio transport without audio hardware`

### Task 3 — Implement the Rodio worker and `PlaybackBackend` contract

**Files:**

- Modify `crates/tz-playback/src/rodio_backend.rs`
- Modify `crates/tz-playback/src/rodio_engine.rs`
- Modify `crates/tz-playback/src/backend.rs` only if native path handling needs
  a backend-wide correction
- Modify `crates/tz-playback/src/events.rs` only for a backend-neutral event fix

**Work:**

- Spawn a named worker that owns the output stream, player, and active decoder.
- Use bounded command/response channels and explicit startup/shutdown handshakes.
- Implement every `PlaybackBackend` method, including transport snapshot reads.
- Open local files lazily, preserve native paths, attach one source at a time,
  and apply stored volume/speed before playback becomes visible.
- Poll Rodio state cheaply, latch final position/duration, and distinguish
  natural completion from explicit Stop.
- Emit existing backend events consistently and report device/worker failure.
- Join the worker on shutdown and in Drop-safe cleanup without an unbounded wait.

**Tests:**

- A backend contract suite shared where practical with Fake.
- Startup failure, command-channel loss, and repeated shutdown.
- Unicode path; non-UTF-8 path where the target supports one.
- Natural-end state is compatible with `PlayerService`'s polling contract.

**Verification:** `cargo test -p tz-playback rodio_backend`

**Commit:** `feat(tz-playback): implement the Rodio playback worker`

### Task 4 — Integrate Rodio selection, persistence, and fallback

**Files:**

- Modify `crates/tz-playback/src/lib.rs`
- Modify `crates/tz-player/src/main.rs`
- Modify `crates/tz-core/src/player.rs`
- Modify `crates/tz-core/src/runtime.rs`
- Modify `crates/tz-core/src/state.rs`
- Modify relevant CLI/runtime/state tests

**Work:**

- Add `BackendKind::Rodio` and `BackendCli::Rodio` with stable string `rodio`.
- Add the Rodio engine variant to `PlayerService` without backend-specific
  behavior leaking into transport operations.
- Persist and restore Rodio using the existing state schema; no migration is
  required for the string field.
- Preserve VLC as `Default` and as the fallback for invalid old state strings.
- On Rodio startup failure, activate Fake and surface the requested/effective
  backend distinction in the persistent error.
- Do not fall from Rodio to VLC on startup or per-track failure.
- Generalize VLC-specific `PlaybackError` names/comments where they now refer
  to any real backend, while retaining precise VLC discovery errors.

**Tests:**

- CLI parsing and help snapshot.
- BackendKind parse/as_str/default.
- State round trip for vlc, rodio, and fake; unknown strings retain current
  fallback behavior.
- Rodio startup failure produces Fake plus a persistent error.
- Existing VLC startup/fallback tests remain unchanged.

**Verification:**

```text
cargo test -p tz-playback
cargo test -p tz-core player
cargo test -p tz-core state
cargo test -p tz-player
```

**Commit:** `feat(tz-core): select and persist the Rodio backend`

### Task 5 — Prove format and end-of-track behavior

**Files:**

- Add or extend tests under `crates/tz-playback/tests/`
- Add or extend `tz-core` player tests
- Add small licensed fixtures under a clearly documented test-fixture directory
  only when deterministic generation is not possible

**Work:**

- Cover the required common set: MP3, FLAC, WAV, Ogg Vorbis, AAC/M4A, and ALAC.
- Add representative AIFF/CAF/MKA cases where a tiny redistributable fixture can
  be produced reliably.
- Verify known unsupported or corrupt inputs fail promptly and leave the backend
  ready for the next supported track.
- Exercise source position after forward/backward seek and at 0.5x, 1x, 2x, and
  4x. Assert tolerances rather than callback-exact wall-clock timing.
- Prove repeat Off/One/All, shuffle, Next, and Previous continue to work and
  natural end advances only once.
- Document fixture provenance and keep binaries small.

**Verification:**

```text
cargo test -p tz-playback --test rodio_formats
cargo test -p tz-core rodio
```

**Commit:** `test(tz-playback): verify Rodio formats and track transitions`

### Task 6 — Add doctor diagnostics and silent/manual smoke tools

**Files:**

- Modify `crates/tz-player/src/main.rs`
- Add `crates/tz-playback/examples/rodio_smoke.rs`
- Add example tests or argument-parser tests as appropriate

**Work:**

- Make `doctor --backend rodio` open and close the default output stream without
  playing audio, then report success/failure and supported format families.
- Avoid running VLC discovery merely because Rodio was selected.
- Add `rodio_smoke --startup-only` for silent device initialization.
- Add an explicit file argument for manual transport playback; do not hide an
  audible test behind the startup-only option.
- Sanitize all printed paths and backend errors.

**Tests:** argument parsing and output helpers are hardware-independent. The
actual output-device probe remains an opt-in/manual test.

**Verification:**

```text
cargo test -p tz-player doctor
cargo run -p tz-playback --example rodio_smoke -- --help
cargo run -p tz-playback --example rodio_smoke -- --startup-only
```

**Commit:** `feat(tz-player): diagnose and smoke-test Rodio output`

### Task 7 — Extend CI and operational documentation

**Files:**

- Modify `.github/workflows/ci.yml`
- Modify `README.md`
- Modify `docs/usage.md`
- Modify `docs/architecture.md`
- Modify `docs/SPEC.md`
- Modify `docs/SECURITY.md`
- Modify `docs/RELEASE.md`
- Modify `docs/PROGRESS.md`
- Modify `docs/CONVERSION_PLAN.md` where its historical decision would
  otherwise be misleading

**Work:**

- Install the ALSA development package in Linux CI before compiling Rodio.
- Preserve Windows, Linux, and macOS compile/test coverage.
- Document `--backend rodio`, VLC default status, runtime requirements, format
  differences, pitch-changing speed, and fallback behavior.
- Update the runtime trust boundary: Symphonia parses playback media in-process.
- Add Rodio dependency/audit checks and silent/manual smoke steps to the release
  checklist.
- Clearly distinguish playback requirements from optional FFmpeg analysis.

**Verification:** search documentation for stale claims that VLC is the only
real backend, then run all CLI/help/documentation-linked tests.

**Commit:** `docs: document the experimental Rodio backend`

### Task 8 — Run the compatibility evaluation and publish the result

**Files:**

- Add `docs/RODIO_EVALUATION.md`
- Modify `docs/TODO.md`
- Modify `docs/PROGRESS.md`
- Make focused source/test/doc fixes discovered by evaluation in separate
  commits before the final evaluation record

**Work:**

- Run the format/transport matrix from the spec on Windows and record results.
- Run silent startup or obtain CI evidence for Linux and macOS; do not claim
  audible playback where no real output device was tested.
- Compare startup, seek, rate, end transition, CPU/memory observations, binary
  size, dependency setup, and supported/unsupported formats with VLC.
- Record one recommendation: keep experimental, promote in a separate proposal,
  or reject/remove. Do not implement promotion in this task.
- Mark the TODO item complete only when the implementation and evaluation
  acceptance criteria pass.

**Commit:** `docs: record the Rodio backend compatibility evaluation`

### Task 9 — Full repository verification

**Files:** no planned changes. Any discovered fix receives a focused commit
associated with the task/issue it corrects.

**Verification commands:**

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo audit --locked
cargo deny check
```

Also manually run VLC and Rodio startup-only smoke checks on Windows and launch
the TUI once with each real backend. Confirm the header reports the effective
backend and that a failed requested backend produces one persistent actionable
error while Fake remains usable.

**Commit:** none unless verification reveals a focused fix.

## Completion and decision boundary

Completing this plan means Rodio is an available, documented, evaluated option.
It does not mean Rodio is the default. Promotion requires a separate proposal
that cites `docs/RODIO_EVALUATION.md`, states the supported-format policy, and
decides whether VLC remains a compatibility backend.

# Bundled Audio Engine Migration Implementation Plan

**Status:** Ready for implementation

**Date:** 2026-08-10

**Specification:** [`AUDIO_ENGINE_MIGRATION_SPEC.md`](AUDIO_ENGINE_MIGRATION_SPEC.md)

**Audience:** An implementation agent with repository access and no chat history

## 1. Mission and working rules

Implement the specification completely: make the composite Audio backend the
default, use native Symphonia decoding first and a package-relative
FFmpeg-backed helper second, migrate offline analysis to the same policy,
package all required native files, and remove VLC from the finished product.

Do not reinterpret `docs/TEMP_NOTES.md` as a competing design. It is the source
discussion; the companion specification is the resolved target.

Before editing:

1. Read the companion specification in full.
2. Read `docs/architecture.md`, `docs/RODIO_EVALUATION.md`, ADR-0003,
   `crates/tz-playback/src/rodio*.rs`, `crates/tz-analysis/src/decode.rs`,
   `crates/tz-core/src/levels.rs`, `scripts/package-release.ps1`,
   `docs/SECURITY.md`, and `docs/LICENSING.md`.
3. Run `git status --short` and preserve user changes. At the time this plan was
   written, `docs/TEMP_NOTES.md` was an untracked user file; do not delete or
   rewrite it.
4. Establish the baseline with the normal quality gates. Record pre-existing
   failures rather than hiding them.

Use test-first, focused commits. Do not begin the next task with a failing gate
from the current task. Do not remove VLC until the new packaged path passes the
comparison gates in Task 10. Do not publish or upload a release as part of
implementation unless the maintainer separately asks.

## 2. Dependency order

```text
Task 1 decision/baseline
  -> Task 2 protocol
      -> Task 3 binding + pinned FFmpeg build
          -> Task 4 helper decoder
              -> Task 5 parent client/discovery
                  +-> Task 6 native analysis and routing
                  +-> Task 7 helper-backed playback
                       -> Task 8 composite backend/state
                           -> Task 9 package/licensing
                               -> Task 10 default switch + VLC removal
                                   -> Task 11 CI/docs/final verification
```

Tasks 6 and 7 may be developed on separate commits once Task 5 is stable, but
they must not edit the same source concurrently.

## 3. Task sequence

### Task 1 — Record the new decision and protect the existing baseline

**Files**

- Add `docs/adr/ADR-0004-adopt-bundled-audio-engine.md`.
- Update `docs/adr/README.md`.
- Add no runtime dependency yet.

**Work**

- Record the final choices from the specification: composite Audio backend,
  native-first/helper-second, child-process FFmpeg boundary, one package,
  analysis cache retained, Fake retained, and VLC removed only after proof.
- Mark ADR-0003 superseded only for default/experimental/backend-removal
  conclusions. Preserve it as history and evidence for the working Rodio code.
- Capture baseline binary size and run time for the current Rodio startup/file
  smoke so later comparison is meaningful.
- Confirm the committed fixture licenses/provenance before adding more media.

**Tests and evidence**

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p tz-playback --test rodio_formats --locked
```

**Commit:** `docs(adr): adopt the bundled composite audio engine`

### Task 2 — Add PCM types and the versioned helper protocol

**Files**

- Add `crates/tz-audio/Cargo.toml`.
- Add `crates/tz-audio/src/lib.rs`.
- Add `crates/tz-audio/src/pcm.rs`.
- Add `crates/tz-audio/src/protocol.rs`.
- Add the crate to workspace `Cargo.toml` and workspace dependencies.

**Work**

- Implement `PcmSpec`, finite/clamped sample validation, frame/time conversion,
  duration metadata, and a streaming `PcmSource` contract matching spec §8.
- Implement protocol-major 1 capability and decode headers from spec §10.
- Encode decode headers as `<u32 little-endian JSON length><JSON>` and enforce
  the 64 KiB maximum before allocation.
- Accept unknown minor-version JSON fields and reject a different major.
- Centralize exit-code meanings and bounded diagnostic text.
- Keep protocol code independent of Symphonia, Rodio, and FFmpeg so the helper
  can reuse it without pulling those implementations into its binary.
- Add optional `native`/`client` crate features only when later tasks need them;
  keep default feature behavior explicit.

**Tests first**

- valid/unknown-minor header round-trip;
- major mismatch, oversized length, truncated JSON, invalid JSON, invalid rate,
  channel count, format, duration/start, and request/header mismatch;
- finite/clamped PCM, frame alignment, overflow-safe frame/time conversions;
- capabilities serialization contains version/configuration identity; and
- error/diagnostic truncation is Unicode-safe.

**Verification**

```text
cargo test -p tz-audio --locked
cargo clippy -p tz-audio --all-targets --locked -- -D warnings
```

**Commit:** `feat(tz-audio): define bounded PCM and helper protocol`

### Task 3 — Pin and automate the LGPL FFmpeg build

This task is a build/licensing gate. Do not write the full helper against an
unreproducible local SDK.

**Files**

- Add `native/ffmpeg/manifest.toml` with exact source URL, version/tag, commit,
  SHA-256, enabled components, expected library majors, and binding version.
- Add `native/ffmpeg/build.ps1` for Windows.
- Add `native/ffmpeg/build.sh` for Linux/macOS, or separate target scripts when
  flags differ.
- Add `native/ffmpeg/README.md` explaining prerequisites and outputs.
- Add a script that emits `FFMPEG_BUILD.json`, `FFMPEG_CHANGES.diff`, and the
  enabled component lists from the actual build.
- Update `.gitignore` for generated SDK/build output only; never ignore the
  manifest, patches, or notices.

**Work**

1. Choose an exact stable FFmpeg release and verify its official checksum.
2. Start with the matching `ffmpeg-next` release. Prove it compiles with Rust
   1.89 on Windows, Linux, and macOS against the selected shared libraries.
3. Prove the binding can provide or safely expose custom AVIO callbacks. If it
   cannot, use `ffmpeg-sys-next` only inside a narrow helper FFI module. Record
   the choice and license in ADR-0004 or an amendment.
4. Derive the exact demuxer/parser/decoder allowlist from that pinned FFmpeg
   source. Enable the tested native+fallback union from spec §7.1.
5. Configure shared-only LGPL libraries with GPL, nonfree, programs, network,
   protocols, devices, filters, encoders, muxers, video output, and automatic
   external dependencies disabled. Enable only avcodec, avformat, avutil,
   swresample, and the audited audio input components.
6. Fail the build if configuration output contains `--enable-gpl`,
   `--enable-nonfree`, an unexpected external library, or an unexpected native
   dependency.
7. Make outputs deterministic enough that manifest/configuration changes are
   reviewable; store hashes rather than checking large generated binaries into
   Git.

Do not copy a third-party prebuilt “latest” FFmpeg archive into releases. Build
from the pinned source so matching source and configuration can be published.

**Tests and evidence**

- clean build from the pinned source on the first Windows x86-64 target;
- helper-link spike that calls library version/configuration functions;
- `cargo check` for the binding/helper skeleton at Rust 1.89;
- dump enabled decoders/demuxers and compare them to the allowlist;
- inspect dynamic dependencies (`dumpbin`, `ldd`/`readelf`, `otool`) and reject
  non-system libraries outside the package plan;
- `cargo deny` and `cargo about` review of the binding crates.

**Commit:** `build(audio): pin the redistributable FFmpeg SDK`

### Task 4 — Implement `tz-audio-decoder`

**Files**

- Add `crates/tz-audio-decoder/Cargo.toml`.
- Add `crates/tz-audio-decoder/src/main.rs`.
- Add focused modules such as `ffi.rs`, `avio.rs`, `decode.rs`, and
  `diagnostics.rs` rather than one large unsafe file.
- Add the helper to the workspace but not to default members.
- Add tiny helper-only media fixtures and update fixture provenance.

**Work**

- Implement exactly `capabilities --json` and `decode` from spec §10.
- Parse the input as `PathBuf`; open it with `std::fs::File` before entering
  FFmpeg.
- Wrap that file in a seekable custom AVIO context. Handle normal read, EOF,
  absolute/relative/end seek, and `AVSEEK_SIZE`; never unwind across callbacks.
- Select one best audio stream, decode with send/receive, resample with
  libswresample to requested stereo `f32le`, and stream complete frames.
- Implement source-time start seek, timestamp rescaling, decoder/resampler
  flush, and leading-frame discard.
- Ignore attached pictures/video/subtitles/data. Reject no-audio inputs.
- Emit the header before PCM, cap diagnostics, map errors to the specified exit
  codes, and treat a parent-closed pipe as clean cancellation.
- Use RAII wrappers for every FFmpeg allocation. Confine and document each
  `unsafe` block with its ownership/thread invariant.
- Install a panic boundary in `main`; unexpected failures exit 70 without a
  backtrace or unbounded terminal text.

**Tests first**

- custom AVIO read/seek/size on spaces, Unicode, and Unix non-UTF-8 paths;
- valid decode header and PCM for one WAV fixture;
- duration and start seek within one output frame/tolerance;
- every helper-only fixture in spec §7.1;
- corrupt, truncated, empty, no-audio, unreadable, and huge-metadata inputs;
- parent closes stdout early;
- no network/protocol path is accepted;
- FFmpeg allocations/process exit stay clean under repeated decode; and
- `capabilities` matches the pinned manifest and actual library versions.

Use integration tests that invoke the built helper by an explicit path; do not
put it on `PATH`.

**Verification**

```text
cargo test -p tz-audio-decoder --locked
cargo clippy -p tz-audio-decoder --all-targets --locked -- -D warnings
```

Run the repository’s malicious-media and license gates as well.

**Commit:** `feat(audio-helper): stream bounded PCM through FFmpeg`

### Task 5 — Implement package-relative helper discovery and client lifecycle

**Files**

- Add `crates/tz-audio/src/helper.rs`.
- Add `crates/tz-audio/src/discovery.rs`.
- Add test-only fake-helper binaries/scripts under a dedicated test fixture
  directory.
- Expose only format-neutral types from `tz-audio/src/lib.rs`.

**Work**

- Resolve the release helper from the main executable using the platform paths
  in spec §9. Never search `PATH` or the working directory.
- Allow tests to inject a helper path through a constructor. If a developer
  environment variable is necessary, compile it only for debug builds and
  require an absolute path.
- Spawn without a shell; use OS-native arguments, null stdin, piped stdout, and
  bounded stderr. Hide the Windows console window.
- Perform and cache a 5-second capability handshake. Require compatible
  protocol/library/manifest identity.
- Parse/validate the decode header before publishing a source.
- Drain PCM on a dedicated reader into a bounded queue. Provide EOF, underrun,
  error, and cancellation signals separately.
- Implement kill/reap on stop, replacement, seek, timeout, protocol error,
  panic, and Drop. A seek restarts the helper at the requested source time;
  never leave the prior child alive.
- Cap stderr and sanitize it before composing errors.
- Expose structured diagnostics used later by doctor.

**Tests first**

Create deterministic fake helpers for:

- valid capability + PCM;
- incompatible major;
- oversized/truncated/mismatched header;
- partial PCM frame and NaN/infinite sample;
- delayed startup and permanent PCM stall;
- stderr flood and terminal-control payload;
- nonzero exit before/after the header;
- ignores/handles closed pipe; and
- records its PID so tests prove the process was reaped.

Also test package paths with spaces/Unicode, missing helper, relative-path
rejection for injected paths, a fake system `ffmpeg` on `PATH`, queue bounds,
and repeated seek/replacement/shutdown.

**Verification**

```text
cargo test -p tz-audio --locked
cargo clippy -p tz-audio --all-targets --locked -- -D warnings
```

**Commit:** `feat(tz-audio): manage the bundled decoder process safely`

### Task 6 — Add native Symphonia analysis and helper fallback

**Files**

- Add `crates/tz-audio/src/native.rs` and `selection.rs`.
- Modify `crates/tz-analysis/Cargo.toml`.
- Modify `crates/tz-analysis/src/decode.rs` and exports.
- Modify `crates/tz-core/src/levels.rs` only if an injected decoder factory is
  needed.
- Bump `ANALYSIS_VERSION` in the four `tz-db` analysis stores from 1 to 2.

**Work**

- Implement streaming native decode through direct Symphonia APIs in
  `tz-audio`; do not depend on `tz-playback` or open an output device.
- Preserve native paths, duration, channel layout, rate, seeking, and bounded
  error text.
- Implement the routing rules in spec §7: native first, helper only for
  inconclusive media/codec initialization failures, never for a conclusive
  missing/permission error.
- Replace native-WAV/FFmpeg-CLI selection in `tz-analysis` with `tz-audio`.
- Retain `DecodedAnalysisAudio`, 44,100 Hz stereo, 11,025 Hz mono, the current
  DSP functions, byte/duration/time limits, and single-decode/multiple-product
  behavior.
- Remove or deprecate `ffmpeg_available`, `FfmpegCliDecoder`, and direct
  `Command::new("ffmpeg")`; no production call may remain.
- Make helper unavailability a normal analysis error. It must not affect native
  playback.
- Bump cache product versions so results rebuild lazily; do not change schema
  version or delete old rows in a migration.

**Tests first**

- all current native fixtures analyze without starting the fake helper;
- helper-only fixtures route once to the helper and create all four products;
- corrupt/both-failed error contains bounded native and helper context;
- missing/permission errors do not spawn helper;
- existing resource-limit/timeout tests pass for native and helper streams;
- old version-1 rows miss and version-2 rows hit; and
- one decode still populates every missing cache product.

Compare old/new DSP outputs for native fixtures using documented tolerances;
exact sample identity is not required if cache consumers remain stable.

**Verification**

```text
cargo test -p tz-audio -p tz-analysis -p tz-db -p tz-core --locked
cargo clippy -p tz-analysis -p tz-core --all-targets --locked -- -D warnings
rg -n 'Command::new\("ffmpeg"\)|ffmpeg_available|FfmpegCliDecoder' crates
```

The final search must find no production use.

**Commit:** `feat(tz-analysis): use native decode with bundled fallback`

### Task 7 — Feed helper PCM through the Rodio playback worker

**Files**

- Modify `crates/tz-playback/src/rodio_engine.rs`.
- Modify `crates/tz-playback/src/rodio_worker.rs`.
- Modify `crates/tz-playback/src/rodio.rs`.
- Add a helper PCM `rodio::Source` adapter module if that keeps ownership clear.
- Extend `crates/tz-playback/tests/rodio_formats.rs` and fixture provenance.

**Work**

- Preserve the current native Rodio path initially; on native initialization
  rejection, request a helper PCM stream from `tz-audio`.
- Adapt the bounded helper queue to a Rodio Source without blocking the audio
  callback on process I/O. Track underrun separately from EOF.
- Apply existing timeline tracking and the 50 ms stereo peak tap after either
  decoder route, before speed adjustment, so public position remains source
  time and live levels behave identically.
- For helper seek, acknowledge success only after the old child is reaped, the
  new child passes its header, and Rodio accepts the replacement source at the
  requested position.
- Preserve volume/speed before observable playback, pause/resume, explicit
  stop position zero, final duration latch, error recovery on the next track,
  and exactly-once natural end.
- Never fall back from a native source after mid-stream playback has started.
- Surface route and helper process failures through existing backend events and
  sanitized errors.

**Tests first**

- helper-only Opus decode through a hardware-independent Rodio mixer;
- native fixtures do not spawn helper;
- helper pause/resume, forward/backward seek, stop, replacement, volume,
  0.5/1/2/4x speed, repeated speed changes, source position, and live levels;
- natural end exactly once and core Repeat Off/One/All + shuffle behavior;
- helper stall/crash/invalid PCM becomes Error and the next valid track clears
  it;
- stop/seek/drop prove child and reader are gone; and
- bounded underrun does not look like natural EOF.

Extend real muted-output opt-in tests to one helper-only fixture. Do not make
normal CI require hardware.

**Verification**

```text
cargo test -p tz-playback --locked
cargo test -p tz-playback --test rodio_formats --locked
cargo test -p tz-core --locked
```

**Commit:** `feat(tz-playback): play bundled helper PCM through Rodio`

### Task 8 — Introduce the composite Audio backend and migrate state

**Files**

- Rename/refactor `RodioPlaybackBackend` to `AudioPlaybackBackend` in
  `tz-playback` and `tz-core`.
- Modify `crates/tz-playback/src/lib.rs` and backend-kind tests.
- Modify `crates/tz-core/src/player.rs`, `runtime.rs`, and `state.rs`.
- Modify `crates/tz-player/src/main.rs` CLI parsing and diagnostic labels.
- Update tests that assert VLC/Rodio defaults.

**Work**

- Add `BackendKind::Audio`; make it the default.
- During this task only, VLC may remain a non-default comparison variant.
- Document `--backend audio`; accept hidden/deprecated `rodio` as an Audio
  alias. Preserve an actionable explicit-VLC removal path for Task 10.
- Sanitize persisted `vlc`, `rodio`, unknown, and missing backend values to
  `audio`; preserve `fake`; save the canonical new value.
- Ensure requested/effective fallback messages now name Audio and Fake.
- Make default CLI/TUI startup initialize Audio without VLC discovery.
- Make doctor use `tz-audio` helper diagnostics; stop treating system FFmpeg
  availability as a warning or capability.
- Keep setup text transitional until Task 10, but the default path must require
  neither installed tool.

**Tests first**

- enum parse/display/default;
- CLI canonical name and Rodio alias;
- all old/new persisted values and a missing field if backward compatibility
  requires `serde(default)`;
- default startup does not call VLC discovery or inspect `PATH`;
- Audio startup failure falls to Fake with requested/effective detail;
- per-track failure remains Audio Error, not Fake transport; and
- doctor detects missing/incompatible helper and exact package-relative helper.

**Verification**

```text
cargo test -p tz-playback -p tz-core -p tz-player --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

**Commit:** `feat(audio): make the composite engine the default`

### Task 9 — Package helper, FFmpeg libraries, licenses, and source metadata

**Files**

- Modify `scripts/package-release.ps1`.
- Modify `scripts/check-distribution-licenses.ps1`.
- Add native-dependency inspection scripts for Windows/Linux/macOS.
- Add a staged-package smoke script.
- Update `about.hbs`, `about.toml`, and generated
  `THIRD_PARTY_LICENSES.html`.
- Add/update `licenses/`, `FFMPEG_SOURCE.md`, and
  `NATIVE_DEPENDENCIES.md` inputs.

**Work**

- Build both `tz-player` and `tz-audio-decoder` with `--release --locked` for
  the selected target.
- Stage the exact layout from spec §16, including versioned FFmpeg libraries,
  build JSON, changes patch, license, source offer, README, and Rust notices.
- Teach the packager platform-specific helper/library paths and macOS bundle
  layout.
- Inspect the helper’s dynamic dependency closure. Copy every audited
  non-system dependency and fail on anything unexpected.
- Run the helper from the staged directory, not `target/release`, and require a
  matching capability/configuration result.
- Fail packaging if any required file, source URL, checksum, patch, library, or
  notice is missing. Never emit a player-only archive.
- Generate SHA-256 checksums for the binary archive and matching FFmpeg source
  archive. The script may prepare the source archive locally; it must not upload
  without separate authorization.
- Update the license gate to reject GPL/nonfree FFmpeg configuration and stale
  source/build metadata.

**Tests**

- package from a clean build and list the archive contents;
- extract under a path with spaces/Unicode and run doctor;
- temporarily remove each required library/file from a copied staging tree and
  prove doctor/packaging fails;
- put fake `ffmpeg` and VLC files on `PATH` and prove package smoke ignores
  them;
- dependency inspection reports only expected package/system libraries; and
- compare source/configuration hashes to `manifest.toml`.

Run `scripts/check-distribution-licenses.ps1` after regenerating notices. Review
the HTML diff; do not accept generated license output blindly.

**Commit:** `build(release): ship the complete bundled audio runtime`

### Task 10 — Prove parity, switch completely, and remove VLC

Do not start removal until Tasks 6–9 pass from a staged package.

**Files**

- Remove `crates/tz-playback/src/vlc.rs`, `vlc_engine.rs`, and `vlc_ffi.rs`.
- Remove VLC exports, backend variants, CLI choices, discovery/setup code,
  examples, tests, and `libloading` if unused.
- Update Cargo manifests and `Cargo.lock`.
- Remove VLC-specific release/license assets only when no longer needed by a
  different system dependency.
- Update media-extension tests to the tested native/helper union and remove
  MIDI.

**Comparison gate before deletion**

- Run the same playlist/transport workflow through current VLC and Audio for
  representative native and helper-only tracks.
- Confirm duration, seek tolerance, natural next advance, repeat/shuffle,
  volume/speed, shutdown, and cache visualizers.
- Record any intentional difference in a migration note.
- Create a reversible checkpoint commit before removing VLC code.

**Removal work**

- Delete VLC implementation and discovery; do not leave dead feature flags.
- Final `BackendKind` is Audio/Fake. `rodio` remains only the temporary alias.
- An explicit `--backend vlc` produces the spec’s actionable removed message.
- Remove VLC from architecture, setup, doctor, security, release, package, and
  license conclusions.
- Confirm no application behavior consults `VLC_PLUGIN_PATH`, Program Files,
  `libvlc`, or VLC executables.

**Tests and searches**

```text
rg -n -i 'libvlc|vlc_plugin_path|vlc\.exe|VlcPlaybackBackend' crates scripts Cargo.toml
rg -n 'BackendKind::Vlc|playback_backend: "vlc"' crates
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
```

Matches may remain only in historical ADR/evaluation/migration documentation or
tests specifically proving old-state migration and obsolete CLI errors.

**Commit:** `refactor(audio): retire the VLC playback backend`

### Task 11 — Cross-platform CI, documentation convergence, and final acceptance

**Files**

- Modify `.github/workflows/ci.yml` and release workflow(s).
- Update `README.md`, `docs/architecture.md`, `docs/usage.md`, `docs/SPEC.md`,
  `docs/CONVERSION_PLAN.md`, `docs/PROGRESS.md`, `docs/TODO.md`,
  `docs/RELEASE.md`, `docs/SECURITY.md`, `docs/LICENSING.md`,
  `NATIVE_DEPENDENCIES.md`, About templates, and relevant ADR indexes.
- Add a concise `docs/AUDIO_ENGINE_MIGRATION_RESULTS.md` with exact evidence,
  target/package hashes, format results, and known limitations.

**Work**

- Build/test the pinned helper and composite backend on Windows, Linux, and
  macOS. Cover x86-64 and macOS ARM64 at minimum; add other targets only when
  the release process can support and smoke them.
- Cache FFmpeg source/build artifacts by manifest hash, never by a floating
  version label. Verify hashes on cache restore.
- Run normal hardware-independent tests everywhere. Keep real-output tests
  opt-in/manual where runners have no device.
- Run staged-package clean-machine tests from spec §18.2. A package test must
  use the archive contents, not workspace binaries.
- Perform and record one human audible native and helper-only smoke on each
  supported OS before declaring its package supported.
- Rewrite permanent docs so users see one Audio engine and no VLC/FFmpeg setup.
  Historical documents remain historical but receive a clear superseded note
  where needed.
- Include the exact FFmpeg source/configuration/license statement and source
  asset link in About, package docs, and release/download instructions.

**Final verification**

Run from a clean tree with the pinned toolchain and SDK:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo audit --locked
cargo deny --locked check advisories bans licenses sources
./scripts/check-distribution-licenses.ps1
./scripts/package-release.ps1
```

Then execute all clean-package steps in spec §18.2 and map evidence to AC-01
through AC-10. Do not mark the migration complete when a required OS package is
untested; label that target unsupported or keep the work open.

**Commit:** `docs(ci): verify the self-contained audio packages`

## 4. Required test-fixture policy

All committed media must be tiny, generated or clearly redistributable, and
documented with:

- generation command/tool version;
- codec/container and duration;
- intended native/helper route;
- license/provenance; and
- SHA-256.

Do not commit private library tracks. Prefer one-second mono tones. Generate
fixtures with the pinned build when its enabled encoders allow it; because the
release FFmpeg build deliberately has no encoders, it is acceptable for a
development-only fixture generator to use a separate trusted FFmpeg tool as
long as playback/tests never depend on that tool and provenance records it.

## 5. Implementation cautions

- Do not route by extension alone.
- Do not run `ffmpeg`, VLC, or the helper through a shell.
- Do not let helper reads block the audio callback.
- Do not hold a mutex across a blocking child-process read or join.
- Do not acknowledge seek before the replacement stream is ready.
- Do not confuse an underrun, explicit stop, child crash, and natural EOF.
- Do not allocate from an untrusted wire-header length before enforcing 64 KiB.
- Do not allow NaN/infinite PCM into meters or DSP.
- Do not leak an old helper on track replacement or application shutdown.
- Do not silently accept an FFmpeg library/configuration different from the
  package manifest.
- Do not change the project MSRV, cache schema, DSP contract, or release license
  based on convenience without an explicit recorded decision.
- Do not delete historical evidence documents; label superseded conclusions.

## 6. Handoff checklist

When implementation is finished, report:

- commits/tasks completed;
- exact FFmpeg version/commit, binding, configuration hash, and source archive;
- supported target/package matrix;
- native/helper format-test matrix;
- clean-package test results;
- full quality/license-gate results;
- helper buffer/timeout measurements and any underruns;
- AC-01 through AC-10 status;
- known limitations or unsupported targets; and
- absolute paths to produced packages and migration-results documentation.

Do not claim success based only on workspace unit tests. The final unit of
delivery is the extracted package running without installed VLC or FFmpeg.

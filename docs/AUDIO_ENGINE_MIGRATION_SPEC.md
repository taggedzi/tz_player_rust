# Bundled Audio Engine Migration Specification

**Status:** Implementation target

**Date:** 2026-08-10

**Audience:** An implementation agent that has repository access but no chat history

**Companion plan:** [`AUDIO_ENGINE_MIGRATION_PLAN.md`](AUDIO_ENGINE_MIGRATION_PLAN.md)

## 1. Decision

Migrate `tz-player` to one composite audio engine that is shipped with the
application:

- Rodio/CPAL remains responsible for audio-device output and transport.
- Symphonia is the first-choice decoder for formats it supports.
- A bundled `tz-audio-decoder` child process, dynamically linked to a pinned,
  audio-only LGPL FFmpeg build, decodes formats Symphonia cannot handle.
- The same native-first/helper-second policy supplies offline PCM to
  `tz-analysis`.
- The existing analysis cache remains the source for spectrum, beat, waveform,
  and history-based visualizers. Playback and analysis may decode a track
  separately.
- The Fake backend remains for deterministic tests and no-audio fallback.
- VLC is retained only while the migration is being proven, then removed from
  the final runtime and release.

The final end-user contract is one archive or application bundle per supported
OS/CPU. A user extracts it and starts `tz-player`; they do not install VLC or
FFmpeg, edit `PATH`, find codec DLLs, or link native libraries.

This decision supersedes the “VLC default / Rodio experimental / FFmpeg on
`PATH`” conclusions in the current architecture, ADR-0003, release, setup, and
security documents. Those files must be updated during implementation, not
silently left contradictory.

## 2. Why this is the selected design

The repository already has a working Rodio backend with transport parity,
source-timeline position tracking, common-format fixtures, real-output smoke
tests, and a live PCM level tap. It also has a mature offline analysis and cache
pipeline. The migration should reuse those assets.

A child helper is deliberately preferred to linking FFmpeg into the main
player process. It keeps the FFmpeg parser/decoder crash boundary outside the
TUI, preserves useful process timeouts, restricts the exposed FFmpeg feature
set, and makes shared-library replacement visible. It also avoids making
Rodio’s device worker own FFmpeg’s C API directly.

The project deliberately accepts duplicate decode work: the listen path reacts
immediately while the analysis path builds or reads a persistent cache. Sharing
one PCM stream between playback and all analysis products would couple pause,
seek, buffering, cache completion, and failure recovery. That optimization is
not part of this migration.

## 3. Goals

The completed system must:

1. Run from one downloaded package without a separately installed VLC or
   FFmpeg runtime.
2. Keep the normal playback path in Rust for the common format matrix already
   proven in `docs/RODIO_EVALUATION.md`.
3. Automatically use the bundled helper for supported local files rejected by
   the native decoder, without asking the user to select a backend.
4. Build envelope, spectrum, beat, and waveform caches without consulting
   `PATH` or executing a system `ffmpeg`.
5. Preserve play, pause, resume, stop, seek, volume, speed, repeat, shuffle,
   next/previous, natural-end, source-position, and live-level behavior.
6. Stream playback with bounded buffers; never decode a complete track into
   memory for listening.
7. Preserve current analysis byte, duration, and execution-time limits.
8. Fail promptly and visibly when the package is incomplete, media is corrupt,
   a codec is unsupported, the helper hangs, or the output device is lost.
9. Keep original project code MIT while satisfying every license attached to
   the distributed FFmpeg libraries, Rust dependencies, and system libraries.
10. Build and test on Windows, Linux, and macOS, with Windows x86-64 as the
    first reference package rather than a Windows-only architecture.

## 4. Non-goals

This work does not add:

- network streams, internet radio, URL playback, HLS/DASH, CD/DVD/Blu-ray, or
  capture-device input;
- MIDI synthesis or a bundled SoundFont; `.mid` and `.midi` must cease to be
  advertised as playable;
- gapless playback, crossfade, EQ, ReplayGain, output-device selection, or
  pitch-preserving speed changes;
- video display, subtitle handling, encoding, transcoding, recording, or
  FFmpeg filters;
- a shared playback/analysis PCM pipeline;
- automatic mid-track switching between native and helper decoders after
  playback has already started;
- static linking of FFmpeg; or
- a universal binary covering every OS and CPU in one file. Each target gets a
  self-contained package.

## 5. Final architecture

```text
                                      +--------------------------+
                                      | native Symphonia decoder |
                                      +-------------+------------+
                                                    |
local audio file -> composite audio selection ------+----> PCM
             |                                      |
             | native open/probe rejected           |
             v                                      |
     bundled tz-audio-decoder ----------------------+
       (child process + FFmpeg shared libraries)
                                                    |
                         +--------------------------+------------------+
                         |                                             |
                         v                                             v
             Rodio/CPAL output worker                      tz-analysis DSP
             + live stereo level tap                       + persistent cache
                         |                                             |
                         v                                             v
                  system audio device                        rich visualizers
```

The frontend boundary remains unchanged:

```text
TUI / CLI -> AppRuntime -> PlayerService -> AudioPlaybackBackend | Fake
```

`tz-tui`, `tz-control`, and `tz-db` must not import Rodio, CPAL, Symphonia, or
FFmpeg APIs.

### 5.1 Crate responsibilities

Add the following workspace crates:

- `tz-audio` (library): PCM types; the helper wire protocol; native Symphonia
  streaming decode for offline analysis; bundled-helper discovery and client
  process management; decoder selection; test seams. It must not open an audio
  device.
- `tz-audio-decoder` (binary): the narrow FFmpeg-backed helper. It depends on
  the wire types from `tz-audio` with native/client features disabled and is
  dynamically linked to the packaged FFmpeg libraries.

Change existing crates as follows:

- `tz-playback`: retain the already-proven Rodio/Symphonia native source,
  adapt helper PCM from `tz-audio` to Rodio, retain the device worker and
  live-level tap, and expose `AudioPlaybackBackend` plus Fake. It uses the same
  native-first/helper-second policy even though native playback remains in the
  existing Rodio adapter.
- `tz-analysis`: consume `tz-audio` PCM rather than native-WAV/`ffmpeg` CLI
  subprocesses; keep the existing normalized `DecodedAnalysisAudio` and DSP
  APIs where practical.
- `tz-core`: make the composite audio backend the default, migrate persisted
  backend state, and keep analysis-cache orchestration unchanged.
- `tz-player`: package-aware doctor/setup/about output and the final CLI names.

The `tz-audio` crate may use features such as `native` and `client` so the
helper can import only protocol types without linking Symphonia into its final
binary. Avoid a crate cycle: `tz-audio` must not depend on `tz-playback` or
`tz-analysis`.

## 6. Backend identity and state migration

The final public real backend is named `audio`, because it is a composite and
is no longer accurately described as only Rodio.

- Final `BackendKind` variants: `Audio` and `Fake`.
- `Audio` is the default for new state and for CLI invocations without an
  override.
- `--backend audio` is the documented real-audio option.
- `--backend rodio` remains an accepted, hidden/deprecated alias for one
  compatibility release and resolves to `Audio`.
- Existing saved values `vlc` and `rodio` migrate to `audio` on load and are
  written back as `audio` on the next normal state save.
- `fake` remains `fake`.
- Unknown values sanitize to `audio`.
- An explicit obsolete `--backend vlc` invocation must fail with an actionable
  message stating that VLC was removed and `--backend audio` is the replacement;
  it must not silently choose a different engine.

During additive implementation, VLC may remain selectable so regressions can
be compared. It must not remain in the final package, dependency graph, doctor
output, or user documentation.

If the real audio device cannot initialize, the existing requested/effective
Fake fallback keeps the TUI usable and reports the failure. A per-track decode
failure does not switch to Fake and must never simulate playback progress.

## 7. Decoder selection contract

Routing is based on actual decode/probe results, never only on a filename
extension.

For both playback and offline analysis:

1. Validate that the input is a local regular file.
2. Attempt native Symphonia open/probe.
3. If native decoding initializes, use it for the complete operation.
4. If native initialization reports an unrecognized container, unsupported
   codec, or equivalent media-decode error, start the bundled helper.
5. Do not start the helper for a missing path, non-file input, or permission
   error that is already conclusive.
6. If the helper also rejects the file, return one sanitized error that names
   both attempts without dumping unbounded library diagnostics.

Once a native track starts playing, a later native read/decode failure ends the
track in Error; it does not splice a helper stream into the middle. The user may
retry after the error is fixed. A helper stream that fails mid-track behaves
the same way.

Log the selected route (`native` or `bundled-helper`) at debug level. The normal
TUI should not make users manage that choice.

### 7.1 Format policy

The native acceptance matrix remains at least:

- WAV/PCM, MP1/MP2/MP3, FLAC, Ogg Vorbis, AAC/M4A, ALAC/M4A, AIFF, CAF/PCM,
  and supported Matroska/WebM audio.

The bundled-helper matrix must add fixtures for at least:

- Ogg Opus, WMA/ASF, Monkey’s Audio, WavPack, AC-3, E-AC-3, DTS, Musepack 7/8,
  TTA, and Speex.

Content probing remains authoritative. A family is documented as supported
only after its committed fixture passes decode, duration, seek, natural-end,
and analysis-cache checks against the exact packaged FFmpeg build. Remove
`.mid`/`.midi` from the media scanner. Any currently admitted extension not in
either tested matrix may remain discoverable only if the UI clearly treats it
as unverified; the preferred final behavior is to admit the tested union.

## 8. PCM abstraction

`tz-audio` must expose a streaming, backend-neutral abstraction equivalent to:

```rust
pub struct PcmSpec {
    pub sample_rate: u32,
    pub channels: u16,
}

pub trait PcmSource: Send {
    fn spec(&self) -> PcmSpec;
    fn duration_frames(&self) -> Option<u64>;
    fn read_interleaved(&mut self, output: &mut [f32])
        -> Result<usize, DecodeError>;
    fn seek_to_frame(&mut self, frame: u64) -> Result<(), DecodeError>;
}
```

The exact Rust spelling may change, but these semantics are required:

- interleaved, normalized finite `f32` samples in `[-1.0, 1.0]`;
- explicit sample rate and channel count;
- optional source duration;
- bounded reads with EOF represented separately from an error;
- source-timeline seek; and
- cancellation/drop that reaps helper processes and reader threads.

Native sources may retain their original rate/channel layout. The helper
returns a requested standardized layout. Analysis requests stereo 44,100 Hz to
preserve current DSP inputs; helper-backed playback requests stereo 48,000 Hz
and lets Rodio perform any final device conversion.

For playback, helper stdout must be drained by a dedicated reader into a
bounded ring/channel. The audio callback must not perform process I/O or wait
unboundedly on a pipe. The queue must hold no more than two seconds of decoded
audio and should target 250–500 ms. An underrun yields bounded silence plus a
counter/log entry; a persistent stall becomes a playback error.

The current Rodio timeline source and live 50 ms stereo peak metering must be
retained after the source adapter. Position is measured in original source
time, including after repeated speed changes and seeks.

## 9. Helper discovery and trust boundary

Release builds locate the helper from the running main executable, never from
the current directory and never through `PATH`:

| Package | Helper location |
|---|---|
| Windows ZIP | `<exe-dir>/audio/tz-audio-decoder.exe` |
| Linux archive/AppImage payload | `<exe-dir>/audio/tz-audio-decoder` |
| macOS app bundle | `Contents/Resources/audio/tz-audio-decoder` |

Tests must inject an explicit helper path through a constructor/config object.
An optional development override may exist only in debug builds and must
require an absolute path. Release builds must ignore it.

The parent process must:

- spawn the exact resolved path directly, without a shell;
- pass the media path as an OS-native argument;
- set stdin to null, stdout to the binary protocol, and stderr to a bounded
  diagnostic reader;
- hide the helper console window on Windows;
- cap the initial handshake, stderr, startup, idle/stall, stop, seek, and total
  analysis time;
- kill and reap the child on cancellation, replacement, timeout, parent drop,
  or protocol violation; and
- sanitize helper/library text before it reaches the terminal or log.

The helper itself opens the input with `std::fs::File` and supplies FFmpeg a
custom seekable AVIO context. It must not give FFmpeg a URL or enable network
protocol discovery. This preserves native Windows/Unix path behavior, allows
seek, and prevents media content from causing a network fetch. FFmpeg protocols,
devices, and network support are disabled in the packaged build.

## 10. Helper command and wire protocol

The helper exposes only these public operations:

```text
tz-audio-decoder capabilities --json
tz-audio-decoder decode --input <PATH> --start-ms <U64> \
  --sample-rate <U32> --channels 2 --format f32le
```

`capabilities --json` reports, at minimum:

- helper semantic version;
- supported protocol major/minor;
- FFmpeg version/commit and library versions;
- the exact FFmpeg configuration string/build-manifest hash; and
- enabled demuxer/decoder families.

For `decode`, stdout contains:

1. a four-byte little-endian unsigned JSON-header length;
2. that many bytes of UTF-8 JSON (maximum 64 KiB); then
3. headerless interleaved `f32le` PCM until EOF.

Protocol-major version 1 header:

```json
{
  "protocol_major": 1,
  "protocol_minor": 0,
  "sample_format": "f32le",
  "sample_rate": 48000,
  "channels": 2,
  "duration_frames": 123456,
  "start_frame": 0
}
```

`duration_frames` may be `null` when unknown. Unknown minor-version fields are
ignored; a different major version is rejected before PCM is consumed.
Header values must match the request and all integer/range/alignment checks.
NaN/infinite samples, partial final frames, an oversized header, premature EOF,
extra non-PCM stdout, or a stalled stream are protocol/decode errors.

Stderr is diagnostics only and is never parsed as PCM. Cap it at 64 KiB per
process. A normal parent-initiated stop that closes the pipe is not reported as
media corruption.

Exit status meanings:

| Code | Meaning |
|---:|---|
| 0 | complete EOF, or a clean broken-pipe exit after parent cancellation |
| 2 | invalid arguments or incompatible protocol request |
| 3 | input open/read/seek failure |
| 4 | unsupported or corrupt media/no audio stream |
| 5 | FFmpeg shared libraries loaded but failed compatibility checks |
| 6 | decode/resample failure |
| 7 | unexpected output/protocol I/O failure |
| 70 | internal helper failure |

The parent must not rely on exit codes alone; protocol validation and bounded
stderr are also required. If the operating-system loader cannot start the
dynamically linked helper because a shared library is absent, the helper cannot
emit its own status. The parent recognizes that failed/early launch as an
incomplete-package or shared-library-load error and doctor names the missing
package component when it can determine it.

## 11. FFmpeg helper implementation

Use a reviewed Rust FFmpeg binding inside `tz-audio-decoder`; `ffmpeg-next`
matching the pinned FFmpeg release is the preferred starting point. Before
committing to it, prove that the exact binding/FFmpeg pair builds on the
workspace’s Rust 1.89 MSRV and all release targets. If the safe wrapper cannot
express custom AVIO or meet MSRV, isolate direct `ffmpeg-sys-next` calls in one
small `unsafe` module with ownership/drop tests. Do not raise the project MSRV
or change the FFmpeg major version merely to make bindings convenient without
a recorded decision.

The helper decode loop must use the supported FFmpeg send/receive API:

1. create a custom AVIO context around the already-open local file;
2. open/probe the container and select the best audio stream;
3. create and open the decoder from stream codec parameters;
4. seek to `start-ms` when requested, flush decoder/resampler state, and discard
   leading frames until the requested source position;
5. send packets and receive audio frames;
6. use `libswresample` to convert to requested stereo interleaved `f32` at the
   requested rate;
7. write the validated header once, then complete PCM frames;
8. flush decoder and resampler at EOF; and
9. free every packet, frame, codec, resampler, format, and AVIO allocation on
   all exits.

No panics may unwind across an FFI callback. The AVIO read/seek callbacks catch
failures and return FFmpeg error codes. The helper must contain no encoder,
muxer, filter, video-output, network, or device behavior.

## 12. FFmpeg build and dynamic linking

Pin an exact stable FFmpeg release tarball and its cryptographic checksum in a
machine-readable build manifest. Do not consume a moving “latest” binary build.
Build the libraries from that source with scripts committed to this repository.

The build policy is:

- `--disable-gpl` and `--disable-nonfree`;
- shared libraries enabled and static libraries disabled;
- programs, docs, debug extras, network, protocols, devices, filters, encoders,
  and muxers disabled;
- `libavcodec`, `libavformat`, `libavutil`, and `libswresample` enabled;
- only the demuxers, parsers, and audio decoders required by the tested format
  matrix enabled;
- no optional external codec library unless its license, source offer, build
  flags, and transitive binaries receive a separate review; and
- the generated configuration and component lists saved with the package.

The first implementation task must derive the exact allowlist from the pinned
release’s `configure --list-*` output and test it; this specification does not
pretend component names are stable across FFmpeg releases.

The helper dynamically links the versioned FFmpeg libraries located beside it
under `audio/`. Configure `$ORIGIN` rpath on Linux and `@loader_path`/appropriate
bundle paths on macOS. Windows ships the corresponding versioned `.dll` files
beside the helper. Do not rename or obfuscate FFmpeg libraries. Package every
non-system transitive library or remove the dependency.

## 13. Analysis and cache behavior

`tz-analysis` retains the current public result shapes and target rates so the
DSP and TUI do not need a redesign. Replace routing as follows:

- native Symphonia PCM first for all native-supported files, including WAV;
- bundled helper PCM for native initialization failures;
- no `Command::new("ffmpeg")`, `PATH` probe, or system FFmpeg fallback.

The existing analysis limits remain the lower of byte and duration ceilings,
plus a wall-clock timeout. The reader must reject excess PCM as it arrives and
terminate/reap the helper exactly as the current CLI reader does.

Bump the analysis cache product versions from 1 to 2 when the decoder route is
switched so old values rebuild lazily. Do not wipe the database or eagerly
reanalyze the library. A single decode allocation still produces every missing
envelope/spectrum/beat/waveform product for a track.

Live meters continue to use playback PCM. Cached data continues to power rich
visualizers, seeking, and history. Missing/corrupt helper files may degrade
analysis visualizers but must not stop native-supported playback.

## 14. Error, timeout, and resource requirements

Required bounds:

- helper capability/startup handshake: 5 seconds;
- playback seek/restart acknowledgement: 5 seconds;
- helper stop/reap grace period: 2 seconds before force-kill;
- stderr: 64 KiB;
- wire header: 64 KiB;
- playback PCM queue: no more than two seconds or 4 MiB, whichever is smaller;
- offline analysis: existing environment-configured limits and compiled hard
  ceilings remain authoritative.

Playback must stay responsive while the helper starts or seeks; do this work on
the existing worker or a child-management thread, not a Tokio or TUI render
thread. Every command acknowledgement represents completed state, not merely a
queued request.

Errors distinguish at least:

- incomplete/tampered package or helper protocol mismatch;
- output device unavailable/lost;
- local file missing/unreadable;
- native unsupported followed by helper unsupported;
- corrupt/truncated media;
- seek unsupported/failed;
- helper startup timeout, PCM stall, crash, or invalid protocol; and
- FFmpeg shared-library load/version failure.

Error text must say what the user can do. It must not tell packaged-release
users to install VLC, install FFmpeg, or edit `PATH`.

## 15. Diagnostics and user experience

`tz-player doctor` for the final package checks:

- the Audio backend and default output device;
- the exact package-relative helper path;
- helper executable presence and protocol compatibility;
- FFmpeg library versions, configuration hash, and source/build manifest;
- native and helper format families; and
- license/source-offer files required by the package.

It must not inspect system VLC or the first `ffmpeg` on `PATH`. A test must put a
fake/malicious `ffmpeg` earlier on `PATH` and prove it is ignored.

`tz-player setup` becomes package guidance, not dependency installation
guidance. README, usage, architecture, security, release, progress, TODO, About,
and native-dependency documents must all describe the same final media roles.

The TUI may show a compact `Audio (native)` or `Audio (bundled helper)` detail
in diagnostics, but there is only one user-selectable real backend.

## 16. Packaging layout

Windows reference archive:

```text
tz-player-<version>-x86_64-pc-windows-msvc/
├── tz-player.exe
├── audio/
│   ├── tz-audio-decoder.exe
│   ├── avcodec-<major>.dll
│   ├── avformat-<major>.dll
│   ├── avutil-<major>.dll
│   ├── swresample-<major>.dll
│   ├── FFMPEG_BUILD.json
│   └── FFMPEG_CHANGES.diff
├── licenses/
│   ├── LGPL-2.1-or-later.txt
│   └── <other required native license files>
├── LICENSE
├── THIRD_PARTY_LICENSES.html
├── FFMPEG_SOURCE.md
├── NATIVE_DEPENDENCIES.md
└── README.md
```

Linux uses the equivalent versioned `.so` files; macOS uses `.dylib` files in
the appropriate app-bundle location. The packager must discover native
dependencies from the built helper and fail on an unexpected non-system
dependency. It must fail rather than create a player-only archive when any
required helper, library, manifest, license, or source-offer file is absent.

The matching FFmpeg source tar/zip and patch must be an asset on the same
release/download host as every binary package. Package and source assets get
SHA-256 checksums.

## 17. Licensing requirements

This section is engineering policy, not legal advice. Before public
distribution, the maintainer must review the exact build and, if appropriate,
obtain legal advice.

At minimum:

- dynamically link the helper to an FFmpeg build made without GPL or nonfree
  components;
- preserve FFmpeg’s LGPL notice and exact license;
- publish the exact corresponding FFmpeg source, including any patches;
- publish the exact configure/build commands and component list;
- identify the FFmpeg version/commit in package diagnostics and About;
- put the source link beside every application download;
- preserve notices/source links for the Rust binding and any external native
  libraries; and
- keep FFmpeg library names recognizable.

The implementation must follow the official
[FFmpeg license checklist](https://ffmpeg.org/legal.html), the selected FFmpeg
source’s license files, and the repository’s existing `cargo-deny`/`cargo-about`
policy. FFmpeg’s own checklist calls for non-GPL/nonfree builds, dynamic
linking, matching source, build configuration, patches, notices, and a source
link on the download page. FFmpeg protocols are disabled/selectively enabled
through its documented build configuration; the packaged helper instead uses
custom local-file AVIO.

## 18. Testing and release acceptance

### 18.1 Hardware-independent tests

Commit tiny redistributable fixtures and test:

- protocol header round-trip and every malformed/oversized/truncated case;
- capability/version mismatch;
- Unicode and spaces in paths on all platforms, plus non-UTF-8 paths on Unix;
- native routing, helper routing, and both-routes-failed error composition;
- every native and helper format family in section 7.1;
- duration and forward/backward seek;
- replacement, pause/resume, stop, speed changes, source timeline, live levels,
  natural end exactly once, repeat, and shuffle;
- child cancellation, timeout, crash, PCM stall, invalid samples, stderr flood,
  and cleanup with no surviving process/thread;
- analysis byte/duration/time limits and lazy version-2 cache rebuild;
- missing helper still permits native playback but is a failing doctor result;
- system `ffmpeg` and VLC are ignored; and
- old `vlc`/`rodio` state migrates to `audio` while `fake` remains unchanged.

Normal CI tests must not require an output device. Use the existing Rodio mixer
and test seams. Retain opt-in real-device tests.

### 18.2 Packaged clean-machine tests

For each release target, run the actual staged package in a clean VM/runner
with no VLC and no FFmpeg on `PATH`:

1. `tz-player doctor` passes.
2. The TUI launches and exits cleanly.
3. One native fixture plays, seeks, stops, and naturally completes.
4. One helper-only fixture plays, seeks, stops, and naturally completes.
5. Native and helper-only fixtures each create all analysis cache products.
6. The package works from a directory containing spaces and Unicode.
7. Removing one FFmpeg library makes doctor fail with the missing component.
8. A fake `ffmpeg` on `PATH` is never executed.
9. Package inspection finds no unexpected native dependency and no VLC file.

Hosted CI may use muted/mixer-level playback. A human audible smoke remains a
release requirement on representative Windows, Linux, and macOS devices.

### 18.3 Quality gates

The final branch passes:

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

Add helper/FFmpeg-specific build and integration checks to CI; do not weaken an
existing gate to land the migration.

## 19. Completion criteria

The migration is complete only when all of the following are true:

- **AC-01:** A packaged build plays the native and helper format matrices with
  no installed VLC or FFmpeg.
- **AC-02:** The main player never searches `PATH` for VLC or FFmpeg.
- **AC-03:** Helper playback preserves transport, timing, live levels, and
  exactly-once natural-end behavior.
- **AC-04:** Native-first/helper-second analysis creates the unchanged cache
  products within existing resource limits.
- **AC-05:** Playback buffering is bounded and helper children are always
  reaped on stop, seek, replacement, error, timeout, and shutdown.
- **AC-06:** Old backend state migrates safely and Audio is the only documented
  real backend.
- **AC-07:** VLC code, dependencies, setup instructions, tests, and package
  assumptions are removed after comparison testing is complete.
- **AC-08:** Every package contains the helper, exact FFmpeg libraries, build
  manifest, licenses, and source offer; matching source is published beside it.
- **AC-09:** Clean-machine package tests and the full repository gates pass on
  Windows, Linux, and macOS targets.
- **AC-10:** All permanent documentation agrees with this architecture and no
  release instruction tells the user to install VLC or FFmpeg.

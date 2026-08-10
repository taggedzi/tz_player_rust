# Experimental Rodio Playback Backend Design

Status: Proposed
Date: 2026-08-10
Related ADR: `docs/adr/ADR-0001-rust-crate-architecture-and-media-split.md`
Plan: `docs/superpowers/plans/2026-08-10-rodio-backend.md`

## Summary

Add a third `PlaybackBackend` implementation named `RodioPlaybackBackend`.
It will use Rodio for transport and output, Symphonia for streaming decode, and
CPAL's platform backend for the operating-system audio device. VLC remains the
default and the existing Fake backend remains the deterministic test/fallback
engine while Rodio is evaluated against real music libraries.

The first release of this backend is deliberately additive:

```text
                         +-> VlcPlaybackBackend   (default, broad formats)
AppRuntime -> PlayerService -> RodioPlaybackBackend (opt-in experiment)
                         +-> FakePlaybackBackend  (tests/no audio)
```

Users select it with `--backend rodio`. Selection persists using the existing
application-state field. Adding Rodio does not silently change an existing
user's backend, remove VLC, or claim VLC-equivalent format support.

The locked dependency baseline is Rodio 0.22.2, its compatible Symphonia 0.5.5,
and CPAL 0.17.3. Rodio's `Player` supports pause, stop, volume, speed, position,
and seek, while its Symphonia decoder features cover the common music formats
needed for an initial candidate. Primary references:

- <https://docs.rs/rodio/0.22.2/rodio/struct.Player.html>
- <https://docs.rs/rodio/0.22.2/rodio/decoder/index.html>
- <https://docs.rs/symphonia/0.5.5/symphonia/index.html>
- <https://docs.rs/cpal/0.17.3/cpal/>

## Goals

- Provide real local-file audio playback without requiring VLC or FFmpeg at
  runtime.
- Satisfy the existing `PlaybackBackend` contract: lifecycle, play,
  pause/resume, stop, seek, volume, rate, duration, position, and state.
- Preserve `PlayerService` repeat, shuffle, next/previous, persistence, and
  natural-end behavior without backend-specific branches in the TUI.
- Stream decode from disk rather than decoding complete tracks into memory.
- Build and run on Windows, Linux, and macOS through CPAL's normal platform
  output APIs.
- Make unsupported media and unavailable output devices fail promptly with a
  useful user-facing error rather than hanging, panicking, or producing fake
  progress.
- Produce an evidence-based compatibility report before considering a default
  backend change.

## Non-goals

- Replacing or removing VLC in this work.
- Changing the default backend from VLC to Rodio.
- Claiming that Rodio plays every extension accepted by the global playlist
  scanner.
- Network streams, internet radio, video playback, MIDI synthesis, CD audio,
  or DVD/Blu-ray input.
- Gapless playback, crossfade, equalization, ReplayGain, device selection, or
  pitch-preserving time stretching.
- Replacing the existing FFmpeg/native-WAV analysis path.
- Feeding live decoded PCM into visualizers in the first implementation. The
  design must not preclude a later sample tap, but backend parity comes first.
- Automatically falling from Rodio to VLC or from VLC to Rodio. Explicit real
  backend selection remains predictable.

## Dependency and platform model

`tz-playback` will depend on Rodio with default features disabled. Enable only
playback, tracing, Symphonia's complete stable format/codec set, and its stable
SIMD optimizations. Disabling defaults avoids Rodio's recording and unrelated
features.

The intended dependency shape is conceptually:

```toml
rodio = {
  version = "0.22.2",
  default-features = false,
  features = ["playback", "symphonia-all", "symphonia-simd", "tracing"]
}
```

The exact compatible version is locked in `Cargo.lock` and must pass the
repository's audit, deny, license, Rust 1.89 MSRV, Clippy, and cross-platform CI
policy.
Rodio uses CPAL for output. Windows uses the normal Windows audio backend and
macOS uses CoreAudio; neither requires shipping VLC-style codec DLLs. Linux
builds using the standard ALSA path require ALSA development files (for example
`libasound2-dev` on Ubuntu), which must be added to CI and release build notes.

Rodio adds no separately installed codec runtime on Windows. The application
does not dynamically discover `libvlc.dll`, `libvlccore.dll`, or VLC plugins
when Rodio is selected.

## Format contract

Rodio probes the stream contents through Symphonia; extensions are discovery
hints, not a guarantee. With Symphonia's complete stable feature set, the
candidate formats/codecs include:

- MP3, MP2, and MP1;
- FLAC;
- WAV/PCM and ADPCM;
- Ogg Vorbis;
- AAC-LC and ALAC, including supported MP4/M4A containers;
- AIFF;
- supported audio in CAF and Matroska/WebM containers.

The current playlist scanner also accepts media that this stack does not
natively decode, including Ogg Opus, WMA, Monkey's Audio, WavPack, AC-3, DTS,
Musepack, TTA, Speex, and MIDI. These entries remain visible because VLC can
play many of them. Under Rodio they must fail on `play()` with an error that
names the selected backend and explains that the container or codec is not
supported.

Documentation will contain a backend capability table rather than shrinking
the shared extension list. A later extension-capability API can improve the
editor presentation, but it is not required for the experimental backend.

## Backend selection and fallback

Add `Rodio` to `BackendKind`, CLI parsing, persisted state parsing, snapshots,
doctor output, and help text.

- No backend override: preserve the stored selection; new state continues to
  default to VLC.
- `--backend rodio`: request Rodio for this run and persist it through the same
  existing workflow used by VLC/Fake.
- Rodio startup failure: start Fake so the TUI remains usable and surface one
  persistent playback error explaining that no usable output device could be
  opened. Do not try VLC implicitly.
- Per-track decode failure: retain Rodio, mark playback as errored, and allow
  the user to select another track. Do not change the saved backend.
- `--backend fake`: retain the current deterministic no-audio behavior.

The TUI header must report the effective backend. If Rodio was requested but
Fake is active after fallback, the persistent error must report both facts so
the header and error do not appear contradictory.

## Runtime architecture

### Worker ownership

`RodioPlaybackBackend` will be a `Send + Sync` command handle. A dedicated
named worker thread owns the Rodio output device, output stream, decoder, and
`Player`. This mirrors the successful isolation of LibVLC and prevents blocking
file/decoder/device operations from running on Tokio executor threads.

Startup is synchronous from the worker's point of view but acknowledged over a
bounded response channel. It must either return a ready output stream or a
specific startup error. Shutdown stops playback, drops the player before the
output stream, closes command channels, and joins the worker without leaking a
thread.

All commands use request/response acknowledgements. A command must never report
success merely because it was queued. Transport polling reads a cheap shared
snapshot and must not perform file I/O or block on the audio callback.

### Playing a track

For `play(item_id, path, start_ms, duration_ms)` the worker:

1. Opens the path as a local file without loading the whole file.
2. Creates a Rodio/Symphonia decoder with seeking and duration information when
   available.
3. Replaces any previous player/source; queued overlap is not allowed.
4. Applies the current volume and speed before observable playback begins.
5. Seeks to `start_ms` when non-zero and returns an error if accurate-enough
   seeking is unavailable.
6. Publishes the decoder duration when known, otherwise uses the supplied
   metadata duration as a display/end-detection fallback.
7. Moves to `Playing` only after the source is attached successfully.

Paths remain native `Path`/`OsStr` values until the file is opened. The backend
must not round-trip a Windows path through UTF-8 merely because the existing
trait currently accepts `&str`; the implementation plan includes making the
trait path-safe if required.

### State, position, and natural end

The shared transport snapshot contains status, source position, duration, the
current item ID, and an optional terminal error. tz-player's millisecond
position is always a position in the original source timeline. Rodio documents
`get_pos()` in terms of its speed-adjusted playback position (for example, at
2x its returned five seconds corresponds to ten seconds in the recording), so
the backend must normalize Rodio's value instead of forwarding it blindly.
Position anchors must remain correct when speed changes more than once or after
a seek; the implementation may not assume one fixed rate for the whole track.

Natural end is detected when the player's queue becomes empty after a source
was active. The worker latches the final source position, publishes `Stopped`,
and distinguishes this from an explicit Stop. This is essential because
`PlayerService` advances repeat/shuffle only for a natural end near the known
duration. Polling the same completed source more than once must not emit or
trigger duplicate end transitions.

Explicit Stop clears the source, sets position to zero, and returns `Stopped`.
Pause preserves position. Seek clamps to a known duration, updates the snapshot
only after Rodio accepts the seek, and produces no synthetic success on error.

### Volume and speed

The public volume range remains `0..=100` and maps linearly to Rodio's
`0.0..=1.0` player volume. The existing `0.5x..=4.0x` speed clamp remains in
`tz-core`; Rodio receives the validated value.

Rodio playback-rate changes affect pitch as well as speed. That behavior must
be documented as backend-specific. Pitch-preserving time stretching is a
separate feature and is not implied by the current speed control.

### Events and errors

The backend implements the existing event handler consistently with Fake and
VLC. Command-driven state/media/position changes emit the corresponding
`BackendEvent`; a runtime device/decoder failure emits `BackendEvent::Error`
once and moves the snapshot to `Error`.

User-facing errors distinguish at least:

- no default output device or output configuration;
- file open/read failure;
- unrecognized container;
- unsupported codec;
- seek failure;
- output stream/device loss;
- worker termination or command timeout.

Paths and decoder-provided text pass through the repository's existing
terminal-sanitization boundary before display.

## Device loss and recovery

The experimental backend opens the default output device only. Device
selection and seamless route switching are deferred.

If CPAL reports that the stream/device is invalidated, Rodio moves to `Error`,
stops the current source, and reports an actionable message. It may make one
bounded attempt to reopen the default device if Rodio exposes the error safely;
otherwise recovery occurs on application restart. There must be no unbounded
retry loop and no silent switch to VLC.

## Doctor and setup behavior

`tz-player doctor --backend rodio` performs a silent startup-only probe:

- confirm the Rodio backend is compiled in;
- open and close the default output stream;
- report the selected output configuration when Rodio exposes it;
- list the supported stable Symphonia codec/container families;
- explain that FFmpeg remains optional and VLC is not required for this
  selected playback backend.

`tz-player setup` and usage documentation show all three backend choices and
the Linux build/runtime prerequisites. Existing VLC discovery remains intact
and should run only when VLC is selected or explicitly diagnosed.

## Security and resource behavior

Rodio/Symphonia decodes untrusted media in the tz-player process, just as
LibVLC currently parses playback media in-process. `docs/SECURITY.md` must name
the new parser boundary. The backend streams file input and must not introduce
an unbounded full-track allocation. Panics from worker setup or decode must not
cross the process boundary as undefined application state; worker termination
is converted into a backend error.

The new dependency graph must pass `cargo audit` and `cargo deny check`. Any
advisory or license exception requires a separate, owned decision rather than
being hidden in the backend commit.

## Testing strategy

CI cannot assume an audio device. Tests are split accordingly.

### Hardware-independent tests

- Backend-kind parsing, serialization, display, and persisted-state round trips.
- Worker state-machine tests using a test output/transport seam rather than the
  physical default device.
- Generated short fixtures for WAV/PCM and the repository's selected common
  compressed formats.
- Open, decode, duration, seek, pause/resume, stop, volume, speed, replacement,
  natural end, and unsupported/corrupt-file errors.
- Natural end advances exactly once through `PlayerService` for repeat Off,
  One, and All.
- Non-UTF-8 paths on supported Unix test targets and Unicode Windows paths.
- Shutdown during idle, active playback, paused playback, and failed startup.

Test fixtures are tiny, redistributable, generated when practical, and never
depend on the user's music library.

### Platform compile and silent smoke

- Windows, Linux, and macOS compile/test jobs build the Rodio backend.
- Linux CI installs the documented ALSA development package.
- A `rodio_smoke --startup-only` example opens and closes the output stream
  without emitting audio. It is manual/opt-in where hosted CI has no device.
- A second explicit smoke accepts a user-selected local media path for real
  playback and transport checks.

### Compatibility evaluation

Record results in `docs/RODIO_EVALUATION.md` using non-private labels. The
evaluation covers representative MP3, FLAC, WAV, Ogg Vorbis, AAC/M4A, ALAC,
AIFF, CAF, and MKA files, plus known unsupported formats. For each supported
case verify start, duration, pause, seek forward/backward, speed, volume, stop,
natural end, next-track advance, and clean shutdown.

## Acceptance criteria

The experimental backend is complete when:

- `tz-player --backend rodio` plays supported local audio without a VLC or
  FFmpeg runtime.
- Every `PlaybackBackend` operation has a tested Rodio implementation.
- Repeat/shuffle natural-end behavior advances once and only once.
- Unsupported/corrupt files and missing devices produce bounded, actionable
  errors while the TUI remains usable.
- VLC remains the default and all existing VLC/Fake tests continue to pass.
- The cross-platform build, full workspace tests, strict Clippy, formatting,
  audit, and deny gates pass.
- README, usage, architecture, security, release, setup, doctor, progress, and
  TODO documentation accurately distinguish backend requirements and formats.
- `docs/RODIO_EVALUATION.md` contains the compatibility results and a clear
  recommendation: keep experimental, promote to default, or reject/remove.

Changing the default backend or removing VLC requires a separate user-approved
decision and ADR after this evidence is available.

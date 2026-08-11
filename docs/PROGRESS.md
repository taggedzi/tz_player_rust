# Implementation progress

Last updated: 2026-08-11

## Current state

The Rust parity slice is implemented. The audio migration replaced VLC and
system-FFmpeg analysis with one composite Audio engine:

- streaming native Symphonia route;
- package-relative FFmpeg 7.1.5 helper fallback using custom local-file AVIO;
- bounded helper lifecycle/PCM queue and shared playback/analysis PCM contract;
- exact helper format/decode/seek tests, package metadata, license/source offer,
  and dependency inspection;
- Audio default plus Fake; old `vlc`/`rodio` state migrates to Audio.

Windows x86-64 has a clean native SDK/package smoke recorded locally. CI now
automatically builds and archive-smokes Windows x86-64 and Linux x86-64. The
macOS ARM64 job remains available through a manual `run_macos` workflow option
and is disabled for automated events. Linux/macOS packages remain unverified
until those CI jobs and required human-audible smokes are recorded. Exact
evidence and limitations are in
[`AUDIO_ENGINE_MIGRATION_RESULTS.md`](AUDIO_ENGINE_MIGRATION_RESULTS.md).

## Completed product areas

| Area | Status |
|---|---|
| Foundation / DB / state / control API | Done |
| Composite playback and Fake backend | Done |
| TUI, themes, mouse, staged editor | Done |
| Metadata and embedded-cover bounds | Done |
| Analysis caches and 26 visualizers | Done |
| Doctor/setup and package integrity | Done |
| Windows native package implementation | Done |
| Cross-platform package verification | CI implemented; results/audible smoke pending |

## Run

```powershell
cargo run -p tz-player -- add path\to\music
cargo run -p tz-player --
cargo run -p tz-player -- doctor
cargo run -p tz-player -- --backend fake
```

## Remaining post-parity work

1. Record Linux x86-64 and macOS ARM64 CI package results and human audible
   native/helper playback smokes on each supported OS.
2. Cross-language/resource comparisons and representative device battery tests.
3. Headless control server / multi-process appliance.
4. Sidecar cover art, Python data import, gapless/EQ/device selection.
5. Voice/appliance features from the future-project brief.

Historical conversion and Rodio evaluation documents are retained as evidence
but their VLC/default-backend conclusions are superseded by ADR-0004.

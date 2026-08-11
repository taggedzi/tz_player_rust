# tz-player Rust specification (parity v1)

## Purpose

A local-first, keyboard-first terminal music player with persistent playlists,
metadata, visualizers, and a headless-ready core.

## Media policy

| Concern | Implementation |
|---|---|
| Playback | Composite Audio backend (`--backend audio`, default) |
| Native route | Streaming Symphonia decoder in process |
| Compatibility route | Package-relative `tz-audio-decoder` + minimal shared FFmpeg 7.1.5 |
| Fallback / CI | Fake backend (`--backend fake`) |
| Analysis | Same bounded streaming PCM layer; no system executable |

There is no VLC backend and no system FFmpeg dependency. `rodio` is a temporary
alias for Audio; `vlc` is accepted only to emit the removal message. Content
probing selects native versus helper. The tested format policy is documented in
`usage.md` and the exact implementation evidence in
`AUDIO_ENGINE_MIGRATION_RESULTS.md`.

## In scope

- SQLite playlist and staged playlist editing.
- Play/pause/stop/seek, volume, 0.5x–4.0x speed, repeat, and shuffle.
- Keyboard/mouse terminal UI and persistent state.
- Cached metadata and 26 built-in visualizers.
- Lazy envelope, spectrum, beat, and waveform caches.
- Live 50 ms stereo levels from decoded playback PCM.
- `doctor`, `setup`, package integrity diagnostics, and structured commands.
- Bounded/cancellable native and helper decoding of local files.

Speed changes pitch; pitch-preserving stretching is out of scope.

## Out of scope

- Streaming services, URL media, or network codec protocols.
- Multi-user/network sync, remote web UI, voice, and local AI.
- Python visualizer plugin compatibility.
- Gapless playback, EQ, and explicit output-device selection.

## Workflows and configuration

The parity acceptance workflows remain launch/state recovery, playlist
navigation/editing, playback control, search, visualization, and diagnostics.
Configuration precedence is current-run CLI, persisted state, then defaults.

## Quality gates

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo audit`
- `cargo deny --locked --workspace --all-features check advisories bans licenses sources`
- `./scripts/check-distribution-licenses.ps1`
- `./scripts/package-release.ps1`
- `./scripts/test-staged-package.ps1 -Archive <package>`

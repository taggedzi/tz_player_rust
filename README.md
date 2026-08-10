# tz-player (Rust)

Rust rewrite of [tz-player](https://github.com/taggedzi/tz-player) — a keyboard-driven, local-first terminal music player.

**Status:** feature-complete for the v1 parity slice, with VLC playback by
default and an evaluated opt-in Rodio backend retained as experimental. See
[`docs/PROGRESS.md`](docs/PROGRESS.md).

**Conversion plan (phases, decisions, backlog for AIs/humans):** [`docs/CONVERSION_PLAN.md`](docs/CONVERSION_PLAN.md)

## Media stack

| Role | Technology |
|------|------------|
| **Playback (default)** | **VLC 3.x / libVLC** (dynamic load from a complete VLC install; other majors fail closed) |
| **Playback (experimental)** | **Rodio 0.22 + Symphonia + system audio** (`--backend rodio`; no VLC/FFmpeg runtime) |
| **Analysis / visualizers** | Rodio live stereo levels when selected; cached envelope/spectrum/beat/waveform from optional **FFmpeg** + native WAV |
| **Tests / fallback** | Fake playback backend |

FFmpeg is **not** used for listening. Rodio supports the common MP3, FLAC,
WAV, Vorbis, AAC/M4A, ALAC, AIFF, CAF, and Matroska families, while VLC remains
the broader-compatibility default. See the backend capability table in
[`docs/usage.md`](docs/usage.md).

Rodio taps its decoded PCM stream for live stereo levels, so level-reactive
visualizers respond without waiting for offline analysis. Spectrum, beat, and
waveform detail still use the bounded backend-neutral analysis cache.

VLC and FFmpeg execute trusted external/native code; Rodio/Symphonia parses
media in the player process. Keep every selected component patched and review
[`docs/SECURITY.md`](docs/SECURITY.md) before processing untrusted media.

## Quick start

Requires Rust 1.89 or newer (the highest MSRV among Ratatui and Lofty).

```powershell
cargo build -p tz-player

# Environment check
cargo run -p tz-player -- doctor
cargo run -p tz-player -- setup

# Add music to the default playlist
cargo run -p tz-player -- add E:\Music\some-album

# Run TUI with real VLC audio
cargo run -p tz-player --

# Experimental real audio without a VLC runtime
cargo run -p tz-player -- --backend rodio

# Simulated playback (no audio device)
cargo run -p tz-player -- --backend fake
```

Linux source builds need the ALSA development package used by Rodio/CPAL (for
example, `sudo apt install libasound2-dev` on Ubuntu). Windows and macOS require
no separately installed Rodio codec runtime.

### Themes

The TUI reads an optional `theme.json` from the config directory reported by
`tz-player paths`. Copy [`docs/theme.example.json`](docs/theme.example.json) to
that location and edit semantic colors with named values, `#RRGGBB`, or
`ansi:0..255`. The same file can override selection bolding and dimmed muted
text. Missing themes keep the built-in palette; invalid themes show a warning
and safely fall back to it. Theme settings belong to `tz-tui` and never enter
the playback/player configuration.

Release binary:

```powershell
cargo build --release -p tz-player
.\target\release\tz-player.exe doctor
```

### TUI keys

| Key | Action |
|-----|--------|
| ↑ / ↓ | Move cursor |
| Home / End | First / last |
| Shift+↑ / Shift+↓ | Reorder item |
| Enter / Space | Play / pause |
| n / p | Next / previous |
| x | Stop |
| ← / → | Seek ±5s (Shift: ±30s) |
| - / + | Volume |
| [ / ] | Speed (`\` = 1.0x) |
| r / s | Repeat cycle / shuffle |
| f | Find (FTS, filters live as you type) |
| o | Cycle playlist view order: Playlist / Track / Artist / Album |
| a | Open staged dual-pane playlist editor |
| d / Delete | Open editor focused on playlist items |
| c | Open editor with a staged clear |
| F10 / Ctrl+Enter (editor) | Apply staged edits; stops playback first |
| m | Refresh metadata |
| z | Cycle visualizers (26 built-ins) |
| Shift+Z | Hide/show visualizer pane (playlist fills the width) |
| g | Locate now-playing track |
| i | About / version info |
| ? | Help (full-screen key reference) |
| q | Quit |
| Mouse | Click to select, double-click a track to play, wheel to navigate, click/drag transport sliders |

Full reference: [`docs/usage.md`](docs/usage.md).

Playlist rows expose aligned Track, Artist, and Album columns. Sorting is a
persisted view preference only; it never rewrites the staged playlist or the
order used by Next/Previous.

Mouse capture is enabled only while the TUI is open. Playlist and editor panes
accept clicks and wheel navigation; the time, volume, and speed rows accept
clicks or left-button drags. Every mouse operation retains a documented
keyboard equivalent, so keyboard-only use remains complete.

## Workspace

```text
crates/
  tz-player      # binary (CLI + TUI entry)
  tz-core        # runtime, player service, metadata, state, levels
  tz-playback    # PlaybackBackend: VLC + experimental Rodio + Fake
  tz-analysis    # FFmpeg/WAV analysis (envelope, spectrum, beat, waveform)
  tz-control     # structured Command + TransportSnapshot
  tz-db          # SQLite schema v8 + stores + FTS + editor drafts
  tz-tui         # ratatui UI + visualizer plugins
  tz-bench       # opt-in DSP/DB/TUI performance + resource runner (not shipped)
```

## Quality gates

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo audit
cargo deny --locked check advisories bans licenses sources
```

Opt-in performance and resource suite:

```powershell
cargo run --release -p tz-bench -- run
```

The suite measures analysis DSP, large-playlist SQLite queries, persistent disk
footprint, and headless TUI idle rendering, including latency, throughput, peak
live heap, process RSS, database/cache bytes, and binary size. Use
`--preset ancient` on very old or low-memory hardware. See
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) for baseline and comparison guidance.

CI: [`.github/workflows/ci.yml`](.github/workflows/ci.yml) (Windows, Ubuntu,
macOS, and locked dependency policy).

## Docs

- [`docs/usage.md`](docs/usage.md) — CLI + TUI guide  
- [`docs/RELEASE.md`](docs/RELEASE.md) — release checklist  
- [`docs/SECURITY.md`](docs/SECURITY.md) — trust boundaries + dependency policy
- [`docs/SPEC.md`](docs/SPEC.md) — product scope  
- [`docs/architecture.md`](docs/architecture.md) — crate boundaries  
- [`docs/PROGRESS.md`](docs/PROGRESS.md) — implementation status  
- [`docs/RODIO_EVALUATION.md`](docs/RODIO_EVALUATION.md) — compatibility evidence and recommendation
- [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) — opt-in performance/resource baselines and comparisons
- [`docs/tz_player_v2_future_project.md`](docs/tz_player_v2_future_project.md) — long-term vision  
- [`docs/adr/`](docs/adr/) — decisions  
- `_ref_tz_player/` — Python reference tree (local)

## License

MIT — see `LICENSE`.

# tz-player (Rust)

Rust rewrite of [tz-player](https://github.com/taggedzi/tz-player) — a keyboard-driven, local-first terminal music player.

**Status:** the v1 parity slice now uses one self-contained Audio engine. VLC
has been removed, and release packages carry their audited fallback decoder.
See [`docs/AUDIO_ENGINE_MIGRATION_RESULTS.md`](docs/AUDIO_ENGINE_MIGRATION_RESULTS.md)
for exact validation status and remaining platform smoke requirements.

**Conversion plan (phases, decisions, backlog for AIs/humans):** [`docs/CONVERSION_PLAN.md`](docs/CONVERSION_PLAN.md)

## Media stack

| Role | Technology |
|------|------------|
| **Playback (default)** | **Audio**: Rodio/CPAL output, native Symphonia first, package-relative FFmpeg helper fallback |
| **Analysis / visualizers** | The same bounded native/helper PCM layer plus live stereo levels |
| **Tests / fallback** | Fake playback backend |

Users do not install VLC or FFmpeg. The helper opens an already-open local file
through custom AVIO; its FFmpeg build has programs, network, protocols,
devices, filters, encoders, and muxers disabled. `PATH` is never used for media
tools. See the tested format table in [`docs/usage.md`](docs/usage.md).

The engine taps its decoded PCM stream for live stereo levels, so level-reactive
visualizers respond without waiting for offline analysis. Spectrum, beat, and
waveform detail still use the bounded backend-neutral analysis cache.

Symphonia parses native-route media in the player process. Helper-route media
is parsed in a bounded child process by the packaged FFmpeg 7.1.5 libraries.
Review [`docs/SECURITY.md`](docs/SECURITY.md) before processing untrusted media.

## Quick start

Requires Rust 1.89 or newer (the highest MSRV among Ratatui and Lofty).

```powershell
cargo build -p tz-player

# Environment check
cargo run -p tz-player -- doctor
cargo run -p tz-player -- setup

# Add music to the default playlist
cargo run -p tz-player -- add E:\Music\some-album

# Run TUI with the default Audio engine
cargo run -p tz-player --

# Simulated playback (no audio device)
cargo run -p tz-player -- --backend fake
```

Linux source builds need the ALSA development package used by Rodio/CPAL (for
example, `sudo apt install libasound2-dev` on Ubuntu). End-user packages need
no separately installed codec runtime. Building the complete release package
from source additionally uses the prerequisites in
[`native/ffmpeg/README.md`](native/ffmpeg/README.md).

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
  tz-audio       # streaming PCM, native decoder, helper protocol/client
  tz-audio-decoder # package-relative FFmpeg helper process
  tz-playback    # composite Audio backend + Fake
  tz-analysis    # backend-neutral bounded analysis products
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
cargo deny --locked --workspace --all-features check advisories bans licenses sources
./scripts/check-distribution-licenses.ps1
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

CI: [`.github/workflows/ci.yml`](.github/workflows/ci.yml) (automated Windows,
Ubuntu, and locked dependency policy; manually enabled macOS verification).

## Docs

- [`docs/usage.md`](docs/usage.md) — CLI + TUI guide  
- [`docs/RELEASE.md`](docs/RELEASE.md) — release checklist  
- [`docs/LICENSING.md`](docs/LICENSING.md) — dependency-license policy and distributable notices
- [`docs/SECURITY.md`](docs/SECURITY.md) — trust boundaries + dependency policy
- [`docs/SPEC.md`](docs/SPEC.md) — product scope  
- [`docs/architecture.md`](docs/architecture.md) — crate boundaries  
- [`docs/PROGRESS.md`](docs/PROGRESS.md) — implementation status  
- [`docs/RODIO_EVALUATION.md`](docs/RODIO_EVALUATION.md) — compatibility evidence and recommendation
- [`docs/AUDIO_ENGINE_MIGRATION_RESULTS.md`](docs/AUDIO_ENGINE_MIGRATION_RESULTS.md) — implementation and package evidence
- [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) — opt-in performance/resource baselines and comparisons
- [`docs/tz_player_v2_future_project.md`](docs/tz_player_v2_future_project.md) — long-term vision  
- [`docs/adr/`](docs/adr/) — decisions  
- `_ref_tz_player/` — Python reference tree (local)

## License

Project-authored code is MIT — see `LICENSE`. Binary releases contain
third-party Rust code and a dynamically linked, minimal LGPL FFmpeg 7.1.5
runtime. Terms and exact source links are preserved in
[`THIRD_PARTY_LICENSES.html`](THIRD_PARTY_LICENSES.html). Use
[`scripts/package-release.ps1`](scripts/package-release.ps1) so those files stay
with the executable and matching FFmpeg source metadata. Native/runtime
boundaries are recorded in
[`NATIVE_DEPENDENCIES.md`](NATIVE_DEPENDENCIES.md).

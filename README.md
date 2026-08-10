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
| **Analysis / visualizers** | **FFmpeg** (optional) + native WAV |
| **Tests / fallback** | Fake playback backend |

FFmpeg is **not** used for listening. Rodio supports the common MP3, FLAC,
WAV, Vorbis, AAC/M4A, ALAC, AIFF, CAF, and Matroska families, while VLC remains
the broader-compatibility default. See the backend capability table in
[`docs/usage.md`](docs/usage.md).

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

Full reference: [`docs/usage.md`](docs/usage.md).

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
- [`docs/tz_player_v2_future_project.md`](docs/tz_player_v2_future_project.md) — long-term vision  
- [`docs/adr/`](docs/adr/) — decisions  
- `_ref_tz_player/` — Python reference tree (local)

## License

MIT — see `LICENSE`.

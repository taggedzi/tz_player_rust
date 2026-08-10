# tz-player (Rust)

Rust rewrite of [tz-player](https://github.com/taggedzi/tz-player) — a keyboard-driven, local-first terminal music player.

**Status:** feature-complete for v1 parity slice (playlist, VLC playback, analysis caches, TUI, built-in visualizers). See [`docs/PROGRESS.md`](docs/PROGRESS.md).

**Conversion plan (phases, decisions, backlog for AIs/humans):** [`docs/CONVERSION_PLAN.md`](docs/CONVERSION_PLAN.md)

## Media stack

| Role | Technology |
|------|------------|
| **Playback (listen path)** | **VLC 3.x / libVLC** (dynamic load from install; required for real audio; other majors fail closed) |
| **Analysis / visualizers** | **FFmpeg** (optional) + native WAV |
| **Tests / fallback** | Fake playback backend |

FFmpeg is **not** used for listening in v1.

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

# Simulated playback (no VLC)
cargo run -p tz-player -- --backend fake
```

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
  tz-playback    # PlaybackBackend: Fake + VLC (dynamic libVLC FFI)
  tz-analysis    # FFmpeg/WAV analysis (envelope, spectrum, beat, waveform)
  tz-control     # structured Command + TransportSnapshot
  tz-db          # SQLite schema v8 + stores + FTS + editor drafts
  tz-tui         # ratatui UI + visualizer plugins
```

## Quality gates

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI: [`.github/workflows/ci.yml`](.github/workflows/ci.yml) (Windows + Ubuntu).

## Docs

- [`docs/usage.md`](docs/usage.md) — CLI + TUI guide  
- [`docs/RELEASE.md`](docs/RELEASE.md) — release checklist  
- [`docs/SPEC.md`](docs/SPEC.md) — product scope  
- [`docs/architecture.md`](docs/architecture.md) — crate boundaries  
- [`docs/PROGRESS.md`](docs/PROGRESS.md) — implementation status  
- [`docs/tz_player_v2_future_project.md`](docs/tz_player_v2_future_project.md) — long-term vision  
- [`docs/adr/`](docs/adr/) — decisions  
- `_ref_tz_player/` — Python reference tree (local)

## License

MIT — see `LICENSE`.

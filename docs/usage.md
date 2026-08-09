# Usage (Rust tz-player)

## Install dependencies

```text
tz-player setup
tz-player doctor
```

- **VLC** — required for real audio (libVLC loaded at runtime).
- **FFmpeg** — optional; improves analysis for spectrum / beat / waveform visualizers.

## Common commands

```powershell
# Interactive TUI
tz-player

# Force fake playback (no audio; good for UI-only tests)
tz-player --backend fake

# Playlist
tz-player add E:\Music\Album
tz-player add track.mp3
tz-player list
tz-player list --limit 20

# Diagnostics
tz-player doctor
tz-player paths
tz-player --version
```

Build from source:

```powershell
cargo run -p tz-player --
cargo build --release -p tz-player
```

## TUI keys

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move cursor |
| `Home` / `End` | First / last track |
| `PageUp` / `PageDown` | Jump 10 rows |
| `Shift+↑` / `Shift+↓` | Reorder selected item |
| `Enter` / `Space` | Play cursor / pause toggle |
| `n` / `p` | Next / previous |
| `x` | Stop |
| `←` / `→` | Seek ±5s (`Shift`: ±30s) |
| `-` / `+` | Volume |
| `[` / `]` | Speed (`\` = 1.0x) |
| `r` / `s` | Repeat cycle / shuffle |
| `f` | Find (FTS filter, live as you type) |
| `a` | Add file or folder path |
| `d` / `Delete` | Remove selected |
| `c` then `y`/`n` | Clear playlist |
| `m` | Refresh metadata |
| `z` | Cycle visualizers |
| `g` | Locate now-playing track |
| `i` | About / version info |
| `?` | Help (full-screen key reference) |
| `q` | Quit (persists state) |

## Visualizers (`z` cycles all built-ins)

| ID | Name |
|----|------|
| `basic` | Basic |
| `vu.reactive` | VU Meter (Reactive) |
| `spectrum.bars` | Spectrum Bars |
| `viz.spectrogram.waterfall` | Spectrogram Waterfall |
| `viz.spectrum.terrain` | Audio Terrain |
| `viz.spectrum.radial` | Radial Spectrum |
| `matrix.green` / `.blue` / `.red` | Matrix Rain themes |
| `viz.waveform.proxy` | Waveform Proxy |
| `viz.waveform.neon` | Waveform Neon |
| `ops.hackscope` | HackScope (Fictional) |
| `viz.typography.glitch` | Typography Glitch |
| `cover.ascii.static` / `.motion` | Cover ASCII |
| `viz.reactor.particles` | Particle Reactor |
| `viz.particle.*` | Gravity well, shockwave, rain, orbital, ember, magnetic, tornado, constellation, data core, plasma |

Analysis caches (envelope `E`, spectrum `S`, beat `B`, waveform `W`) fill in the background on play/add. Transport shows `analysis:ESBW` when ready, or `analysis:analyzing`.

## Empty playlist

Press `a`, type a path, Enter — or use `tz-player add` from the shell first.

## Logging

```powershell
tz-player --verbose
tz-player --quiet
# or
$env:RUST_LOG = "debug"
```

## Data identity

Rust data is **not** shared with the Python app (`tz-player-rs` vs `tz-player`). See `tz-player paths` and [ADR-0002](adr/ADR-0002-data-directory-identity.md).

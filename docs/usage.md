# Usage (Rust tz-player)

## Install dependencies

```text
tz-player setup
tz-player doctor
```

- **VLC 3.x** — the default real-audio backend (libVLC loaded at runtime). A
  complete VLC installation is required: `libvlc.dll` alone is not enough
  because libVLC also loads `libvlccore` and codec/output plugins. The VLC 4
  ABI changes player construction, seek signatures, and clocks to
  microseconds; tz-player rejects it until a complete VLC 4 backend exists.
- **Rodio** — an experimental real-audio backend selected with `--backend
  rodio`. Rodio, Symphonia, and CPAL are compiled into the application; no VLC
  or FFmpeg runtime is required. Linux source builds need the platform audio
  development files (for example, `libasound2-dev` on Ubuntu).
- **FFmpeg** — optional; improves analysis for spectrum / beat / waveform visualizers.

Install both only through a trusted operating-system package manager or another
verified distribution channel. FFmpeg is the first `ffmpeg` executable found
on `PATH`, while LibVLC and its plugins are dynamically loaded into the player
process. Rodio opens the operating system's default output device and parses
supported playback media in-process through Symphonia. Do not shadow FFmpeg
with a binary in a user-writable directory or set `VLC_PLUGIN_PATH` to an
untrusted directory. Run `tz-player --backend <name> doctor` after installation
or environment changes, and separately verify which `ffmpeg` executable is
first on `PATH`. See [the security policy](SECURITY.md) for the complete runtime
trust boundaries and resource limits.

## Playback backends

| Backend | Select | Runtime | Format policy |
|---------|--------|---------|---------------|
| VLC | `--backend vlc` (default) | Complete supported VLC 3.x installation | Broad compatibility through VLC plugins |
| Rodio | `--backend rodio` | Built-in Rodio/Symphonia plus a working default audio device | MP1/2/3, FLAC, WAV/ADPCM, Ogg Vorbis, AAC, ALAC, AIFF, CAF, supported Matroska/WebM audio |
| Fake | `--backend fake` | None | No audio; deterministic UI/tests |

The shared playlist accepts some formats Rodio does not decode, including Ogg
Opus, WMA, Monkey's Audio, WavPack, AC-3, DTS, Musepack, TTA, Speex, and MIDI.
Selecting one of those entries under Rodio reports a bounded per-track error;
it does not silently switch to VLC. If the selected real backend cannot start,
the TUI stays usable with Fake, identifies both the requested and effective
backend, and preserves the requested preference for the next run.

Rodio's speed control changes pitch with rate. VLC behavior depends on its
audio output pipeline. Pitch-preserving time stretching is not currently part
of the playback contract.

## Common commands

```powershell
# Interactive TUI
tz-player

# Experimental Rodio playback
tz-player --backend rodio

# Force fake playback (no audio; good for UI-only tests)
tz-player --backend fake

# Playlist
tz-player add E:\Music\Album
tz-player add track.mp3
tz-player list
tz-player list --limit 20

# Diagnostics
tz-player doctor
tz-player --backend rodio doctor
tz-player paths
tz-player --version
```

All plain CLI output treats filenames and metadata as untrusted. C0/C1
terminal controls (including ANSI/OSC ESC and BEL), embedded CR/LF, and Unicode
directional controls are printed as visible `\\xNN` / `\\u{NNNN}` escapes.
They cannot alter terminal state or visually reorder surrounding diagnostics.

Build from source:

```powershell
cargo run -p tz-player --
cargo build --release -p tz-player

# Silent Rodio output check; does not play audio
cargo run -p tz-playback --example rodio_smoke -- --startup-only

# Explicit manual Rodio playback smoke
cargo run -p tz-playback --example rodio_smoke -- path\to\track.flac
```

## TUI themes

Run `tz-player paths` to print the exact `theme.json` location, then copy
[`theme.example.json`](theme.example.json) there. Every field is optional.
Colors accept standard names (`cyan`, `dark_gray`, `light_magenta`, etc.),
24-bit `#RRGGBB`, or indexed `ansi:0..255` values. `selection_bold` and
`muted_dim` accept booleans; omitting them preserves each widget's built-in
formatting. Unknown fields, invalid colors, malformed JSON, and files over 64
KiB are rejected as a whole with a visible warning and the built-in fallback.

This is presentation-only configuration loaded by `tz-tui`; it is not part of
the player service or playback backend state. Restart the TUI after changing
the file.

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
| `o` | Cycle view order: Playlist / Track / Artist / Album |
| `a` | Open staged dual-pane editor |
| `d` / `Delete` | Open the staged playlist editor (select an item, then remove/apply) |
| `c` | Open editor with a staged clear (Apply to commit, Esc to undo) |
| `F10` / `Ctrl+Enter` (editor) | Apply staged edits; playback stops first |
| `Tab`, `i`, `a`, `d`, `Ctrl+↑/↓` (editor) | Switch panes, insert, append, remove, reorder |
| `s`, `S`, `l`, `r`, `D` (editor) | Save, save-as, load, rename, delete saved playlist |
| `m` | Refresh metadata |
| `z` | Cycle visualizers |
| `Shift+Z` | Hide/show visualizer pane (playlist fills the width) |
| `g` | Locate now-playing track |
| `i` | About / version info |
| `?` | Help (full-screen key reference) |
| `q` | Quit (persists state) |
| Left click / double-click | Select a playlist/editor row / play a playlist track |
| Mouse wheel | Move three rows in the playlist or focused editor pane |
| Click or left-drag transport | Seek; set volume; set speed |

The playlist shows Track, Artist, and Album as aligned columns. `o` changes
only the displayed order and preserves the selected item; playback order and
the staged playlist remain unchanged. Switch back to Playlist order before
using the main-view `Shift+Up` / `Shift+Down` reorder shortcut.

Mouse reporting is enabled when the alternate-screen TUI starts and disabled
again on exit. Mouse support is additive: the complete keyboard map above is
still available, including every action exposed by a click, wheel, or drag.

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

Offline decoding is bounded and runs once per track even when several caches
are missing. Defaults limit decoded stereo PCM to 256 MiB, media duration to
one hour, and FFmpeg/native-WAV execution to two minutes. Advanced users can
lower or raise those limits with
`TZ_PLAYER_ANALYSIS_MAX_DECODED_BYTES`,
`TZ_PLAYER_ANALYSIS_MAX_DURATION_SECS`, and
`TZ_PLAYER_ANALYSIS_TIMEOUT_SECS`; compiled ceilings remain 1 GiB, six hours,
and fifteen minutes. FFmpeg stdin is disabled, and any process that reaches a
limit is killed and reaped.

Embedded cover art is treated as untrusted input. Individual picture payloads
are capped at 8 MiB, all pictures in a tag at 16 MiB, cumulative cover-metadata
reads at 32 MiB, and decoded images at 4096x4096 / 32 MiB. Artwork outside
those limits is ignored and the cover visualizer uses its normal empty state.

## Empty playlist

Press `a` to open the folder browser (arrows to navigate, Enter to open a
folder or add a file, `a`/Space to add a highlighted folder recursively,
Esc to cancel) — or use `tz-player add` from the shell first.

## Logging

```powershell
tz-player --verbose
tz-player --quiet
# or
$env:RUST_LOG = "debug"
```

## Data identity

Rust data is **not** shared with the Python app (`tz-player-rs` vs `tz-player`). See `tz-player paths` and [ADR-0002](adr/ADR-0002-data-directory-identity.md).

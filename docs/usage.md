# Usage (Rust tz-player)

## Install and verify

```text
tz-player setup
tz-player doctor
```

The release archive is self-contained for codec decoding: do not install VLC
or a system FFmpeg. Keep the `audio/` directory, license files, and build/source
metadata beside the player. `tz-player doctor` verifies the default output
device, helper protocol, FFmpeg identity, and required package files.

Rodio/CPAL opens the operating system's default output device. Linux source
builds need platform audio development files such as `libasound2-dev`; normal
system audio libraries/drivers remain operating-system dependencies. Building
the bundled decoder SDK from source also needs the tools listed in
[`../native/ffmpeg/README.md`](../native/ffmpeg/README.md).

## Playback backends

| Backend | Select | Runtime | Format policy |
|---------|--------|---------|---------------|
| Audio | `--backend audio` (default) | Built-in output/native decoder plus package-relative helper | Tested native and helper union below |
| Fake | `--backend fake` | None | No audio; deterministic UI/tests |

`--backend rodio` is a temporary compatibility alias for Audio. The old
`--backend vlc` spelling exits with an actionable removal message. Persisted
`vlc` and `rodio` values migrate to `audio`; Fake remains unchanged.

The native acceptance matrix is WAV/PCM, MP1/MP2/MP3, FLAC, Ogg Vorbis,
AAC/M4A, ALAC/M4A, AIFF, CAF/PCM, and supported Matroska/WebM audio. The
packaged helper additionally verifies Ogg Opus, WMA/ASF, Monkey's Audio,
WavPack, AC-3, E-AC-3, DTS, Musepack SV7/SV8, TTA, and Speex. Content probing,
not the extension alone, decides the route. MIDI is not admitted.

If Audio cannot initialize, the TUI stays usable with Fake and shows requested
versus effective backend status. A per-track decode failure remains an Audio
error and does not silently change the backend.

Speed control changes pitch with rate. Pitch-preserving time stretching is not
currently part of the playback contract.

During Audio playback, decoded PCM is metered in lock-free 50 ms stereo peak
windows before volume/output processing. This live signal drives level-reactive
visualizers and is cleared on pause, seek, stop, natural end, or error. Cached
spectrum, beat, and waveform inputs remain backend-neutral and continue to use
the bounded analysis pipeline described below.

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
tz-player --backend fake doctor
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

# Silent output check; does not play audio
cargo run -p tz-playback --example audio_smoke -- --startup-only

# Explicit manual Audio playback smoke
cargo run -p tz-playback --example audio_smoke -- path\to\track.flac

# Complete audited package (after building native/ffmpeg/build/sdk)
./scripts/package-release.ps1
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

Offline decoding uses the same native/helper layer as playback, is bounded,
and runs once per track even when several caches
are missing. Defaults limit decoded stereo PCM to 256 MiB, media duration to
one hour, and FFmpeg/native-WAV execution to two minutes. Advanced users can
lower or raise those limits with
`TZ_PLAYER_ANALYSIS_MAX_DECODED_BYTES`,
`TZ_PLAYER_ANALYSIS_MAX_DURATION_SECS`, and
`TZ_PLAYER_ANALYSIS_TIMEOUT_SECS`; compiled ceilings remain 1 GiB, six hours,
and fifteen minutes. The helper receives null stdin, uses bounded custom local
file I/O, and is killed and reaped when a limit is reached.

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

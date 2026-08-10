# Architecture (Rust tz-player)

## Media roles

```text
LISTEN PATH                         ANALYSIS PATH
───────────                         ─────────────
tz-playback                         tz-analysis
  VlcBackend   ──► system audio        FFmpeg CLI / WAV
  RodioBackend ─► system audio           │
  FakeBackend  ─► tests/fallback         │
                                        ▼
                              spectrum / beat / waveform / envelope
                                        │
                                        ▼
                              visualizers (tz-tui)
```

- **VLC** owns decoding and device output for the default listen path.
- **Rodio** is an opt-in experimental listen path. A dedicated worker owns
  Rodio's player and CPAL output; Symphonia streams decode in-process.
- **FFmpeg** is only for offline PCM used by visualizers and level caches.
- Missing FFmpeg must never block playback.
- A selected real backend that cannot initialize falls back to Fake with the
  requested/effective distinction; Rodio and VLC never silently switch to one
  another.

## Crate dependency direction

```text
tz-player (bin)
  ├── tz-tui ──────────────► tz-core, tz-control, tz-db (rows only)
  ├── tz-core ─────────────► tz-playback, tz-db, tz-control
  │     AppRuntime + PlayerService + metadata (lofty)
  ├── tz-control             structured Command / TransportSnapshot
  ├── tz-playback            VLC FFI + Rodio/Symphonia/CPAL + Fake (listen path)
  ├── tz-analysis            FFmpeg only here
  └── tz-db                  schema + PlaylistStore + FTS
```

`tz-bench` is a development-only executable. It depends on `tz-analysis`,
`tz-db`, and a feature-gated headless render adapter in `tz-tui`; no production
crate depends on it, and it is not part of the shipped player binary.

Frontends must not import VLC, Rodio, Symphonia, CPAL, or FFmpeg APIs directly.

## Runtime flow

```text
CLI / TUI
   │ Command
   ▼
AppRuntime ──► PlaylistStore (SQLite)
   │
   └──► PlayerService ──► PlaybackBackend (VLC | Rodio | Fake)
```

VLC and Rodio each isolate device/decoder ownership on a dedicated worker.
Backend commands receive bounded acknowledgements, while transport polling
reads a cheap snapshot. Rodio tracks decoded source samples before its rate
filter so the public position remains in the original media timeline.

## Headless core

Playback, playlists, and state live outside the TUI. `tz-control` commands are the stable boundary for:

- TUI (now)
- `tz-player serve` / IPC (later)
- other frontends (later)

## Data

- SQLite: schema version 8 (Python baseline plus transient editor drafts)
- JSON state: atomic writes under a **new** app identity (`tz-player-rs`) so Python installs are not corrupted

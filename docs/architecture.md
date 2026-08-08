# Architecture (Rust tz-player)

## Media roles (non-negotiable for v1)

```text
LISTEN PATH                         ANALYSIS PATH
───────────                         ─────────────
tz-playback                         tz-analysis
  VlcBackend  ──► system audio        FFmpeg CLI / WAV
  FakeBackend ──► tests/fallback        │
                                        ▼
                              spectrum / beat / waveform / envelope
                                        │
                                        ▼
                              visualizers (tz-tui)
```

- **VLC** owns decoding and device output for listening.
- **FFmpeg** is only for offline PCM used by visualizers and level caches.
- Missing FFmpeg must never block playback.
- Missing VLC falls back to Fake + doctor/setup guidance.

## Crate dependency direction

```text
tz-player (bin)
  ├── tz-tui ──────────────► tz-core, tz-control, tz-db (rows only)
  ├── tz-core ─────────────► tz-playback, tz-db, tz-control
  │     AppRuntime + PlayerService + metadata (lofty)
  ├── tz-control             structured Command / TransportSnapshot
  ├── tz-playback            Fake + VLC dynamic libVLC FFI (listen path only)
  ├── tz-analysis            FFmpeg only here
  └── tz-db                  schema + PlaylistStore + FTS
```

Frontends must not import VLC or FFmpeg APIs directly.

## Runtime flow

```text
CLI / TUI
   │ Command
   ▼
AppRuntime ──► PlaylistStore (SQLite)
   │
   └──► PlayerService ──► PlaybackBackend (Fake | VLC)
```

## Headless core

Playback, playlists, and state live outside the TUI. `tz-control` commands are the stable boundary for:

- TUI (now)
- `tz-player serve` / IPC (later)
- other frontends (later)

## Data

- SQLite: schema version 7 baseline (Python compatibility)
- JSON state: atomic writes under a **new** app identity (`tz-player-rs`) so Python installs are not corrupted

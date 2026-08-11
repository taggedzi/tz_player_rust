# Architecture (Rust tz-player)

## Audio roles

```text
local media file
      |
      v
tz-audio composite decoder
  | native succeeds                 | native unsupported
  v                                 v
Symphonia streaming PCM       packaged tz-audio-decoder
                                    |
                              custom local-file AVIO
                                    |
                           minimal shared FFmpeg 7.1.5
      |                                 |
      +---------------+-----------------+
                      v
             bounded streaming PCM
                |             |
                v             v
        Rodio/CPAL output   tz-analysis caches
```

There is one user-facing real backend, `Audio`, plus deterministic `Fake`.
Native decoding is attempted first. Unsupported native media falls back once
to the helper; corrupt media reports the composed native/helper error. Routing
is based on probing, not an extension.

The helper is a package-relative child process. It is never found through
`PATH`, receives no stdin, opens a `std::fs::File`, and exposes that file to
FFmpeg through seekable custom AVIO. The packaged FFmpeg build has network,
protocols, programs, devices, filters, encoders, muxers, GPL, and nonfree
components disabled. Parent queues, startup/stall/stop timeouts, diagnostics,
and analysis limits are bounded; cancellation kills and reaps the child.

## Crate dependency direction

```text
tz-player (bin)
  |-- tz-tui ----------> tz-core, tz-control, tz-db (rows only)
  |-- tz-core ---------> tz-playback, tz-db, tz-control
  |-- tz-playback -----> tz-audio, Rodio/CPAL
  |-- tz-analysis -----> tz-audio
  |-- tz-audio --------> Symphonia + versioned helper protocol/client
  |-- tz-audio-decoder -> narrow FFmpeg binding/AVIO implementation
  |-- tz-control ------> structured commands/snapshots
  `-- tz-db -----------> schema and stores
```

Frontends do not import Rodio, CPAL, Symphonia, or FFmpeg APIs. FFmpeg FFI is
confined to `tz-audio-decoder`; the protocol and `PcmSource` contract are
backend-neutral.

`tz-bench` is development-only and is not shipped. No production crate depends
on it.

## Runtime flow

```text
CLI / TUI
   | Command
   v
AppRuntime ---> PlaylistStore (SQLite)
   |
   `--> PlayerService ---> PlaybackBackend (Audio | Fake)
                              |
                              `--> dedicated output/decode worker
```

Commands receive bounded acknowledgements while transport polling reads a
cheap snapshot. Position is tracked in source time before playback-rate
filtering. Helper-backed seeks stop/reap the old process and start a validated
replacement stream.

## Headless core and data

Playback, playlists, and state live outside the TUI. `tz-control` is the stable
boundary for this TUI and future frontends.

- SQLite schema: version 8.
- JSON state uses the separate `tz-player-rs` identity.
- Analysis cache schema: version 2, rebuilt lazily when stale.

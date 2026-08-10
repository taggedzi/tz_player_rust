# Implementation progress

Last updated: 2026-08-10

**Full conversion plan (actionable, AI-consumable):** [`CONVERSION_PLAN.md`](CONVERSION_PLAN.md)

## Done

| Phase | Status | Notes |
|-------|--------|-------|
| 0 Foundation | **Done** | Cargo workspace, CI, ADRs, SPEC, media split |
| 1 DB + state | **Done** | Schema v8, FTS5, `PlaylistStore`, transient editor drafts, `AppState` |
| 2 Playback | **Done + experiment** | Fake + VLC dynamic libVLC FFI; opt-in Rodio/Symphonia/CPAL backend implemented and under compatibility evaluation |
| 3 Control API | **Done** | Structured `Command` + `AppRuntime` |
| 4 TUI | **Done** | ratatui playlist, transport, visualizer pane, find, staged dual-pane playlist editor, help |
| 5 Metadata | **Done** | lofty tags + embedded cover for Cover ASCII |
| 6 Analysis | **Done** | Envelope + spectrum + beat + waveform-proxy caches via `LevelService` |
| 7 Visualizers | **Done (full built-in pack)** | 26 plugins: spectrum, matrix×3, waveform×2, hackscope, typography, cover×2, particle pack×11 |
| 8 Doctor/setup | **Done** | Selected-backend VLC/Rodio diagnostics, silent smoke tools, paths, version banner, release build tip |
| 9 Hardening | **Done (v1)** | Status TTL, empty playlist UX, analysis readiness, better feedback |
| 10 Packaging | **Done (docs)** | README, usage, RELEASE checklist |
| 11+ Future | Deferred | See CONVERSION_PLAN §5 Phase 11+ |

## How to run

```powershell
cargo run -p tz-player -- add path\to\music
cargo run -p tz-player --
cargo run -p tz-player -- --backend rodio
cargo run -p tz-player -- doctor
cargo run -p tz-player -- --backend rodio doctor
```

### Keys (summary)

| Key | Action |
|-----|--------|
| `f` | Find (FTS filter, live as you type) |
| `a` | Open staged dual-pane playlist editor |
| `d` / `Delete` | Open editor focused on the playlist pane |
| `F10` / `Ctrl+Enter` | Apply staged editor changes and stop playback |
| `z` | Cycle visualizers |
| `Shift+Z` | Hide/show visualizer pane |
| `g` | Locate now-playing track |
| `i` | About / version info |
| `?` | Help (full-screen key reference) |
| `q` | Quit |

Envelope / spectrum / beat / waveform-proxy caches fill on play/add. Transport shows `analysis:ESBW` when caches are warm.

## Recent hardening (2026-08-08)

Investigated Phase 11+ item A (live VLC PCM sampling); measured that `libvlc_media_player_get_time()` only refreshes ~every 270-300ms internally, so a live tap wasn't the actual fix for perceived visualizer lag. Deferred the live-tap feature (unchanged resource/architecture cost for a gain not needed on a CLI) and instead:

- `PlayerService` now interpolates position between real backend reads (wall-clock * speed), reset on seek/track-change/stop, frozen while paused — closes the ~270ms staleness window without extra backend calls.
- `LevelService` bucket lookups (envelope/spectrum/beat/waveform) now apply a small forward offset (half the bucket's own hop width) to compensate for the inherently backward-looking `position_ms <= ?` cache query.
- `WaveformProxyVisualizer` renders a scrolling amplitude sparkline from a rolling window of cached waveform-proxy buckets (`WaveformStore::get_waveform_range`, new) instead of a single static min/max bar, falling back to the old bar when no history is available.

## Remaining (optional / post-parity)

1. Live playback-backend PCM sampling (true oscilloscope-class visualizers)
2. Perf benches vs Python
3. Headless control server / multi-process appliance (`tz-control` IPC)
4. Sidecar cover art, Python data import, engine upgrades (gapless/EQ/devices)
5. Slim custom FFmpeg (analysis packaging), voice/appliance features (future brief)

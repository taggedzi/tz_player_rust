# tz-player Python → Rust Conversion Plan

**Status:** v1 parity largely complete; this document is the durable plan reference  
**Last updated:** 2026-08-10
**Audience:** humans and AIs continuing the rewrite or post-parity work  

This plan was drafted in an interactive planning session and approved before Phase 0. It was executed in conversation but was **not originally written into the repo**. This file reconstructs that plan from session decisions, ADRs, SPEC, architecture notes, and current tree state so work can resume without chat history.

---

## 1. Mission

Rewrite [Python tz-player](https://github.com/taggedzi/tz-player) as a **Rust** local-first terminal music player with:

1. **Feature parity** with the current Python product (playlist, VLC playback, analysis caches, TUI, visualizers, doctor/setup), plus an additive experimental Rodio backend.
2. **Lower resource use** and a path to a **headless multi-process** appliance later.
3. **Safe coexistence** with Python installs (separate data identity).

**Not goals for v1 parity:** streaming services, cloud accounts, web UI,
voice/AI, replacing VLC with FFmpeg for listening, promoting Rodio over VLC
without a later evaluated decision, or Python visualizer plugin ABI
compatibility.

### Source and target

| Item | Location / note |
|------|-----------------|
| Python reference (in-repo) | `_ref_tz_player/` |
| Python upstream | https://github.com/taggedzi/tz-player |
| Future vision brief | [`tz_player_v2_future_project.md`](tz_player_v2_future_project.md) |
| Rust workspace root | this repo (`tz_player3`) |
| Spec / architecture | [`SPEC.md`](SPEC.md), [`architecture.md`](architecture.md) |
| Progress tracker | [`PROGRESS.md`](PROGRESS.md) |
| ADRs | [`adr/`](adr/) |

When behavior is ambiguous, prefer **Python runtime behavior + `_ref_tz_player` tests/docs** over inventing new UX.

---

## 2. Locked decisions (do not re-litigate without an ADR)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Playback (listen path) | **VLC / libVLC** (dynamic load) by default; experimental **Rodio/Symphonia/CPAL** opt-in per [ADR-0003](adr/ADR-0003-add-experimental-rodio-backend.md) | Preserve parity and broad VLC coverage while evaluating a no-VLC runtime |
| Analysis path | **FFmpeg CLI + native WAV** only | Visualizers/levels; must not block listening |
| Fake backend | Required for tests + fallback when a selected real backend fails startup | CI and safe degradation |
| TUI | **ratatui + crossterm** | Keyboard-first terminal UI |
| CLI | **clap** | `doctor`, `setup`, `paths`, `add`, `list`, default TUI |
| DB | **SQLite schema v8** + **FTS5** with LIKE fallback | Python-compatible analysis/playlist model plus transient editor drafts |
| Metadata tags | **lofty** | Embedded tags + cover art for Cover ASCII |
| Config/state identity | App id **`tz-player-rs`** (org `taggedzi`) | Avoid corrupting Python `tz-player` data — see [ADR-0002](adr/ADR-0002-data-directory-identity.md) |
| Control boundary | Structured **`Command` + snapshots** in `tz-control` | Headless / multi-frontend later |
| Crate layout | Workspace split below | Aligns with future Controller / Audio Engine / Library Manager |
| Custom slim FFmpeg | **Deferred**; analysis-only if ever | Not a v1 playback engine |
| Speed clamp | **0.5x–4.0x**, step 0.25 | Match Python ADR |

Media split (the listen/analysis boundary remains non-negotiable):

```text
LISTEN PATH                         ANALYSIS PATH
───────────                         ─────────────
tz-playback                         tz-analysis
  VlcBackend   ──► system audio        FFmpeg CLI / WAV
  RodioBackend ─► system audio           │
  FakeBackend  ─► tests/fallback         │
                                        ▼
                              envelope / spectrum / beat / waveform
                                        │
                                        ▼
                              visualizers (tz-tui)
```

Frontends must **not** link VLC, Rodio, Symphonia, CPAL, or FFmpeg APIs
directly. See [ADR-0001](adr/ADR-0001-rust-crate-architecture-and-media-split.md)
and its additive amendment [ADR-0003](adr/ADR-0003-add-experimental-rodio-backend.md).

---

## 3. Target crate map

```text
crates/
  tz-player      # binary: CLI entry + TUI launch
  tz-core        # AppRuntime, PlayerService, paths, AppState, metadata, LevelService
  tz-playback    # PlaybackBackend trait; VLC; experimental Rodio; Fake
  tz-analysis    # decode + envelope/spectrum/beat/waveform analysis
  tz-control     # Command / TransportSnapshot (stable boundary)
  tz-db          # schema migrations, PlaylistStore, analysis stores, FTS
  tz-tui         # ratatui UI + visualizer host/plugins
```

Dependency direction (simplified):

```text
tz-player
  ├── tz-tui  ──► tz-core, tz-control, tz-db (rows only)
  ├── tz-core ──► tz-playback, tz-db, tz-control, tz-analysis
  ├── tz-control
  ├── tz-playback
  ├── tz-analysis
  └── tz-db
```

Runtime flow:

```text
CLI / TUI
   │ Command
   ▼
AppRuntime ──► PlaylistStore (SQLite)
   │
   └──► PlayerService ──► PlaybackBackend (VLC | Rodio | Fake)
                │
                └── LevelService (analysis caches for visualizers)
```

Future multi-process mapping (post-v1, from FUTURE_PROJECT):

| Future process | Near-term crate ownership |
|----------------|---------------------------|
| Controller (TUI/CLI/API) | `tz-player`, `tz-tui`, `tz-control` |
| Audio Engine | `tz-playback`, parts of `tz-core` player |
| Library Manager | `tz-db`, metadata, later library scan |

IPC (Unix sockets / Windows named pipes, length-prefixed binary messages) is **out of v1**; keep `tz-control` clean so it can become the wire protocol later.

---

## 4. Parity scope (v1)

### In scope

- Default playlist CRUD: add paths, remove, clear, reorder, wrap next/prev, shuffle, sparse `pos_key`
- Search: FTS5 + LIKE fallback (title/artist/album/path-oriented ranking as in Python)
- Playback: play/pause/stop/seek/volume/speed/repeat/shuffle via VLC,
  experimental Rodio, or Fake
- State persistence: volume, speed, repeat, shuffle, backend, visualizer id, paths
- Metadata upsert/invalidate (lofty); snapshots for UI
- Lazy analysis caches: envelope, spectrum, beat, waveform-proxy → SQLite
- Visualizer host + **built-in plugins** matching Python IDs (see §6)
- TUI: playlist + transport + visualizer pane, find, add path, help
- CLI: `doctor`, `setup`, `paths`, `add`, `list`, default TUI
- VLC discovery (Windows Program Files, `PATH`/`AddDllDirectory`, `VLC_PLUGIN_PATH`)
- Quality gates: `fmt`, `clippy -D warnings`, `test --workspace`

### Out of scope (v1)

- Multi-library browser / recursive library manager process
- Gapless / crossfade / EQ / replay gain / device picker (future engine)
- Live PCM sample stream from VLC for true oscilloscope/Lissajous
- Folder sidecar cover art (`cover.jpg` / `folder.jpg`) — only embedded art for Cover ASCII
- Headless `serve` / remote control / MPRIS / web UI
- Packaging installers beyond release binary + docs checklist
- Migrating Python DB/state into `tz-player-rs` automatically

### Workflow acceptance (Python WF-01..WF-07 intent)

| ID | Workflow | v1 intent |
|----|----------|-----------|
| WF-01 | Launch and recover state | Load state; selected real backend fail → Fake + requested/effective message; usable TUI |
| WF-02 | Navigate playlist | Cursor, Home/End, reorder; deterministic rows |
| WF-03 | Playback control | Transport keys; status reflects player |
| WF-04 | Find/search | `f` filter via FTS/LIKE; Esc exits |
| WF-05 | Playlist editing | Add path, delete, clear confirm, reorder |
| WF-06 | Visualization | Host + cycle plugins; analysis feeds when ready |
| WF-07 | Config / diagnostics | doctor/setup/paths; state file feedback |

Python checklists live under `_ref_tz_player/docs/workflow-acceptance.md` for mapping tests.

---

## 5. Phased implementation plan

Phases are sequential for dependencies but many sub-tasks inside a phase can be parallelized. **Status reflects the repo as of 2026-08-08.**

### Phase 0 — Foundation

**Goal:** Empty folder becomes a buildable multi-crate workspace with docs and gates.

| Deliverable | Done when |
|-------------|-----------|
| Cargo workspace + crate stubs | `cargo test --workspace` green |
| Media-split ADR + architecture docs | ADRs 0001–0002, SPEC, architecture |
| CLI skeleton | `tz-player paths` / `doctor` run |
| CI-ready clippy/fmt config | clippy `-D warnings` clean |

**Status: DONE**

---

### Phase 1 — Database + app state

**Goal:** Python-compatible SQLite baseline and durable UI state.

| Deliverable | Done when |
|-------------|-----------|
| Schema v8 migrations | Fresh DB creates; older DBs migrate |
| FTS5 + triggers + LIKE fallback | Search tests pass |
| `PlaylistStore` API parity | add/remove/reorder/nav/search/metadata |
| Sparse `pos_key` (`POS_STEP = 10_000`) | Reorder stable |
| Windows path case-fold normalize | Duplicate path behavior matches intent |
| `AppState` load/save under `tz-player-rs` | Atomic write; speed clamp |

**PlaylistStore surface (reference):**

- Playlists: `create_playlist`, `ensure_playlist`, `clear_playlist`
- Tracks: `add_tracks`, `remove_items`, `count`, `fetch_window`, `get_item_row`
- Lookup: by track/item ids, `list_item_ids`
- Nav: next/prev (wrap), random
- Reorder: `move_selection`, `renumber_playlist`
- Search: `search_item_ids` (FTS → LIKE)
- Metadata: upsert / invalidate / mark invalid / snapshots

**Status: DONE** (`crates/tz-db`, `tz-core` state/paths)

---

### Phase 2 — Playback backends

**Goal:** Pluggable listen path with VLC as the compatibility default and an
evaluated opt-in Rodio path.

| Deliverable | Done when |
|-------------|-----------|
| `PlaybackBackend` trait | Fake implements fully |
| Fake backend | Unit tests for transport without audio hardware |
| Dynamic libVLC load (`libloading`) | No link-time VLC SDK required |
| Worker thread + command queue | Non-blocking API from UI thread |
| Windows discovery | Program Files VideoLAN; `PATH` / `AddDllDirectory`; `VLC_PLUGIN_PATH` |
| Quiet default; `TZ_PLAYER_VLC_VERBOSE=1` | Doctor/smoke usable |
| Smoke example | `cargo run -p tz-playback --example vlc_smoke` advances position |
| Fail closed → Fake | Runtime falls back if the requested real backend cannot start |
| Rodio worker + streaming decode | Dedicated output worker; Symphonia common-format coverage; no audio hardware in normal tests |
| Rodio selection + diagnostics | `--backend rodio`, persisted preference, selected-backend doctor, silent/manual smoke |
| Format/end matrix | MP3, FLAC, WAV, Vorbis, AAC, ALAC, AIFF, CAF, MKA; repeat/shuffle transitions |

**Implementation notes (locked):**

- Prefer dynamic install DLL over static linking.
- Modern VLC dropped `--plugin-path`; set env `VLC_PLUGIN_PATH`.
- Modules: `vlc_ffi.rs`, `vlc_engine.rs`, `vlc.rs`.
- Rodio remains experimental and never silently switches to/from VLC. See the
  approved design and plan under `docs/superpowers/` and ADR-0003.

**Status: DONE (VLC parity) + RODIO EXPERIMENT IMPLEMENTED; evaluation pending**

---

### Phase 3 — Control API + PlayerService + runtime

**Goal:** One orchestration layer all frontends use.

| Deliverable | Done when |
|-------------|-----------|
| Structured `Command` types | Serializable / headless-ready |
| `PlayerService` | play/pause/stop/seek/volume/speed/repeat/shuffle/next/prev on Fake+store |
| `AppRuntime` | Routes commands; owns store + player + status |
| Transport snapshots | TUI can render without calling backends |

**Status: DONE** (`tz-control`, `tz-core`)

---

### Phase 4 — Minimal TUI

**Goal:** Keyboard-first interactive player.

| Deliverable | Done when |
|-------------|-----------|
| Playlist pane | Navigate, select, play |
| Transport + progress | Volume/speed/repeat/shuffle |
| Layout | Playlist \| visualizer **columns** (~50% / rest); transport full width |
| Find (`f`) | FTS filter mode |
| Add path (`a`) | File or folder into store |
| Clear confirm (`c` + y/n) | Destructive guard |
| Help (`?`), quit (`q`) | Discoverable keys |
| Status TTL | Transient messages clear (~4s) |

**Status: DONE** (polish included in Phase 9)

---

### Phase 5 — Metadata

**Goal:** Tag display and cover inputs without blocking playback.

| Deliverable | Done when |
|-------------|-----------|
| lofty read path | Title/artist/album/etc. upserted |
| Invalidate on demand (`m`) | Refresh from disk |
| Embedded cover for Cover ASCII | Cached per path; “NO EMBEDDED COVER ART” when missing |

**Status: DONE** (sidecar folder art still deferred)

---

### Phase 6 — Analysis pipeline

**Goal:** Offline PCM analysis for reactive visualizers; never required for audio out.

| Channel | Cache table (concept) | Notes |
|---------|----------------------|--------|
| Envelope | `analysis_scalar_frames` | ~50 ms buckets |
| Spectrum | `analysis_spectrum_frames` | Log-spaced Goertzel; defaults **48 bands**, **40 ms** hop; `u8` frames |
| Beat | `analysis_beat_frames` | Onset + beat flags + BPM; same decode pass when possible |
| Waveform proxy | waveform store | Stereo min/max buckets |

| Deliverable | Done when |
|-------------|-----------|
| WAV + FFmpeg decode traits | Missing FFmpeg → soft fallback motion only |
| `LevelService` | Sample by position; `ensure_*` background on play/add |
| Single-decode multi-channel | One background job can fill ESBW |
| Snapshot fields | level, spectrum_bands, beat, waveform min/max |
| UI readiness | `analysis:ESBW` / `analyzing` style status |

**Status: DONE**

---

### Phase 7 — Visualizers

**Goal:** Port Python built-in visualizer IDs to ratatui styled `Line`/`Span` (not ANSI strings).

**Rules:**

- Color required when `ansi_enabled`; gray fallback when off.
- Center-based plugins use **panel geometric center**; energy must not move origin or recompute diameter each frame (bounce bug).
- Exact panel width/height; avoid wrap; avoid double-width Unicode cores that reflow rows.
- Prefer fidelity to `_ref_tz_player` plugins over creative reinterpretation for user-tuned visuals (constellation, reactor, gravity well, orbital, etc.).

**Built-in pack (IDs / families):**

| Family | Plugins |
|--------|---------|
| Core | `basic`, `vu.reactive`, `spectrum.bars` |
| Spectrum | waterfall, terrain, radial |
| Matrix | green / blue / red |
| Waveform | proxy, neon (proxy path; not live PCM) |
| Ops/text | hackscope, typography glitch |
| Cover | ascii static + motion (embedded art) |
| Particles | reactor, gravity well, shockwave, rain, orbital, ember, magnetic, tornado, constellation, data core, plasma |

**Status: DONE (full built-in pack)**  
**Deferred:** true live-sample oscilloscope / Lissajous (needs live PCM gate).

---

### Phase 8 — Doctor / setup

**Goal:** First-run and supportability.

| Deliverable | Done when |
|-------------|-----------|
| `doctor` | Version, paths, log dir, selected VLC/Rodio output check, FFmpeg hints |
| `setup` | Guidance for VLC, Rodio, Fake, and optional FFmpeg |
| `paths` | Data/state locations for `tz-player-rs` |

**Status: DONE**

---

### Phase 9 — Hardening / UX polish

| Item | Notes |
|------|--------|
| Empty playlist hints | First-run clarity |
| Status TTL | ~4s via `set_status` |
| Analysis readiness indicators | ESBW / analyzing |
| Visualizer name in header | Discoverability |
| Clearer errors | remove/clear/find/VLC-fallback/state-file |
| Progress bar width scaling | Responsive terminal |
| Home/End track jump | Playlist nav |

**Status: DONE (v1)**

---

### Phase 10 — Packaging / docs

| Deliverable | Done when |
|-------------|-----------|
| README quick start | Build/run/doctor |
| `docs/usage.md` | Keys and workflows |
| `docs/RELEASE.md` | Release checklist |
| Release binary path | `cargo build --release -p tz-player` |

**Status: DONE (docs-level)** — installers/signing still optional

---

### Phase 11+ — Post-parity / future

Ordered backlog for later work (not blocking v1 claim):

| Priority | Workstream | Notes |
|----------|------------|--------|
| A | Live playback-backend PCM sampling | True oscilloscope-class visualizers |
| B | Perf benches vs Python | Rust opt-in DSP/DB/UI-idle harness landed; shared-corpus Python and live-playback comparison remain |
| C | Headless control server | `tz-player serve` over IPC using `tz-control` |
| D | Multi-process split | Controller / Engine / Library Manager processes |
| E | Library manager features | Watch/rescan, multi-playlist UX, smart playlists |
| F | Slim custom FFmpeg (analysis) | Optional packaging size win |
| G | Sidecar cover art | `cover.jpg` / `folder.jpg` |
| H | State/DB import from Python | Optional migrator |
| I | Engine upgrades | Gapless, crossfade, EQ, devices (FUTURE_PROJECT) |
| J | Intelligence / appliance UX | Voice, themes, remote UIs — future brief only |

---

## 6. Recommended historical execution order

This is the order actually used after plan approval (useful if restarting from an empty clone):

1. Phase 0 scaffold  
2. Phase 1 DB + `PlaylistStore` first (user chose store before VLC spike)  
3. Phase 3 runtime + Phase 2 Fake, then full libVLC FFI  
4. Phase 5 metadata + Phase 4 minimal TUI  
5. Phase 6 analysis (envelope → spectrum → beat → waveform)  
6. Phase 7 visualizers (host + core → spectrum pack → full pack)  
7. Phase 9 polish, Phase 8 doctor, Phase 10 docs  
8. Visualizer fidelity fixes (centering, bounce, cover art, particle ports matching Python)

Do **not** start with pure-Rust decode/output (Symphonia+CPAL) as the default
listen path; that was explicitly rejected for v1. ADR-0003 later approved an
additive, opt-in Rodio implementation for evaluation, not a default change.

---

## 7. Quality gates (every meaningful change)

```powershell
cargo fmt --all
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo audit
cargo deny --locked check advisories bans licenses sources
```

Notes:

- Clippy args after `--` go to rustc. Do **not** append unrelated tokens (e.g. stray `- clean`); run `cargo clean` as its own command.
- VLC smoke (manual, optional): `cargo run -p tz-playback --example vlc_smoke`
- Rodio silent smoke: `cargo run -p tz-playback --example rodio_smoke -- --startup-only`
- Prefer Fake backend in automated tests.

---

## 8. Environment and ops notes

| Topic | Detail |
|-------|--------|
| Primary OS | Windows (PowerShell) |
| VLC | System install; dynamic load |
| Rodio | Compiled in; default OS output; Linux source builds need ALSA development files |
| FFmpeg | Optional on PATH for analysis quality |
| Verbose VLC | `TZ_PLAYER_VLC_VERBOSE=1` |
| Backend override | `tz-player --backend vlc|rodio|fake` |
| Data dir | platformdirs under **`tz-player-rs`** identity |
| Reference Python | `_ref_tz_player/` for behavior and visualizer math |

---

## 9. How an AI should use this document

1. Read **§2 Locked decisions** and **§5** phase status before proposing architecture changes.  
2. Use [`PROGRESS.md`](PROGRESS.md) for a short done/remaining snapshot; update both when completing work.  
3. For behavior parity, open `_ref_tz_player` (services, visualizers, tests) rather than guessing.  
4. For long-term product direction beyond parity, read [`tz_player_v2_future_project.md`](tz_player_v2_future_project.md) but do not expand v1 scope without user approval.  
5. New media or data-identity choices need an **ADR** under `docs/adr/`.  
6. Prefer small vertical slices: store → service → command → TUI → test/clippy.  
7. When porting visualizers, match Python motion constants and status strings unless the user asks for redesign.

### Suggested “next work” prompts

Pick from Phase 11+ or fidelity/polish:

- “Implement live VLC PCM/level sampling and one true waveform visualizer.”  
- “Add headless `serve` + IPC using `tz-control`.”  
- “Write perf comparison harness vs Python analysis.”  
- “Import playlist DB from Python `tz-player` identity.”  
- “Port remaining UX gaps from `_ref_tz_player/docs/gap-analysis.md`.”

---

## 10. Related documents

| Doc | Role |
|-----|------|
| [SPEC.md](SPEC.md) | Parity product requirements |
| [architecture.md](architecture.md) | Media roles + crate deps |
| [PROGRESS.md](PROGRESS.md) | Checklist status |
| [usage.md](usage.md) | End-user keys and workflows |
| [RELEASE.md](RELEASE.md) | Release steps |
| [adr/ADR-0001…](adr/ADR-0001-rust-crate-architecture-and-media-split.md) | Crate + media split |
| [adr/ADR-0002…](adr/ADR-0002-data-directory-identity.md) | Data dir identity |
| [adr/ADR-0003…](adr/ADR-0003-add-experimental-rodio-backend.md) | Additive experimental Rodio backend |
| [tz_player_v2_future_project.md](tz_player_v2_future_project.md) | Post-parity vision |
| `_ref_tz_player/docs/*` | Python ADRs, workflows, gap analysis |

---

## 11. Provenance

- **Planning session:** interactive plan mode over Python clone + FUTURE_PROJECT brief; conversion approved starting Phase 0.  
- **Playback decision:** plan revised mid-session to keep **VLC-first** listen path (earlier draft wrongly leaned pure-Rust/FFmpeg playback).  
- **Execution:** Phases 0–10 and full visualizer pack implemented in subsequent sessions; details lived in chat/session memory until this file.  
- **Reconstruction date:** 2026-08-08 from session memory + current tree + existing docs.

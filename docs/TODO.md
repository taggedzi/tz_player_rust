# TODO List

> **Historical backlog — audio conclusions superseded 2026-08-11.** VLC was
> removed and the composite Audio engine became default under ADR-0004. Checked
> VLC/Rodio items below describe prior implementation evidence, not current setup.

Ordered by triage (see bottom for tier rationale). Items previously listed under
both "Core" and "TUI" (About info) have been merged. The Shift+Left/Right seek
item has been removed — verified already implemented in
`crates/tz-tui/src/lib.rs` (bound to ±30s).

## Security hardening backlog

Triaged from the 2026-08-09 security review. There are no Tier 0 emergency
findings. Complete Tiers 1 and 2 before a public release; Tier 3 is the
release-hardening phase.

### Security Tier 1 — Address before public release

- [x] **Eliminate multithreaded environment mutation.** `VLC_PLUGIN_PATH` is
  now configured only on Windows during synchronous binary/example startup,
  before the Tokio runtime is built. The VLC worker no longer mutates the
  environment, Unix configuration is a tested no-op, late `PATH` mutation was
  removed from the loader, and CI covers Windows, Linux, and macOS builds.
  Remove or relocate the
  `VLC_PLUGIN_PATH` mutation from the VLC worker. Configure it before Tokio
  starts, avoid the mutation on Unix, or restrict it to Windows if that is the
  only platform requiring it. Verify VLC startup on Windows, Linux, and macOS.
  **Done when:** no Unix code mutates the process environment after threads
  start.
- [x] **Bound media-analysis resource consumption.** FFmpeg PCM is consumed in
  bounded streaming chunks with null stdin and decoded-byte, duration, and
  wall-clock limits; limit and timeout paths kill and reap the child. Native
  WAV samples are streamed under the same limits, and all four cache products
  share one decode per track. User overrides have documented hard ceilings.
  Oversized WAV and FFmpeg paths have regression tests.
- [x] **Limit embedded cover-art decoding.** Lofty now enforces an 8 MiB
  per-item allocation cap on every metadata-reading thread; cover parsing also
  has a cumulative 32 MiB read budget and rejects more than 16 MiB of picture
  payload. Pictures over 8 MiB are skipped before image parsing, and strict
  4096x4096 / 32 MiB decoder limits are tested before the 160px resize.
- [x] **Sanitize plain terminal output.** Shared `terminal_safe` /
  `terminal_safe_path` helpers visibly escape C0/C1 controls, ESC/BEL, CR/LF,
  line separators, and directional controls. Playlist metadata, CLI errors,
  diagnostic paths/notes, log setup failures, and terminal-facing tracing use
  them. ANSI, OSC, newline, C1 CSI, and bidi payloads have regression tests.

### Security Tier 2 — Security hardening

- [x] **Remove the LibVLC ABI and backend.** The earlier VLC 3.x fail-closed
  loading work is retained in history, but the final Audio migration removed
  LibVLC discovery, FFI, runtime selection, and packaging entirely.
- [x] **Remove the affected `lru 0.12.5` dependency.** Ratatui 0.30.2 and
  Crossterm 0.29 replace it with `lru 0.18.2`; the explicit MSRV is now Rust
  1.88. TUI/player tests pass and `cargo audit` no longer reports
  RUSTSEC-2026-0002. The full strict Clippy gate is completed in the dedicated
  checklist task below.
- [x] **Resolve the unmaintained `paste` dependency.** Ratatui 0.30.2 removed
  its dependency, and Lofty was upgraded to 0.25.0, but the current Lofty
  release still requires `paste` 1.0.15. RUSTSEC-2024-0436 is therefore tracked
  in `docs/SECURITY.md` as an accepted build-time risk owned by the repository
  maintainer and expiring 2026-11-08. `Cargo.lock` pins the version and the
  dependency path is now Lofty-only.
- [x] **Restore the documented Clippy gate.** The VLC tests now follow all
  helper functions, and the waveform visualizer passes its rendering inputs in
  a focused parameter object rather than suppressing `too_many_arguments`.
  `cargo clippy --workspace --all-targets -- -D warnings` succeeds.

### Security Tier 3 — Supply-chain and operational controls

- [x] **Add dependency security policy to CI.** CI runs pinned versions of
  `cargo audit` and `cargo deny`; `deny.toml` rejects vulnerabilities, unsound
  and unmaintained advisories, yanked crates, unapproved licenses, unknown
  registries, and all unapproved Git dependencies. The single `paste`
  unmaintained advisory is explicitly linked to its owned, time-bounded
  exception. `Cargo.lock` remains committed, so vulnerable or disallowed
  dependency changes block pull requests.
- [x] **Pin GitHub Actions by immutable commit SHA.** Checkout, Rust toolchain,
  and Rust cache references use full upstream commit SHAs with adjacent release
  identifiers. The workflow grants only read access to repository contents,
  and weekly Dependabot updates provide the reviewable update process. CI no
  longer executes mutable action tags.
- [x] **Document runtime trust boundaries.** `docs/SECURITY.md` now identifies
  FFmpeg-on-`PATH`, dynamically loaded LibVLC/plugins, VLC 3.x-only ABI support,
  untrusted in-process media parsing, and the compiled analysis/cover limits.
  User and release docs require trusted distribution channels and environment
  review. The release checklist now includes dependency policy, exception
  expiry, and malformed/oversized-media regression checks.

## Tier 0 — Correctness fixes (small, isolated, do first)

All three done.

- [x] analysis-cache pruning — ported as `tz_db::AnalysisCachePruner` (age + byte-budget eviction, same SQL shape as the Python reference) and wired into `LevelService::maybe_prune_cache`, called from `ensure_analysis_inner` after any cache write. Limits live in `CacheLimits` (`crates/tz-core/src/levels.rs`), currently hardcoded to the Python defaults (2 GiB cap / 180 day max age / 200 recent entries protected / 0.90 trigger threshold) and not yet user-configurable — feeds Tier 4's "read from a config" item. Also throttled to at most once per `check_interval` (default 30s) so a big recursive folder-add (one `ensure_analysis` call per track) doesn't do a full cache scan per track.
- [x] Folder-add isn't recursive — `expand_media_paths` in `crates/tz-core/src/runtime.rs` now recurses into subdirectories via `collect_media_files_recursive`.
- [x] Add missing music files that are supported by vlc — `is_media_extension` extended with mp4, m4b, mka, ac3, dts, mpc, tta, spx, caf, mid, midi.

## Tier 1 — Missing core functionality

Both done.

- [x] "about" information output — `tz_core::about_info()` (`crates/tz-core/src/about.rs`) is the single shared source: name, version, description, repository, license, schema version, target/profile. No local paths or machine-specific data (that's `tz-player doctor`'s job). Surfaced via `tz-player about` (CLI) and the `i` key in the TUI (About modal). Note: `about_info()`'s `env!("CARGO_PKG_*")` calls resolve to `tz-core`'s own package metadata, which only matches the product because every crate inherits `[workspace.package]` — see the comment on `about_info()` if that ever changes.
- [x] Warnings/Errors surfacing — footer status line now carries severity (`tz_core::StatusLevel`: Info/Warn/Error), colored in the TUI (plain/yellow/red) with a `[WARN]`/`[ERROR]` prefix. Only playback-backend failures (`PlayerError::Playback`, i.e. the active audio-engine path) are `Error` and persist until dismissed with Esc; everything else (metadata refresh, clear-playlist, add-paths DB errors, missing/unreadable track rows) is a `Warn` that auto-clears like a normal status message (~4s). Previously several of these failures (refresh metadata, clear playlist, add-paths, most transport commands) were silently discarded entirely — they now actually reach the user for the first time.

## Tier 2 — Usability gaps in daily TUI use

All four done.

- [x] Now-playing marker + locate — the cursor-selection highlight already existed (cyan row background). Added the second marking system: a `>` marker in green rendered as its own `Span` (so it survives under the cursor highlight too) on whichever playlist row's `item_id` matches the player's currently-/last-played track (`TransportSnapshot::item_id`, unchanged by Stop — same semantics the header's now-playing line already used). Added a `g` key (`Command::LocatePlaying` in `crates/tz-core/src/runtime.rs`) that moves the cursor straight to that row; it resolves within the active find-filtered view when possible, and only clears an active find if the playing track isn't in the filtered results. TDD'd with `ratatui::backend::TestBackend` render assertions (`crates/tz-tui/src/lib.rs`) and runtime unit tests covering the found/filtered/filtered-out/never-played cases (`crates/tz-core/src/runtime.rs`).
- [x] Key-legend / player-state clarity — replaced the cramped single-line `?` help (which had silently fallen out of sync: `r`/`s`/`d`/`c`/`m`/Home/End/PageUp/PageDown/Shift+arrows weren't documented at all) with a full-screen modal popup (`crates/tz-tui/src/lib.rs`: `draw_help_overlay`/`help_lines`), grouped by category (Playback/Navigation/Playlist/View), listing every binding including the previously-undocumented `\` (reset speed) and `g` (locate playing, from the marker work above). Sized from actual content rather than a screen percentage — a percentage-of-screen popup silently clipped on a standard 80x24 terminal during review; content is now packed two mnemonics per line and centered via `centered_fixed_rect`, verified against an 80x24 fixture. Repeat/shuffle state in the transport bar is now bold-green when active vs. dim gray when off (`state_style`), instead of blending into the rest of the line as plain text. TDD'd via `ratatui::backend::TestBackend`.
- [x] Real-time fuzzy search — `f` still enters find mode, but every keystroke (`Char`/`Backspace`) now dispatches `Command::SetFindQuery` immediately instead of waiting for Enter (`crates/tz-tui/src/lib.rs::handle_key`); the playlist filters live as you type. `Enter` now just exits editing (results are already applied); `Esc` still fully clears the find (query + filter), same as before. No debounce — the existing per-frame DB-connection-per-call pattern already runs at ~12Hz, well above typing speed, so this wasn't a case for pre-optimizing (consistent with prior "measure before building" guidance). Footer hint corrected from the now-false "Enter=apply" to "live — Enter=keep Esc=cancel". TDD'd at the `handle_key` dispatch boundary — the first test written at that boundary — asserting through `playlist_count()`/`fetch_rows` (the same read path the UI renders from) rather than the internal `find_ids` field, seeded per `tz-db`'s existing FTS-backed search fixture. Also caught and fixed stale keybinding tables in `README.md`/`docs/usage.md`/`docs/PROGRESS.md` (missing `g`/`i`, "Help strip" no longer describes the Tier 2 modal).
- [x] Visualizer pane collapse/maximize — there was no existing "disabled visualizer" state to key off (`z` only cycled the 26 built-in plugins, no off/none stop), so this needed a decision: added a dedicated `Shift+Z` toggle (`crates/tz-tui/src/lib.rs`) rather than folding an off-state into the `z` cycle. Hiding gives the playlist the full width (`main_layout(area, visualizer_hidden) -> (Rect, Option<Rect>)`, shared by both the pre-render sizing pass and the actual draw pass — replaces the old duplicated `Layout::split` calls, which is also a correctness fix since a hidden pane now can't index out of bounds by construction). While hidden the visualizer host itself isn't torn down but its `render()` isn't called either (frozen, not ticking, to avoid animating something invisible) — showing it again resumes instantly rather than reinitializing. State is session-only (`AppRuntime::visualizer_hidden`, not written to `AppState`); if that turns out to be wanted across restarts, that's Tier 4 config territory. TDD'd `main_layout` as a pure function plus the `Shift+Z`/`z` key dispatch via `handle_key`. Documented in the help modal and `README.md`/`docs/usage.md`/`docs/PROGRESS.md`.

## Tier 3 — Larger feature work
- [ ] **Major goal: create a single-stage release builder for every supported
  platform.** Replace the current sequence of prerequisite, SDK, package, and
  smoke-test commands with one documented maintainer action, backed by one
  reusable script/workflow. Given a version (and, when useful, an explicit
  target selection), it must run the release gates, validate or clearly report
  missing build prerequisites, build the pinned native FFmpeg SDK plus the
  player/helper, create the complete licensed package and checksums, run the
  staged-package smoke test, and leave only predictable publish-ready artifacts
  and a short build summary in `target/dist`. The normal release action should
  produce the supported target set—Windows x86-64, Linux x86-64, and macOS
  ARM64 when that target is enabled—without the maintainer manually invoking
  multiple scripts, setting environment variables, or selecting intermediate
  paths. Keep a current-host/local mode for fast iteration, but do not allow it
  to bypass the same packaging, licensing, and smoke-test gates. Update
  `docs/RELEASE.md`, CI, and release notes so the final process is a single
  copy/paste command or one clearly named workflow action. Implementation plan:
  [`single-stage release builder`](superpowers/plans/2026-08-11-single-stage-release-builder.md).
- [ ] **Next major goal: broaden playback compatibility with FFmpeg-backed PCM
  streaming.** Route files rejected by the native decoder through the packaged
  FFmpeg helper, stream normalized PCM to playback and visualizers, and keep
  buffering bounded instead of loading an entire WAV into memory. The goal,
  phases, constraints, and completion criteria are documented in
  [`FFMPEG_PLAYBACK_EXPANSION_GOAL.md`](FFMPEG_PLAYBACK_EXPANSION_GOAL.md).
- [ ] **Major goal: add offline MIDI playback with a bundled, replaceable
  SoundFont.** Treat MIDI as a synthesis path (`MIDI events -> sequencer and
  SoundFont synth -> normalized PCM -> existing Rodio mixer`), not as an
  ordinary compressed-audio decoder. Evaluate FluidSynth as the primary
  cross-platform engine, with support for General MIDI/GS behavior including
  tempo maps, program changes, percussion, sustain, pitch bend, resets, and
  correct duration/tail handling. Package a vetted default SoundFont so the
  feature works on a clean offline install, while allowing users to select and
  persist a custom SF2/SF3 bank with validation and a safe built-in fallback.
  The default bank must be approved for redistribution in software; preserve
  its exact upstream license, source/version metadata, checksum, and notices
  in the release and third-party-license documentation. GeneralUser GS is a
  candidate pending license/provenance confirmation, not an assumed asset.
  Plan bounded streaming, synth-state reset/replay seeking, MIDI-specific
  speed semantics, visualizer level integration, malformed-input/resource
  limits, and cross-platform release smoke tests. Do not bundle copyrighted
  MIDI content merely as test data. Completion requires an offline default
  install, custom SoundFont replacement, stable playback through the existing
  transport/UI, and reviewed distribution notices.
- [x] **Dual-pane staged playlist editor** — implemented in `crates/tz-core`, `crates/tz-db`, and `crates/tz-tui` using the approved design in `docs/superpowers/specs/2026-08-09-dual-pane-playlist-editor-design.md`. `a` opens a full-screen files/playlist editor; edits use transient SQLite draft rows; F10/Ctrl+Enter applies after a successful stop; saved playlists support load/save-as/rename/delete with the active Default playlist protected. Recursive scans are iterative, deterministic, and skip symlink directories. Portable-media path rebasing remains a separate future feature.
- [x] **Port every Python visualization.** The Rust host preserves all 25
  built-in Python plugin IDs and adds `spectrum.bars` for 26 total. The
  inventory is locked by a registry test, while focused rendering tests cover
  the shared spectrum, waveform, cover-art, matrix, and particle families.
- [x] Remove multi-select from the playlist-edit workflow; ordering and additions now happen in the staged editor. Existing legacy browse commands remain only for compatibility and are not used by the `a` path.
- [x] **Add and evaluate an experimental Rodio playback backend alongside VLC.**
  Keep VLC as the default while `--backend rodio` is tested for transport parity,
  common-format coverage, natural-end behavior, and cross-platform output. The
  approved implementation must follow the
  [design](superpowers/specs/2026-08-10-rodio-backend-design.md) and
  [commit-by-task plan](superpowers/plans/2026-08-10-rodio-backend.md). Promotion
  to default or removal of VLC is a separate decision based on the published
  compatibility evaluation.

## Tier 4 — Nice to haves (do last)
- [x] **Add Track, Artist, and Album columns with sorting.** The playlist uses
  width-aware aligned columns and `o` cycles persisted Playlist, Track, Artist,
  and Album view orders while preserving selection and active find results.
  Sorting is deliberately non-destructive: playback/editor order remains the
  playlist's stored order, and main-view reordering requires Playlist mode.
- [x] **Load configurable TUI colors and formatting outside the player.**
  `tz-tui` reads the optional user-owned `theme.json`, remaps semantic colors,
  and supports selection-bold / muted-dim overrides. Named, RGB, and ANSI
  colors are validated under a 64 KiB limit; missing or invalid files safely
  use the built-in theme with an actionable warning. Player and playback state
  contain no theme configuration, and a documented example is included.
- [x] **Add mouse support while retaining complete keyboard operation.** Mouse
  capture is scoped to the alternate-screen TUI. Clicks select playlist and
  editor rows, a playlist double-click plays, wheels move three rows, and
  click/drag controls seek, volume, and speed through the same structured
  commands as the keyboard. Capture is disabled on exit, hitboxes share the
  render layouts, and the full keyboard map remains documented and tested.

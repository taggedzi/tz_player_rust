# TODO List

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

- [x] **Make the LibVLC ABI explicit and fail closed.** The stable
  `libvlc_get_version` discovery symbol is loaded first; only validated VLC 3.x
  libraries resolve the dedicated V3 function table and millisecond
  conversions. VLC 4 and unknown majors are rejected before ABI-specific
  lookup because V4 construction/seeking differs. C states cross as integers
  and are checked before conversion; unsupported versions, unknown states, and
  V3 time units have regression tests.
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
- [ ] **Pin GitHub Actions by immutable commit SHA.** Pin checkout, Rust
  toolchain, and cache actions while retaining comments identifying their
  release tags, and establish a periodic update process. **Done when:** CI no
  longer executes mutable action tags.
- [ ] **Document runtime trust boundaries.** State that FFmpeg is resolved
  through `PATH` and LibVLC is loaded dynamically; recommend trusted
  package-manager installations; document supported VLC majors and analysis
  limits; and add malicious-media and dependency-audit checks to the release
  checklist. **Done when:** release documentation accurately describes
  external-code and untrusted-media risks.

## Tier 0 — Correctness fixes (small, isolated, do first)

All three done.

- [x] analysis-cache pruning — ported as `tz_db::AnalysisCachePruner` (age + byte-budget eviction, same SQL shape as the Python reference) and wired into `LevelService::maybe_prune_cache`, called from `ensure_analysis_inner` after any cache write. Limits live in `CacheLimits` (`crates/tz-core/src/levels.rs`), currently hardcoded to the Python defaults (2 GiB cap / 180 day max age / 200 recent entries protected / 0.90 trigger threshold) and not yet user-configurable — feeds Tier 4's "read from a config" item. Also throttled to at most once per `check_interval` (default 30s) so a big recursive folder-add (one `ensure_analysis` call per track) doesn't do a full cache scan per track.
- [x] Folder-add isn't recursive — `expand_media_paths` in `crates/tz-core/src/runtime.rs` now recurses into subdirectories via `collect_media_files_recursive`.
- [x] Add missing music files that are supported by vlc — `is_media_extension` extended with mp4, m4b, mka, ac3, dts, mpc, tta, spx, caf, mid, midi.

## Tier 1 — Missing core functionality

Both done.

- [x] "about" information output — `tz_core::about_info()` (`crates/tz-core/src/about.rs`) is the single shared source: name, version, description, repository, license, schema version, target/profile. No local paths or machine-specific data (that's `tz-player doctor`'s job). Surfaced via `tz-player about` (CLI) and the `i` key in the TUI (footer status line). Note: `about_info()`'s `env!("CARGO_PKG_*")` calls resolve to `tz-core`'s own package metadata, which only matches the product because every crate inherits `[workspace.package]` — see the comment on `about_info()` if that ever changes.
- [x] Warnings/Errors surfacing — footer status line now carries severity (`tz_core::StatusLevel`: Info/Warn/Error), colored in the TUI (plain/yellow/red) with a `[WARN]`/`[ERROR]` prefix. Only playback-backend failures (`PlayerError::Playback`, i.e. the actual VLC/libVLC audio path) are `Error` and persist until dismissed with Esc; everything else (metadata refresh, clear-playlist, add-paths DB errors, missing/unreadable track rows) is a `Warn` that auto-clears like a normal status message (~4s). Previously several of these failures (refresh metadata, clear playlist, add-paths, most transport commands) were silently discarded entirely — they now actually reach the user for the first time.

## Tier 2 — Usability gaps in daily TUI use

All four done.

- [x] Now-playing marker + locate — the cursor-selection highlight already existed (cyan row background). Added the second marking system: a `>` marker in green rendered as its own `Span` (so it survives under the cursor highlight too) on whichever playlist row's `item_id` matches the player's currently-/last-played track (`TransportSnapshot::item_id`, unchanged by Stop — same semantics the header's now-playing line already used). Added a `g` key (`Command::LocatePlaying` in `crates/tz-core/src/runtime.rs`) that moves the cursor straight to that row; it resolves within the active find-filtered view when possible, and only clears an active find if the playing track isn't in the filtered results. TDD'd with `ratatui::backend::TestBackend` render assertions (`crates/tz-tui/src/lib.rs`) and runtime unit tests covering the found/filtered/filtered-out/never-played cases (`crates/tz-core/src/runtime.rs`).
- [x] Key-legend / player-state clarity — replaced the cramped single-line `?` help (which had silently fallen out of sync: `r`/`s`/`d`/`c`/`m`/Home/End/PageUp/PageDown/Shift+arrows weren't documented at all) with a full-screen modal popup (`crates/tz-tui/src/lib.rs`: `draw_help_overlay`/`help_lines`), grouped by category (Playback/Navigation/Playlist/View), listing every binding including the previously-undocumented `\` (reset speed) and `g` (locate playing, from the marker work above). Sized from actual content rather than a screen percentage — a percentage-of-screen popup silently clipped on a standard 80x24 terminal during review; content is now packed two mnemonics per line and centered via `centered_fixed_rect`, verified against an 80x24 fixture. Repeat/shuffle state in the transport bar is now bold-green when active vs. dim gray when off (`state_style`), instead of blending into the rest of the line as plain text. TDD'd via `ratatui::backend::TestBackend`.
- [x] Real-time fuzzy search — `f` still enters find mode, but every keystroke (`Char`/`Backspace`) now dispatches `Command::SetFindQuery` immediately instead of waiting for Enter (`crates/tz-tui/src/lib.rs::handle_key`); the playlist filters live as you type. `Enter` now just exits editing (results are already applied); `Esc` still fully clears the find (query + filter), same as before. No debounce — the existing per-frame DB-connection-per-call pattern already runs at ~12Hz, well above typing speed, so this wasn't a case for pre-optimizing (consistent with prior "measure before building" guidance). Footer hint corrected from the now-false "Enter=apply" to "live — Enter=keep Esc=cancel". TDD'd at the `handle_key` dispatch boundary — the first test written at that boundary — asserting through `playlist_count()`/`fetch_rows` (the same read path the UI renders from) rather than the internal `find_ids` field, seeded per `tz-db`'s existing FTS-backed search fixture. Also caught and fixed stale keybinding tables in `README.md`/`docs/usage.md`/`docs/PROGRESS.md` (missing `g`/`i`, "Help strip" no longer describes the Tier 2 modal).
- [x] Visualizer pane collapse/maximize — there was no existing "disabled visualizer" state to key off (`z` only cycled the 26 built-in plugins, no off/none stop), so this needed a decision: added a dedicated `Shift+Z` toggle (`crates/tz-tui/src/lib.rs`) rather than folding an off-state into the `z` cycle. Hiding gives the playlist the full width (`main_layout(area, visualizer_hidden) -> (Rect, Option<Rect>)`, shared by both the pre-render sizing pass and the actual draw pass — replaces the old duplicated `Layout::split` calls, which is also a correctness fix since a hidden pane now can't index out of bounds by construction). While hidden the visualizer host itself isn't torn down but its `render()` isn't called either (frozen, not ticking, to avoid animating something invisible) — showing it again resumes instantly rather than reinitializing. State is session-only (`AppRuntime::visualizer_hidden`, not written to `AppState`); if that turns out to be wanted across restarts, that's Tier 4 config territory. TDD'd `main_layout` as a pure function plus the `Shift+Z`/`z` key dispatch via `handle_key`. Documented in the help modal and `README.md`/`docs/usage.md`/`docs/PROGRESS.md`.

## Tier 3 — Larger feature work
- [x] **Dual-pane staged playlist editor** — implemented in `crates/tz-core`, `crates/tz-db`, and `crates/tz-tui` using the approved design in `docs/superpowers/specs/2026-08-09-dual-pane-playlist-editor-design.md`. `a` opens a full-screen files/playlist editor; edits use transient SQLite draft rows; F10/Ctrl+Enter applies after a successful stop; saved playlists support load/save-as/rename/delete with the active Default playlist protected. Recursive scans are iterative, deterministic, and skip symlink directories. Portable-media path rebasing remains a separate future feature.
- All of the visualizations from the python version need to be ported. some already are, but the rest should be.
- [x] Remove multi-select from the playlist-edit workflow; ordering and additions now happen in the staged editor. Existing legacy browse commands remain only for compatibility and are not used by the `a` path.

## Tier 4 — Nice to haves (do last)
- (nice to have) add columns for Artist/album/track with sorting options.
- Allow usage of some color/formatting in the TUI. It needs to be easily configurable, the config should not happen IN the player, but read from a config so people who want to make themes can. (this is a nice to have, a wanted nice to have)
- Addition of mouse support. the program MUST remain fully keyboard functional, but mouse functionality is a very strong desire.

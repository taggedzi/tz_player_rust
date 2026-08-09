# TODO List

Ordered by triage (see bottom for tier rationale). Items previously listed under
both "Core" and "TUI" (About info) have been merged. The Shift+Left/Right seek
item has been removed — verified already implemented in
`crates/tz-tui/src/lib.rs` (bound to ±30s).

## Tier 0 — Correctness fixes (small, isolated, do first)

All three done.

- [x] analysis-cache pruning — ported as `tz_db::AnalysisCachePruner` (age + byte-budget eviction, same SQL shape as the Python reference) and wired into `LevelService::maybe_prune_cache`, called from `ensure_analysis_inner` after any cache write. Limits live in `CacheLimits` (`crates/tz-core/src/levels.rs`), currently hardcoded to the Python defaults (2 GiB cap / 180 day max age / 200 recent entries protected / 0.90 trigger threshold) and not yet user-configurable — feeds Tier 4's "read from a config" item. Also throttled to at most once per `check_interval` (default 30s) so a big recursive folder-add (one `ensure_analysis` call per track) doesn't do a full cache scan per track.
- [x] Folder-add isn't recursive — `expand_media_paths` in `crates/tz-core/src/runtime.rs` now recurses into subdirectories via `collect_media_files_recursive`.
- [x] Add missing music files that are supported by vlc — `is_media_extension` extended with mp4, m4b, mka, ac3, dts, mpc, tta, spx, caf, mid, midi.

## Tier 1 — Missing core functionality
- Project needs an "about" information output. CLI, TUI, and any other eventual user exposed interface need to display the product information, github page (does not exist yet), version and any useful debug info without exposing sensitive information about a user.
- Warnings and Errors surface to the TUI interface. Warnings/Errors need a clean way to surface that do not interfear with playback UNLESS the Warning/Error cause is disrupting playback.

## Tier 2 — Usability gaps in daily TUI use
- There is no marker that shows (in the playlist) which song is playing, AND in a large playlist no way to easily locate which song is playing. There needs to be 2 marking systems 1. displays a cursor type effect showing the user what song is being selected, and 2. a marker of some kind that easily allows a user to find the current playing song. (possibly even a key that navigates directly to it.)
- TUI needs a bit of UI/UX love. It is completely functional, and I like the overall layout, but the command keys, player state (repeat/shuffle) need to be more clear.
- Fuzzy search should react in real time instead of waiting for an enter or click event.
- IF the visualizer is disabled allow the collapse of the visualizer pane and allow the maximization of the playlist.

## Tier 3 — Larger feature work
- Folder-add needs a modal that allows tree navigation and file/folder selection; not just a short prompt line. (Sequence after the Tier 0 recursion fix so the modal isn't built on top of the broken scan.)
- All of the visualizations from the python version need to be ported. some already are, but the rest should be.
- Remove multi-select. -> Move to a seperate screen for playlist creation that allows moving up/down/adding/etc. almost a Norton Commander or Midnight commander style interface with ordering controls and the ability to save/open play lists. (Largest single item — a new screen, not a fix. Sequence last so it's built on top of the marker/key-legend work above rather than duplicating it.)

## Tier 4 — Nice to haves (do last)
- (nice to have) add columns for Artist/album/track with sorting options.
- Allow usage of some color/formatting in the TUI. It needs to be easily configurable, the config should not happen IN the player, but read from a config so people who want to make themes can. (this is a nice to have, a wanted nice to have)
- Addition of mouse support. the program MUST remain fully keyboard functional, but mouse functionality is a very strong desire.

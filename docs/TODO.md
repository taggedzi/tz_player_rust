# TODO List

Ordered by triage (see bottom for tier rationale). Items previously listed under
both "Core" and "TUI" (About info) have been merged. The Shift+Left/Right seek
item has been removed — verified already implemented in
`crates/tz-tui/src/lib.rs` (bound to ±30s).

## Tier 0 — Correctness fixes (small, isolated, do first)
- analysis-cache pruning - Python evicts old/oversized entries from the analysis cache by age + byte budget; Rust's schema has the exact columns/indexes for it (byte_size, last_accessed_at) but no code ever prunes. The cache DB will grow unbounded.
- Folder-add isn't recursive — Rust's add only does a single-level read_dir; Python recursively scans. Adding a music folder in Rust silently skips every album subfolder.
- Add missing music files that are supported by vlc in the open, playback, and selection areas.

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
